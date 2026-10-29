#![allow(unsafe_code)]

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventSourceSecondsSinceLastEventType(state: i32, event_type: u32) -> f64;
}

pub(super) fn read() -> Option<bool> {
    // SAFETY: the aggregate session clock takes scalar constants and returns no user input.
    let seconds = unsafe { CGEventSourceSecondsSinceLastEventType(0, u32::MAX) };
    super::idle_seconds(seconds)
}
