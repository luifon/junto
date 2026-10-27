//! The decoding of a picture of a message (ADR 0740): a still picture, or the frames of a
//! GIF, an animated WebP or an animated PNG, reduced to the size at which the window draws
//! it, with a bound on the bytes of its frames and on the memory of the decodings that run
//! at one time.

use std::collections::VecDeque;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use futures_channel::oneshot;
use gpui::RenderImage;
use image::{AnimationDecoder, Delay, Frame, ImageDecoder, RgbaImage};
use junto_client_core::protocol::message::Attachment;

/// The most pixels of a still picture, 128 MiB as RGBA, and of the canvas of an animation,
/// 16 MiB as RGBA, and the most frames: the bounds of the server (ADR 0408).
pub(crate) const MOST_PICTURE_PIXELS: u64 = 33_554_432;
pub(crate) const MOST_FRAME_PIXELS: u64 = 4_194_304;
pub(crate) const MOST_FRAMES: u32 = 2_048;
/// The most bytes of the frames of one animation in a message list, and in the viewer.
pub(crate) const LIST_FRAME_BYTES: u64 = 16 * 1024 * 1024;
pub(crate) const VIEWER_FRAME_BYTES: u64 = 64 * 1024 * 1024;
/// The most bytes that the decodings of pictures of the process hold at one time: the
/// read files, the decoded pictures at their own size, and the reduced copies. A decoding
/// that needs more starts when no other runs.
pub(crate) const MOST_DECODING_BYTES: u64 = 160 * 1024 * 1024;

/// What the server states of a picture: its size, or the size of the canvas of an
/// animation, and its frames, 1 for a still picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Stated {
    pub width: u32,
    pub height: u32,
    pub frames: u32,
}

impl Stated {
    /// What the server states of a file that it marked as a picture that a client draws
    /// (ADR 0408). `None` for every other file.
    pub(crate) fn of(attachment: &Attachment) -> Option<Self> {
        if !matches!(
            attachment.media_type.as_str(),
            "image/png" | "image/jpeg" | "image/webp" | "image/gif"
        ) {
            return None;
        }
        let (width, height) = attachment.width.zip(attachment.height)?;
        let pixels = u64::from(width) * u64::from(height);
        let frames = attachment.frames.unwrap_or(1);
        let within = width >= 1
            && height >= 1
            && pixels <= MOST_PICTURE_PIXELS
            && (frames == 1
                || ((2..=MOST_FRAMES).contains(&frames) && pixels <= MOST_FRAME_PIXELS));
        within.then_some(Self {
            width,
            height,
            frames,
        })
    }

    fn pixels(self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }
}

/// The size of the decoded frames, in pixels, and the step between the frames that are
/// kept: 1 keeps every frame, 2 every second frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Plan {
    pub width: u32,
    pub height: u32,
    pub step: u32,
}

impl Plan {
    /// The bytes of the decoded picture as RGBA.
    pub(crate) fn bytes(self, stated: Stated) -> u64 {
        u64::from(self.width)
            * u64::from(self.height)
            * 4
            * u64::from(stated.frames.max(1).div_ceil(self.step.max(1)))
    }

    /// The decoded picture has the size and every frame of the file.
    pub(crate) fn whole(self, stated: Stated) -> bool {
        (self.width, self.height, self.step) == (stated.width, stated.height, 1)
    }
}

fn scaled((width, height): (u32, u32), scale: f64) -> (u32, u32) {
    let side = |value: u32| ((f64::from(value) * scale).round() as u32).max(1);
    (side(width), side(height))
}

/// A size reduced by `scale` that holds at most `scale` squared of the pixels.
fn scaled_down((width, height): (u32, u32), scale: f64) -> (u32, u32) {
    let side = |value: u32| ((f64::from(value) * scale).floor() as u32).max(1);
    (side(width), side(height))
}

