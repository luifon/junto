use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

#[cfg(target_os = "macos")]
use core_foundation::{
    base::{CFType, TCFType},
    boolean::CFBoolean,
    dictionary::CFDictionary,
    string::CFString,
};
#[cfg(target_os = "macos")]
use core_video::pixel_buffer::{
    CVPixelBuffer, kCVPixelBufferIOSurfacePropertiesKey, kCVPixelBufferMetalCompatibilityKey,
    kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
};
use futures::{StreamExt as _, channel::mpsc};
use gpui::{
    Context, ImageSource, IntoElement, ObjectFit, ParentElement as _, Render, RenderImage,
    SharedString, Styled as _, StyledImage as _, Task, Window, div, img,
    prelude::FluentBuilder as _, px,
};
#[cfg(target_os = "macos")]
use gpui::surface;
use gpui_component::{
    Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};

use crate::support::{Palette, note, out_dir, process_cpu_seconds, tabular_numerals, u};

static FRAME_SIZE: std::sync::OnceLock<(usize, usize)> = std::sync::OnceLock::new();

fn frame_size() -> (usize, usize) {
    *FRAME_SIZE.get_or_init(|| {
        std::env::var("SPIKE_VIDEO_SIZE")
            .ok()
            .and_then(|value| {
                let (width, height) = value.split_once('x')?;
                Some((width.parse().ok()?, height.parse().ok()?))
            })
            .unwrap_or((1280, 720))
    })
}

fn width() -> usize {
    frame_size().0
}

fn height() -> usize {
    frame_size().1
}

fn counter_cell() -> usize {
    width() / 26
}
const FRAMES_PER_SECOND: f64 = 30.0;

#[derive(Clone, Copy, PartialEq, Debug)]
enum DrawPath {
    #[cfg(target_os = "macos")]
    Surface,
    Image,
}

impl DrawPath {
    fn label(self) -> &'static str {
        match self {
            #[cfg(target_os = "macos")]
            DrawPath::Surface => "surface (CVPixelBuffer NV12)",
            DrawPath::Image => "img (BGRA RenderImage)",
        }
    }
}

#[cfg(target_os = "macos")]
struct SendBuffer(CVPixelBuffer);

#[cfg(target_os = "macos")]
unsafe impl Send for SendBuffer {}

enum Payload {
    #[cfg(target_os = "macos")]
    Surface(SendBuffer),
    Image(Arc<RenderImage>),
}

struct Delivered {
    generation: u64,
    tile: usize,
    sequence: u64,
    payload: Payload,
}

struct Template {
    #[cfg(target_os = "macos")]
    luma: Vec<u8>,
    #[cfg(target_os = "macos")]
    chroma: Vec<u8>,
    bgra: Vec<u8>,
}

impl Template {
    fn new() -> Self {
        #[cfg(target_os = "macos")]
        let mut luma = vec![0u8; width() * height()];
        let mut bgra = vec![0u8; width() * height() * 4];
        for row in 0..height() {
            for column in 0..width() {
                let value = 40 + (column * 150 / width()) as u8 + (((row / 90) % 2) * 24) as u8;
                #[cfg(target_os = "macos")]
                {
                    luma[row * width() + column] = value;
                }
                let pixel = (row * width() + column) * 4;
                bgra[pixel] = value;
                bgra[pixel + 1] = value;
                bgra[pixel + 2] = value;
                bgra[pixel + 3] = 255;
            }
        }
        Self {
            #[cfg(target_os = "macos")]
            luma,
            #[cfg(target_os = "macos")]
            chroma: vec![128u8; width() * height() / 2],
            bgra,
        }
    }
}

struct Marks {
    bar_start: usize,
    band_start: usize,
    counter: u64,
}

impl Marks {
    fn new(tile: usize, sequence: u64) -> Self {
        Self {
            bar_start: (sequence as usize * 8) % (width() - width() / 20),
            band_start: 40 + tile * 60,
            counter: sequence,
        }
    }

    fn bright(&self, row: usize, column: usize) -> Option<bool> {
        if column >= self.bar_start && column < self.bar_start + width() / 20 {
            return Some(true);
        }
        if row >= self.band_start && row < self.band_start + 30 && column < width() * 5 / 32 {
            return Some(true);
        }
        if row >= height() - counter_cell() && column < 16 * counter_cell() {
            let bit = 15 - column / counter_cell();
            return Some((self.counter >> bit) & 1 == 1);
        }
        None
    }

