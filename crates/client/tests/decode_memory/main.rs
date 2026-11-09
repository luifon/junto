//! The memory and the time of the decoding of a large photo for the viewer, before and
//! after the reduced decodings of ADR 0408: the bytes of Rust allocations that the decoding
//! holds at most beyond the file, and the time on one thread. It does nothing without
//! `JUNTO_TEST_DECODE_MEMORY=1`.
//!
//! ```sh
//! JUNTO_TEST_DECODE_MEMORY=1 cargo test --release -p junto-client --features test-support --test decode_memory
//! ```

use std::time::Instant;

#[global_allocator]
static ALLOCATOR: junto_alloc_count::Counting = junto_alloc_count::Counting::new();

/// The size of the viewer on a screen of 2560 × 1440, and the largest picture of a list at
/// the display scale of 2 (ADR 0740).
const VIEWER: (u32, u32) = (2400, 1300);
const LIST: (u32, u32) = (584, 440);
const MIB: f64 = 1024.0 * 1024.0;

/// A photo-like picture: smooth colours with fine detail, so that the files have the size
/// of a camera picture of the same size.
fn pixel(x: u32, y: u32, (width, height): (u32, u32)) -> [u8; 3] {
    let fine = ((x * 7 + y * 13) % 23) as u8;
    [
        (x * 255 / width) as u8 ^ fine,
        (y * 255 / height) as u8,
        ((x + y) * 127 / (width + height)) as u8 + fine,
    ]
}

fn jpeg((width, height): (u32, u32)) -> Vec<u8> {
    let mut rgb = Vec::with_capacity(width as usize * height as usize * 3);
    for y in 0..height {
        for x in 0..width {
            rgb.extend_from_slice(&pixel(x, y, (width, height)));
        }
    }
    let mut file = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut file, 90)
        .encode(&rgb, width, height, image::ExtendedColorType::Rgb8)
        .unwrap_or_else(|error| panic!("the JPEG is encoded: {error}"));
    file
}

/// A PNG written row by row, so that the picture is not held whole to make it.
fn png((width, height): (u32, u32)) -> Vec<u8> {
    let mut file = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut file, width, height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .unwrap_or_else(|error| panic!("the PNG header is written: {error}"));
        let mut stream = writer
            .stream_writer()
            .unwrap_or_else(|error| panic!("the PNG stream starts: {error}"));
        let mut row = Vec::with_capacity(width as usize * 3);
        for y in 0..height {
            row.clear();
            for x in 0..width {
                row.extend_from_slice(&pixel(x, y, (width, height)));
            }
            std::io::Write::write_all(&mut stream, &row)
                .unwrap_or_else(|error| panic!("a PNG row is written: {error}"));
        }
        stream
            .finish()
            .unwrap_or_else(|error| panic!("the PNG ends: {error}"));
    }
    file
}

/// Decodes the file as the window does and gives the result, the most bytes held beyond
/// those held before, and the time.
fn measure(
    file: &[u8],
    media_type: &str,
    size: (u32, u32),
    fit: (u32, u32),
    reduce: bool,
) -> (Option<(u32, u32)>, f64, f64) {
    ALLOCATOR.reset_peak();
    let before = ALLOCATOR.held();
    let started = Instant::now();
    let decoded = junto_client::shell::decode_still(file, media_type, size, fit, reduce);
    let time = started.elapsed().as_secs_f64() * 1000.0;
    let peak = ALLOCATOR.peak().saturating_sub(before) as f64 / MIB;
    (decoded, peak, time)
}

/// A photo of the measurement: its format, its size, the name and the size of the plan,
/// and whether it is decoded reduced.
type Photo = (&'static str, (u32, u32), (&'static str, (u32, u32)), bool);

fn main() {
    if std::env::var_os("JUNTO_TEST_DECODE_MEMORY").is_none() {
        return;
    }
    println!(
        "| Photo | File | Plan | Decoding | Result | Most memory beyond the file | Time |\n\
         |---|---|---|---|---|---|---|"
    );
    let photos: [Photo; 8] = [
        ("JPEG", (6000, 4000), ("viewer", VIEWER), true),
        ("JPEG", (6000, 4000), ("viewer", VIEWER), false),
        ("JPEG", (9000, 6000), ("viewer", VIEWER), true),
        ("JPEG", (9000, 6000), ("list", LIST), true),
        ("JPEG", (6000, 4000), ("list", LIST), false),
        ("PNG", (6000, 4000), ("viewer", VIEWER), true),
        ("PNG", (6000, 4000), ("viewer", VIEWER), false),
        ("PNG", (6000, 4000), ("list", LIST), true),
    ];
    let mut failures = Vec::new();
    for (kind, size, (plan, fit), reduce) in photos {
        let (file, media_type) = if kind == "JPEG" {
            (jpeg(size), "image/jpeg")
        } else {
            (png(size), "image/png")
        };
        let (decoded, peak, time) = measure(&file, media_type, size, fit, reduce);
        println!(
            "| {kind} {} × {} | {:.1} MiB | {plan} | {} | {} | {peak:.1} MiB | {time:.0} ms |",
            size.0,
            size.1,
            file.len() as f64 / MIB,
            if reduce {
                "reduced"
            } else {
                "whole, then reduced"
            },
            decoded.map_or("none".to_owned(), |(width, height)| format!(
                "{width} × {height}"
            )),
        );
        // The bound that the gate of the decodings reserves for a reduced decoding besides
        // the file: 24 bytes for each planned pixel, the result of 4 bytes, 40 bytes for
        // each pixel of the longest side, and 1 MiB. A copy of the file would exceed it
        // for a list plan.
        let planned = decoded.map_or(0.0, |(width, height)| f64::from(width) * f64::from(height));
        let bound = (planned * 28.0) / MIB + 1.0 + f64::from(size.0.max(size.1)) * 40.0 / MIB;
        if reduce && (decoded.is_none() || peak > bound) {
            failures.push(format!(
                "{kind} {size:?} for the {plan}: a reduced decoding held {peak:.1} MiB, above {bound:.1} MiB"
            ));
        }
    }
    if !failures.is_empty() {
        eprintln!("{failures:#?}");
        std::process::exit(1);
    }
}
