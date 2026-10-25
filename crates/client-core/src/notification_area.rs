//! The icon of the application in the notification area of Windows, with its menu
//! (ADR 0759). A hidden window of the thread receives the messages of the icon; the message
//! loop of the thread, which the window library runs, gives them to its window procedure.
//! One icon exists per thread.

#![allow(unsafe_code)]

use std::cell::RefCell;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION,
    NIN_SELECT, NINF_KEY, NOTIFY_ICON_MESSAGE, NOTIFYICON_VERSION_4, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIcon, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyIcon,
    DestroyMenu, DestroyWindow, GetSystemMetrics, HICON, MF_GRAYED, MF_SEPARATOR, MF_STRING,
    PostMessageW, RegisterClassW, RegisterWindowMessageW, SM_CXSMICON, SetForegroundWindow,
    TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, WINDOW_EX_STYLE, WINDOW_STYLE,
    WM_APP, WM_CONTEXTMENU, WM_NULL, WNDCLASSW,
};
use windows::core::{PCWSTR, w};

/// One entry of the menu.
pub struct Entry {
    pub label: String,
    /// The number that a press of the entry gives to the callback of the icon.
    pub code: u32,
    pub enabled: bool,
    /// A separator line comes before the entry.
    pub separated: bool,
}

/// What the person did with the icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Press {
    /// The entry with this code.
    Entry(u32),
    /// The primary press on the icon, or the key that selects it.
    Icon,
}

/// The message with which the shell reports an event of the icon to its window.
const CALLBACK: u32 = WM_APP + 1;
/// The identifier of the icon among the icons of its window.
const ICON_ID: u32 = 1;
/// The selection of the icon with the keyboard, with version 4 of the icon messages.
const NIN_KEYSELECT: u32 = NIN_SELECT | NINF_KEY;
/// The longest tooltip of the shell, in UTF-16 units without the end mark.
const TIP: usize = 127;

struct Shown {
    pressed: Box<dyn Fn(Press)>,
    entries: Vec<Entry>,
    icon: Option<HICON>,
    tooltip: Vec<u16>,
    /// The shell holds the icon.
    added: bool,
    /// The icon was taken away for the end of the application, and is not shown again.
    removed: bool,
    /// The message that a new shell (Explorer after a restart) sends to every top-level
    /// window, after which the icons are added again.
    shell_started: u32,
}

thread_local! {
    static SHOWN: RefCell<Option<Shown>> = const { RefCell::new(None) };
}

fn error(error: windows::core::Error) -> std::io::Error {
    std::io::Error::other(error.message())
}

/// The icon in the notification area. The icon and its window go when it is dropped.
pub struct NotificationIcon {
    window: HWND,
}

impl NotificationIcon {
    /// A hidden window for the messages of the icon, on this thread. The icon is shown at the
    /// first `set`.
    pub fn new(pressed: Box<dyn Fn(Press)>) -> std::io::Result<Self> {
        if SHOWN.with_borrow(Option::is_some) {
            return Err(std::io::Error::other(
                "this thread has an icon in the notification area",
            ));
        }
        let class_name = w!("JuntoNotificationIcon");
        // SAFETY: the calls get valid arguments; the class names a window procedure of this
        // module, and a second registration of the class fails without an effect.
        let window = unsafe {
            let instance = GetModuleHandleW(PCWSTR::null()).map_err(error)?;
            let class = WNDCLASSW {
                lpfnWndProc: Some(procedure),
                hInstance: instance.into(),
                lpszClassName: class_name,
                ..WNDCLASSW::default()
            };
            let _ = RegisterClassW(&class);
            // A top-level window that is never shown: a window for messages only would not
            // receive the message of a new shell, which goes to the top-level windows.
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class_name,
                w!("Junto"),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance.into()),
                None,
            )
            .map_err(error)?
        };
        // SAFETY: a constant string with its end mark.
        let shell_started = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
        SHOWN.set(Some(Shown {
            pressed,
            entries: Vec::new(),
            icon: None,
            tooltip: Vec::new(),
            added: false,
            removed: false,
            shell_started,
        }));
        Ok(Self { window })
    }

    /// Shows the picture of `side` by `side` pixels with straight RGBA pixels, the tooltip, and
    /// the entries of the menu.
    pub fn set(
        &self,
        rgba: &[u8],
        side: u32,
        tooltip: &str,
        entries: Vec<Entry>,
    ) -> std::io::Result<()> {
        let icon = icon(rgba, side)?;
        let tooltip: Vec<u16> = tooltip.encode_utf16().take(TIP).collect();
        let replaced = SHOWN.with_borrow_mut(|shown| {
            let shown = shown.as_mut()?;
            shown.entries = entries;
            shown.tooltip = tooltip;
            shown.icon.replace(icon)
        });
        let shown = show(self.window);
        if let Some(replaced) = replaced {
            // SAFETY: the shell copied the new icon; nothing uses the replaced one.
            let _ = unsafe { DestroyIcon(replaced) };
        }
        shown
    }
}

