//! The decoding of a picture of a message (ADR 0740): a still picture, or the frames of a
//! GIF, an animated WebP or an animated PNG, reduced to the size at which the window draws
//! it, with a bound on the bytes of its frames and on the memory of the decodings that run
//! at one time. A still JPEG of sequential coding and a still PNG without interlacing are
//! decoded at that size without the whole picture in memory: the JPEG decoder reduces the
//! picture by 1/2, 1/4 or 1/8 while it decodes it, and the rows of the PNG are reduced as
//! they are read.

use std::collections::VecDeque;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use futures_channel::oneshot;
use gpui::RenderImage;
use image::metadata::Orientation;
use image::{AnimationDecoder, Delay, Frame, ImageDecoder, RgbaImage};
use junto_client_core::protocol::message::Attachment;

/// The most pixels of a still picture that is decoded whole, 128 MiB as RGBA, and of the
/// canvas of an animation, 16 MiB as RGBA, and the most frames: the bounds of the server
/// (ADR 0408).
pub(crate) const MOST_PICTURE_PIXELS: u64 = 33_554_432;
/// The most pixels and the longest side of a still JPEG or PNG picture, which is decoded at
/// the size at which the window draws it (ADR 0408).
pub(crate) const MOST_REDUCED_PIXELS: u64 = 268_435_456;
pub(crate) const LONGEST_SIDE: u32 = 16_384;
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
    /// A still JPEG or PNG picture: its decoding can reduce it without the whole picture in
    /// memory, when the file allows it.
    pub reducible: bool,
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
        let reducible =
            frames == 1 && matches!(attachment.media_type.as_str(), "image/png" | "image/jpeg");
        let most = if reducible {
            MOST_REDUCED_PIXELS
        } else {
            MOST_PICTURE_PIXELS
        };
        let within = width >= 1
            && height >= 1
            && width.max(height) <= LONGEST_SIDE
            && pixels <= most
            && (frames == 1
                || ((2..=MOST_FRAMES).contains(&frames) && pixels <= MOST_FRAME_PIXELS));
        within.then_some(Self {
            width,
            height,
            frames,
            reducible,
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
    // A reduced picture is not drawn larger than a picture that is decoded whole.
    let pixels = u64::from(size.0) * u64::from(size.1);
    if pixels > MOST_PICTURE_PIXELS {
        size = scaled_down(size, (MOST_PICTURE_PIXELS as f64 / pixels as f64).sqrt());
    }
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
/// canvases), and the reduced copy. A picture above [`MOST_PICTURE_PIXELS`] is decoded only
/// reduced, within [`reduced_working_bytes`].
pub(crate) fn decoding_bytes(bytes: u64, stated: Stated, plan: Plan) -> u64 {
    let canvas = stated.pixels() * 4;
    let working = if stated.frames > 1 {
        canvas * 3
    } else if stated.pixels() > MOST_PICTURE_PIXELS {
        reduced_working_bytes(stated, plan)
    } else {
        canvas
    };
    bytes + working + plan.bytes(stated)
}

/// The bytes that a reduced decoding holds besides the file and the result. A JPEG decoder
/// reduces by a power of two to at most twice the planned size on each side, and holds the
/// planes of the colours and the decoded pixels at that size (24 bytes for each planned
/// pixel); the rows of a PNG hold one row of the file and the sums of one planned row.
fn reduced_working_bytes(stated: Stated, plan: Plan) -> u64 {
    let planned = u64::from(plan.width) * u64::from(plan.height);
    planned * 24 + u64::from(stated.width.max(stated.height)) * 40 + 1024 * 1024
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

fn reduced_frame(picture: &RgbaImage, plan: Plan) -> RgbaImage {
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
    } else if let Some(picture) = stated
        .reducible
        .then(|| reduced(bytes, stated, plan))
        .flatten()
    {
        vec![into_frame(picture, Delay::from_numer_denom_ms(0, 1))]
    } else if stated.pixels() > MOST_PICTURE_PIXELS {
        return None;
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

/// A turn of a quarter or three quarters exchanges the width and the height.
fn turns_a_quarter(orientation: Orientation) -> bool {
    matches!(
        orientation,
        Orientation::Rotate90
            | Orientation::Rotate270
            | Orientation::Rotate90FlipH
            | Orientation::Rotate270FlipH
    )
}

/// The size at which the file is decoded for a plan: the planned size, with the width and
/// the height exchanged when the picture is turned by a quarter.
fn in_file(plan: Plan, orientation: Orientation) -> (u32, u32) {
    if turns_a_quarter(orientation) {
        (plan.height, plan.width)
    } else {
        (plan.width, plan.height)
    }
}

/// The file states a size within the stated size in one of its orientations.
fn within_stated((width, height): (u32, u32), stated: Stated) -> bool {
    let side = stated.width.max(stated.height);
    width >= 1
        && height >= 1
        && width.max(height) <= side
        && u64::from(width) * u64::from(height) <= stated.pixels()
}

/// A still JPEG or PNG picture decoded at the planned size without the whole picture in
/// memory, turned by its orientation. `None` for another file, a JPEG of progressive or
/// lossless coding or with CMYK or 16-bit samples, an interlaced PNG, and a file that
/// does not decode: a picture within [`MOST_PICTURE_PIXELS`] is then decoded whole.
fn reduced(bytes: &[u8], stated: Stated, plan: Plan) -> Option<RgbaImage> {
    let limit = usize::try_from(reduced_working_bytes(stated, plan)).ok()?;
    let picture = match image::guess_format(bytes).ok()? {
        image::ImageFormat::Jpeg => reduced_jpeg(bytes, stated, plan, limit)?,
        image::ImageFormat::Png => reduced_png(bytes, stated, plan, limit)?,
        _ => return None,
    };
    Some(picture)
}

fn reduced_jpeg(bytes: &[u8], stated: Stated, plan: Plan, limit: usize) -> Option<RgbaImage> {
    use image::ImageDecoder as _;
    let orientation = image::codecs::jpeg::JpegDecoder::new(std::io::Cursor::new(bytes))
        .ok()?
        .orientation()
        .unwrap_or(Orientation::NoTransforms);
    let mut decoder = jpeg_decoder::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_max_decoding_buffer_size(limit);
    decoder.read_info().ok()?;
    let info = decoder.info()?;
    if info.coding_process != jpeg_decoder::CodingProcess::DctSequential
        || !matches!(
            info.pixel_format,
            jpeg_decoder::PixelFormat::L8 | jpeg_decoder::PixelFormat::RGB24
        )
        || !within_stated((u32::from(info.width), u32::from(info.height)), stated)
    {
        return None;
    }
    let wanted = in_file(plan, orientation);
    let (width, height) = decoder
        .scale(u16::try_from(wanted.0).ok()?, u16::try_from(wanted.1).ok()?)
        .ok()?;
    // The decoder is a library that a crafted file could make panic; a panic leaves no
    // state behind, as the decoder is dropped with it.
    let pixels = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| decoder.decode()))
        .ok()?
        .ok()?;
    let (width, height) = (u32::from(width), u32::from(height));
    let decoded = match info.pixel_format {
        jpeg_decoder::PixelFormat::L8 => {
            image::DynamicImage::ImageLuma8(image::GrayImage::from_raw(width, height, pixels)?)
        }
        _ => image::DynamicImage::ImageRgb8(image::RgbImage::from_raw(width, height, pixels)?),
    };
    let mut fitted = if (width, height) == wanted {
        decoded
    } else {
        decoded.thumbnail_exact(wanted.0, wanted.1)
    };
    fitted.apply_orientation(orientation);
    Some(fitted.into_rgba8())
}

fn reduced_png(bytes: &[u8], stated: Stated, plan: Plan, limit: usize) -> Option<RgbaImage> {
    let mut decoder =
        png::Decoder::new_with_limits(std::io::Cursor::new(bytes), png::Limits { bytes: limit });
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let info = reader.info();
    let size = (info.width, info.height);
    if info.interlaced || info.animation_control.is_some() || !within_stated(size, stated) {
        return None;
    }
    let orientation = info
        .exif_metadata
        .as_deref()
        .and_then(Orientation::from_exif_chunk)
        .unwrap_or(Orientation::NoTransforms);
    let (color, _) = reader.output_color_type();
    let channels = color.samples();
    let wanted = in_file(plan, orientation);
    let mut rows = Reducer::new(size, wanted);
    let mut rgba = vec![0u8; size.0 as usize * 4];
    while let Ok(Some(row)) = reader.next_row() {
        let data = row.data();
        for (pixel, out) in data.chunks_exact(channels).zip(rgba.as_chunks_mut::<4>().0) {
            let [r, g, b, a] = match (color, pixel) {
                (png::ColorType::Grayscale, [l]) => [*l, *l, *l, 255],
                (png::ColorType::GrayscaleAlpha, [l, a]) => [*l, *l, *l, *a],
                (png::ColorType::Rgb, [r, g, b]) => [*r, *g, *b, 255],
                (png::ColorType::Rgba, [r, g, b, a]) => [*r, *g, *b, *a],
                _ => return None,
            };
            out.copy_from_slice(&[r, g, b, a]);
        }
        rows.add(&rgba);
    }
    let mut fitted = image::DynamicImage::ImageRgba8(rows.finish()?);
    fitted.apply_orientation(orientation);
    Some(fitted.into_rgba8())
}

/// Reduces rows of RGBA pixels to a smaller size as they come: each pixel of the result is
/// the mean of the pixels of the file that fall in it, weighted by their alpha, so that a
/// transparent pixel does not darken its neighbours.
struct Reducer {
    from: (u32, u32),
    to: (u32, u32),
    /// The planned column of each column of the file.
    columns: Vec<u32>,
    /// The sums of the planned row that the rows of the file fill now: red, green and blue
    /// times alpha, alpha, and the count of pixels.
    sums: Vec<[u64; 5]>,
    row: u32,
    at: u32,
    made: Vec<u8>,
}

impl Reducer {
    fn new(from: (u32, u32), to: (u32, u32)) -> Self {
        let to = (to.0.clamp(1, from.0), to.1.clamp(1, from.1));
        Self {
            from,
            to,
            columns: (0..from.0)
                .map(|x| (u64::from(x) * u64::from(to.0) / u64::from(from.0)) as u32)
                .collect(),
            sums: vec![[0; 5]; to.0 as usize],
            row: 0,
            at: 0,
            made: Vec::with_capacity(to.0 as usize * to.1 as usize * 4),
        }
    }

    fn add(&mut self, row: &[u8]) {
        if self.at >= self.from.1 {
            return;
        }
        let planned = (u64::from(self.at) * u64::from(self.to.1) / u64::from(self.from.1)) as u32;
        while self.row < planned {
            self.flush();
        }
        for (pixel, column) in row.as_chunks::<4>().0.iter().zip(&self.columns) {
            let alpha = u64::from(pixel[3]);
            let sum = &mut self.sums[*column as usize];
            sum[0] += u64::from(pixel[0]) * alpha;
            sum[1] += u64::from(pixel[1]) * alpha;
            sum[2] += u64::from(pixel[2]) * alpha;
            sum[3] += alpha;
            sum[4] += 1;
        }
        self.at += 1;
    }

    fn flush(&mut self) {
        for sum in &mut self.sums {
            let count = sum[4].max(1);
            let alpha = sum[3];
            let colour = |value: u64| value.checked_div(alpha).unwrap_or(0) as u8;
            self.made.extend_from_slice(&[
                colour(sum[0]),
                colour(sum[1]),
                colour(sum[2]),
                (alpha / count) as u8,
            ]);
            *sum = [0; 5];
        }
        self.row += 1;
    }

    /// The reduced picture, when every row of the file came.
    fn finish(mut self) -> Option<RgbaImage> {
        if self.at != self.from.1 {
            return None;
        }
        while self.row < self.to.1 {
            self.flush();
        }
        RgbaImage::from_raw(self.to.0, self.to.1, self.made)
    }
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
            kept.push((reduced_frame(canvas, plan), delay));
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

/// Decodes a still picture of a message to fit `fit` as the window does, and gives the
/// size of the result (test support, for the measurement of its memory). With `reduce`
/// false the picture is decoded whole and then reduced, as the client did before ADR 0408
/// allowed reduced decodings.
#[cfg(feature = "test-support")]
pub fn decode_still(
    bytes: &[u8],
    media_type: &str,
    (width, height): (u32, u32),
    fit: (u32, u32),
    reduce: bool,
) -> Option<(u32, u32)> {
    let stated = Stated {
        width,
        height,
        frames: 1,
        reducible: reduce && matches!(media_type, "image/png" | "image/jpeg"),
    };
    let decoded = decode(bytes, stated, plan(stated, Some(fit), VIEWER_FRAME_BYTES))?;
    let size = decoded.size(0);
    Some((size.width.0 as u32, size.height.0 as u32))
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

/// The gate of every decoding of the process.
pub(crate) fn decoding_gate() -> Arc<Gate> {
    Arc::clone(&DECODING)
}

/// Reads a picture of `file` bytes with `read`, and returns its bytes with the bytes of the
/// gate that its decoding to `plan` holds. The reservation comes before the read, so that the
/// bound of the gate counts each file from the start of its read and not only from its
/// decoding (review M6). `None` when the gate is gone.
pub(crate) async fn read_within<Read, Failure>(
    gate: Arc<Gate>,
    file: u64,
    stated: Stated,
    plan: Plan,
    read: impl FnOnce() -> Read,
) -> Result<Option<(Vec<u8>, Reserved)>, Failure>
where
    Read: std::future::Future<Output = Result<Vec<u8>, Failure>>,
{
    let Some(reserved) = gate.reserve(decoding_bytes(file, stated, plan)).await else {
        return Ok(None);
    };
    let bytes = read().await?;
    Ok(Some((bytes, reserved)))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn stated(width: u32, height: u32, frames: u32) -> Stated {
        Stated {
            width,
            height,
            frames,
            reducible: false,
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

    fn reducible(stated: Stated) -> Stated {
        Stated {
            reducible: true,
            ..stated
        }
    }

    /// The red, green, blue and yellow quarters of a picture.
    fn quarters(width: u32, height: u32) -> image::RgbaImage {
        image::RgbaImage::from_fn(width, height, |x, y| {
            match (x < width / 2, y < height / 2) {
                (true, true) => image::Rgba([220, 30, 30, 255]),
                (false, true) => image::Rgba([30, 200, 30, 255]),
                (true, false) => image::Rgba([30, 30, 220, 255]),
                (false, false) => image::Rgba([230, 220, 30, 255]),
            }
        })
    }

    fn encoded(picture: image::DynamicImage, format: image::ImageFormat) -> Vec<u8> {
        let mut bytes = Vec::new();
        picture
            .write_to(&mut std::io::Cursor::new(&mut bytes), format)
            .unwrap();
        bytes
    }

    fn near(got: [u8; 3], expected: [u8; 3]) -> bool {
        got.iter()
            .zip(expected)
            .all(|(got, expected)| got.abs_diff(expected) <= 24)
    }

    /// A JPEG of sequential coding and a PNG without interlacing are decoded reduced, at
    /// the planned size, with the colours of their quarters.
    #[test]
    fn a_jpeg_and_a_png_are_decoded_reduced_at_the_planned_size() {
        let picture = quarters(1600, 1200);
        for (format, bytes) in [
            (
                "JPEG",
                encoded(
                    image::DynamicImage::ImageRgb8(
                        image::DynamicImage::ImageRgba8(picture.clone()).into_rgb8(),
                    ),
                    image::ImageFormat::Jpeg,
                ),
            ),
            (
                "PNG",
                encoded(
                    image::DynamicImage::ImageRgba8(picture.clone()),
                    image::ImageFormat::Png,
                ),
            ),
        ] {
            let stated = reducible(stated(1600, 1200, 1));
            let planned = plan(stated, Some((200, 150)), LIST_FRAME_BYTES);
            let reduced_picture = reduced(&bytes, stated, planned)
                .unwrap_or_else(|| panic!("the {format} is decoded reduced"));
            assert_eq!(reduced_picture.dimensions(), (200, 150), "{format}");
            let image = decode(&bytes, stated, planned).unwrap();
            assert_eq!(sizes(&image), vec![(200, 150)], "{format}");
            for ((x, y), expected) in [
                ((50, 37), [220, 30, 30]),
                ((150, 37), [30, 200, 30]),
                ((50, 112), [30, 30, 220]),
                ((150, 112), [230, 220, 30]),
            ] {
                let got = colour_at(&image, x, y);
                assert!(near(got, expected), "{format} at {x} {y}: {got:?}");
            }
        }
    }

    /// The rows of a PNG are reduced with their alpha: a transparent part stays transparent
    /// and does not darken the colour of the pixels beside it.
    #[test]
    fn a_reduced_png_keeps_transparent_parts_without_darkening_their_edge() {
        let picture = image::RgbaImage::from_fn(300, 100, |x, _| {
            if x < 155 {
                image::Rgba([220, 30, 30, 255])
            } else {
                image::Rgba([0, 0, 0, 0])
            }
        });
        let bytes = encoded(
            image::DynamicImage::ImageRgba8(picture),
            image::ImageFormat::Png,
        );
        let stated = reducible(stated(300, 100, 1));
        // 300 to 40 columns: column 20 holds the file columns 150 to 157, red to 154.
        let reduced_picture = reduced(
            &bytes,
            stated,
            plan(stated, Some((40, 40)), LIST_FRAME_BYTES),
        )
        .unwrap();
        assert_eq!(reduced_picture.dimensions(), (40, 13));
        let edge = reduced_picture.get_pixel(20, 6).0;
        assert_eq!(
            &edge[..3],
            &[220, 30, 30],
            "the edge keeps its colour: {edge:?}"
        );
        assert!(
            edge[3] > 0 && edge[3] < 255,
            "the edge is part transparent: {edge:?}"
        );
        assert_eq!(reduced_picture.get_pixel(30, 6).0[3], 0);
        assert_eq!(reduced_picture.get_pixel(5, 6).0, [220, 30, 30, 255]);
    }

    /// A reduced JPEG is turned by its EXIF orientation as a whole decoding is.
    #[test]
    fn a_reduced_photo_is_decoded_upright_by_its_exif_orientation() {
        let portrait = reducible(stated(40, 80, 1));
        let planned = plan(portrait, Some((20, 40)), LIST_FRAME_BYTES);
        let bytes = turned_jpeg(80, 40, 6);
        assert!(reduced(&bytes, portrait, planned).is_some());
        let image = decode(&bytes, portrait, planned).unwrap();
        assert_eq!(sizes(&image), vec![(20, 40)]);
        let [red, _, blue] = colour_at(&image, 10, 3);
        assert!(red > 150 && blue < 100, "the top is red: {red} {blue}");
        let [red, _, blue] = colour_at(&image, 10, 37);
        assert!(blue > 150 && red < 100, "the bottom is blue: {red} {blue}");
    }

    /// A photo above the bound of a whole decoding is planned within that bound, and its
    /// decoding reserves the bytes of a reduced decoding, not of the whole picture.
    #[test]
    fn a_photo_above_the_whole_bound_is_planned_and_reserved_reduced() {
        let large = reducible(stated(16_000, 12_000, 1));
        let own = plan(large, None, VIEWER_FRAME_BYTES);
        assert!(u64::from(own.width) * u64::from(own.height) <= MOST_PICTURE_PIXELS);
        let viewer = plan(large, Some((2400, 1300)), VIEWER_FRAME_BYTES);
        assert_eq!((viewer.width, viewer.height), (1733, 1300));
        let reserved = decoding_bytes(40 * 1024 * 1024, large, viewer);
        assert!(reserved < 110 * 1024 * 1024, "{reserved}");
        // A picture that cannot be reduced is not decoded above the whole bound.
        let whole = stated(16_000, 12_000, 1);
        let bytes = encoded(
            image::DynamicImage::ImageRgba8(quarters(64, 48)),
            image::ImageFormat::WebP,
        );
        assert!(decode(&bytes, whole, viewer).is_none());
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
            Stated::of(&file("image/webp", 8192, 4096, None)),
            Some(stated(8192, 4096, 1))
        );
        // A still JPEG or PNG is decoded reduced up to 16384 pixels on each side.
        for media in ["image/png", "image/jpeg"] {
            assert_eq!(
                Stated::of(&file(media, 16_384, 16_384, None)),
                Some(reducible(stated(16_384, 16_384, 1)))
            );
        }
        for refused in [
            file("image/webp", 8192, 4097, None),
            file("image/png", 16_385, 8, None),
            file("image/jpeg", 8, 16_385, None),
            file("image/gif", 2049, 2049, Some(2)),
            file("image/gif", 8, 8, Some(2049)),
            file("image/svg+xml", 8, 8, None),
            file("application/octet-stream", 8, 8, None),
        ] {
            assert_eq!(Stated::of(&refused), None, "{refused:?}");
        }
    }

    /// A file is read only when its bytes and the bytes of its decoding fit next to the
    /// decodings that run: three files of 30 MiB that waited outside the bound were held
    /// beside a decoding of 8K (review M6).
    #[test]
    fn a_file_is_read_only_when_its_bytes_fit_in_the_bound() {
        use futures_util::FutureExt as _;
        use std::sync::atomic::{AtomicBool, Ordering};
        let gate = Gate::new();
        let full = Box::pin(Arc::clone(&gate).reserve(MOST_DECODING_BYTES))
            .now_or_never()
            .flatten();
        assert!(full.is_some());
        let started = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&started);
        let stated = stated(100, 100, 1);
        let plan = plan(stated, None, LIST_FRAME_BYTES);
        let mut reading = Box::pin(read_within(
            Arc::clone(&gate),
            30 * 1024 * 1024,
            stated,
            plan,
            move || {
                flag.store(true, Ordering::SeqCst);
                async { Ok::<_, ()>(vec![0; 16]) }
            },
        ));
        assert!(reading.as_mut().now_or_never().is_none());
        assert!(
            !started.load(Ordering::SeqCst),
            "the file was read while the bound was full"
        );
        drop(full);
        let read = reading.as_mut().now_or_never();
        assert!(matches!(read, Some(Ok(Some(_)))));
        assert!(started.load(Ordering::SeqCst));
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
