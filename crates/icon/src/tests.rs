use super::*;

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

/// The discs of the source in the coordinates of its 1024 by 1024 view: the body of the
/// icon is moved by 100 to the right and down.
const A: (f64, f64) = (512.0, 400.0);
const B: (f64, f64) = (622.9, 592.0);
const C: (f64, f64) = (401.1, 592.0);
const RADIUS: f64 = 196.0;

fn inside(point: (f64, f64), centre: (f64, f64)) -> bool {
    (point.0 - centre.0).hypot(point.1 - centre.1) < RADIUS
}

/// Points inside each area of the drawing, far from every edge, with the colour that the
/// source gives that area.
const AREAS: [(&str, (f64, f64), [u8; 3]); 9] = [
    ("all three discs", (512.0, 528.0), [0xfa, 0xf5, 0xea]),
    ("the upper disc only", (512.0, 250.0), [0xb4, 0x53, 0x2f]),
    ("the right disc only", (752.9, 667.0), [0xc7, 0x9a, 0x45]),
    ("the left disc only", (271.1, 667.0), [0x2f, 0x4b, 0x3c]),
    (
        "the upper and the right disc",
        (638.4, 455.0),
        [0x8a, 0x4a, 0x22],
    ),
    (
        "the right and the left disc",
        (512.0, 674.0),
        [0x4f, 0x5a, 0x2e],
    ),
    (
        "the upper and the left disc",
        (385.6, 455.0),
        [0x5a, 0x30, 0x22],
    ),
    (
        "the background above the discs",
        (512.0, 140.0),
        [0xe3, 0xd7, 0xc1],
    ),
    (
        "the background left of the discs",
        (140.0, 512.0),
        [0xe3, 0xd7, 0xc1],
    ),
];

#[test]
fn the_sample_points_lie_in_the_areas_that_they_name() {
    let discs = |point| [inside(point, A), inside(point, B), inside(point, C)];
    let expected = [
        [true, true, true],
        [true, false, false],
        [false, true, false],
        [false, false, true],
        [true, true, false],
        [false, true, true],
        [true, false, true],
        [false, false, false],
        [false, false, false],
    ];
    for ((name, point, _), discs_of_area) in AREAS.iter().zip(expected) {
        assert_eq!(discs(*point), discs_of_area, "{name}");
    }
}

fn check_areas(size: u32) {
    let picture = render(size).unwrap();
    assert_eq!(picture.rgba.len(), (size * size * 4) as usize);
    let at = |point: (f64, f64)| {
        let scale = f64::from(size) / 1024.0;
        picture.pixel((point.0 * scale) as u32, (point.1 * scale) as u32)
    };
    for (name, point, colour) in AREAS {
        let pixel = at(point);
        assert_eq!(pixel[3], 255, "{name} at {size} pixels is not opaque");
        for channel in 0..3 {
            assert!(
                pixel[channel].abs_diff(colour[channel]) <= 1,
                "{name} at {size} pixels: {pixel:?}, not {colour:?}"
            );
        }
    }
    for corner in [
        (20.0, 20.0),
        (1004.0, 20.0),
        (20.0, 1004.0),
        (1004.0, 1004.0),
    ] {
        assert_eq!(at(corner)[3], 0, "the corner {corner:?} at {size} pixels");
    }
}

#[test]
fn a_rendering_has_the_colours_of_the_source_in_each_area() {
    check_areas(LARGEST);
    check_areas(32);
}

#[test]
fn the_rounded_square_casts_a_shadow_below_it() {
    let picture = render(LARGEST).unwrap();
    let [red, green, blue, alpha] = picture.pixel(512, 934);
    assert_eq!([red, green, blue], [0, 0, 0]);
    assert!(
        (20..=60).contains(&alpha),
        "the shadow has the alpha {alpha}"
    );
    assert_eq!(picture.pixel(512, 1010)[3], 0);
}

#[test]
fn a_size_outside_the_range_is_refused() {
    assert!(matches!(render(0), Err(Error::Size(0))));
    assert!(matches!(png(LARGEST + 1), Err(Error::Size(1025))));
}

#[test]
fn the_png_picture_is_the_rendering() {
    let picture = png(16).unwrap();
    assert!(picture.starts_with(PNG_SIGNATURE));
    assert_eq!(&picture[12..16], b"IHDR");
    assert_eq!(&picture[16..24], &[0, 0, 0, 16, 0, 0, 0, 16]);
}

