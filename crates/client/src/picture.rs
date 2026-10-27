//! A picture that a person chooses for an account, for the starting account of new servers
//! or as the logo of a server (ADR 0119): decoded from a file of at most 10 MiB, cut to the
//! square that the person keeps, scaled to at most 1024 by 1024 pixels and encoded under the
//! limit of a server.

use std::io::Cursor;

use image::{DynamicImage, ImageFormat, ImageReader, Limits, RgbaImage, imageops::FilterType};
use junto_client_core::profile::{AVATAR_MAX_BYTES, CHOSEN_PICTURE_MAX_BYTES};

/// The longest side of a stored picture, as a server takes it.
pub const STORED_SIDE: u32 = 1024;
/// The longest side that a chosen picture keeps for the cut. A larger one is scaled down
/// at once: the stored picture has at most [`STORED_SIDE`], and memory stays bounded.
const HELD_SIDE: u32 = 4096;
/// The largest side and the memory that a decoded file may take.
const DECODED_SIDE: u32 = 16_384;
const DECODED_BYTES: u64 = 512 * 1024 * 1024;

/// Why a chosen file is not a picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The file could not be read.
    Unreadable,
    /// The file is larger than 10 MiB.
    TooLarge,
    /// The file is not a PNG, JPEG, WebP or GIF picture that can be read.
    Unsupported,
    /// The picture has more pixels than a picture can be read with.
    TooManyPixels,
}

/// A chosen picture, held for the cut, with its longest side at most 4096 pixels.
#[derive(Debug, Clone)]
pub struct Chosen {
    pub image: RgbaImage,
}

impl Chosen {
    pub fn width(&self) -> u32 {
        self.image.width()
    }

    pub fn height(&self) -> u32 {
        self.image.height()
    }
}

/// Decodes the bytes of a chosen file. GIF gives its first frame.
pub fn decode(bytes: &[u8]) -> Result<Chosen, Refusal> {
    if bytes.len() > CHOSEN_PICTURE_MAX_BYTES {
        return Err(Refusal::TooLarge);
    }
    let format = image::guess_format(bytes).map_err(|_| Refusal::Unsupported)?;
    if !matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP | ImageFormat::Gif
    ) {
        return Err(Refusal::Unsupported);
    }
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(DECODED_SIDE);
    limits.max_image_height = Some(DECODED_SIDE);
    limits.max_alloc = Some(DECODED_BYTES);
    reader.limits(limits);
    let decoded = reader.decode().map_err(|error| match error {
        image::ImageError::Limits(_) => Refusal::TooManyPixels,
        _ => Refusal::Unsupported,
    })?;
    if decoded.width() == 0 || decoded.height() == 0 {
        return Err(Refusal::Unsupported);
    }
    let held = if decoded.width().max(decoded.height()) > HELD_SIDE {
        decoded.resize(HELD_SIDE, HELD_SIDE, FilterType::Triangle)
    } else {
        decoded
    };
    Ok(Chosen {
        image: held.into_rgba8(),
    })
}

/// The square of a chosen picture that the person keeps, in pixels of that picture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Crop {
    pub left: f32,
    pub top: f32,
    pub side: f32,
}

impl Crop {
    /// The square of side `min(width, height) / zoom` around `centre`, kept inside the
    /// picture. A zoom of 1 is the largest square.
    pub fn around(width: u32, height: u32, zoom: f32, centre: (f32, f32)) -> Self {
        let (width, height) = (width as f32, height as f32);
        let side = width.min(height) / zoom.max(1.0);
        let left = (centre.0 - side / 2.0).clamp(0.0, width - side);
        let top = (centre.1 - side / 2.0).clamp(0.0, height - side);
        Self { left, top, side }
    }

    /// The centre of the square.
    pub fn centre(&self) -> (f32, f32) {
        (self.left + self.side / 2.0, self.top + self.side / 2.0)
    }
}

