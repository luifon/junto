//! One still picture of a screen or a window for the dialog of a share, with GDI: a copy of
//! the part of the desktop that a monitor shows, and the drawing that a window makes of
//! itself on request (`PrintWindow`), so that a window behind other windows gives its own
//! picture (ADR 0736). A share itself uses Windows Graphics Capture.

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleBitmap, CreateCompatibleDC,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, GetMonitorInfoW, HALFTONE, HBITMAP,
    HDC, HGDIOBJ, HMONITOR, MONITORINFO, ReleaseDC, SRCCOPY, SelectObject, SetBrushOrgEx,
    SetStretchBltMode, StretchBlt,
};
use windows::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow};
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowRect, IsHungAppWindow, PW_RENDERFULLCONTENT,
};

use crate::share::CaptureError;
use crate::share::picture::{PictureFrame, fit};

/// What a still picture shows.
pub(super) enum Still {
    Monitor(HMONITOR),
    Window(HWND),
}

/// A memory device context with one bitmap selected into it. The drop restores the context
/// and releases both.
struct Canvas {
    context: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    size: (i32, i32),
}

impl Canvas {
    /// A canvas compatible with the desktop.
    ///
    /// # Safety
    ///
    /// `screen` is the device context of the desktop, valid while the canvas lives.
    unsafe fn new(screen: HDC, size: (i32, i32)) -> Result<Self, CaptureError> {
        // SAFETY: the caller gives a valid device context; each object made here is
        // released by the drop of the canvas.
        unsafe {
            let context = CreateCompatibleDC(Some(screen));
            if context.is_invalid() {
                return Err(failed("no device context"));
            }
            let bitmap = CreateCompatibleBitmap(screen, size.0, size.1);
            if bitmap.is_invalid() {
                let _ = DeleteDC(context);
                return Err(failed("no bitmap"));
            }
            let previous = SelectObject(context, bitmap.into());
            Ok(Self {
                context,
                bitmap,
                previous,
                size,
            })
        }
    }

    /// The pixels of the canvas in rows from the top: blue, green, red and 255.
    fn pixels(&self) -> Result<Vec<u8>, CaptureError> {
        let (width, height) = self.size;
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                // A negative height gives the rows from the top.
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut pixels = vec![0u8; (width * height * 4) as usize];
        // SAFETY: the bitmap is taken out of the context before it is read, as GetDIBits
        // requires, and put back after; the buffer holds every row of 32-bit pixels.
        let read = unsafe {
            let _ = SelectObject(self.context, self.previous);
            let read = GetDIBits(
                self.context,
                self.bitmap,
                0,
                height as u32,
                Some(pixels.as_mut_ptr().cast()),
                &mut info,
                DIB_RGB_COLORS,
            );
            let _ = SelectObject(self.context, self.bitmap.into());
            read
        };
        if read != height {
            return Err(failed("the pixels were not read"));
        }
        // GDI leaves the fourth byte at zero.
        for pixel in pixels.as_chunks_mut::<4>().0 {
            pixel[3] = 255;
        }
        Ok(pixels)
    }

    /// Copies a part of `source` into the whole canvas, scaled with the averaging of GDI.
    fn take(&self, source: HDC, from: (i32, i32), size: (i32, i32)) -> Result<(), CaptureError> {
        // SAFETY: both contexts are valid for the time of the call.
        let copied = unsafe {
            SetStretchBltMode(self.context, HALFTONE);
            let _ = SetBrushOrgEx(self.context, 0, 0, None);
            StretchBlt(
                self.context,
                0,
                0,
                self.size.0,
                self.size.1,
                Some(source),
                from.0,
                from.1,
                size.0,
                size.1,
                SRCCOPY,
            )
        };
        if copied.as_bool() {
            Ok(())
        } else {
            Err(failed("the picture was not copied"))
        }
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        // SAFETY: the objects were made by `new` and are released once.
        unsafe {
            let _ = SelectObject(self.context, self.previous);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.context);
        }
    }
}

fn failed(what: &str) -> CaptureError {
    CaptureError::Failed(format!("preview: {what}"))
}

/// The device context of the desktop, released at its drop.
struct Screen(HDC);

impl Drop for Screen {
    fn drop(&mut self) {
        // SAFETY: the context was given by `GetDC(None)` and is released once.
        unsafe {
            let _ = ReleaseDC(None, self.0);
        }
    }
}

/// The picture of a screen or a window at most `bound` pixels, with its proportions.
pub(super) fn picture(still: &Still, bound: (u32, u32)) -> Result<PictureFrame, CaptureError> {
    // SAFETY: the device context of the desktop is released by the drop of `Screen`.
    let screen = Screen(unsafe { GetDC(None) });
    if screen.0.is_invalid() {
        return Err(failed("no device context of the desktop"));
    }
    let area = match still {
        Still::Monitor(monitor) => {
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            // SAFETY: the structure has the size that its first field states.
            if !unsafe { GetMonitorInfoW(*monitor, &mut info) }.as_bool() {
                return Err(CaptureError::SourceGone);
            }
            info.rcMonitor
        }
        Still::Window(window) => {
            let mut area = RECT::default();
            // SAFETY: the rectangle outlives the call.
            unsafe { GetWindowRect(*window, &mut area) }.map_err(|_| CaptureError::SourceGone)?;
            area
        }
    };
    let size = (area.right - area.left, area.bottom - area.top);
    if size.0 <= 0 || size.1 <= 0 {
        return Err(CaptureError::SourceGone);
    }
    let (width, height) = fit((size.0 as u32, size.1 as u32), bound);
    // SAFETY: the canvases live shorter than the context of the desktop.
    let shown = unsafe { Canvas::new(screen.0, (width as i32, height as i32))? };
    match still {
        Still::Monitor(_) => shown.take(screen.0, (area.left, area.top), size)?,
        Still::Window(window) => {
            // PrintWindow waits for the application of the window. A window that the system
            // marks as not answering gets no picture.
            // SAFETY: the handle names a listed window; a closed one answers false.
            if unsafe { IsHungAppWindow(*window) }.as_bool() {
                return Err(failed("the application of the window does not answer"));
            }
            // SAFETY: as above.
            let full = unsafe { Canvas::new(screen.0, size)? };
            // SAFETY: the window handle names a listed window; a window that has closed
            // makes the call fail.
            let drawn = unsafe {
                PrintWindow(
                    *window,
                    full.context,
                    PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT),
                )
            };
            if !drawn.as_bool() {
                return Err(CaptureError::SourceGone);
            }
            shown.take(full.context, (0, 0), size)?;
        }
    }
    Ok(PictureFrame {
        width,
        height,
        bgra: shown.pixels()?,
        source: (size.0 as u32, size.1 as u32),
    })
}