    fn spans(&self, row: usize) -> Vec<(usize, usize, u8)> {
        let mut spans = vec![(self.bar_start, self.bar_start + width() / 20, 235u8)];
        if row >= self.band_start && row < self.band_start + 30 {
            spans.push((0, width() * 5 / 32, 235));
        }
        if row >= height() - counter_cell() {
            let cell = counter_cell();
            for bit in 0..16usize {
                let on = self.bright(row, bit * cell).unwrap_or(false);
                spans.push((bit * cell, bit * cell + cell - 4, if on { 235 } else { 16 }));
            }
        }
        spans
    }
}

#[cfg(target_os = "macos")]
fn new_pixel_buffer() -> CVPixelBuffer {
    let empty: CFDictionary<CFString, CFType> = CFDictionary::from_CFType_pairs(&[]);
    let options: CFDictionary<CFString, CFType> = CFDictionary::from_CFType_pairs(&[
        (
            unsafe { CFString::wrap_under_get_rule(kCVPixelBufferIOSurfacePropertiesKey) },
            empty.as_CFType(),
        ),
        (
            unsafe { CFString::wrap_under_get_rule(kCVPixelBufferMetalCompatibilityKey) },
            CFBoolean::true_value().as_CFType(),
        ),
    ]);
    CVPixelBuffer::new(
        kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
        width(),
        height(),
        Some(&options),
    )
    .expect("pixel buffer")
}

#[cfg(target_os = "macos")]
fn fill_pixel_buffer(buffer: &CVPixelBuffer, template: &Template, marks: &Marks) {
    buffer.lock_base_address(0);
    unsafe {
        let luma_stride = buffer.get_bytes_per_row_of_plane(0);
        let luma_base = buffer.get_base_address_of_plane(0) as *mut u8;
        for row in 0..height() {
            let target = std::slice::from_raw_parts_mut(luma_base.add(row * luma_stride), width());
            target.copy_from_slice(&template.luma[row * width()..(row + 1) * width()]);
            for (start, end, value) in marks.spans(row) {
                target[start..end].fill(value);
            }
        }
        let chroma_stride = buffer.get_bytes_per_row_of_plane(1);
        let chroma_base = buffer.get_base_address_of_plane(1) as *mut u8;
        for row in 0..height() / 2 {
            let target = std::slice::from_raw_parts_mut(chroma_base.add(row * chroma_stride), width());
            target.copy_from_slice(&template.chroma[row * width()..(row + 1) * width()]);
        }
    }
    buffer.unlock_base_address(0);
}

fn bgra_image(template: &Template, marks: &Marks) -> RenderImage {
    let mut pixels = template.bgra.clone();
    for row in 0..height() {
        for (start, end, value) in marks.spans(row) {
            let line = &mut pixels[(row * width() + start) * 4..(row * width() + end) * 4];
            for pixel in line.chunks_exact_mut(4) {
                pixel[0] = value;
                pixel[1] = value;
                pixel[2] = value;
            }
        }
    }
    let buffer = image::RgbaImage::from_raw(width() as u32, height() as u32, pixels).expect("frame");
    RenderImage::new(vec![image::Frame::new(buffer)])
}

fn thread_cpu_nanos() -> u64 {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut time) };
    time.tv_sec as u64 * 1_000_000_000 + time.tv_nsec as u64
}

struct Producer {
    stop: Arc<AtomicBool>,
    cpu_nanos: Arc<AtomicU64>,
}

