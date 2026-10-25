//! Handles COM initialization and cleanup.

// Modified: this file differs from the published cpal 0.18.2. `com_initialized` makes no
// thread enter an apartment of COM. The published version entered the single-threaded
// apartment on each calling thread, also on threads that cpal does not own, such as the
// threads of the system pool on which its device notifications and the background tasks of
// a window run, and left it only when the thread ended. Code that later ran on such a thread
// and needed the multithreaded apartment failed with RPC_E_CHANGED_MODE.

use std::sync::OnceLock;

use windows::Win32::System::Com::{CoIncrementMTAUsage, CoTaskMemFree};

/// RAII wrapper for COM-allocated wide strings freed with `CoTaskMemFree`.
pub(super) struct ComString(pub windows::core::PWSTR);

impl Drop for ComString {
    fn drop(&mut self) {
        unsafe { CoTaskMemFree(Some(self.0.as_ptr() as *mut _)) }
    }
}

/// Ensures that this thread can use COM. A thread that has entered no apartment uses the
/// multithreaded apartment of the process, which is kept for the life of the process, so that
/// the objects made in it stay usable after the thread that made them has ended. A thread that
/// has entered an apartment keeps it.
pub fn com_initialized() {
    // Modified: the published version entered the single-threaded apartment here.
    static KEPT: OnceLock<()> = OnceLock::new();
    KEPT.get_or_init(|| {
        // The cookie is never given back. A failure leaves COM as it was; the calls that
        // follow then report it.
        let _ = unsafe { CoIncrementMTAUsage() };
    });
}
