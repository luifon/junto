use std::time::Duration;

use anyhow::{anyhow, Result};
use livekit::webrtc::video_frame::{I420Buffer, VideoFrame, VideoRotation};
use livekit::webrtc::video_source::native::NativeVideoSource;
use tokio::time::{interval, MissedTickBehavior};

use crate::events::unix_ms;
use crate::tone::Marker;

#[derive(Clone, Copy, PartialEq)]
pub enum PatternKind {
    Camera,
    Screen,
}

#[derive(Clone, Copy)]
struct Yuv(u8, u8, u8);

const WHITE: Yuv = Yuv(235, 128, 128);
const BLACK: Yuv = Yuv(16, 128, 128);

fn yuv(r: u8, g: u8, b: u8) -> Yuv {
    let (r, g, b) = (r as f64, g as f64, b as f64);
    Yuv(
        (16.0 + 0.257 * r + 0.504 * g + 0.098 * b) as u8,
        (128.0 - 0.148 * r - 0.291 * g + 0.439 * b) as u8,
        (128.0 + 0.439 * r - 0.368 * g - 0.071 * b) as u8,
    )
}

pub fn parse_size(text: &str) -> Result<(u32, u32)> {
    let (w, h) = text.split_once('x').ok_or_else(|| anyhow!("size must be WIDTHxHEIGHT"))?;
    let (w, h): (u32, u32) = (w.parse()?, h.parse()?);
    if w < 64 || h < 64 || w % 2 != 0 || h % 2 != 0 {
        return Err(anyhow!("size must be even and at least 64x64"));
    }
    Ok((w, h))
}

pub fn parse_color(text: &str) -> Result<(u8, u8, u8)> {
    let value = u32::from_str_radix(text.trim_start_matches('#'), 16)?;
    Ok(((value >> 16) as u8, (value >> 8) as u8, value as u8))
}

pub fn color_for(identity: &str) -> (u8, u8, u8) {
    const PALETTE: [(u8, u8, u8); 6] =
        [(32, 72, 200), (200, 48, 48), (24, 140, 70), (150, 60, 180), (210, 120, 20), (20, 140, 150)];
    let sum: usize = identity.bytes().map(|b| b as usize).sum();
    PALETTE[sum % PALETTE.len()]
}

struct Canvas {
    width: usize,
    height: usize,
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
}

impl Canvas {
    fn new(width: usize, height: usize) -> Self {
        let chroma = (width / 2) * (height / 2);
        Self { width, height, y: vec![16; width * height], u: vec![128; chroma], v: vec![128; chroma] }
    }

    fn fill(&mut self, x: usize, y: usize, w: usize, h: usize, color: Yuv) {
        let x1 = (x + w).min(self.width);
        let y1 = (y + h).min(self.height);
        for row in y.min(y1)..y1 {
            self.y[row * self.width + x.min(x1)..row * self.width + x1].fill(color.0);
        }
        let cw = self.width / 2;
        for row in y / 2..y1.div_ceil(2).min(self.height / 2) {
            let (a, b) = (row * cw + (x / 2).min(cw), row * cw + x1.div_ceil(2).min(cw));
            self.u[a..b].fill(color.1);
            self.v[a..b].fill(color.2);
        }
    }

    fn text(&mut self, x: usize, y: usize, scale: usize, text: &str) {
        let width = text.chars().count() * 6 * scale + scale;
        self.fill(x, y, width, 9 * scale, BLACK);
        for (i, ch) in text.chars().enumerate() {
            let rows = glyph(ch);
            for (r, bits) in rows.iter().enumerate() {
                for c in 0..5 {
                    if bits & (0b10000 >> c) != 0 {
                        let px = x + scale + (i * 6 + c) * scale;
                        self.fill(px, y + (r + 1) * scale, scale, scale, WHITE);
                    }
                }
            }
        }
    }
}

pub struct Pattern {
    kind: PatternKind,
    label: String,
    background: Canvas,
    marker: Option<Marker>,
}