/// How a picture is decoded: its own size reduced with its proportions to fit `fit`, the
/// size in pixels of the device at which the window draws it (`None` keeps its own size).
/// The frames of an animation take at most `budget` bytes: above it the frames are reduced
/// further, to half of that size at the most, and then only every n-th frame is kept, with
/// the delays of the frames between them added to it.
pub(crate) fn plan(stated: Stated, fit: Option<(u32, u32)>, budget: u64) -> Plan {
    let own = (stated.width.max(1), stated.height.max(1));
    let scale = fit.map_or(1.0, |(width, height)| {
        (f64::from(width.max(1)) / f64::from(own.0))
            .min(f64::from(height.max(1)) / f64::from(own.1))
            .min(1.0)
    });
    let mut size = if scale < 1.0 { scaled(own, scale) } else { own };
    let frames = u64::from(stated.frames.max(1));
    let bytes = |(width, height): (u32, u32)| u64::from(width) * u64::from(height) * 4;
    if frames > 1 && bytes(size) * frames > budget {
        let reduce = (budget as f64 / (bytes(size) * frames) as f64)
            .sqrt()
            .max(0.5);
        size = scaled_down(size, reduce);
    }
    let fitting = (budget / bytes(size)).max(1);
    let step = if frames > 1 {
        u32::try_from(frames.div_ceil(fitting)).unwrap_or(u32::MAX)
    } else {
        1
    };
    Plan {
        width: size.0,
        height: size.1,
        step,
    }
}

/// The bytes that a decoding of a file of `bytes` with this plan holds at most: the file,
/// the picture or one canvas of an animation at its own size (the decoder holds about three
/// canvases), and the reduced copy.
pub(crate) fn decoding_bytes(bytes: u64, stated: Stated, plan: Plan) -> u64 {
    let canvas = stated.pixels() * 4;
    let working = if stated.frames > 1 {
        canvas * 3
    } else {
        canvas
    };
    bytes + working + plan.bytes(stated)
}

/// The bytes that the frames of an image hold.
pub(crate) fn held_bytes(image: &RenderImage) -> u64 {
    (0..image.frame_count())
        .filter_map(|frame| image.as_bytes(frame))
        .map(|bytes| bytes.len() as u64)
        .sum()
}

/// The window draws the colours in the order blue, green, red.
fn into_frame(mut picture: RgbaImage, delay: Delay) -> Frame {
    for pixel in picture.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Frame::from_parts(picture, 0, 0, delay)
}

fn reduced(picture: &RgbaImage, plan: Plan) -> RgbaImage {
    if picture.dimensions() == (plan.width, plan.height) {
        picture.clone()
    } else {
        image::imageops::thumbnail(picture, plan.width, plan.height)
    }
}

/// The decoder refuses a picture larger than the stated size, and an allocation of more
/// than `allocation` bytes.
fn limits(stated: Stated, allocation: u64) -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(stated.width);
    limits.max_image_height = Some(stated.height);
    limits.max_alloc = Some(allocation);
    limits
}

/// Decodes a picture that another account sent, with a plan from [`plan`]. The decoder
/// refuses a picture that is larger than the size that the server states, and an animation
/// with more frames than the server states, whatever the file states. An animation whose
/// file ends early keeps the frames that were decoded.
pub(crate) fn decode(bytes: &[u8], stated: Stated, plan: Plan) -> Option<Arc<RenderImage>> {
    let frames = if stated.frames > 1 {
        frames(bytes, stated, plan)?
    } else {
        use image::ImageDecoder as _;
        let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
            .with_guessed_format()
            .ok()?;
        // The server states the size after the turn of the EXIF orientation (ADR 0408); the
        // stored picture can have its width and height exchanged.
        let side = stated.width.max(stated.height);
        let either = Stated {
            width: side,
            height: side,
            ..stated
        };
        reader.limits(limits(either, MOST_PICTURE_PIXELS * 4));
        let mut decoder = reader.into_decoder().ok()?;
        let orientation = decoder
            .orientation()
            .unwrap_or(image::metadata::Orientation::NoTransforms);
        let mut decoded = image::DynamicImage::from_decoder(decoder).ok()?;
        decoded.apply_orientation(orientation);
        let landscape = |(width, height): (u32, u32)| width >= height;
        let decoded = if (decoded.width(), decoded.height()) == (plan.width, plan.height) {
            decoded
        } else if landscape((decoded.width(), decoded.height()))
            == landscape((plan.width, plan.height))
        {
            decoded.thumbnail_exact(plan.width, plan.height)
        } else {
            // A picture sent before the server read the orientation states the size before
            // the turn: it keeps its proportions in the planned bounds.
            decoded.thumbnail(plan.width, plan.height)
        };
        vec![into_frame(
            decoded.into_rgba8(),
            Delay::from_numer_denom_ms(0, 1),
        )]
    };
    Some(Arc::new(RenderImage::new(frames)))
}

