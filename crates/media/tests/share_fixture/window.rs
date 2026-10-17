//! The window of the fixture, drawn with GDI. The fixture program shows it, and the test
//! program shows one of its own with the same code.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
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
    RegisterClassW, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER, SetWindowPos,
    ShowWindow, TranslateMessage, WM_APP, WM_CLOSE, WM_DESTROY, WM_PAINT, WNDCLASSW, WS_EX_TOPMOST,
    WS_OVERLAPPEDWINDOW,
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
const STYLE: windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE = WS_OVERLAPPEDWINDOW;
const EXTENDED: windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE = WS_EX_TOPMOST;

static TURNS: AtomicUsize = AtomicUsize::new(0);
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
                for (quarter, rect) in quarters.iter().enumerate() {
                    let (r, g, b) = COLOURS[(quarter + turns) % 4];
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
            WM_CLOSE => {
                let _ = DestroyWindow(window);
                LRESULT(0)
            }
            WM_DESTROY => {
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
/// `resized <screen number> <left> <top> <right> <bottom>`, and `close`, like the end of the
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
            160,
            160,
            width,
            height,
            None,
            None,
            Some(instance.into()),
            None,
        ) else {
            return 1;
        };
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
                match words.as_slice() {
                    ["turn"] => {
                        let _ = PostMessageW(Some(target), TURN, WPARAM(0), LPARAM(0));
                    }
                    ["resize", width, height] => {
                        if let (Ok(width), Ok(height)) = (width.parse(), height.parse()) {
                            let _ =
                                PostMessageW(Some(target), RESIZE, WPARAM(width), LPARAM(height));
                        }
                    }
                    ["close"] => break,
                    _ => {}
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
