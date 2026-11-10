//! The camera list of Windows: the cameras of Media Foundation, and then the video input
//! devices of DirectShow that Media Foundation does not list (ADR 0770). The two
//! enumerations run on threads of their own. The DirectShow part has its own time bound, so
//! that a DirectShow enumeration that does not finish delays neither the Media Foundation
//! cameras nor the reads that wait for the list.

use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant};

use super::{CameraDevice, CameraError};

/// A DirectShow device of the list, with its device path when it has one.
pub(super) type Listed = (CameraDevice, Option<String>);

/// The DirectShow enumeration of the process: whether one runs, and the devices of the last
/// one that finished.
pub(super) struct DirectShow {
    running: bool,
    last: Vec<Listed>,
}

impl DirectShow {
    pub(super) const fn new() -> Self {
        Self {
            running: false,
            last: Vec::new(),
        }
    }
}

/// How long a list waits for each enumeration, from its start.
pub(super) struct Bounds {
    pub media_foundation: Duration,
    pub direct_show: Duration,
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(super) const BOUNDS: Bounds = Bounds {
    media_foundation: Duration::from_secs(10),
    direct_show: Duration::from_secs(3),
};

/// The device instance of a device interface path: the path without its interface class and
/// reference string, which differ between the camera category of Media Foundation and the
/// capture category of DirectShow for one device.
fn instance(path: &str) -> &str {
    path.rfind('#').map_or(path, |at| &path[..at])
}

/// The cameras of `listed` and then those of `direct_show` that are not in it: a DirectShow
/// entry with a device path whose device instance is that of a listed camera, compared
/// without case, or with the identifier of a listed camera, is left out.
pub(super) fn merge<T>(
    mut listed: Vec<(CameraDevice, T)>,
    direct_show: Vec<(CameraDevice, Option<String>, T)>,
) -> Vec<(CameraDevice, T)> {
    for (device, path, item) in direct_show {
        let known = listed.iter().any(|(known, _)| {
            known.id == device.id
                || path
                    .as_deref()
                    .is_some_and(|path| instance(&known.id).eq_ignore_ascii_case(instance(path)))
        });
        if !known {
            listed.push((device, item));
        }
    }
    listed
}

fn held(state: &Mutex<DirectShow>) -> std::sync::MutexGuard<'_, DirectShow> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The DirectShow enumeration of a state, taken while one runs. Dropping the claim ends it,
/// also when the enumeration fails or the thread that should run it does not start.
struct Claim(&'static Mutex<DirectShow>);

impl Claim {
    /// The claim of a new enumeration, when none runs.
    fn take(state: &'static Mutex<DirectShow>) -> Option<Self> {
        let mut held = held(state);
        if held.running {
            return None;
        }
        held.running = true;
        Some(Self(state))
    }

    /// Keeps the devices of a finished enumeration as the last ones.
    fn keep(&self, listed: Vec<Listed>) {
        held(self.0).last = listed;
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        held(self.0).running = false;
    }
}

fn last(state: &Mutex<DirectShow>) -> Vec<Listed> {
    held(state).last.clone()
}

/// Runs the DirectShow enumeration of a capture start under the claim of `state`, and keeps
/// the devices that it lists as the last ones. While another enumeration runs, the start does
/// not wait for it and does not start one: it fails at once, so that a DirectShow enumeration
/// that does not end holds one thread for all lists and starts.
pub(super) fn claimed<D>(
    state: &'static Mutex<DirectShow>,
    enumerate: impl FnOnce() -> Result<Vec<D>, CameraError>,
    listed: impl Fn(&D) -> Listed,
) -> Result<Vec<D>, CameraError> {
    let claim = Claim::take(state).ok_or_else(|| {
        CameraError::Failed("an earlier list of the DirectShow cameras still runs".into())
    })?;
    let devices = enumerate()?;
    claim.keep(devices.iter().map(listed).collect());
    Ok(devices)
}

/// The camera list: the cameras of `media_foundation`, then the devices of `direct_show`
/// that are not among them (`merge`).
///
/// Each enumeration runs on a new thread. The list waits for the Media Foundation cameras up
/// to `bounds.media_foundation`; their failure, or no answer in that time, is the failure of
/// the list. It waits for the DirectShow devices up to `bounds.direct_show` from its start,
/// and at least until the Media Foundation answer. A failed DirectShow enumeration adds no
/// device. A DirectShow enumeration that did not answer in time, or that still runs from an
/// earlier list, adds the devices of the last one that finished; while one runs, no other one
/// starts, so a DirectShow enumeration that does not end holds one thread only.
pub(super) fn devices(
    media_foundation: impl FnOnce() -> Result<Vec<CameraDevice>, CameraError> + Send + 'static,
    direct_show: impl FnOnce() -> Result<Vec<Listed>, CameraError> + Send + 'static,
    state: &'static Mutex<DirectShow>,
    bounds: &Bounds,
) -> Result<Vec<CameraDevice>, CameraError> {
    let start = Instant::now();
    let direct_show_answer = if let Some(claim) = Claim::take(state) {
        let (send, receive) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("camera-devices-direct-show".into())
            .spawn(move || {
                let listed = direct_show();
                if let Ok(listed) = &listed {
                    claim.keep(listed.clone());
                }
                drop(claim);
                let _ = send.send(listed);
            });
        match spawned {
            Ok(_) => Some(receive),
            Err(error) => {
                tracing::warn!(%error, "the DirectShow cameras could not be listed");
                None
            }
        }
    } else {
        tracing::info!(
            "an earlier list of the DirectShow cameras still runs; its last devices are used"
        );
        None
    };
    let (send, receive) = mpsc::channel();
    std::thread::Builder::new()
        .name("camera-devices".into())
        .spawn(move || {
            let _ = send.send(media_foundation());
        })
        .map_err(|error| CameraError::Failed(error.to_string()))?;
    let listed = receive
        .recv_timeout(bounds.media_foundation)
        .map_err(|error| {
            tracing::error!(%error, "camera enumeration did not finish");
            CameraError::Failed(format!("camera enumeration did not finish: {error}"))
        })??;
    let direct_show = match direct_show_answer {
        None => last(state),
        Some(receive) => {
            let left = bounds.direct_show.saturating_sub(start.elapsed());
            match receive.recv_timeout(left) {
                Ok(Ok(listed)) => listed,
                Ok(Err(error)) => {
                    tracing::warn!(?error, "the DirectShow cameras could not be listed");
                    Vec::new()
                }
                Err(_) => {
                    tracing::warn!(
                        waited_ms = start.elapsed().as_millis() as u64,
                        "the DirectShow cameras were not listed in time; the last devices are used"
                    );
                    last(state)
                }
            }
        }
    };
    Ok(merge(
        listed.into_iter().map(|device| (device, ())).collect(),
        direct_show
            .into_iter()
            .map(|(device, path)| (device, path, ()))
            .collect(),
    )
    .into_iter()
    .map(|(device, ())| device)
    .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn device(id: &str, name: &str) -> CameraDevice {
        CameraDevice {
            id: id.into(),
            name: name.into(),
        }
    }

    const QUICK: Bounds = Bounds {
        media_foundation: Duration::from_secs(5),
        direct_show: Duration::from_millis(100),
    };

    #[test]
    fn a_direct_show_device_of_the_media_foundation_list_is_listed_once() {
        let listed = vec![(
            device(
                r"\\?\usb#vid_046d&pid_085e&mi_00#7&1a2b3c&0&0000#{e5323777-f976-4f5b-9b55-b94699c46e44}\global",
                "Webcam",
            ),
            "media foundation",
        )];
        let direct_show = vec![
            (
                device(
                    r"\\?\USB#VID_046D&PID_085E&MI_00#7&1A2B3C&0&0000#{65e8773d-8f56-11d0-a3b9-00a0c9223196}\global",
                    "Webcam",
                ),
                Some(r"\\?\USB#VID_046D&PID_085E&MI_00#7&1A2B3C&0&0000#{65e8773d-8f56-11d0-a3b9-00a0c9223196}\global".to_owned()),
                "direct show webcam",
            ),
            (
                device(
                    r"@device:sw:{860BB310-5D01-11D0-BD3B-00A0C911CE86}\{A3FCE0F5-3493-419F-958A-ABA1250EC20B}",
                    "OBS Virtual Camera",
                ),
                None,
                "direct show virtual camera",
            ),
            (
                device(
                    r"@device:sw:{860BB310-5D01-11D0-BD3B-00A0C911CE86}\{A3FCE0F5-3493-419F-958A-ABA1250EC20B}",
                    "OBS Virtual Camera",
                ),
                None,
                "the same filter listed again",
            ),
        ];
        let merged: Vec<_> = merge(listed, direct_show)
            .into_iter()
            .map(|(device, from)| (device.name, from))
            .collect();
        assert_eq!(
            merged,
            [
                ("Webcam".to_owned(), "media foundation"),
                (
                    "OBS Virtual Camera".to_owned(),
                    "direct show virtual camera"
                ),
            ]
        );
    }

    #[test]
    fn the_list_holds_the_media_foundation_cameras_and_then_the_direct_show_devices() {
        static STATE: Mutex<DirectShow> = Mutex::new(DirectShow::new());
        let listed = devices(
            || Ok(vec![device("webcam", "Webcam")]),
            || {
                Ok(vec![
                    (device("webcam", "Webcam"), None),
                    (device("filter", "Virtual camera"), None),
                ])
            },
            &STATE,
            &QUICK,
        )
        .unwrap();
        assert_eq!(
            listed,
            [
                device("webcam", "Webcam"),
                device("filter", "Virtual camera")
            ]
        );
        // A failed DirectShow enumeration leaves the Media Foundation cameras.
        let listed = devices(
            || Ok(vec![device("webcam", "Webcam")]),
            || Err(CameraError::Failed("no enumerator".into())),
            &STATE,
            &QUICK,
        )
        .unwrap();
        assert_eq!(listed, [device("webcam", "Webcam")]);
        // A failed Media Foundation enumeration fails the list.
        assert_eq!(
            devices(
                || Err(CameraError::Unsupported),
                || Ok(Vec::new()),
                &STATE,
                &QUICK,
            ),
            Err(CameraError::Unsupported)
        );
    }

    #[test]
    fn a_slow_direct_show_enumeration_delays_neither_the_list_nor_starts_again() {
        static STATE: Mutex<DirectShow> = Mutex::new(DirectShow::new());
        static STARTED: AtomicUsize = AtomicUsize::new(0);
        let (finish, finished) = mpsc::channel::<()>();
        let slow = move || {
            STARTED.fetch_add(1, Ordering::SeqCst);
            let _ = finished.recv();
            Ok(vec![(device("filter", "Virtual camera"), None)])
        };
        let start = Instant::now();
        let listed = devices(
            || Ok(vec![device("webcam", "Webcam")]),
            slow,
            &STATE,
            &QUICK,
        )
        .unwrap();
        assert_eq!(listed, [device("webcam", "Webcam")]);
        assert!(start.elapsed() < Duration::from_secs(2));
        // While the first enumeration runs, a second list starts none.
        let listed = devices(
            || Ok(vec![device("webcam", "Webcam")]),
            || {
                STARTED.fetch_add(1, Ordering::SeqCst);
                Ok(Vec::new())
            },
            &STATE,
            &QUICK,
        )
        .unwrap();
        assert_eq!(listed, [device("webcam", "Webcam")]);
        assert_eq!(STARTED.load(Ordering::SeqCst), 1);
        // Once it ends, its devices are the last ones, and the next list starts one again.
        finish.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while STATE.lock().unwrap().running {
            assert!(Instant::now() < deadline, "the enumeration did not end");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(last(&STATE), [(device("filter", "Virtual camera"), None)]);
        let listed = devices(
            || Ok(Vec::new()),
            || {
                STARTED.fetch_add(1, Ordering::SeqCst);
                Ok(vec![(device("filter", "Virtual camera"), None)])
            },
            &STATE,
            &QUICK,
        )
        .unwrap();
        assert_eq!(listed, [device("filter", "Virtual camera")]);
        assert_eq!(STARTED.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_capture_start_does_not_enumerate_while_another_enumeration_runs() {
        static STATE: Mutex<DirectShow> = Mutex::new(DirectShow::new());
        let held = Claim::take(&STATE).unwrap();
        let mut ran = false;
        let refused = claimed(
            &STATE,
            || {
                ran = true;
                Ok(vec![device("filter", "Virtual camera")])
            },
            |device| (device.clone(), None),
        );
        assert!(matches!(refused, Err(CameraError::Failed(_))));
        assert!(!ran);
        drop(held);
        let devices = claimed(
            &STATE,
            || Ok(vec![device("filter", "Virtual camera")]),
            |device| (device.clone(), None),
        )
        .unwrap();
        assert_eq!(devices, [device("filter", "Virtual camera")]);
        assert_eq!(last(&STATE), [(device("filter", "Virtual camera"), None)]);
        assert!(!STATE.lock().unwrap().running);
        // A failed enumeration ends the claim too.
        assert!(
            claimed(
                &STATE,
                || Err::<Vec<CameraDevice>, _>(CameraError::Unsupported),
                |device| (device.clone(), None)
            )
            .is_err()
        );
        assert!(!STATE.lock().unwrap().running);
    }

    #[test]
    fn a_media_foundation_enumeration_without_an_answer_fails_the_list() {
        static STATE: Mutex<DirectShow> = Mutex::new(DirectShow::new());
        let bounds = Bounds {
            media_foundation: Duration::from_millis(50),
            direct_show: Duration::from_millis(50),
        };
        let result = devices(
            || {
                std::thread::sleep(Duration::from_millis(500));
                Ok(Vec::new())
            },
            || Ok(Vec::new()),
            &STATE,
            &bounds,
        );
        assert!(
            matches!(&result, Err(CameraError::Failed(message)) if message.starts_with("camera enumeration did not finish")),
            "{result:?}"
        );
    }
}
