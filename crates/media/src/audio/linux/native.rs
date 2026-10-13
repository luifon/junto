//! Initialise the native library before any PipeWire entry point is used.

use std::sync::OnceLock;

unsafe extern "C" {
    fn junto_initialize_pipewire() -> bool;
}

pub(super) fn initialise() -> bool {
    static READY: OnceLock<bool> = OnceLock::new();
    *READY.get_or_init(|| {
        // SAFETY: the pinned native bridge takes no arguments, retains no Rust memory,
        // and reports whether the symbols behind WebRTC's PipeWire stubs were loaded.
        unsafe { junto_initialize_pipewire() }
    })
}
