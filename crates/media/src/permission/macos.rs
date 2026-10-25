//! The microphone permission of macOS, read and asked for through AVFoundation.

use std::sync::Mutex;

use block2::RcBlock;
use objc2::runtime::Bool;
use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};
use tokio::sync::oneshot;

use super::MicrophoneAccess;

pub(super) fn state() -> MicrophoneAccess {
    // SAFETY: the constant is a static string of the framework, and the method reads the
    // state of this process without a side effect.
    let status = unsafe {
        let Some(audio) = AVMediaTypeAudio else {
            return MicrophoneAccess::Refused;
        };
        AVCaptureDevice::authorizationStatusForMediaType(audio)
    };
    if status == AVAuthorizationStatus::Authorized {
        MicrophoneAccess::Allowed
    } else if status == AVAuthorizationStatus::NotDetermined {
        MicrophoneAccess::NotAsked
    } else {
        MicrophoneAccess::Refused
    }
}

/// Makes the system ask. The answer arrives on `answered`.
fn ask(answered: oneshot::Sender<bool>) -> bool {
    let answered = Mutex::new(Some(answered));
    let handler = RcBlock::new(move |granted: Bool| {
        // A panic cannot unwind into the system that calls the block (ADR 0757).
        crate::callback::stop_panic(
            "microphone access answer",
            || (),
            || {
                if let Ok(mut answered) = answered.lock()
                    && let Some(answered) = answered.take()
                {
                    let _ = answered.send(granted.as_bool());
                }
            },
        )
    });
    // SAFETY: the constant is a static string of the framework. The system copies the
    // block and calls it once on a queue of its own; the block owns what it uses.
    unsafe {
        let Some(audio) = AVMediaTypeAudio else {
            return false;
        };
        AVCaptureDevice::requestAccessForMediaType_completionHandler(audio, &handler);
    }
    true
}

pub(super) async fn request() -> MicrophoneAccess {
    if state() != MicrophoneAccess::NotAsked {
        return state();
    }
    let (answered, answer) = oneshot::channel::<bool>();
    if !ask(answered) {
        return MicrophoneAccess::Refused;
    }
    match answer.await {
        Ok(true) => MicrophoneAccess::Allowed,
        Ok(false) => MicrophoneAccess::Refused,
        Err(_) => state(),
    }
}
