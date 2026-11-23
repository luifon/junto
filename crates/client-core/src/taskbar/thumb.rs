//! The buttons of the taskbar preview of the main window during a call (ADR 0755): the
//! microphone, the sound and leaving the call, with the state of the controls of the window.
//! Windows draws them under the preview of the window and sends a press to the window as a
//! command. The buttons are added once to a taskbar button and then only changed: a hidden
//! button stands for no call.

use std::collections::HashMap;
use std::sync::mpsc::{Sender, channel};
use std::sync::{Mutex, OnceLock, PoisonError};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{CreateBitmap, DeleteObject};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::UI::Shell::{
    DefSubclassProc, ITaskbarList3, RemoveWindowSubclass, SetWindowSubclass, THB_FLAGS, THB_ICON,
    THB_TOOLTIP, THBF_DISABLED, THBF_ENABLED, THBF_HIDDEN, THBN_CLICKED, THUMBBUTTON, TaskbarList,
};
use windows::Win32::UI::WindowsAndMessaging::{
    ChangeWindowMessageFilterEx, CreateIconIndirect, GetSystemMetrics, HICON, ICONINFO,
    MSGFLT_ALLOW, RegisterWindowMessageW, SM_CXSMICON, WM_COMMAND, WM_NCDESTROY,
};
use windows::core::w;

include!(concat!(env!("OUT_DIR"), "/thumb_glyphs.rs"));

/// A press on a button of the preview.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThumbPress {
    Microphone,
    Sound,
    Leave,
}

impl ThumbPress {
    const ALL: [ThumbPress; 3] = [ThumbPress::Microphone, ThumbPress::Sound, ThumbPress::Leave];

    /// The command identifier of the button. Windows sends it with the press.
    fn id(self) -> u32 {
        match self {
            ThumbPress::Microphone => 1,
            ThumbPress::Sound => 2,
            ThumbPress::Leave => 3,
        }
    }

    fn of(id: u32) -> Option<ThumbPress> {
        ThumbPress::ALL.into_iter().find(|press| press.id() == id)
    }
}

/// What the buttons show in a call: the state of the controls of the window, and the
/// tooltip of each button in the order microphone, sound, leave.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThumbButtons {
    pub microphone_open: bool,
    /// The person can use the microphone control: the server did not close it, and the
    /// person may speak.
    pub microphone_usable: bool,
    pub sound_off: bool,
    pub tips: [String; 3],
}

impl ThumbButtons {
    fn glyph(&self, press: ThumbPress) -> &'static str {
        match press {
            ThumbPress::Microphone if self.microphone_open => "microphone",
            ThumbPress::Microphone => "microphone-off",
            ThumbPress::Sound if self.sound_off => "sound-off",
            ThumbPress::Sound => "sound",
            ThumbPress::Leave => "leave",
        }
    }
}

/// The context of the subclass of the main window.
struct Watch {
    /// The message that Explorer sends when it made the taskbar button of the window.
    created: u32,
    window: isize,
    pressed: Box<dyn Fn(ThumbPress)>,
}

const SUBCLASS: usize = 0x6a75_6e74;