fn frames(bytes: &[u8], stated: Stated, plan: Plan) -> Option<Vec<Frame>> {
    let cursor = || std::io::Cursor::new(bytes);
    let format = image::guess_format(bytes).ok()?;
    let limits = limits(stated, MOST_FRAME_PIXELS * 4 * 2);
    let decoded: image::Frames<'_> = match format {
        image::ImageFormat::Gif => {
            let mut decoder = image::codecs::gif::GifDecoder::new(cursor()).ok()?;
            decoder.set_limits(limits).ok()?;
            decoder.into_frames()
        }
        image::ImageFormat::WebP => {
            let mut decoder = image::codecs::webp::WebPDecoder::new(cursor()).ok()?;
            if !decoder.has_animation() {
                return None;
            }
            decoder.set_limits(limits).ok()?;
            decoder.into_frames()
        }
        image::ImageFormat::Png => {
            let decoder = image::codecs::png::PngDecoder::with_limits(cursor(), limits).ok()?;
            if !decoder.is_apng().ok()? {
                return None;
            }
            decoder.apng().ok()?.into_frames()
        }
        _ => return None,
    };
    let step = plan.step.max(1) as usize;
    let mut kept: Vec<(RgbaImage, Duration)> = Vec::new();
    for (index, frame) in decoded.enumerate() {
        if index >= stated.frames as usize {
            return None;
        }
        let Ok(frame) = frame else {
            break;
        };
        let delay = Duration::from(frame.delay());
        if index % step == 0 {
            let canvas = frame.buffer();
            if canvas.width() > stated.width || canvas.height() > stated.height {
                return None;
            }
            kept.push((reduced(canvas, plan), delay));
        } else if let Some((_, last)) = kept.last_mut() {
            *last = last.saturating_add(delay);
        }
    }
    if kept.is_empty() {
        return None;
    }
    Some(
        kept.into_iter()
            .map(|(picture, delay)| into_frame(picture, Delay::from_saturating_duration(delay)))
            .collect(),
    )
}

/// The memory of the decodings that run (see [`MOST_DECODING_BYTES`]). The decodings that
/// wait start in their order.
pub(crate) struct Gate {
    state: Mutex<GateState>,
}

struct GateState {
    reserved: u64,
    waiting: VecDeque<(u64, oneshot::Sender<Reserved>)>,
}

/// The gate of every decoding of the process, in every window.
static DECODING: LazyLock<Arc<Gate>> = LazyLock::new(Gate::new);

/// Bytes of a [`Gate`] that a decoding holds until the value is dropped.
pub(crate) struct Reserved {
    gate: Arc<Gate>,
    bytes: u64,
}

impl Drop for Reserved {
    fn drop(&mut self) {
        Gate::release(&self.gate, self.bytes);
    }
}