impl NotificationIcon {
    /// Takes the icon away from the notification area. The shell keeps the icon of a process
    /// that ended until the pointer passes over it; the application calls this before it ends.
    pub fn remove(&self) {
        let data = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.window,
            uID: ICON_ID,
            ..NOTIFYICONDATAW::default()
        };
        // SAFETY: the data is valid during the call.
        let _ = unsafe { Shell_NotifyIconW(NIM_DELETE, &data) };
        SHOWN.with_borrow_mut(|shown| {
            if let Some(shown) = shown {
                shown.added = false;
                shown.removed = true;
            }
        });
    }
}

impl Drop for NotificationIcon {
    fn drop(&mut self) {
        self.remove();
        let shown = SHOWN.take();
        // SAFETY: the window and the icon belong to this value and are destroyed once.
        unsafe {
            let _ = DestroyWindow(self.window);
            if let Some(icon) = shown.and_then(|shown| shown.icon) {
                let _ = DestroyIcon(icon);
            }
        }
    }
}

/// The side of the small icons of the system in pixels, at the scale of the system: 16 at
/// 100 %.
pub fn small_icon_side() -> u32 {
    // SAFETY: a read of a value of the system.
    let side = unsafe { GetSystemMetrics(SM_CXSMICON) };
    u32::try_from(side).unwrap_or(16).max(16)
}

/// An icon of the system from straight RGBA pixels.
fn icon(rgba: &[u8], side: u32) -> std::io::Result<HICON> {
    let pixels = (side * side) as usize;
    if rgba.len() != pixels * 4 {
        return Err(std::io::Error::other("the picture has another size"));
    }
    let side = i32::try_from(side).map_err(std::io::Error::other)?;
    // Blue, green, red and alpha; the alpha of a picture of 32 bits decides the transparency,
    // and the mask of one bit per pixel, with rows of whole 16-bit words, is empty.
    let bgra: Vec<u8> = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|&[red, green, blue, alpha]| [blue, green, red, alpha])
        .collect();
    let mask = vec![0_u8; (side as usize).div_ceil(16) * 2 * side as usize];
    // SAFETY: both buffers hold the bytes that the side and the bits per pixel name, and the
    // function copies them.
    unsafe { CreateIcon(None, side, side, 1, 32, mask.as_ptr(), bgra.as_ptr()) }.map_err(error)
}

/// Gives the shell the icon of now: adds it when the shell does not hold it.
fn show(window: HWND) -> std::io::Result<()> {
    let Some((data, added)) = SHOWN.with_borrow(|shown| {
        shown
            .as_ref()
            .filter(|shown| !shown.removed)
            .map(|shown| (data(window, shown), shown.added))
    }) else {
        return Ok(());
    };
    let message: NOTIFY_ICON_MESSAGE = if added { NIM_MODIFY } else { NIM_ADD };
    // SAFETY: the data is complete and valid during the call; the shell copies it.
    let done = unsafe {
        Shell_NotifyIconW(message, &data).as_bool()
            && (added || Shell_NotifyIconW(NIM_SETVERSION, &data).as_bool())
    };
    SHOWN.with_borrow_mut(|shown| {
        if let Some(shown) = shown {
            shown.added = done;
        }
    });
    if done {
        Ok(())
    } else {
        Err(std::io::Error::other(
            "the shell did not take the icon of the notification area",
        ))
    }
}

fn data(window: HWND, shown: &Shown) -> NOTIFYICONDATAW {
    let mut data = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: window,
        uID: ICON_ID,
        uFlags: NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP,
        uCallbackMessage: CALLBACK,
        ..NOTIFYICONDATAW::default()
    };
    if let Some(icon) = shown.icon {
        data.hIcon = icon;
        data.uFlags |= NIF_ICON;
    }
    for (slot, unit) in data.szTip.iter_mut().zip(&shown.tooltip) {
        *slot = *unit;
    }
    data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
    data
}

