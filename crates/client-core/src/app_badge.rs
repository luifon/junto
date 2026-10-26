//! The count of the messages for the person on the icon of the application (ADR 0741): its
//! text, the picture of the count that the taskbar button of Windows shows over the icon, and
//! the badge of the Dock of macOS.

/// The highest count that is written; a higher one is written as "99+".
pub const MOST: u64 = 99;

/// The text of a count. Absent for no count.
pub fn label(count: u64) -> Option<String> {
    match count {
        0 => None,
        count if count > MOST => Some(format!("{MOST}+")),
        count => Some(count.to_string()),
    }
}

/// The colour of the disc of the count: the terracotta of the design, and its text white.
const DISC: [u8; 3] = [0xB4, 0x4A, 0x2E];
const INK: [u8; 3] = [0xFF, 0xFF, 0xFF];

/// The glyphs of the count, 3 by 5 points, one row per element, the left point the high bit.
fn glyph(character: char) -> Option<[u8; 5]> {
    Some(match character {
        '0' => [0b111, 0b101, 0b101, 0b101, 0b111],
        '1' => [0b010, 0b110, 0b010, 0b010, 0b111],
        '2' => [0b111, 0b001, 0b111, 0b100, 0b111],
        '3' => [0b111, 0b001, 0b111, 0b001, 0b111],
        '4' => [0b101, 0b101, 0b111, 0b001, 0b001],
        '5' => [0b111, 0b100, 0b111, 0b001, 0b111],
        '6' => [0b111, 0b100, 0b111, 0b101, 0b111],
        '7' => [0b111, 0b001, 0b010, 0b010, 0b010],
        '8' => [0b111, 0b101, 0b111, 0b101, 0b111],
        '9' => [0b111, 0b101, 0b111, 0b001, 0b111],
        '+' => [0b000, 0b010, 0b111, 0b010, 0b000],
        _ => return None,
    })
}

/// The picture of a count: a disc with the count in it, `side` pixels square, as rows of
/// red, green, blue and alpha from the top. The outside of the disc is transparent. The
/// glyphs take the largest whole scale at which the text fits inside the disc.
pub fn picture(count: u64, side: u32) -> Vec<u8> {
    let side = side.max(8);
    let mut pixels = vec![0u8; (side * side * 4) as usize];
    let Some(text) = label(count) else {
        return pixels;
    };
    let centre = (side as f32 - 1.0) / 2.0;
    let radius = side as f32 / 2.0;
    let mut put = |x: u32, y: u32, colour: [u8; 3], alpha: u8| {
        let at = ((y * side + x) * 4) as usize;
        pixels[at..at + 3].copy_from_slice(&colour);
        pixels[at + 3] = alpha;
    };
    for y in 0..side {
        for x in 0..side {
            let distance = ((x as f32 - centre).powi(2) + (y as f32 - centre).powi(2)).sqrt();
            // One pixel of the edge is partly covered, so that the disc is round.
            let cover = (radius - distance).clamp(0.0, 1.0);
            if cover > 0.0 {
                put(x, y, DISC, (cover * 255.0) as u8);
            }
        }
    }
    let glyphs: Vec<[u8; 5]> = text.chars().filter_map(glyph).collect();
    let count = glyphs.len() as u32;
    // The text fits in the square inside the disc, with one point between two glyphs.
    let room = (side as f32 * 0.72) as u32;
    let width = |scale: u32| count * 3 * scale + count.saturating_sub(1) * scale;
    let scale = (1..=side)
        .rev()
        .find(|scale| width(*scale) <= room && 5 * scale <= room)
        .unwrap_or(1);
    let left = (side - width(scale).min(side)) / 2;
    let top = (side - (5 * scale).min(side)) / 2;
    for (index, rows) in glyphs.iter().enumerate() {
        let start = left + index as u32 * 4 * scale;
        for (row, bits) in rows.iter().enumerate() {
            for column in 0..3u32 {
                if bits & (0b100 >> column) == 0 {
                    continue;
                }
                for dy in 0..scale {
                    for dx in 0..scale {
                        let (x, y) = (start + column * scale + dx, top + row as u32 * scale + dy);
                        if x < side && y < side {
                            put(x, y, INK, 255);
                        }
                    }
                }
            }
        }
    }
    pixels
}

/// Sets the badge of the Dock of macOS: the count, or none. It runs on the main thread, as
/// every call of AppKit does; elsewhere it changes nothing.
#[cfg(target_os = "macos")]
pub fn set_dock(label: Option<&str>) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    use objc2_foundation::NSString;
    let Some(main) = MainThreadMarker::new() else {
        return;
    };
    let tile = NSApplication::sharedApplication(main).dockTile();
    let text = label.map(NSString::from_str);
    tile.setBadgeLabel(text.as_deref());
}

