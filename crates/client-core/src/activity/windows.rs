#![allow(unsafe_code)]
use windows_sys::Win32::{
    System::SystemInformation::GetTickCount,
    UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO},
};

pub(super) fn read() -> Option<std::time::Duration> {
    let mut info = LASTINPUTINFO {
        cbSize: u32::try_from(std::mem::size_of::<LASTINPUTINFO>()).ok()?,
        dwTime: 0,
    };
    // SAFETY: the initialized structure has its exact size and a valid writable address.
    if unsafe { GetLastInputInfo(&mut info) } == 0 {
        return None;
    }
    // SAFETY: GetTickCount takes no pointers and reads the system's millisecond counter.
    let now = unsafe { GetTickCount() };
    Some(super::tick_elapsed(info.dwTime, now))
}