impl Producer {
    fn start(
        generation: u64,
        path: DrawPath,
        tiles: usize,
        sender: mpsc::UnboundedSender<Delivered>,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let cpu_nanos = Arc::new(AtomicU64::new(0));
        let thread_stop = stop.clone();
        let thread_cpu = cpu_nanos.clone();
        std::thread::spawn(move || {
            let template = Template::new();
            #[cfg(target_os = "macos")]
            let rings: Vec<Vec<CVPixelBuffer>> = if path == DrawPath::Surface {
                (0..tiles)
                    .map(|_| (0..3).map(|_| new_pixel_buffer()).collect())
                    .collect()
            } else {
                Vec::new()
            };
            let cpu_base = thread_cpu_nanos();
            let interval = Duration::from_secs_f64(1.0 / FRAMES_PER_SECOND);
            let mut deadline = Instant::now();
            let mut sequence = 0u64;
            while !thread_stop.load(Ordering::Relaxed) {
                for tile in 0..tiles {
                    let marks = Marks::new(tile, sequence);
                    let payload = match path {
                        #[cfg(target_os = "macos")]
                        DrawPath::Surface => {
                            let buffer = &rings[tile][(sequence % 3) as usize];
                            fill_pixel_buffer(buffer, &template, &marks);
                            Payload::Surface(SendBuffer(buffer.clone()))
                        }
                        DrawPath::Image => Payload::Image(Arc::new(bgra_image(&template, &marks))),
                    };
                    let delivered = Delivered {
                        generation,
                        tile,
                        sequence,
                        payload,
                    };
                    if sender.unbounded_send(delivered).is_err() {
                        return;
                    }
                }
                sequence += 1;
                thread_cpu.store(thread_cpu_nanos() - cpu_base, Ordering::Relaxed);
                deadline = (deadline + interval).max(Instant::now());
                std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
            }
        });
        Self { stop, cpu_nanos }
    }
}

impl Drop for Producer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[derive(Clone, Copy, Default)]
struct Sample {
    delivered_per_tile: f64,
    shown_per_tile: f64,
    process_cpu: f64,
    generator_cpu: f64,
    replaced_before_shown: u64,
}

struct Counters {
    since: Instant,
    cpu: f64,
    generator_nanos: u64,
    delivered: u64,
    shown: u64,
    replaced_before_shown: u64,
}

#[cfg(target_os = "macos")]
const BENCH_CONFIGS: [(DrawPath, usize); 6] = [
    (DrawPath::Surface, 1),
    (DrawPath::Surface, 4),
    (DrawPath::Surface, 9),
    (DrawPath::Image, 1),
    (DrawPath::Image, 4),
    (DrawPath::Image, 9),
];
#[cfg(not(target_os = "macos"))]
const BENCH_CONFIGS: [(DrawPath, usize); 3] = [
    (DrawPath::Image, 1),
    (DrawPath::Image, 4),
    (DrawPath::Image, 9),
];
const WARMUP_SECONDS: usize = 2;
const MEASURE_SECONDS: usize = 8;

struct BenchRun {
    index: usize,
    seconds: usize,
    samples: Vec<Sample>,
    lines: Vec<String>,
}

struct TileFrame {
    sequence: u64,
    payload: Payload,
}

pub struct VideoGate {
    path: DrawPath,
    tiles: usize,
    generation: u64,
    frames: Vec<Option<TileFrame>>,
    shown_sequences: Vec<Option<u64>>,
    stale_images: Vec<Arc<RenderImage>>,
    sender: mpsc::UnboundedSender<Delivered>,
    producer: Option<Producer>,
    counters: Counters,
    latest: Sample,
    bench: Option<BenchRun>,
    bench_report: SharedString,
    _receiver: Task<()>,
}