impl Gate {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(GateState {
                reserved: 0,
                waiting: VecDeque::new(),
            }),
        })
    }

    fn fits(state: &GateState, bytes: u64) -> bool {
        state.reserved == 0 || state.reserved + bytes <= MOST_DECODING_BYTES
    }

    fn release(gate: &Arc<Self>, bytes: u64) {
        let Ok(mut state) = gate.state.lock() else {
            return;
        };
        state.reserved = state.reserved.saturating_sub(bytes);
        while let Some(&(next, _)) = state.waiting.front() {
            if !Self::fits(&state, next) {
                break;
            }
            let Some((_, sender)) = state.waiting.pop_front() else {
                break;
            };
            state.reserved += next;
            let given = Reserved {
                gate: Arc::clone(gate),
                bytes: next,
            };
            // A decoding that no longer waits gives the bytes back, which starts the
            // next one.
            if let Err(unused) = sender.send(given) {
                drop(state);
                drop(unused);
                return;
            }
        }
    }

    /// Waits until `bytes` fit next to the decodings that run, or until no other decoding
    /// runs, and holds them.
    pub(crate) async fn reserve(self: Arc<Self>, bytes: u64) -> Option<Reserved> {
        let waiting = {
            let mut state = self.state.lock().ok()?;
            if state.waiting.is_empty() && Self::fits(&state, bytes) {
                state.reserved += bytes;
                drop(state);
                return Some(Reserved { gate: self, bytes });
            }
            let (sender, receiver) = oneshot::channel();
            state.waiting.push_back((bytes, sender));
            receiver
        };
        waiting.await.ok()
    }
}

