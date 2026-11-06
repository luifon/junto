//! The window of the fixture, drawn with GDI. The fixture program shows it, and the test
//! program shows one of its own with the same code.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicI32, AtomicIsize, AtomicUsize, Ordering};

use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, ClientToScreen, CreateSolidBrush, DeleteObject, EndPaint, FillRect,
    GetMonitorInfoW, InvalidateRect, MONITOR_DEFAULTTONEAREST, MONITORINFO, MONITORINFOEXW,
    MonitorFromWindow, PAINTSTRUCT, UpdateWindow,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow,
    DispatchMessageW, GetClientRect, GetMessageW, MSG, PostMessageW, PostQuitMessage,
    RegisterClassW, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    SetWindowPos, ShowWindow, TranslateMessage, WM_APP, WM_CLOSE, WM_DESTROY, WM_PAINT, WNDCLASSW,
    WS_CAPTION, WS_EX_NOACTIVATE, WS_EX_TOPMOST, WS_OVERLAPPEDWINDOW, WS_POPUP, WS_SYSMENU,
    WS_VISIBLE,
};
use windows::core::{PCWSTR, w};

/// The colours of the quarters, as red, green and blue: top left, top right, bottom left,
/// bottom right. After `turn` was read `k` times, quarter `q` shows colour `(q + k) % 4`.
const COLOURS: [(u8, u8, u8); 4] = [(200, 40, 40), (40, 180, 60), (40, 60, 200), (220, 200, 40)];
/// The size of the inner area in pixels at the start.
const SIZE: (i32, i32) = (640, 400);
const TURN: u32 = WM_APP + 1;
/// A change of the size of the inner area to the width and height of the message.
const RESIZE: u32 = WM_APP + 2;
/// The window answers no message for the seconds of the message, as an application that
/// does not answer: after five seconds Windows marks it as not answering.
const STALL: u32 = WM_APP + 3;
/// Opens a second window of the process: a dialog that the window owns for `0`, a window
/// without an owner for `1`.
const OPEN: u32 = WM_APP + 4;
/// Moves the window by the pixels of the message, to the right and down.
const MOVE: u32 = WM_APP + 5;
/// The colours of a dialog and of a later window, as red, green and blue.
pub const DIALOG_COLOUR: (u8, u8, u8) = (204, 26, 204);
pub const LATER_COLOUR: (u8, u8, u8) = (26, 204, 204);
/// The size of the inner area of a dialog and of a later window. The outer frame of the
/// dialog starts 40 pixels right of and 60 pixels below the outer frame of the window, over
/// its left half; the later window is 20 pixels right of the window. An owned dialog does
/// not move with its window.
const SECOND_SIZE: (i32, i32) = (200, 150);
/// The place of the outer frame of the window at the start.
const START: (i32, i32) = (160, 160);
const STYLE: windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE = WS_OVERLAPPEDWINDOW;
const EXTENDED: windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE = WS_EX_TOPMOST;

static TURNS: AtomicUsize = AtomicUsize::new(0);
/// The windows of the process, by their numbers: the first window, the dialog, the later
/// window.
static MAIN: AtomicIsize = AtomicIsize::new(0);
static DIALOG: AtomicIsize = AtomicIsize::new(0);
static LATER: AtomicIsize = AtomicIsize::new(0);
/// The module of the program, for the second windows.
static INSTANCE: AtomicIsize = AtomicIsize::new(0);
/// The place of the outer frame of the first window, which only `move` changes.
static LEFT: AtomicI32 = AtomicI32::new(START.0);
static TOP: AtomicI32 = AtomicI32::new(START.1);
/// Where the window writes its lines: one window per process.
static SAY: OnceLock<Box<dyn Fn(String) + Send + Sync>> = OnceLock::new();

fn say(line: String) {
    if let Some(say) = SAY.get() {
        say(line);
    }
}