impl VideoGate {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (sender, mut receiver) = mpsc::unbounded::<Delivered>();
        let task = cx.spawn(async move |this, cx| {
            while let Some(first) = receiver.next().await {
                let mut batch = vec![first];
                while let Ok(more) = receiver.try_recv() {
                    batch.push(more);
                }
                if this.update(cx, |this, cx| this.accept(batch, cx)).is_err() {
                    break;
                }
            }
        });
        let (first_path, first_tiles) = BENCH_CONFIGS[0];
        let mut gate = Self {
            path: first_path,
            tiles: 1,
            generation: 0,
            frames: Vec::new(),
            shown_sequences: Vec::new(),
            stale_images: Vec::new(),
            sender,
            producer: None,
            counters: Counters {
                since: Instant::now(),
                cpu: process_cpu_seconds(),
                generator_nanos: 0,
                delivered: 0,
                shown: 0,
                replaced_before_shown: 0,
            },
            latest: Sample::default(),
            bench: None,
            bench_report: "no benchmark run yet".into(),
            _receiver: task,
        };
        gate.configure(first_path, first_tiles, cx);
        gate
    }

    fn configure(&mut self, path: DrawPath, tiles: usize, cx: &mut Context<Self>) {
        self.producer = None;
        self.generation += 1;
        self.path = path;
        self.tiles = tiles;
        for frame in self.frames.drain(..).flatten() {
            if let Payload::Image(image) = frame.payload {
                self.stale_images.push(image);
            }
        }
        self.frames = (0..tiles).map(|_| None).collect();
        self.shown_sequences = vec![None; tiles];
        self.producer = Some(Producer::start(
            self.generation,
            path,
            tiles,
            self.sender.clone(),
        ));
        self.reset_counters();
        note(cx, format!("video: {} tiles, {}", tiles, path.label()));
        cx.notify();
    }

    fn reset_counters(&mut self) {
        self.counters = Counters {
            since: Instant::now(),
            cpu: process_cpu_seconds(),
            generator_nanos: self
                .producer
                .as_ref()
                .map(|producer| producer.cpu_nanos.load(Ordering::Relaxed))
                .unwrap_or(0),
            delivered: 0,
            shown: 0,
            replaced_before_shown: 0,
        };
    }

    fn accept(&mut self, batch: Vec<Delivered>, cx: &mut Context<Self>) {
        for delivered in batch {
            if delivered.generation != self.generation || delivered.tile >= self.frames.len() {
                if let Payload::Image(image) = delivered.payload {
                    self.stale_images.push(image);
                }
                continue;
            }
            self.counters.delivered += 1;
            let slot = &mut self.frames[delivered.tile];
            if let Some(previous) = slot.take() {
                if self.shown_sequences[delivered.tile] != Some(previous.sequence) {
                    self.counters.replaced_before_shown += 1;
                }
                if let Payload::Image(image) = previous.payload {
                    self.stale_images.push(image);
                }
            }
            *slot = Some(TileFrame {
                sequence: delivered.sequence,
                payload: delivered.payload,
            });
        }
        if self.counters.since.elapsed() >= Duration::from_secs(1) {
            self.take_sample(cx);
        }
        cx.notify();
    }

    fn take_sample(&mut self, cx: &mut Context<Self>) {
        let elapsed = self.counters.since.elapsed().as_secs_f64();
        let generator_nanos = self
            .producer
            .as_ref()
            .map(|producer| producer.cpu_nanos.load(Ordering::Relaxed))
            .unwrap_or(0);
        let tiles = self.tiles.max(1) as f64;
        self.latest = Sample {
            delivered_per_tile: self.counters.delivered as f64 / tiles / elapsed,
            shown_per_tile: self.counters.shown as f64 / tiles / elapsed,
            process_cpu: (process_cpu_seconds() - self.counters.cpu) / elapsed * 100.0,
            generator_cpu: generator_nanos.saturating_sub(self.counters.generator_nanos) as f64
                / 1e9
                / elapsed
                * 100.0,
            replaced_before_shown: self.counters.replaced_before_shown,
        };
        self.reset_counters();
        self.advance_bench(cx);
    }

    fn start_bench(&mut self, cx: &mut Context<Self>) {
        self.bench = Some(BenchRun {
            index: 0,
            seconds: 0,
            samples: Vec::new(),
            lines: Vec::new(),
        });
        let (path, tiles) = BENCH_CONFIGS[0];
        note(cx, "video: benchmark started");
        self.configure(path, tiles, cx);
    }

    fn advance_bench(&mut self, cx: &mut Context<Self>) {
        let latest = self.latest;
        let Some(bench) = self.bench.as_mut() else {
            return;
        };
        bench.seconds += 1;
        if bench.seconds <= WARMUP_SECONDS {
            return;
        }
        bench.samples.push(latest);
        if bench.samples.len() < MEASURE_SECONDS {
            return;
        }
        let count = bench.samples.len() as f64;
        let mean = |field: fn(&Sample) -> f64| bench.samples.iter().map(field).sum::<f64>() / count;
        let replaced: u64 = bench.samples.iter().map(|sample| sample.replaced_before_shown).sum();
        let (path, tiles) = BENCH_CONFIGS[bench.index];
        let line = format!(
            "{} | tiles={} | delivered {:.1} fps per tile | shown {:.1} fps per tile | process CPU {:.0} % of one core | of which frame generation {:.0} % | frames replaced before shown {}",
            path.label(),
            tiles,
            mean(|sample| sample.delivered_per_tile),
            mean(|sample| sample.shown_per_tile),
            mean(|sample| sample.process_cpu),
            mean(|sample| sample.generator_cpu),
            replaced
        );
        bench.lines.push(line.clone());
        bench.samples.clear();
        bench.seconds = 0;
        bench.index += 1;
        let next = BENCH_CONFIGS.get(bench.index).copied();
        let report = bench.lines.join("\n");
        note(cx, format!("video bench: {line}"));
        self.bench_report = report.clone().into();
        match next {
            Some((path, tiles)) => self.configure(path, tiles, cx),
            None => {
                std::fs::write(out_dir().join("c9-bench.txt"), report + "\n").ok();
                note(cx, "video: benchmark finished");
                self.bench = None;
            }
        }
    }
}