fn be(bytes: &[u8]) -> u32 {
    u32::from_be_bytes(bytes.try_into().unwrap())
}

fn le32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes(bytes.try_into().unwrap())
}

fn le16(bytes: &[u8]) -> u16 {
    u16::from_le_bytes(bytes.try_into().unwrap())
}

fn run_length_decoded(code: &[u8], count: usize) -> (Vec<u8>, usize) {
    let mut values = Vec::new();
    let mut position = 0;
    while values.len() < count {
        let byte = code[position];
        if byte >= 128 {
            values.extend(std::iter::repeat_n(code[position + 1], byte as usize - 125));
            position += 2;
        } else {
            values.extend_from_slice(&code[position + 1..position + 2 + byte as usize]);
            position += 2 + byte as usize;
        }
    }
    assert_eq!(values.len(), count);
    (values, position)
}

#[test]
fn the_run_length_code_repeats_runs_and_copies_the_other_values() {
    let mut values = vec![7, 8, 8, 9, 9, 9];
    values.extend(std::iter::repeat_n(1, 135));
    values.extend((0..=255).cycle().take(300));
    let code = run_length(&values);
    assert_eq!(&code[..9], &[2, 7, 8, 8, 128, 9, 255, 1, 130]);
    assert_eq!(
        run_length_decoded(&code, values.len()),
        (values, code.len())
    );
    assert!(run_length(&[]).is_empty());
}

#[test]
fn argb_rounds_half_channel_values_up() {
    let data = argb(&[57, 54, 49, 98]);
    assert_eq!(&data[..4], b"ARGB");
    assert_eq!(&data[4..], &[0, 98, 0, 148, 0, 141, 0, 128]);
}

#[test]
fn the_macos_icon_file_holds_a_rendering_for_each_entry() {
    let file = icns().unwrap();
    assert_eq!(&file[..4], b"icns");
    assert_eq!(be(&file[4..8]) as usize, file.len());
    let mut position = 8;
    for (kind, size, encoding) in ICNS {
        assert_eq!(&file[position..position + 4], kind);
        let end = position + be(&file[position + 4..position + 8]) as usize;
        let data = &file[position + 8..end];
        match encoding {
            Encoding::Png => assert_eq!(data, png(size).unwrap().as_slice()),
            Encoding::Argb => {
                assert_eq!(&data[..4], b"ARGB");
                let count = (size * size) as usize;
                let mut channels = Vec::new();
                let mut read = 4;
                for _ in 0..4 {
                    let (values, used) = run_length_decoded(&data[read..], count);
                    channels.push(values);
                    read += used;
                }
                assert_eq!(read, data.len());
                let rgba: Vec<u8> = (0..count)
                    .flat_map(|pixel| [1, 2, 3, 0].map(|channel| channels[channel][pixel]))
                    .collect();
                for (actual, expected) in rgba
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .zip(render(size).unwrap().rgba.as_chunks::<4>().0)
                {
                    assert_eq!(actual[3], expected[3]);
                    for channel in 0..3 {
                        assert!(actual[channel].abs_diff(expected[channel]) <= 1);
                    }
                }
            }
        }
        position = end;
    }
    assert_eq!(position, file.len());
}

/// The picture that Windows reads from one entry of the icon file or of the resource.
fn windows_picture(data: &[u8], size: u32) -> Picture {
    if size == 256 {
        assert_eq!(data, png(256).unwrap().as_slice());
        return render(256).unwrap();
    }
    assert_eq!(le32(&data[0..4]), 40);
    assert_eq!(le32(&data[4..8]), size);
    assert_eq!(le32(&data[8..12]), size * 2);
    assert_eq!(le16(&data[12..14]), 1);
    assert_eq!(le16(&data[14..16]), 32);
    let pixels = &data[40..40 + (size * size * 4) as usize];
    let mask = &data[40 + pixels.len()..];
    let mask_row = size.div_ceil(32) as usize * 4;
    assert_eq!(mask.len(), mask_row * size as usize);
    let mut rgba = Vec::new();
    for y in (0..size as usize).rev() {
        for x in 0..size as usize {
            let start = (y * size as usize + x) * 4;
            let [blue, green, red, alpha] = pixels[start..start + 4].try_into().unwrap();
            let transparent = mask[y * mask_row + x / 8] & (0x80 >> (x % 8)) != 0;
            assert_eq!(transparent, alpha == 0, "the mask at {x}, {y} of {size}");
            rgba.extend_from_slice(&[red, green, blue, alpha]);
        }
    }
    Picture { size, rgba }
}