fn press(press: Press) {
    SHOWN.with_borrow(|shown| {
        if let Some(shown) = shown {
            (shown.pressed)(press);
        }
    });
}

/// Shows the menu at a point of the screen and reports the chosen entry.
fn menu(window: HWND, x: i32, y: i32) {
    let entries: Vec<(Vec<u16>, u32, bool, bool)> = SHOWN.with_borrow(|shown| {
        shown.as_ref().map_or_else(Vec::new, |shown| {
            shown
                .entries
                .iter()
                .map(|entry| {
                    let label = entry.label.encode_utf16().chain([0]).collect();
                    (label, entry.code, entry.enabled, entry.separated)
                })
                .collect()
        })
    });
    // SAFETY: the menu is created, shown and destroyed here; each label lives during its
    // call, and the menu copies it.
    let chosen = unsafe {
        let Ok(popup) = CreatePopupMenu() else {
            return;
        };
        for (label, code, enabled, separated) in &entries {
            if *separated {
                let _ = AppendMenuW(popup, MF_SEPARATOR, 0, PCWSTR::null());
            }
            let flags = if *enabled {
                MF_STRING
            } else {
                MF_STRING | MF_GRAYED
            };
            let _ = AppendMenuW(popup, flags, *code as usize, PCWSTR(label.as_ptr()));
        }
        // The menu closes at a press outside it only when its window is the foreground
        // window, and the message after it ends the menu loop of the shell.
        let _ = SetForegroundWindow(window);
        let chosen = TrackPopupMenu(
            popup,
            TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
            x,
            y,
            None,
            window,
            None,
        );
        let _ = PostMessageW(Some(window), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(popup);
        chosen.0
    };
    if let Ok(code) = u32::try_from(chosen)
        && code != 0
    {
        press(Press::Entry(code));
    }
}

/// The low and the high signed 16 bits of a parameter: a point of the screen.
fn point(value: usize) -> (i32, i32) {
    (
        i32::from(value as u16 as i16),
        i32::from((value >> 16) as u16 as i16),
    )
}

/// SAFETY: the system calls it with the arguments of a message of the window.
unsafe extern "system" fn procedure(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == CALLBACK {
        // Version 4: the low word of `lparam` is the event, and `wparam` the point of a menu.
        match (lparam.0 as u32) & 0xffff {
            WM_CONTEXTMENU => {
                let (x, y) = point(wparam.0);
                menu(window, x, y);
            }
            NIN_SELECT | NIN_KEYSELECT => press(Press::Icon),
            _ => {}
        }
        return LRESULT(0);
    }
    let shell_started = SHOWN.with_borrow(|shown| shown.as_ref().map(|shown| shown.shell_started));
    if shell_started == Some(message) {
        SHOWN.with_borrow_mut(|shown| {
            if let Some(shown) = shown {
                shown.added = false;
            }
        });
        if let Err(error) = show(window) {
            tracing::warn!(%error, "the icon was not added to the new notification area");
        }
        return LRESULT(0);
    }
    // SAFETY: the arguments of the system, given back unchanged.
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_icon_is_made_from_rgba_and_a_wrong_size_is_refused() {
        let rgba = vec![200_u8; 16 * 16 * 4];
        let made = icon(&rgba, 16).unwrap();
        unsafe { DestroyIcon(made) }.unwrap();
        assert!(icon(&rgba, 15).is_err());
    }

    #[test]
    fn a_menu_point_is_two_signed_words() {
        assert_eq!(point(0x0010_0020), (32, 16));
        assert_eq!(point(0xfff0_fffe), (-2, -16));
    }

    #[test]
    fn an_icon_taken_away_for_the_end_is_not_shown_again() {
        let icon = NotificationIcon::new(Box::new(|_| {})).unwrap();
        icon.remove();
        icon.set(&vec![0_u8; 16 * 16 * 4], 16, "Junto", Vec::new())
            .unwrap();
        assert!(SHOWN.with_borrow(|shown| {
            shown
                .as_ref()
                .is_some_and(|shown| shown.removed && !shown.added && shown.icon.is_some())
        }));
    }

    #[test]
    fn one_icon_per_thread_and_it_goes_with_its_value() {
        let first = NotificationIcon::new(Box::new(|_| {})).unwrap();
        assert!(NotificationIcon::new(Box::new(|_| {})).is_err());
        drop(first);
        assert!(SHOWN.with_borrow(Option::is_none));
        let second = NotificationIcon::new(Box::new(|_| {})).unwrap();
        drop(second);
    }
}