impl Render for VideoGate {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        for image in self.stale_images.drain(..) {
            window.drop_image(image).ok();
        }
        let columns = match self.tiles {
            1 => 1,
            2..=4 => 2,
            _ => 3,
        };
        let rows = self.tiles.div_ceil(columns);
        let viewport = window.viewport_size();
        let rem: f32 = window.rem_size().into();
        let available_width = f32::from(viewport.width) - 32.0;
        let available_height = f32::from(viewport.height) - 14.0 * rem;
        let tile_width = (available_width / columns as f32)
            .min(available_height / rows as f32 * 16.0 / 9.0)
            .max(80.0)
            - 6.0;
        let tile_height = tile_width * 9.0 / 16.0;

        let mut tiles = Vec::new();
        for (index, frame) in self.frames.iter().enumerate() {
            let content = frame.as_ref().map(|frame| {
                if self.shown_sequences[index] != Some(frame.sequence) {
                    self.shown_sequences[index] = Some(frame.sequence);
                    self.counters.shown += 1;
                }
                match &frame.payload {
                    #[cfg(target_os = "macos")]
                    Payload::Surface(buffer) => surface(buffer.0.clone())
                        .size_full()
                        .object_fit(ObjectFit::Contain)
                        .into_any_element(),
                    Payload::Image(image) => img(ImageSource::Render(image.clone()))
                        .size_full()
                        .object_fit(ObjectFit::Contain)
                        .into_any_element(),
                }
            });
            tiles.push(
                div()
                    .w(px(tile_width))
                    .h(px(tile_height))
                    .bg(Palette::rail())
                    .rounded(u(4.0))
                    .overflow_hidden()
                    .when_some(content, |this, content| this.child(content)),
            );
        }

        let config_button = |path: DrawPath, tiles: usize, cx: &mut Context<Self>| {
            let active = self.path == path && self.tiles == tiles;
            Button::new(SharedString::from(format!("video-{path:?}-{tiles}")))
                .small()
                .label(format!(
                    "{} × {}",
                    if path == DrawPath::Image { "img" } else { "surface" },
                    tiles
                ))
                .when(active, |button| button.primary())
                .on_click(cx.listener(move |this, _, _, cx| this.configure(path, tiles, cx)))
        };
        let mut buttons = Vec::new();
        for (path, tiles) in BENCH_CONFIGS {
            buttons.push(config_button(path, tiles, cx));
        }

        v_flex()
            .size_full()
            .p(u(12.0))
            .gap(u(8.0))
            .child(
                h_flex()
                    .flex_none()
                    .gap(u(6.0))
                    .children(buttons)
                    .child(
                        Button::new("video-bench")
                            .small()
                            .label(if self.bench.is_some() {
                                "Benchmark running"
                            } else {
                                "Run benchmark"
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.start_bench(cx))),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(u(12.0))
                    .font_features(tabular_numerals())
                    .child(format!(
                        "{} · {} tiles · source {}×{} at {} fps · delivered {:.1} fps per tile · shown {:.1} fps per tile · process CPU {:.0} % · generator {:.0} % · replaced before shown {}",
                        self.path.label(),
                        self.tiles,
                        width(),
                        height(),
                        FRAMES_PER_SECOND,
                        self.latest.delivered_per_tile,
                        self.latest.shown_per_tile,
                        self.latest.process_cpu,
                        self.latest.generator_cpu,
                        self.latest.replaced_before_shown
                    )),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .flex_wrap()
                    .content_start()
                    .gap(px(6.0))
                    .w(px((tile_width + 6.0) * columns as f32))
                    .children(tiles),
            )
            .child(
                v_flex()
                    .flex_none()
                    .text_size(u(11.0))
                    .font_family("Menlo")
                    .children(
                        self.bench_report
                            .lines()
                            .map(|line| div().child(SharedString::from(line.to_string()))),
                    ),
            )
    }
}