#[test]
fn the_windows_icon_file_holds_each_size_as_windows_reads_it() {
    let file = ico().unwrap();
    assert_eq!(&file[..6], &[0, 0, 1, 0, ICO.len() as u8, 0]);
    for (index, size) in ICO.into_iter().enumerate() {
        let entry = &file[6 + 16 * index..22 + 16 * index];
        let side = if size == 256 { 0 } else { size as u8 };
        assert_eq!(&entry[..4], &[side, side, 0, 0]);
        assert_eq!(le16(&entry[4..6]), 1);
        assert_eq!(le16(&entry[6..8]), 32);
        let length = le32(&entry[8..12]) as usize;
        let offset = le32(&entry[12..16]) as usize;
        let picture = windows_picture(&file[offset..offset + length], size);
        assert_eq!(
            picture,
            render(size).unwrap(),
            "the picture of {size} pixels"
        );
    }
}

#[test]
fn the_windows_resource_holds_the_pictures_and_the_group_that_lists_them() {
    let file = windows_resource().unwrap();
    let mut resources = Vec::new();
    let mut position = 0;
    while position < file.len() {
        let length = le32(&file[position..position + 4]) as usize;
        assert_eq!(le32(&file[position + 4..position + 8]), 32);
        assert_eq!(le16(&file[position + 8..position + 10]), 0xffff);
        assert_eq!(le16(&file[position + 12..position + 14]), 0xffff);
        let kind = le16(&file[position + 10..position + 12]);
        let name = le16(&file[position + 14..position + 16]);
        let data = &file[position + 32..position + 32 + length];
        resources.push((kind, name, data.to_vec()));
        position = (position + 32 + length).next_multiple_of(4);
    }
    assert_eq!(position, file.len());
    assert_eq!((resources[0].0, resources[0].1), (0, 0));
    assert!(resources[0].2.is_empty());
    let (kind, name, group) = resources.last().unwrap();
    assert_eq!((*kind, *name), (RT_GROUP_ICON, ICON_GROUP));
    assert_eq!(&group[..6], &[0, 0, 1, 0, ICO.len() as u8, 0]);
    for (index, size) in ICO.into_iter().enumerate() {
        let entry = &group[6 + 14 * index..20 + 14 * index];
        let number = le16(&entry[12..14]);
        let (_, _, data) = resources
            .iter()
            .find(|(kind, name, _)| (*kind, *name) == (RT_ICON, number))
            .unwrap();
        assert_eq!(le32(&entry[8..12]) as usize, data.len());
        let picture = windows_picture(data, size);
        assert_eq!(
            picture,
            render(size).unwrap(),
            "the picture of {size} pixels"
        );
    }
    assert_eq!(resources.len(), ICO.len() + 2);
}

fn hex(colour: &str) -> [u8; 3] {
    let value = u32::from_str_radix(colour.trim_start_matches('#'), 16).unwrap();
    [(value >> 16) as u8, (value >> 8) as u8, value as u8]
}

/// True when two colours differ by at most `by` in each channel.
fn near(seen: [u8; 4], wanted: [u8; 3], by: u8) -> bool {
    seen[..3]
        .iter()
        .zip(wanted)
        .all(|(seen, wanted)| seen.abs_diff(wanted) <= by)
}