/// Takes the presses on the buttons of the preview of the main window `window` (its `HWND`)
/// and gives them to `pressed`, on the thread of the window. It also adds the buttons again
/// when Explorer makes the taskbar button again, as after its restart. Call it on the thread
/// that made the window.
pub fn watch(window: isize, pressed: Box<dyn Fn(ThumbPress)>) -> std::io::Result<()> {
    let hwnd = HWND(window as *mut core::ffi::c_void);
    // SAFETY: the string is a constant with its terminating zero.
    let created = unsafe { RegisterWindowMessageW(w!("TaskbarButtonCreated")) };
    if created == 0 {
        return Err(std::io::Error::last_os_error());
    }
    // An elevated process takes the message of Explorer only when it allows it. The call
    // changes nothing for a process that is not elevated.
    // SAFETY: the handle names a window of this process; no filter structure is passed.
    let _ = unsafe { ChangeWindowMessageFilterEx(hwnd, created, MSGFLT_ALLOW, None) };
    let watch = Box::into_raw(Box::new(Watch {
        created,
        window,
        pressed,
    }));
    // SAFETY: the procedure lives for the process; the context is a box that the procedure
    // frees when the window is destroyed (`WM_NCDESTROY`), and that is freed here when the
    // subclass was not installed.
    let installed = unsafe { SetWindowSubclass(hwnd, Some(subclass), SUBCLASS, watch as usize) };
    if !installed.as_bool() {
        // SAFETY: the box was not given to Windows.
        drop(unsafe { Box::from_raw(watch) });
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// The subclass procedure of the main window: a press on a button of the preview, the new
/// taskbar button of Explorer, and the end of the window.
unsafe extern "system" fn subclass(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _: usize,
    context: usize,
) -> LRESULT {
    // SAFETY: `context` is the box of `watch`, which lives until `WM_NCDESTROY` below.
    let watch = unsafe { &*(context as *const Watch) };
    if message == WM_COMMAND
        && (wparam.0 >> 16) as u32 & 0xffff == THBN_CLICKED
        && let Some(press) = ThumbPress::of((wparam.0 & 0xffff) as u32)
    {
        (watch.pressed)(press);
        return LRESULT(0);
    } else if message == watch.created {
        send(Job::Created(watch.window));
    } else if message == WM_NCDESTROY {
        send(Job::Gone(watch.window));
        // SAFETY: the subclass is removed before its context is freed; the window gets no
        // further message through it.
        unsafe {
            let _ = RemoveWindowSubclass(hwnd, Some(subclass), SUBCLASS);
            drop(Box::from_raw(context as *mut Watch));
        }
    }
    // SAFETY: the message goes on to the procedure of gpui with its own arguments.
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

/// Shows the buttons of a call on the preview of the window `window`, or hides them outside
/// a call. The calls of COM run on a thread of their own, as the count of the taskbar does
/// (ADR 0755).
pub fn show(window: isize, buttons: Option<ThumbButtons>) {
    send(Job::Show(window, buttons));
}

enum Job {
    Show(isize, Option<ThumbButtons>),
    /// Explorer made the taskbar button of the window: its buttons are added again.
    Created(isize),
    Gone(isize),
}

fn send(job: Job) {
    static WORKER: OnceLock<Option<Mutex<Sender<Job>>>> = OnceLock::new();
    let worker = WORKER.get_or_init(|| {
        let (sender, jobs) = channel::<Job>();
        let started = super::on_apartment_thread("junto-taskbar-buttons", move || {
            let mut toolbar = Toolbar::default();
            while let Ok(job) = jobs.recv() {
                toolbar.take(job);
            }
        });
        match started {
            Ok(()) => Some(Mutex::new(sender)),
            Err(error) => {
                tracing::warn!(%error, "the thread of the taskbar buttons did not start");
                None
            }
        }
    });
    if let Some(sender) = worker {
        let _ = sender
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .send(job);
    }
}

/// The state of the buttons of each window, on the thread of the taskbar.
#[derive(Default)]
struct Toolbar {
    list: Option<ITaskbarList3>,
    /// The buttons are on the taskbar button of the window, and what they show.
    windows: HashMap<isize, (bool, Option<ThumbButtons>)>,
    /// The icons of the glyphs, by glyph and shade. They live as long as the process: the
    /// taskbar may draw an icon after the call that gave it.
    icons: HashMap<(&'static str, bool), HICON>,
}

impl Toolbar {
    fn take(&mut self, job: Job) {
        let (window, retry) = match job {
            Job::Show(window, buttons) => {
                let entry = self.windows.entry(window).or_insert((false, None));
                if entry.0 && entry.1 == buttons {
                    return;
                }
                entry.1 = buttons;
                (window, false)
            }
            Job::Created(window) => {
                self.windows.entry(window).or_insert((false, None)).0 = false;
                (window, true)
            }
            Job::Gone(window) => {
                self.windows.remove(&window);
                return;
            }
        };
        if let Err(error) = self.apply(window) {
            // Before Explorer made the taskbar button the buttons cannot be added; its
            // message adds them.
            if retry {
                tracing::warn!(%error, "the buttons of the taskbar preview were not set");
            } else {
                tracing::debug!(%error, "the buttons of the taskbar preview wait for the button");
            }
        }
    }

    fn apply(&mut self, window: isize) -> windows::core::Result<()> {
        let Some((added, buttons)) = self.windows.get(&window).cloned() else {
            return Ok(());
        };
        if !added && buttons.is_none() {
            return Ok(());
        }
        let light = light_taskbar();
        let mut rows = Vec::with_capacity(3);
        for press in ThumbPress::ALL {
            let mut row = THUMBBUTTON {
                dwMask: THB_ICON | THB_TOOLTIP | THB_FLAGS,
                iId: press.id(),
                dwFlags: THBF_HIDDEN,
                ..Default::default()
            };
            if let Some(buttons) = &buttons {
                let index = ThumbPress::ALL
                    .iter()
                    .position(|other| *other == press)
                    .unwrap_or(0);
                row.hIcon = self.icon(buttons.glyph(press), light)?;
                tip(&mut row.szTip, &buttons.tips[index]);
                row.dwFlags = if press == ThumbPress::Microphone && !buttons.microphone_usable {
                    THBF_DISABLED
                } else {
                    THBF_ENABLED
                };
            }
            rows.push(row);
        }
        let list = self.list()?;
        let hwnd = HWND(window as *mut core::ffi::c_void);
        // SAFETY: the rows are valid for the call; the icons live as long as the process.
        unsafe {
            if added {
                list.ThumbBarUpdateButtons(hwnd, &rows)?;
            } else {
                list.ThumbBarAddButtons(hwnd, &rows)?;
            }
        }
        if let Some(entry) = self.windows.get_mut(&window) {
            entry.0 = true;
        }
        Ok(())
    }

    fn list(&mut self) -> windows::core::Result<ITaskbarList3> {
        if let Some(list) = &self.list {
            return Ok(list.clone());
        }
        // SAFETY: the class is the system taskbar list; the thread is in an apartment of COM.
        let list: ITaskbarList3 = unsafe {
            CoCreateInstance(
                &TaskbarList,
                None::<&windows::core::IUnknown>,
                CLSCTX_INPROC_SERVER,
            )?
        };
        // SAFETY: the list is initialised before its first use, as its documentation asks.
        unsafe { list.HrInit()? };
        self.list = Some(list.clone());
        Ok(list)
    }

    fn icon(&mut self, glyph: &'static str, light: bool) -> windows::core::Result<HICON> {
        if let Some(icon) = self.icons.get(&(glyph, light)) {
            return Ok(*icon);
        }
        // SAFETY: a query of a system metric without side effects.
        let wanted = unsafe { GetSystemMetrics(SM_CXSMICON) }.clamp(16, 32) as u32;
        let picture = GLYPHS
            .iter()
            .filter(|(name, shade, _, _)| *name == glyph && *shade == light)
            .min_by_key(|(_, _, size, _)| (*size < wanted, size.abs_diff(wanted)))
            .ok_or_else(windows::core::Error::empty)?;
        let (side, pixels) = (picture.2 as i32, picture.3);
        let mask = vec![0u8; mask_row(picture.2) * picture.2 as usize];
        // SAFETY: each buffer holds the bits of a bitmap of `side` by `side` pixels at its
        // depth; the bitmaps are deleted after the icon copied them.
        let icon = unsafe {
            let colour = CreateBitmap(side, side, 1, 32, Some(pixels.as_ptr().cast()));
            let shape = CreateBitmap(side, side, 1, 1, Some(mask.as_ptr().cast()));
            let icon = CreateIconIndirect(&ICONINFO {
                fIcon: true.into(),
                xHotspot: 0,
                yHotspot: 0,
                hbmMask: shape,
                hbmColor: colour,
            });
            let _ = DeleteObject(colour.into());
            let _ = DeleteObject(shape.into());
            icon?
        };
        self.icons.insert((glyph, light), icon);
        Ok(icon)
    }
}

/// The bytes of a row of a bitmap of one bit per pixel: rows are aligned to 16 bits.
fn mask_row(side: u32) -> usize {
    side.div_ceil(16) as usize * 2
}

/// Writes a tooltip into the field of a button, cut to its length with the ending zero.
fn tip(field: &mut [u16; 260], text: &str) {
    let mut written = 0;
    for unit in text.encode_utf16().take(field.len() - 1) {
        field[written] = unit;
        written += 1;
    }
    field[written] = 0;
}

/// The taskbar is light: the glyphs are dark. Windows states it for the account.
fn light_taskbar() -> bool {
    windows_registry::CURRENT_USER
        .open(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize")
        .and_then(|key| key.get_u32("SystemUsesLightTheme"))
        .is_ok_and(|light| light != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_glyph_has_both_shades_at_each_size() {
        for glyph in [
            "microphone",
            "microphone-off",
            "sound",
            "sound-off",
            "leave",
        ] {
            for light in [true, false] {
                let sizes: Vec<u32> = GLYPHS
                    .iter()
                    .filter(|(name, shade, _, _)| *name == glyph && *shade == light)
                    .map(|(_, _, size, pixels)| {
                        assert_eq!(pixels.len(), (*size * *size * 4) as usize);
                        *size
                    })
                    .collect();
                assert_eq!(sizes, [16, 20, 24, 32], "{glyph} {light}");
            }
        }
    }

    #[test]
    fn a_tooltip_is_cut_to_its_field_with_the_ending_zero() {
        let mut field = [1u16; 260];
        tip(&mut field, "Mute");
        assert_eq!(&field[..5], &[77, 117, 116, 101, 0]);
        tip(&mut field, &"x".repeat(400));
        assert_eq!((field[258], field[259]), (120, 0));
    }

    #[test]
    fn a_press_names_its_button_and_the_glyphs_follow_the_controls() {
        for press in ThumbPress::ALL {
            assert_eq!(ThumbPress::of(press.id()), Some(press));
        }
        assert_eq!(ThumbPress::of(9), None);
        let buttons = ThumbButtons {
            microphone_open: false,
            microphone_usable: true,
            sound_off: true,
            tips: Default::default(),
        };
        assert_eq!(buttons.glyph(ThumbPress::Microphone), "microphone-off");
        assert_eq!(buttons.glyph(ThumbPress::Sound), "sound-off");
        assert_eq!(mask_row(16), 2);
        assert_eq!(mask_row(20), 4);
    }
}