/// Holds `bytes` of the gate of the process for a decoding (see [`Gate::reserve`]).
pub(crate) async fn reserve(bytes: u64) -> Option<Reserved> {
    Arc::clone(&DECODING).reserve(bytes).await
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn stated(width: u32, height: u32, frames: u32) -> Stated {
        Stated {
            width,
            height,
            frames,
        }
    }

    fn colour(index: u32) -> image::Rgba<u8> {
        image::Rgba([(index * 40 % 256) as u8, 90, 200, 255])
    }

    /// A GIF of this size whose frames have different colours, each shown 40 ms.
    pub(crate) fn gif(width: u32, height: u32, frames: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new_with_speed(&mut bytes, 30);
            encoder
                .set_repeat(image::codecs::gif::Repeat::Infinite)
                .unwrap();
            for index in 0..frames {
                let picture = RgbaImage::from_pixel(width, height, colour(index));
                encoder
                    .encode_frame(Frame::from_parts(
                        picture,
                        0,
                        0,
                        Delay::from_numer_denom_ms(40, 1),
                    ))
                    .unwrap();
            }
        }
        bytes
    }

    fn chunks(bytes: &[u8], start: usize, big_endian: bool) -> Vec<([u8; 4], Vec<u8>)> {
        let mut found = Vec::new();
        let mut at = start;
        while at + 8 <= bytes.len() {
            let length = if big_endian {
                u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap())
            } else {
                u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap())
            } as usize;
            let (kind, data) = if big_endian {
                (&bytes[at + 4..at + 8], &bytes[at + 8..at + 8 + length])
            } else {
                (&bytes[at..at + 4], &bytes[at + 8..at + 8 + length])
            };
            found.push((kind.try_into().unwrap(), data.to_vec()));
            at += 8 + length + if big_endian { 4 } else { length & 1 };
        }
        found
    }

    fn crc(data: &[u8]) -> u32 {
        let mut crc = 0xffff_ffffu32;
        for byte in data {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xedb8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    fn png_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut chunk = (data.len() as u32).to_be_bytes().to_vec();
        let mut checked = kind.to_vec();
        checked.extend_from_slice(data);
        chunk.extend_from_slice(&checked);
        chunk.extend_from_slice(&crc(&checked).to_be_bytes());
        chunk
    }

    /// An animated PNG of this size with frames of different colours, each shown 40 ms.
    pub(crate) fn animated_png(width: u32, height: u32, frames: u32) -> Vec<u8> {
        let mut image = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        let mut header = Vec::new();
        let mut sequence = 0u32;
        for index in 0..frames {
            let mut encoded = Vec::new();
            RgbaImage::from_pixel(width, height, colour(index))
                .write_to(
                    &mut std::io::Cursor::new(&mut encoded),
                    image::ImageFormat::Png,
                )
                .unwrap();
            let parts = chunks(&encoded, 8, true);
            if index == 0 {
                header = parts[0].1.clone();
                image.extend(png_chunk(b"IHDR", &header));
                let mut control = frames.to_be_bytes().to_vec();
                control.extend_from_slice(&0u32.to_be_bytes());
                image.extend(png_chunk(b"acTL", &control));
            }
            let mut control = sequence.to_be_bytes().to_vec();
            sequence += 1;
            control.extend_from_slice(&width.to_be_bytes());
            control.extend_from_slice(&height.to_be_bytes());
            control.extend_from_slice(&[0; 8]);
            control.extend_from_slice(&40u16.to_be_bytes());
            control.extend_from_slice(&1000u16.to_be_bytes());
            control.extend_from_slice(&[0, 0]);
            image.extend(png_chunk(b"fcTL", &control));
            for (kind, data) in parts.iter().filter(|(kind, _)| kind == b"IDAT") {
                if index == 0 {
                    image.extend(png_chunk(kind, data));
                } else {
                    let mut frame = sequence.to_be_bytes().to_vec();
                    sequence += 1;
                    frame.extend_from_slice(data);
                    image.extend(png_chunk(b"fdAT", &frame));
                }
            }
        }
        assert!(!header.is_empty());
        image.extend(png_chunk(b"IEND", &[]));
        image
    }

    /// An animated WebP of this size with lossless frames of different colours, each shown
    /// 40 ms.
    pub(crate) fn animated_webp(width: u32, height: u32, frames: u32) -> Vec<u8> {
        let side = |value: u32| (value - 1).to_le_bytes()[..3].to_vec();
        let mut body = b"WEBP".to_vec();
        let add = |body: &mut Vec<u8>, kind: &[u8], data: &[u8]| {
            body.extend_from_slice(kind);
            body.extend_from_slice(&(data.len() as u32).to_le_bytes());
            body.extend_from_slice(data);
            if data.len() % 2 == 1 {
                body.push(0);
            }
        };
        let mut header = vec![0x02 | 0x10, 0, 0, 0];
        header.extend(side(width));
        header.extend(side(height));
        add(&mut body, b"VP8X", &header);
        add(&mut body, b"ANIM", &[0, 0, 0, 0, 0, 0]);
        for index in 0..frames {
            let mut encoded = Vec::new();
            RgbaImage::from_pixel(width, height, colour(index))
                .write_to(
                    &mut std::io::Cursor::new(&mut encoded),
                    image::ImageFormat::WebP,
                )
                .unwrap();
            let still = chunks(&encoded, 12, false);
            let mut frame = vec![0; 6];
            frame.extend(side(width));
            frame.extend(side(height));
            frame.extend_from_slice(&40u32.to_le_bytes()[..3]);
            frame.push(0x02);
            for (kind, data) in still {
                add(&mut frame, &kind, &data);
            }
            add(&mut body, b"ANMF", &frame);
        }
        let mut image = b"RIFF".to_vec();
        image.extend_from_slice(&(body.len() as u32).to_le_bytes());
        image.extend(body);
        image
    }

    /// A JPEG of `width` by `height` pixels with a red left half and a blue right half, and
    /// an EXIF segment that states this orientation.
    fn turned_jpeg(width: u32, height: u32, orientation: u8) -> Vec<u8> {
        let picture = image::RgbImage::from_fn(width, height, |x, _| {
            if x < width / 2 {
                image::Rgb([220, 30, 30])
            } else {
                image::Rgb([30, 30, 220])
            }
        });
        let mut encoded = Vec::new();
        picture
            .write_to(
                &mut std::io::Cursor::new(&mut encoded),
                image::ImageFormat::Jpeg,
            )
            .unwrap();
        let mut block = b"Exif\0\0MM\0\x2a\0\0\0\x08\0\x01".to_vec();
        block.extend_from_slice(&[
            0x01,
            0x12,
            0,
            3,
            0,
            0,
            0,
            1,
            0,
            orientation,
            0,
            0,
            0,
            0,
            0,
            0,
        ]);
        let length = u16::try_from(block.len() + 2).unwrap().to_be_bytes();
        let mut bytes = encoded[..2].to_vec();
        bytes.extend_from_slice(&[0xff, 0xe1, length[0], length[1]]);
        bytes.extend(block);
        bytes.extend_from_slice(&encoded[2..]);
        bytes
    }

    /// The colour of a pixel of the first frame, as red, green and blue.
    fn colour_at(image: &RenderImage, x: u32, y: u32) -> [u8; 3] {
        let width = image.size(0).width.0 as usize;
        let bytes = image.as_bytes(0).unwrap();
        let at = (y as usize * width + x as usize) * 4;
        [bytes[at + 2], bytes[at + 1], bytes[at]]
    }

    /// A phone stores a portrait photo on its side with the EXIF orientation 6, and the
    /// server states the size after the turn: the decoded copy is upright, with the red left
    /// half of the stored pixels at the top. The orientation 3 turns it by a half and keeps
    /// its size: the red half is then at the right.
    #[test]
    fn a_photo_is_decoded_upright_by_its_exif_orientation() {
        let portrait = stated(40, 80, 1);
        let planned = plan(portrait, None, LIST_FRAME_BYTES);
        let image = decode(&turned_jpeg(80, 40, 6), portrait, planned).unwrap();
        assert_eq!(sizes(&image), vec![(40, 80)]);
        let [red, _, blue] = colour_at(&image, 20, 5);
        assert!(red > 150 && blue < 100, "the top is red: {red} {blue}");
        let [red, _, blue] = colour_at(&image, 20, 75);
        assert!(blue > 150 && red < 100, "the bottom is blue: {red} {blue}");

        let landscape = stated(80, 40, 1);
        let image = decode(
            &turned_jpeg(80, 40, 3),
            landscape,
            plan(landscape, None, LIST_FRAME_BYTES),
        )
        .unwrap();
        assert_eq!(sizes(&image), vec![(80, 40)]);
        let [red, _, _] = colour_at(&image, 75, 20);
        assert!(red > 150, "the right is red: {red}");
    }

    fn sizes(image: &RenderImage) -> Vec<(u32, u32)> {
        (0..image.frame_count())
            .map(|frame| {
                let size = image.size(frame);
                (size.width.0 as u32, size.height.0 as u32)
            })
            .collect()
    }

    fn delays(image: &RenderImage) -> Vec<Duration> {
        (0..image.frame_count())
            .map(|frame| Duration::from(image.delay(frame)))
            .collect()
    }

    #[test]
    fn a_picture_is_planned_at_the_size_that_the_window_draws_with_its_proportions() {
        // A screenshot of 2560 by 1440 in a list that draws at most 584 by 440 pixels.
        let fit = Some((584, 440));
        let still = plan(stated(2560, 1440, 1), fit, LIST_FRAME_BYTES);
        assert_eq!(
            still,
            Plan {
                width: 584,
                height: 329,
                step: 1
            }
        );
        // A tall picture fits the height, a small one keeps its size.
        assert_eq!(
            plan(stated(1000, 4000, 1), fit, LIST_FRAME_BYTES),
            Plan {
                width: 110,
                height: 440,
                step: 1
            }
        );
        assert_eq!(
            plan(stated(300, 200, 1), fit, LIST_FRAME_BYTES),
            Plan {
                width: 300,
                height: 200,
                step: 1
            }
        );
        // Without a bound the viewer keeps the size of the file.
        assert!(plan(stated(2560, 1440, 1), None, VIEWER_FRAME_BYTES).whole(stated(2560, 1440, 1)));
    }

    #[test]
    fn the_frames_of_an_animation_stay_within_their_bytes() {
        let fit = Some((584, 440));
        // A GIF of 498 by 280 keeps its size and every frame up to 30 frames.
        for frames in [10, 30] {
            let short = stated(498, 280, frames);
            assert!(plan(short, fit, LIST_FRAME_BYTES).whole(short));
        }
        for frames in [31, 120, 500, 2048] {
            let long = stated(498, 280, frames);
            let planned = plan(long, fit, LIST_FRAME_BYTES);
            assert!(
                planned.bytes(long) <= LIST_FRAME_BYTES,
                "{frames} frames: {planned:?} holds {} bytes",
                planned.bytes(long)
            );
            // The frames are reduced to half of their size at the most.
            assert!(planned.width >= 249 && planned.height >= 140, "{planned:?}");
            if planned.step > 1 {
                assert_eq!((planned.width, planned.height), (249, 140));
            }
        }
        assert_eq!(plan(stated(498, 280, 500), fit, LIST_FRAME_BYTES).step, 5);
        let viewer = stated(1920, 1080, 300);
        let planned = plan(viewer, None, VIEWER_FRAME_BYTES);
        assert!(planned.bytes(viewer) <= VIEWER_FRAME_BYTES, "{planned:?}");
    }

    #[test]
    fn a_still_picture_is_decoded_at_the_size_of_its_plan() {
        let mut bytes = Vec::new();
        RgbaImage::from_pixel(2560, 1440, image::Rgba([10, 20, 30, 255]))
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        let file = stated(2560, 1440, 1);
        let planned = plan(file, Some((584, 440)), LIST_FRAME_BYTES);
        let decoded = decode(&bytes, file, planned).unwrap();
        assert_eq!(sizes(&decoded), [(584, 329)]);
        assert_eq!(held_bytes(&decoded), 584 * 329 * 4);
        // The colours are in the order of the window.
        assert_eq!(&decoded.as_bytes(0).unwrap()[..4], &[30, 20, 10, 255]);
        // A file larger than the size that the server states is refused.
        assert!(decode(&bytes, stated(2000, 1440, 1), planned).is_none());
    }

    #[test]
    fn every_kind_of_animation_is_decoded_into_reduced_frames_with_their_delays() {
        let fit = Some((50, 50));
        for (name, bytes) in [
            ("gif", gif(200, 100, 6)),
            ("webp", animated_webp(200, 100, 6)),
            ("png", animated_png(200, 100, 6)),
        ] {
            let file = stated(200, 100, 6);
            let planned = plan(file, fit, LIST_FRAME_BYTES);
            assert_eq!(
                planned,
                Plan {
                    width: 50,
                    height: 25,
                    step: 1
                },
                "{name}"
            );
            let decoded = decode(&bytes, file, planned)
                .unwrap_or_else(|| panic!("{name}: the animation is not decoded"));
            assert_eq!(sizes(&decoded), vec![(50, 25); 6], "{name}");
            assert_eq!(
                delays(&decoded),
                vec![Duration::from_millis(40); 6],
                "{name}"
            );
            // Each frame has its own colour. The colours of a GIF are reduced to its table.
            for frame in 0..6 {
                let red = decoded.as_bytes(frame).unwrap()[2];
                assert!(red.abs_diff(frame as u8 * 40) <= 8, "{name} {frame}: {red}");
            }
            assert_eq!(held_bytes(&decoded), 6 * 50 * 25 * 4, "{name}");
        }
    }

    #[test]
    fn an_animation_keeps_every_nth_frame_with_the_delays_of_the_frames_between() {
        let bytes = gif(20, 20, 7);
        let file = stated(20, 20, 7);
        let planned = Plan {
            width: 20,
            height: 20,
            step: 3,
        };
        let decoded = decode(&bytes, file, planned).unwrap();
        assert_eq!(decoded.frame_count(), 3);
        assert_eq!(
            delays(&decoded),
            [120, 120, 40].map(Duration::from_millis).to_vec()
        );
        for (frame, red) in [0u8, 120, 240].into_iter().enumerate() {
            let drawn = decoded.as_bytes(frame).unwrap()[2];
            assert!(drawn.abs_diff(red) <= 8, "{frame}: {drawn}");
        }
    }

    #[test]
    fn an_animation_with_more_frames_or_a_larger_canvas_than_stated_is_refused() {
        let bytes = gif(20, 20, 5);
        let planned = Plan {
            width: 20,
            height: 20,
            step: 1,
        };
        assert!(decode(&bytes, stated(20, 20, 4), planned).is_none());
        assert!(decode(&bytes, stated(19, 20, 5), planned).is_none());
        assert!(decode(&bytes, stated(20, 20, 5), planned).is_some());
        // A file cut inside its last frame keeps the frames before it.
        let cut = &bytes[..bytes.len() - 6];
        let decoded = decode(cut, stated(20, 20, 5), planned).unwrap();
        assert!(decoded.frame_count() >= 4, "{}", decoded.frame_count());
    }

    #[test]
    fn the_server_bounds_decide_which_files_are_pictures() {
        let file = |media: &str, width, height, frames| Attachment {
            id: junto_client_core::protocol::ids::AttachmentId::generate(),
            name: "file".to_owned(),
            bytes: 1,
            sha256: String::new(),
            media_type: media.to_owned(),
            width: Some(width),
            height: Some(height),
            frames,
        };
        assert_eq!(
            Stated::of(&file("image/gif", 498, 280, Some(37))),
            Some(stated(498, 280, 37))
        );
        assert_eq!(
            Stated::of(&file("image/png", 8192, 4096, None)),
            Some(stated(8192, 4096, 1))
        );
        for refused in [
            file("image/png", 8192, 4097, None),
            file("image/gif", 2049, 2049, Some(2)),
            file("image/gif", 8, 8, Some(2049)),
            file("image/svg+xml", 8, 8, None),
            file("application/octet-stream", 8, 8, None),
        ] {
            assert_eq!(Stated::of(&refused), None, "{refused:?}");
        }
    }

    #[test]
    fn decodings_wait_while_their_memory_is_above_the_bound() {
        use futures_util::FutureExt as _;
        let gate = Gate::new();
        let reserve = |bytes| Box::pin(Arc::clone(&gate).reserve(bytes));
        let first = reserve(MOST_DECODING_BYTES / 2).now_or_never().flatten();
        let second = reserve(MOST_DECODING_BYTES / 2).now_or_never().flatten();
        assert!(first.is_some() && second.is_some());
        let mut third = reserve(MOST_DECODING_BYTES / 4);
        let mut fourth = reserve(1);
        assert!(third.as_mut().now_or_never().is_none());
        // A later decoding waits for an earlier one, also when it would fit.
        assert!(fourth.as_mut().now_or_never().is_none());
        // A decoding that stops waiting takes no place.
        let mut dropped = reserve(MOST_DECODING_BYTES / 4);
        assert!(dropped.as_mut().now_or_never().is_none());
        drop(first);
        let third = third.as_mut().now_or_never().flatten();
        let fourth = fourth.as_mut().now_or_never().flatten();
        assert!(third.is_some() && fourth.is_some());
        drop(dropped);
        drop((second, third, fourth));
        assert_eq!(gate.state.lock().unwrap().reserved, 0);
        // A decoding larger than the bound runs alone.
        let alone = reserve(MOST_DECODING_BYTES * 2).now_or_never().flatten();
        assert!(alone.is_some());
        let mut next = reserve(1);
        assert!(next.as_mut().now_or_never().is_none());
        drop(alone);
        let next = next.as_mut().now_or_never().flatten();
        assert!(next.is_some());
        drop(next);
        assert_eq!(gate.state.lock().unwrap().reserved, 0);
        assert!(gate.state.lock().unwrap().waiting.is_empty());
    }
}