#[test]
fn a_colour_tray_picture_is_a_disc_of_its_state_with_a_white_sign() {
    for side in TRAY {
        // Outside a call: the body of the icon fills the picture. The middle of each edge
        // is opaque, a corner outside the rounded body is transparent.
        let plain = tray(TrayState::Plain, false, TrayStyle::Colour, side).unwrap();
        assert_eq!(plain.rgba.len(), (side * side * 4) as usize);
        assert_eq!(plain.pixel(0, side / 2)[3], 255, "{side}");
        assert_eq!(plain.pixel(side - 1, side / 2)[3], 255, "{side}");
        assert_eq!(plain.pixel(0, 0)[3], 0, "{side}");
        for state in [
            TrayState::Call,
            TrayState::Muted,
            TrayState::Held,
            TrayState::SoundOff,
        ] {
            let picture = tray(state, false, TrayStyle::Colour, side).unwrap();
            let disc = hex(state.disc().unwrap());
            // Near the left edge of the disc, between the edge and the sign: the colour of
            // the state, opaque. A corner is outside the disc.
            let edge = picture.pixel(side * 3 / 48, side / 2);
            assert!(
                edge[3] == 255 && near(edge, disc, 2),
                "{state:?} {side}: {edge:?}"
            );
            assert_eq!(picture.pixel(0, 0)[3], 0, "{state:?} {side}");
            // Some pixel of the middle half is near white: the sign.
            let white = (side / 4..side * 3 / 4).any(|y| {
                (side / 4..side * 3 / 4).any(|x| near(picture.pixel(x, y), [255, 255, 255], 40))
            });
            assert!(white, "{state:?} {side}: no white sign");
        }
    }
    // The states differ from each other in the middle of the picture.
    let middle = |state| {
        let picture = tray(state, false, TrayStyle::Colour, 48).unwrap();
        (0..48)
            .flat_map(|y| (0..48).map(move |x| (x, y)))
            .map(|(x, y)| picture.pixel(x, y))
            .collect::<Vec<_>>()
    };
    let all: Vec<Vec<[u8; 4]>> = TrayState::ALL.into_iter().map(middle).collect();
    for (index, one) in all.iter().enumerate() {
        for other in &all[index + 1..] {
            assert_ne!(one, other);
        }
    }
    assert!(matches!(
        tray(TrayState::Call, false, TrayStyle::Colour, 0),
        Err(Error::Size(0))
    ));
}

#[test]
fn a_template_tray_picture_is_its_sign_in_black_on_transparent() {
    for side in MENU_BAR {
        for state in TrayState::ALL {
            let picture = tray(state, false, TrayStyle::Template, side).unwrap();
            let mut drawn = 0;
            for y in 0..side {
                for x in 0..side {
                    let [red, green, blue, alpha] = picture.pixel(x, y);
                    if alpha > 0 {
                        drawn += 1;
                        // A template picture has no colour: the system takes its alpha.
                        assert_eq!([red, green, blue], [0, 0, 0], "{state:?} {side}");
                    }
                }
            }
            assert!(
                drawn > (side * side / 16) as usize,
                "{state:?} {side}: {drawn}"
            );
            assert_eq!(picture.pixel(0, side - 1)[3], 0, "{state:?} {side}");
        }
    }
    assert!(
        tray_png(TrayState::Call, false, TrayStyle::Template, MENU_BAR[1])
            .unwrap()
            .starts_with(PNG_SIGNATURE)
    );
}

#[test]
fn the_mention_mark_is_a_dot_at_the_upper_right_corner_in_each_state_and_style() {
    for state in TrayState::ALL {
        for side in [MENU_BAR[0], MENU_BAR[1], 16, 32] {
            let dot = (side * 19 / 24, side * 5 / 24);
            // The template: the dot is drawn, and a ring around it is cut out of the sign.
            let marked = tray(state, true, TrayStyle::Template, side).unwrap();
            assert_eq!(marked.pixel(dot.0, dot.1)[3], 255, "{state:?} {side}");
            // A pixel whose centre is between the dot (radius 4) and the edge of the cut
            // (radius 6), below the dot.
            let ring = (side * 19 / 24, side * 10 / 24);
            let cut = marked.pixel(ring.0, ring.1)[3];
            assert!(cut < 32, "{state:?} {side}: the cut {cut}");
            // The colour style: the red dot.
            let marked = tray(state, true, TrayStyle::Colour, side).unwrap();
            let seen = marked.pixel(dot.0, dot.1);
            assert!(
                seen[3] == 255 && near(seen, hex(MENTION_RED), 30),
                "{state:?} {side}: {seen:?}"
            );
            let plain = tray(state, false, TrayStyle::Colour, side).unwrap();
            assert_ne!(plain.pixel(dot.0, dot.1), seen, "{state:?} {side}");
        }
    }
}
