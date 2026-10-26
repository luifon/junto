//! The empty space between the parts of the areas of a screen (ADR 0730). An area names a
//! region whose parts start at its top and follow each other: a list, a page, the content
//! of a dialog or of a panel. A band of the ground colour of an area that is taller than
//! the design allows, above its first part or between two parts, is an empty band. Space
//! after the last part is the end of the area.

use gpui::{Bounds, Pixels, SharedString};
use image::RgbaImage;

/// The tallest space above the first part of an area and between two of its parts, in
/// pixels of the design.
pub const MOST_GAP: f32 = 40.0;
/// The width at each side of an area that is not read: the scroll bar and the borders.
const SIDE: f32 = 12.0;
/// The height at the top and at the bottom of an area that is not read, in pixels of the
/// window.
const EDGE: f32 = 2.0;
/// The largest difference of a colour channel from the ground that is still the ground.
const TOLERANCE: u8 = 6;

/// The empty bands of one drawn screen. `window_width` is the width of the window in
/// pixels of the window and `scale` its interface scale in percent.
pub fn empty_bands(
    image: &RgbaImage,
    areas: &[(SharedString, Bounds<Pixels>)],
    window_width: f32,
    scale: u32,
) -> Vec<String> {
    let device = image.width() as f32 / window_width;
    let design = device * scale as f32 / 100.0;
    let most = (MOST_GAP * design).round() as u32;
    let mut found = Vec::new();
    // An area drawn before a layer that covers it is not seen; the areas of the layer are
    // drawn after its cover.
    let covered = |index: usize, bounds: &Bounds<Pixels>| {
        areas[index + 1..]
            .iter()
            .any(|(name, cover)| name.as_ref() == junto_client::ui::COVER && cover.intersects(bounds))
    };
    for (index, (name, bounds)) in areas.iter().enumerate() {
        if name.as_ref() == junto_client::ui::COVER || covered(index, bounds) {
            continue;
        }
        let left = ((f32::from(bounds.left()) + SIDE) * device).max(0.0) as u32;
        let right = (((f32::from(bounds.right()) - SIDE) * device) as u32).min(image.width());
        // The rows at the top and at the bottom edge hold the borders of the parts next to
        // the area.
        let edge = (EDGE * device).ceil() as u32;
        let top = (f32::from(bounds.top()) * device).max(0.0) as u32 + edge;
        let bottom = (((f32::from(bounds.bottom()) * device) as u32).min(image.height()))
            .saturating_sub(edge);
        if right <= left + 1 || bottom <= top + most {
            continue;
        }
        let ground = ground(image, left, right, top, bottom);
        let raw = image.as_raw();
        let stride = image.width() as usize * 4;
        let blank = |y: u32| {
            let start = y as usize * stride + left as usize * 4;
            let end = y as usize * stride + right as usize * 4;
            raw[start..end].chunks_exact(4).all(|pixel| {
                pixel[0].abs_diff(ground[0]) <= TOLERANCE
                    && pixel[1].abs_diff(ground[1]) <= TOLERANCE
                    && pixel[2].abs_diff(ground[2]) <= TOLERANCE
            })
        };
        let at = |y: u32| (y as f32 / design).round();
        let mut last_part: Option<u32> = None;
        for y in top..bottom {
            if blank(y) {
                continue;
            }
            let from = last_part.map_or(top, |part| part + 1);
            let space = y - from;
            if space > most {
                let place = if last_part.is_some() {
                    "between two parts"
                } else {
                    "above the first part"
                };
                found.push(format!(
                    "{name}: {} design px of empty space {place}, from {} to {} design px of \
                     the window",
                    at(space),
                    at(from),
                    at(y),
                ));
            }
            last_part = Some(y);
        }
        if last_part.is_none() {
            found.push(format!(
                "{name}: {} design px with no part, from {} design px of the window",
                at(bottom - top),
                at(top)
            ));
        }
    }
    found
}

/// The colour that most pixels of the region have.
fn ground(image: &RgbaImage, left: u32, right: u32, top: u32, bottom: u32) -> [u8; 3] {
    let mut counts = std::collections::HashMap::<[u8; 3], usize>::new();
    // A sparse grid finds the colour of the ground: it covers most of an area.
    for y in (top..bottom).step_by(7) {
        for x in (left..right).step_by(7) {
            let pixel = image.get_pixel(x, y).0;
            *counts.entry([pixel[0], pixel[1], pixel[2]]).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map_or([0, 0, 0], |(colour, _)| colour)
}

#[cfg(test)]
mod tests {
    use gpui::{Bounds, point, px, size};
    use image::{Rgba, RgbaImage};

    use super::empty_bands;

    fn screen(parts: &[(u32, u32)]) -> RgbaImage {
        let mut image = RgbaImage::from_pixel(200, 400, Rgba([240, 235, 225, 255]));
        for &(top, bottom) in parts {
            for y in top..bottom {
                for x in 40..120 {
                    image.put_pixel(x, y, Rgba([30, 30, 30, 255]));
                }
            }
        }
        image
    }

    fn area() -> Vec<(gpui::SharedString, Bounds<gpui::Pixels>)> {
        vec![(
            "list".into(),
            Bounds::new(point(px(0.0), px(0.0)), size(px(200.0), px(400.0))),
        )]
    }

    #[test]
    fn parts_at_the_spacing_of_the_design_have_no_empty_band() {
        let image = screen(&[(10, 30), (50, 70), (100, 120)]);
        assert_eq!(empty_bands(&image, &area(), 200.0, 100), Vec::<String>::new());
    }

    #[test]
    fn a_band_between_two_parts_and_above_the_first_part_is_found() {
        let image = screen(&[(60, 80), (90, 100), (200, 220)]);
        let found = empty_bands(&image, &area(), 200.0, 100);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found[0].contains("above the first part"), "{found:?}");
        assert!(found[1].contains("100 design px of empty space between two parts"));
    }

    #[test]
    fn an_area_without_parts_is_an_empty_band() {
        let image = screen(&[]);
        let found = empty_bands(&image, &area(), 200.0, 100);
        assert_eq!(found.len(), 1, "{found:?}");
    }

    #[test]
    fn the_space_after_the_last_part_ends_the_area() {
        let image = screen(&[(10, 30)]);
        assert_eq!(empty_bands(&image, &area(), 200.0, 100), Vec::<String>::new());
    }
}
