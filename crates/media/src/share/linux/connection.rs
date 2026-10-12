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

impl Connection {
    pub fn new(fd: Option<OwnedFd>, life: Arc<Life>) -> Result<Self, CaptureError> {
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
            .error(move |_, _, _, message| life.fail(error(message)))
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
                if !life.stopped() && !done() {
                    if timeout.is_none_or(|limit| started.elapsed() < limit) {
                        return;
                    }
                    life.fail(error("PipeWire did not finish preparing the capture"));
                }
                if let Some(mainloop) = mainloop.upgrade() {
                    mainloop.quit();
                }
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
                    if id == pw::core::PW_ID_CORE && received == sequence {
                        done.set(true);
                    }
                }
            })
            .register();
        let result = self.drive(life, Some(Duration::from_secs(10)), move || done.get());
        drop(listener);
        result
    }
}