fn text(buffer: &[u16]) -> String {
    let end = buffer
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

/// The number of the screen of the window and its inner area in pixels of that screen:
/// `<screen> <left> <top> <right> <bottom>`.
fn place(window: HWND) -> String {
    // SAFETY: the calls read properties of a window of this thread into the values.
    unsafe {
        let mut area = RECT::default();
        let _ = GetClientRect(window, &mut area);
        let mut corner = POINT { x: 0, y: 0 };
        let _ = ClientToScreen(window, &mut corner);
        let monitor = MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        let _ = GetMonitorInfoW(
            monitor,
            (&mut info as *mut MONITORINFOEXW).cast::<MONITORINFO>(),
        );
        let device = text(&info.szDevice);
        let screen = device.trim_start_matches(r"\\.\DISPLAY").to_owned();
        let origin = info.monitorInfo.rcMonitor;
        let (left, top) = (corner.x - origin.left, corner.y - origin.top);
        format!(
            "{screen} {left} {top} {} {}",
            left + area.right,
            top + area.bottom
        )
    }
}

/// The size of the frame of a window whose inner area has a size.
fn frame((width, height): (i32, i32)) -> (i32, i32) {
    let mut frame = RECT {
        left: 0,
        top: 0,
        right: width,
        bottom: height,
    };
    // SAFETY: the call changes the rectangle in place.
    let _ = unsafe { AdjustWindowRectEx(&mut frame, STYLE, false, EXTENDED) };
    (frame.right - frame.left, frame.bottom - frame.top)
}

unsafe extern "system" fn procedure(window: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    // SAFETY: the system calls this procedure on the thread of the window with valid
    // arguments; each brush is deleted after its use.
    unsafe {
        match message {
            WM_PAINT => {
                let mut paint = PAINTSTRUCT::default();
                let device = BeginPaint(window, &mut paint);
                let mut area = RECT::default();
                let _ = GetClientRect(window, &mut area);
                let (half_x, half_y) = (area.right / 2, area.bottom / 2);
                let quarters = [
                    RECT {
                        left: 0,
                        top: 0,
                        right: half_x,
                        bottom: half_y,
                    },
                    RECT {
                        left: half_x,
                        top: 0,
                        right: area.right,
                        bottom: half_y,
                    },
                    RECT {
                        left: 0,
                        top: half_y,
                        right: half_x,
                        bottom: area.bottom,
                    },
                    RECT {
                        left: half_x,
                        top: half_y,
                        right: area.right,
                        bottom: area.bottom,
                    },
                ];
                let turns = TURNS.load(Ordering::Relaxed);
                let number = window.0 as isize;
                let single = if number == DIALOG.load(Ordering::Relaxed) {
                    Some(DIALOG_COLOUR)
                } else if number == LATER.load(Ordering::Relaxed) {
                    Some(LATER_COLOUR)
                } else {
                    None
                };
                for (quarter, rect) in quarters.iter().enumerate() {
                    let (r, g, b) = single.unwrap_or(COLOURS[(quarter + turns) % 4]);
                    let brush = CreateSolidBrush(COLORREF(
                        u32::from(r) | u32::from(g) << 8 | u32::from(b) << 16,
                    ));
                    FillRect(device, rect, brush);
                    let _ = DeleteObject(brush.into());
                }
                let _ = EndPaint(window, &paint);
                LRESULT(0)
            }
            TURN => {
                TURNS.fetch_add(1, Ordering::Relaxed);
                let _ = InvalidateRect(Some(window), None, false);
                LRESULT(0)
            }
            RESIZE => {
                let (width, height) = frame((w.0 as i32, l.0 as i32));
                let _ = SetWindowPos(
                    window,
                    None,
                    0,
                    0,
                    width,
                    height,
                    SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
                let _ = InvalidateRect(Some(window), None, false);
                let _ = UpdateWindow(window);
                say(format!("resized {}", place(window)));
                LRESULT(0)
            }
            OPEN => {
                let dialog = w.0 == 0;
                let owner = HWND(MAIN.load(Ordering::Relaxed) as *mut core::ffi::c_void);
                let (width, height) = frame(SECOND_SIZE);
                let (left, top) = (LEFT.load(Ordering::Relaxed), TOP.load(Ordering::Relaxed));
                let (main_width, _) = frame(SIZE);
                let (x, y, style, parent) = if dialog {
                    (
                        left + 40,
                        top + 60,
                        WS_POPUP | WS_CAPTION | WS_SYSMENU,
                        Some(owner),
                    )
                } else {
                    (left + main_width + 20, top, STYLE, None)
                };
                let instance =
                    HINSTANCE(INSTANCE.load(Ordering::Relaxed) as *mut core::ffi::c_void);
                // The second call that creates a window, for the dialog and for the later
                // window (ADR 0706): the class and the instance are
                // those of the first window, the owner of a dialog is that window of this
                // thread, and the title is static. It shows the window without activating it.
                let Ok(second) = CreateWindowExW(
                    EXTENDED | WS_EX_NOACTIVATE,
                    w!("JuntoShareFixture"),
                    if dialog {
                        w!("Dialog")
                    } else {
                        w!("Later window")
                    },
                    style | WS_VISIBLE,
                    x,
                    y,
                    width,
                    height,
                    parent,
                    None,
                    Some(instance),
                    None,
                ) else {
                    say("not opened".to_owned());
                    return LRESULT(0);
                };
                (if dialog { &DIALOG } else { &LATER }).store(second.0 as isize, Ordering::Relaxed);
                say(format!(
                    "opened {} {}",
                    if dialog { "dialog" } else { "window" },
                    place(second)
                ));
                LRESULT(0)
            }
            MOVE => {
                let left = LEFT.fetch_add(w.0 as i32, Ordering::Relaxed) + w.0 as i32;
                let top = TOP.fetch_add(l.0 as i32, Ordering::Relaxed) + l.0 as i32;
                // The second call that places the window (ADR 0706): only its place changes,
                // on the thread of the window.
                let _ = SetWindowPos(
                    window,
                    None,
                    left,
                    top,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
                say(format!("moved {}", place(window)));
                LRESULT(0)
            }
            STALL => {
                say("stalling".to_owned());
                std::thread::sleep(std::time::Duration::from_secs(w.0 as u64));
                LRESULT(0)
            }
            WM_CLOSE => {
                let _ = DestroyWindow(window);
                LRESULT(0)
            }
            // A dialog or a later window that closes leaves the first window open.
            WM_DESTROY if window.0 as isize == MAIN.load(Ordering::Relaxed) => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(window, message, w, l),
        }
    }
}

/// Opens the window and runs its messages until it is closed; returns the exit code. The
/// window writes its lines with `out`, the first one when it is drawn:
///
/// `ready <window number> <screen number> <left> <top> <right> <bottom>`
///
/// It reads `commands` on another thread: `turn` turns the colours by one quarter,
/// `resize <width> <height>` changes the size of the inner area and writes
/// `resized <screen number> <left> <top> <right> <bottom>`, `open dialog` and `open window`
/// open a dialog that the window owns or a later window without an owner, in one colour
/// each, and write `opened <dialog|window> <screen number> <left> <top> <right> <bottom>`,
/// `move <right> <down>` moves the window and writes
/// `moved <screen number> <left> <top> <right> <bottom>`, and `close`, like the end of the
/// commands, closes the window. One window per process.
pub fn run(
    title: &str,
    commands: impl Iterator<Item = String> + Send + 'static,
    out: impl Fn(String) + Send + Sync + 'static,
) -> i32 {
    if SAY.set(Box::new(out)).is_err() {
        return 1;
    }
    let title: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: the class, the title and the rectangles outlive the calls that read them; the
    // window is used on this thread only, and the input thread only posts messages to it.
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let Ok(instance) = GetModuleHandleW(None) else {
            return 1;
        };
        let class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(procedure),
            hInstance: instance.into(),
            lpszClassName: w!("JuntoShareFixture"),
            ..Default::default()
        };
        if RegisterClassW(&class) == 0 {
            return 1;
        }
        let (width, height) = frame(SIZE);
        let Ok(window) = CreateWindowExW(
            EXTENDED,
            w!("JuntoShareFixture"),
            PCWSTR(title.as_ptr()),
            STYLE,
            START.0,
            START.1,
            width,
            height,
            None,
            None,
            Some(instance.into()),
            None,
        ) else {
            return 1;
        };
        MAIN.store(window.0 as isize, Ordering::Relaxed);
        INSTANCE.store(instance.0 as isize, Ordering::Relaxed);
        let _ = ShowWindow(window, SW_SHOWNOACTIVATE);
        let _ = UpdateWindow(window);
        say(format!(
            "ready {} {}",
            window.0 as isize as u32,
            place(window)
        ));

        let handle = window.0 as isize;
        std::thread::spawn(move || {
            let target = HWND(handle as *mut core::ffi::c_void);
            for line in commands {
                let words: Vec<&str> = line.split_whitespace().collect();
                let number = |word: &&str| word.parse::<i32>().ok();
                let message = match words.as_slice() {
                    ["turn"] => Some((TURN, 0, 0)),
                    ["resize", width, height] => number(width)
                        .zip(number(height))
                        .map(|(width, height)| (RESIZE, width, height)),
                    ["stall", seconds] => number(seconds).map(|seconds| (STALL, seconds, 0)),
                    ["open", "dialog"] => Some((OPEN, 0, 0)),
                    ["open", "window"] => Some((OPEN, 1, 0)),
                    ["move", right, down] => number(right)
                        .zip(number(down))
                        .map(|(right, down)| (MOVE, right, down)),
                    ["close"] => break,
                    _ => None,
                };
                if let Some((message, w, l)) = message {
                    let _ = PostMessageW(
                        Some(target),
                        message,
                        WPARAM(w as usize),
                        LPARAM(l as isize),
                    );
                }
            }
            let _ = PostMessageW(Some(target), WM_CLOSE, WPARAM(0), LPARAM(0));
        });

        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        0
    }
}
