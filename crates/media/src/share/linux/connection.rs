//! A PipeWire connection kept on the thread that drives its main loop.

use std::cell::Cell;
use std::os::fd::OwnedFd;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pipewire as pw;

use super::{Life, error};
use crate::share::CaptureError;

pub(super) struct Connection {
    _errors: pw::core::Listener,
    pub core: pw::core::CoreRc,
    _context: pw::context::ContextRc,
    pub mainloop: pw::main_loop::MainLoopRc,
    pub nonce: String,
}

/// Whether an error that the daemon reports on the connection ends the capture.
///
/// The daemon reports every error here: a link that it refused, with the identifier of
/// the link, and a request for an object that it had already removed, with the identifier
/// of the connection. Only the loss of the connection ends the capture, which is also the
/// rule of the stream of the PipeWire library. A stream reports its own error through its
/// state, and a link that could not be made is made again (registry.rs).
fn ends_the_capture(id: u32, result: i32) -> bool {
    let lost = result.checked_neg().is_some_and(|code| {
        std::io::Error::from_raw_os_error(code).kind() == std::io::ErrorKind::BrokenPipe
    });
    id == pw::core::PW_ID_CORE && lost
}

impl Connection {
    pub fn new(fd: Option<OwnedFd>, life: Arc<Life>) -> Result<Self, CaptureError> {
        PANICKED.with(|panicked| panicked.set(false));
        if !crate::audio::linux::initialise() {
            return Err(error("PipeWire is not available"));
        }
        pw::init();
        let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(error)?;
        let nonce = uuid::Uuid::new_v4().to_string();
        let properties = pw::properties::properties! {
            "application.name" => "Junto", "junto.capture.token" => nonce.clone(),
        };
        let context = pw::context::ContextRc::new(&mainloop, Some(properties)).map_err(error)?;
        let core = match fd {
            Some(fd) => context.connect_fd_rc(fd, None),
            None => context.connect_rc(None),
        }
        .map_err(error)?;
        let errors = core
            .add_listener_local()
            .error(move |id, _, result, message| {
                super::connection::in_callback("PipeWire connection error", || {
                    if ends_the_capture(id, result) {
                        life.fail(error(message));
                    } else {
                        tracing::debug!(
                            id,
                            result,
                            message,
                            "the PipeWire daemon reported an error that does not end the capture"
                        );
                    }
                })
            })
            .register();
        Ok(Self {
            _errors: errors,
            core,
            _context: context,
            mainloop,
            nonce,
        })
    }

    pub fn drive(
        &self,
        life: Arc<Life>,
        timeout: Option<Duration>,
        done: impl Fn() -> bool + 'static,
    ) -> Result<(), CaptureError> {
        let started = Instant::now();
        let timer = self.mainloop.loop_().add_timer({
            let mainloop = self.mainloop.downgrade();
            let life = life.clone();
            move |_| {
                crate::callback::stop_panic(
                    "PipeWire timer",
                    || (),
                    || {
                        end_after_a_panic(&life);
                        if !life.stopped() && !done() {
                            if timeout.is_none_or(|limit| started.elapsed() < limit) {
                                return;
                            }
                            life.fail(error("PipeWire did not finish preparing the capture"));
                        }
                        if let Some(mainloop) = mainloop.upgrade() {
                            mainloop.quit();
                        }
                    },
                )
            }
        });
        timer
            .update_timer(
                Some(Duration::from_millis(10)),
                Some(Duration::from_millis(10)),
            )
            .into_result()
            .map_err(error)?;
        self.mainloop.run();
        life.result()
    }

    pub fn roundtrip(&self, life: Arc<Life>) -> Result<(), CaptureError> {
        let done = Rc::new(Cell::new(false));
        let sequence = self.core.sync(0).map_err(error)?;
        let listener = self
            .core
            .add_listener_local()
            .done({
                let done = done.clone();
                move |id, received| {
                    super::connection::in_callback("PipeWire connection done", || {
                        if id == pw::core::PW_ID_CORE && received == sequence {
                            done.set(true);
                        }
                    })
                }
            })
            .register();
        let result = self.drive(life, Some(Duration::from_secs(10)), move || done.get());
        drop(listener);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_loss_of_the_connection_ends_the_capture() {
        let (lost, absent) = (-32, -2);
        assert!(ends_the_capture(pw::core::PW_ID_CORE, lost));
        // "unknown resource": a request that crossed the removal of its object.
        assert!(!ends_the_capture(pw::core::PW_ID_CORE, absent));
        // The error of another object, such as a link that the daemon refused.
        assert!(!ends_the_capture(57, absent));
        assert!(!ends_the_capture(57, lost));
        assert!(!ends_the_capture(pw::core::PW_ID_CORE, 0));
        assert!(!ends_the_capture(pw::core::PW_ID_CORE, i32::MIN));
    }
}

thread_local! {
    /// A callback that PipeWire called on this thread of a share panicked.
    static PANICKED: Cell<bool> = const { Cell::new(false) };
}

/// Runs the body of a callback that PipeWire calls on the thread of a share. A panic cannot
/// unwind into PipeWire; after one, the callbacks of the share on this thread do nothing
/// more, and the timer of [`Connection::drive`] ends the share with a stated reason
/// (ADR 0757).
pub(super) fn in_callback(site: &'static str, body: impl FnOnce()) {
    if PANICKED.with(Cell::get) {
        return;
    }
    crate::callback::stop_panic(site, || PANICKED.with(|panicked| panicked.set(true)), body);
}

/// Ends the share when a callback of its thread panicked.
fn end_after_a_panic(life: &Life) {
    if PANICKED.with(Cell::get) {
        life.fail(error("a PipeWire callback of the share panicked"));
    }
}

#[cfg(test)]
mod panic_tests {
    use super::*;

    #[test]
    fn a_panic_in_a_callback_ends_the_share_and_the_later_callbacks_do_nothing() {
        std::thread::spawn(|| {
            let life = Life::default();
            in_callback("test", || panic!("a panic of the share test"));
            let mut ran = false;
            in_callback("test", || ran = true);
            assert!(!ran);
            end_after_a_panic(&life);
            assert!(life.stopped());
            assert!(
                matches!(life.result(), Err(CaptureError::Failed(reason)) if reason.contains("panicked")),
                "{:?}",
                life.result()
            );
        })
        .join()
        .unwrap();
    }
}