/// Sets the overlay of the taskbar button of a window: the picture of the count, or none,
/// with its description for a screen reader.
#[cfg(windows)]
#[allow(unsafe_code)]
pub fn set_overlay(window: isize, count: u64, description: &str) -> windows::core::Result<()> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Gdi::{CreateBitmap, DeleteObject};
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
    use windows::Win32::UI::Shell::{ITaskbarList3, TaskbarList};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateIconIndirect, DestroyIcon, GetSystemMetrics, HICON, ICONINFO, SM_CXSMICON,
    };
    use windows::core::HSTRING;

    let window = HWND(window as *mut core::ffi::c_void);
    // SAFETY: the class is the system taskbar list; the call creates a COM object on a
    // thread on which the window has started COM.
    let taskbar: ITaskbarList3 = unsafe {
        CoCreateInstance(&TaskbarList, None::<&windows::core::IUnknown>, CLSCTX_INPROC_SERVER)?
    };
    // SAFETY: the list is initialised before its first use, as its documentation asks.
    unsafe { taskbar.HrInit()? };
    if count == 0 {
        // SAFETY: a null icon takes the overlay away from the button of a valid window.
        return unsafe { taskbar.SetOverlayIcon(window, HICON::default(), &HSTRING::new()) };
    }
    // SAFETY: a query of a system metric without side effects.
    let side = unsafe { GetSystemMetrics(SM_CXSMICON) }.clamp(16, 64) as u32;
    // Windows reads a bitmap of 32 bits per pixel as blue, green, red and alpha.
    let mut pixels = picture(count, side);
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    let mask = vec![0u8; (side * side / 8) as usize];
    let side = side as i32;
    // SAFETY: each buffer holds the bits of a bitmap of `side` by `side` pixels at its
    // depth; the bitmaps are deleted after the icon copied them.
    unsafe {
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
        let icon = icon?;
        let set = taskbar.SetOverlayIcon(window, icon, &HSTRING::from(description));
        let _ = DestroyIcon(icon);
        set
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha(pixels: &[u8], side: u32, x: u32, y: u32) -> u8 {
        pixels[((y * side + x) * 4 + 3) as usize]
    }

    fn white(pixels: &[u8], side: u32) -> usize {
        pixels
            .chunks_exact(4)
            .filter(|pixel| pixel[..3] == INK && pixel[3] == 255)
            .count()
            .min((side * side) as usize)
    }

    #[test]
    fn a_count_is_written_up_to_99_and_above_as_99_plus() {
        assert_eq!(label(0), None);
        assert_eq!(label(7).as_deref(), Some("7"));
        assert_eq!(label(99).as_deref(), Some("99"));
        assert_eq!(label(100).as_deref(), Some("99+"));
    }

    #[test]
    fn the_picture_of_a_count_is_a_disc_with_its_glyphs_and_a_transparent_outside() {
        for side in [16, 24, 32] {
            let one = picture(1, side);
            assert_eq!(one.len(), (side * side * 4) as usize);
            // The corners lie outside the disc; the centre is covered.
            for (x, y) in [(0, 0), (side - 1, 0), (0, side - 1), (side - 1, side - 1)] {
                assert_eq!(alpha(&one, side, x, y), 0, "{side}: corner {x},{y}");
            }
            assert_eq!(alpha(&one, side, side / 2, side / 2 - 1), 255);
            // A count has white points, more of them for more glyphs, and none without a
            // count.
            let (two, many) = (picture(28, side), picture(100, side));
            assert!(white(&one, side) > 0, "{side}");
            assert!(white(&two, side) > white(&one, side), "{side}");
            assert!(white(&many, side) > 0, "{side}");
            assert!(picture(0, side).iter().all(|byte| *byte == 0));
        }
    }

    #[test]
    fn the_glyphs_of_a_count_stay_inside_the_disc() {
        for side in [16, 24, 32] {
            for count in [1, 9, 42, 100] {
                let pixels = picture(count, side);
                let centre = (side as f32 - 1.0) / 2.0;
                for y in 0..side {
                    for x in 0..side {
                        let at = ((y * side + x) * 4) as usize;
                        if pixels[at..at + 3] == INK {
                            let distance =
                                ((x as f32 - centre).powi(2) + (y as f32 - centre).powi(2)).sqrt();
                            assert!(distance < side as f32 / 2.0, "{side} {count}: {x},{y}");
                        }
                    }
                }
            }
        }
    }
}