/// Cuts the square out of the picture, scales it to at most 1024 by 1024 pixels and
/// encodes it under the limit of a server: a picture with transparent points as PNG, the
/// others as JPEG. A PNG above the limit is scaled down further; a JPEG above it is
/// encoded with a lower quality.
pub fn encode(chosen: &Chosen, crop: Crop) -> Result<Vec<u8>, Refusal> {
    let image = &chosen.image;
    let left = crop.left.max(0.0).floor() as u32;
    let top = crop.top.max(0.0).floor() as u32;
    let side = (crop.side.round() as u32)
        .max(1)
        .min(image.width() - left.min(image.width() - 1))
        .min(image.height() - top.min(image.height() - 1));
    let square = image::imageops::crop_imm(image, left, top, side, side).to_image();
    let transparent = square.pixels().any(|pixel| pixel[3] < 255);
    let mut target = side.min(STORED_SIDE);
    loop {
        let scaled = if target == side {
            square.clone()
        } else {
            image::imageops::resize(&square, target, target, FilterType::Lanczos3)
        };
        let encoded = if transparent {
            let mut bytes = Vec::new();
            DynamicImage::ImageRgba8(scaled)
                .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
                .map_err(|_| Refusal::Unsupported)?;
            Some(bytes)
        } else {
            let opaque = DynamicImage::ImageRgba8(scaled).into_rgb8();
            [90u8, 80, 70, 60]
                .into_iter()
                .map(|quality| {
                    let mut bytes = Vec::new();
                    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, quality)
                        .encode_image(&opaque)
                        .map(|()| bytes)
                })
                .find_map(|bytes| bytes.ok().filter(|bytes| bytes.len() <= AVATAR_MAX_BYTES))
        };
        match encoded {
            Some(bytes) if bytes.len() <= AVATAR_MAX_BYTES => return Ok(bytes),
            _ if target > 64 => target = target * 3 / 4,
            _ => return Err(Refusal::TooLarge),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32, alpha: u8) -> Vec<u8> {
        let image = RgbaImage::from_fn(width, height, |x, y| {
            image::Rgba([(x % 251) as u8, (y % 241) as u8, ((x + y) % 239) as u8, alpha])
        });
        let mut bytes = Vec::new();
        DynamicImage::ImageRgba8(image)
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();
        bytes
    }

    #[test]
    fn a_large_picture_is_cut_and_scaled_to_what_a_server_stores() {
        let chosen = decode(&png(3000, 2000, 255)).unwrap();
        assert_eq!((chosen.width(), chosen.height()), (3000, 2000));
        let whole = Crop::around(3000, 2000, 1.0, (1500.0, 1000.0));
        assert_eq!((whole.left, whole.top, whole.side), (500.0, 0.0, 2000.0));
        let stored = encode(&chosen, whole).unwrap();
        assert!(stored.len() <= AVATAR_MAX_BYTES);
        let read = image::load_from_memory(&stored).unwrap();
        assert_eq!((read.width(), read.height()), (STORED_SIDE, STORED_SIDE));
        assert_eq!(image::guess_format(&stored).unwrap(), ImageFormat::Jpeg);
        // A zoomed square near an edge stays inside the picture.
        let zoomed = Crop::around(3000, 2000, 4.0, (0.0, 1990.0));
        assert_eq!((zoomed.left, zoomed.top, zoomed.side), (0.0, 1500.0, 500.0));
        let small = image::load_from_memory(&encode(&chosen, zoomed).unwrap()).unwrap();
        assert_eq!(small.width(), 500);
    }

    #[test]
    fn a_picture_with_transparent_points_stays_a_png() {
        let chosen = decode(&png(300, 300, 128)).unwrap();
        let stored = encode(&chosen, Crop::around(300, 300, 1.0, (150.0, 150.0))).unwrap();
        assert_eq!(image::guess_format(&stored).unwrap(), ImageFormat::Png);
    }

    #[test]
    fn a_gif_gives_its_first_frame_and_other_files_are_refused() {
        let image = RgbaImage::from_pixel(40, 30, image::Rgba([10, 20, 30, 255]));
        let mut gif = Vec::new();
        DynamicImage::ImageRgba8(image)
            .write_to(&mut Cursor::new(&mut gif), ImageFormat::Gif)
            .unwrap();
        let chosen = decode(&gif).unwrap();
        assert_eq!((chosen.width(), chosen.height()), (40, 30));
        assert_eq!(decode(b"not a picture").unwrap_err(), Refusal::Unsupported);
        let mut large = png(8, 8, 255);
        large.resize(CHOSEN_PICTURE_MAX_BYTES + 1, 0);
        assert_eq!(decode(&large).unwrap_err(), Refusal::TooLarge);
    }

    #[test]
    fn a_held_picture_has_at_most_its_side() {
        let chosen = decode(&png(5000, 100, 255)).unwrap();
        assert_eq!(chosen.width(), HELD_SIDE);
    }
}