impl Pattern {
    pub fn new(
        kind: PatternKind,
        (width, height): (u32, u32),
        label: &str,
        color: (u8, u8, u8),
        marker: Option<Marker>,
    ) -> Self {
        let (w, h) = (width as usize, height as usize);
        let mut background = Canvas::new(w, h);
        match kind {
            PatternKind::Camera => {
                let bars = [
                    yuv(192, 192, 192),
                    yuv(192, 192, 0),
                    yuv(0, 192, 192),
                    yuv(0, 192, 0),
                    yuv(192, 0, 192),
                    yuv(192, 0, 0),
                    yuv(0, 0, 192),
                    yuv(32, 32, 32),
                ];
                for (i, bar) in bars.iter().enumerate() {
                    background.fill(i * w / bars.len(), 0, w / bars.len() + 1, h, *bar);
                }
                background.fill(0, h - h / 6, w, h / 6, yuv(color.0, color.1, color.2));
            }
            PatternKind::Screen => {
                background.fill(0, 0, w, h, yuv(color.0, color.1, color.2));
                let line = yuv(color.0 / 2, color.1 / 2, color.2 / 2);
                for x in (0..w).step_by(w / 16) {
                    background.fill(x, 0, 2, h, line);
                }
                for y in (0..h).step_by(h / 9) {
                    background.fill(0, y, w, 2, line);
                }
            }
        }
        let prefix = if kind == PatternKind::Camera { "CAM" } else { "SCREEN" };
        Self { kind, label: format!("{prefix} {}", label.to_uppercase()), background, marker }
    }

    fn draw(&self, counter: u64) -> I420Buffer {
        let (w, h) = (self.background.width, self.background.height);
        let mut canvas = Canvas {
            width: w,
            height: h,
            y: self.background.y.clone(),
            u: self.background.u.clone(),
            v: self.background.v.clone(),
        };
        let scale = (h / 60).max(2);
        let step = counter as usize;
        match self.kind {
            PatternKind::Camera => canvas.fill((step * 8) % w, 0, scale * 2, h, WHITE),
            PatternKind::Screen => {
                let side = h / 8;
                canvas.fill((step * 16) % (w - side), (step * 9) % (h - side), side, side, WHITE)
            }
        }
        if self.marker.is_some_and(|m| m.is_active(unix_ms())) {
            canvas.fill(0, h * 2 / 3, w, h / 3, WHITE);
        }
        canvas.text(scale * 2, scale * 2, scale, &self.label);
        canvas.text(scale * 2, scale * 13, scale, &format!("{counter:06}"));

        let mut buffer = I420Buffer::new(w as u32, h as u32);
        let (stride_y, stride_u, stride_v) = buffer.strides();
        let (dst_y, dst_u, dst_v) = buffer.data_mut();
        copy_plane(&canvas.y, w, dst_y, stride_y as usize);
        copy_plane(&canvas.u, w / 2, dst_u, stride_u as usize);
        copy_plane(&canvas.v, w / 2, dst_v, stride_v as usize);
        buffer
    }
}

fn copy_plane(src: &[u8], width: usize, dst: &mut [u8], stride: usize) {
    for (src_row, dst_row) in src.chunks_exact(width).zip(dst.chunks_mut(stride)) {
        dst_row[..width].copy_from_slice(src_row);
    }
}

/// Feeds the pattern into the source at the given frame rate.
pub async fn run_pattern(source: NativeVideoSource, pattern: Pattern, fps: u32) {
    let mut ticker = interval(Duration::from_secs_f64(1.0 / fps.max(1) as f64));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut counter: u64 = 0;
    loop {
        ticker.tick().await;
        let frame = VideoFrame::new(VideoRotation::VideoRotation0, pattern.draw(counter));
        source.capture_frame(&frame);
        counter += 1;
    }
}

fn glyph(ch: char) -> [u8; 7] {
    match ch {
        '0' => [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
        '1' => [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        '2' => [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111],
        '3' => [0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110],
        '4' => [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
        '5' => [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
        '6' => [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
        '7' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
        '8' => [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
        '9' => [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100],
        'A' => [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'B' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110],
        'C' => [0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110],
        'D' => [0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110],
        'E' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111],
        'F' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000],
        'G' => [0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01111],
        'H' => [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'I' => [0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        'J' => [0b00111, 0b00010, 0b00010, 0b00010, 0b00010, 0b10010, 0b01100],
        'K' => [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001],
        'L' => [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111],
        'M' => [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001],
        'N' => [0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001],
        'O' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'P' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000],
        'Q' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101],
        'R' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001],
        'S' => [0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110],
        'T' => [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        'U' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'V' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100],
        'W' => [0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b11011, 0b10001],
        'X' => [0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001],
        'Y' => [0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100],
        'Z' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111],
        '-' => [0, 0, 0, 0b11111, 0, 0, 0],
        '_' => [0, 0, 0, 0, 0, 0, 0b11111],
        '.' => [0, 0, 0, 0, 0, 0b01100, 0b01100],
        ':' => [0, 0b00100, 0b00100, 0, 0b00100, 0b00100, 0],
        ' ' => [0; 7],
        _ => [0b11111, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11111],
    }
}
