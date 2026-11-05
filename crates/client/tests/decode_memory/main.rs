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

/// The size of the viewer on a screen of 2560 × 1440.
const FIT: (u32, u32) = (2400, 1300);
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
    reduce: bool,
) -> (Option<(u32, u32)>, f64, f64) {
    ALLOCATOR.reset_peak();
    let before = ALLOCATOR.held();
    let started = Instant::now();
    let decoded = junto_client::shell::decode_still(file, media_type, size, FIT, reduce);
    let time = started.elapsed().as_secs_f64() * 1000.0;
    let peak = ALLOCATOR.peak().saturating_sub(before) as f64 / MIB;
    (decoded, peak, time)
}

fn main() {
    if std::env::var_os("JUNTO_TEST_DECODE_MEMORY").is_none() {
        return;
    }
    println!(
        "| Photo | File | Decoding | Result | Most memory beyond the file | Time |\n|---|---|---|---|---|---|"
    );
    let photos: [(&str, (u32, u32), bool); 5] = [
        ("JPEG", (6000, 4000), true),
        ("JPEG", (6000, 4000), false),
        ("JPEG", (9000, 6000), true),
        ("PNG", (6000, 4000), true),
        ("PNG", (6000, 4000), false),
    ];
    let mut failures = Vec::new();
    for (kind, size, reduce) in photos {
        let (file, media_type) = if kind == "JPEG" {
            (jpeg(size), "image/jpeg")
        } else {
            (png(size), "image/png")
        };
        let (decoded, peak, time) = measure(&file, media_type, size, reduce);
        println!(
            "| {kind} {} × {} | {:.1} MiB | {} | {} | {peak:.1} MiB | {time:.0} ms |",
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
        // The bound that the gate of the decodings reserves for a reduced decoding: 24
        // bytes for each planned pixel, the result of 4 bytes, and 1 MiB.
        let planned = decoded.map_or(0.0, |(width, height)| f64::from(width) * f64::from(height));
        let bound = (planned * 28.0) / MIB + 1.0 + f64::from(size.0.max(size.1)) * 40.0 / MIB;
        if reduce && (decoded.is_none() || peak > bound) {
            failures.push(format!(
                "{kind} {size:?}: a reduced decoding held {peak:.1} MiB, above {bound:.1} MiB"
            ));
        }
    }
    if !failures.is_empty() {
        eprintln!("{failures:#?}");
        std::process::exit(1);
    }
}
