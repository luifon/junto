use std::{
    borrow::Cow,
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};

use gpui::{App, FontFeatures, Global, Hsla, Rems, SharedString, rems, rgb};

pub const FONT_FAMILY: &str = "Atkinson Hyperlegible Next";
pub const BASE_FONT_PX: f32 = 16.0;

pub fn u(design_px: f32) -> Rems {
    rems(design_px / BASE_FONT_PX)
}

pub struct Palette;

impl Palette {
    pub fn stone() -> Hsla {
        rgb(0xefe9dd).into()
    }
    pub fn panel() -> Hsla {
        rgb(0xe6dfd0).into()
    }
    pub fn rail() -> Hsla {
        rgb(0x2a2622).into()
    }
    pub fn rail_text() -> Hsla {
        rgb(0xf3ecdf).into()
    }
    pub fn ink() -> Hsla {
        rgb(0x1f1b16).into()
    }
    pub fn muted() -> Hsla {
        rgb(0x6b6358).into()
    }
    pub fn border() -> Hsla {
        rgb(0xcfc6b4).into()
    }
    pub fn terracotta() -> Hsla {
        rgb(0xb5502f).into()
    }
    pub fn green() -> Hsla {
        rgb(0x3f6b4f).into()
    }
    pub fn ochre() -> Hsla {
        rgb(0xc99a2e).into()
    }
    pub fn red() -> Hsla {
        rgb(0xc0392b).into()
    }
    pub fn white() -> Hsla {
        rgb(0xfbf8f1).into()
    }
}

pub fn tabular_numerals() -> FontFeatures {
    FontFeatures(Arc::new(vec![("tnum".into(), 1)]))
}

static START: OnceLock<Instant> = OnceLock::new();
static OUT_DIR: OnceLock<PathBuf> = OnceLock::new();
static LOG_FILE: OnceLock<Mutex<fs::File>> = OnceLock::new();

pub fn init_out_dir(dir: PathBuf) {
    START.get_or_init(Instant::now);
    fs::create_dir_all(&dir).ok();
    let file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("events.txt"))
        .expect("events file");
    LOG_FILE.set(Mutex::new(file)).ok();
    OUT_DIR.set(dir).ok();
}

pub fn out_dir() -> &'static Path {
    OUT_DIR.get().expect("out dir")
}

pub fn elapsed() -> Duration {
    START.get_or_init(Instant::now).elapsed()
}

pub fn record(line: &str) {
    let stamped = format!("[{:9.3}] {line}", elapsed().as_secs_f64());
    eprintln!("{stamped}");
    if let Some(file) = LOG_FILE.get() {
        if let Ok(mut file) = file.lock() {
            writeln!(file, "{stamped}").ok();
        }
    }
}

#[derive(Default)]
pub struct EventLog {
    pub lines: Vec<SharedString>,
}

impl Global for EventLog {}

pub fn note(cx: &mut App, message: impl Into<String>) {
    let message = message.into();
    record(&message);
    let log = cx.default_global::<EventLog>();
    log.lines.push(message.into());
    if log.lines.len() > 200 {
        log.lines.remove(0);
    }
}

struct StderrLogger;

impl log::Log for StderrLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Info
    }
    fn log(&self, entry: &log::Record) {
        if self.enabled(entry.metadata()) {
            record(&format!(
                "log {} {}: {}",
                entry.level(),
                entry.target(),
                entry.args()
            ));
        }
    }
    fn flush(&self) {}
}

pub fn init_logger() {
    static LOGGER: StderrLogger = StderrLogger;
    log::set_logger(&LOGGER).ok();
    log::set_max_level(log::LevelFilter::Info);
}

pub fn load_fonts(cx: &App) -> usize {
    let dir = out_dir().join("fonts");
    let mut fonts = Vec::new();
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "ttf") {
                if let Ok(bytes) = fs::read(&path) {
                    fonts.push(Cow::Owned(bytes));
                }
            }
        }
    }
    let count = fonts.len();
    if let Err(error) = cx.text_system().add_fonts(fonts) {
        record(&format!("font load failed: {error:#}"));
        return 0;
    }
    count
}

pub struct Assets {
    pub png: PathBuf,
    pub gif: PathBuf,
    pub http_base: String,
    pub https_image: Option<String>,
}

impl Global for Assets {}

pub fn prepare_assets() -> Assets {
    let dir = out_dir().join("assets");
    fs::create_dir_all(&dir).ok();
    let png = dir.join("sample.png");
    let gif = dir.join("anim.gif");
    if !png.exists() {
        write_png(&png);
    }
    if !gif.exists() {
        write_gif(&gif);
    }
    let http_base = serve_directory(dir);
    Assets {
        png,
        gif,
        http_base,
        https_image: std::env::var("SPIKE_HTTPS_IMAGE").ok(),
    }
}

fn write_png(path: &Path) {
    let image = image::RgbaImage::from_fn(320, 180, |x, y| {
        let stripe = ((x / 40) + (y / 30)) % 2 == 0;
        let r = (x * 255 / 320) as u8;
        let g = (y * 255 / 180) as u8;
        if stripe {
            image::Rgba([r, g, 0x60, 0xff])
        } else {
            image::Rgba([0xb5, 0x50, 0x2f, 0xff])
        }
    });
    image.save(path).expect("png");
}

fn write_gif(path: &Path) {
    use image::codecs::gif::{GifEncoder, Repeat};
    let file = fs::File::create(path).expect("gif");
    let mut encoder = GifEncoder::new_with_speed(file, 10);
    encoder.set_repeat(Repeat::Infinite).ok();
    for step in 0..16u32 {
        let image = image::RgbaImage::from_fn(160, 120, |x, y| {
            let bar = step * 10;
            if x >= bar && x < bar + 20 {
                image::Rgba([0xc9, 0x9a, 0x2e, 0xff])
            } else if (y / 20) % 2 == 0 {
                image::Rgba([0x3f, 0x6b, 0x4f, 0xff])
            } else {
                image::Rgba([0x2a, 0x26, 0x22, 0xff])
            }
        });
        let frame = image::Frame::from_parts(
            image,
            0,
            0,
            image::Delay::from_numer_denom_ms(80, 1),
        );
        encoder.encode_frame(frame).expect("gif frame");
    }
}

fn serve_directory(dir: PathBuf) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut buffer = [0u8; 2048];
            let read = stream.read(&mut buffer).unwrap_or(0);
            let request = String::from_utf8_lossy(&buffer[..read]).to_string();
            let name = request
                .split_whitespace()
                .nth(1)
                .unwrap_or("/")
                .trim_start_matches('/')
                .to_string();
            let safe = !name.contains("..") && !name.contains('/');
            let body = if safe { fs::read(dir.join(&name)).ok() } else { None };
            record(&format!(
                "http server: GET /{name} -> {}",
                if body.is_some() { 200 } else { 404 }
            ));
            match body {
                Some(body) => {
                    let kind = if name.ends_with(".gif") {
                        "image/gif"
                    } else {
                        "image/png"
                    };
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    stream.write_all(header.as_bytes()).ok();
                    stream.write_all(&body).ok();
                }
                None => {
                    stream
                        .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                        .ok();
                }
            }
        }
    });
    format!("http://127.0.0.1:{port}")
}

pub fn process_cpu_seconds() -> f64 {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    let result = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if result != 0 {
        return 0.0;
    }
    let usage = unsafe { usage.assume_init() };
    let seconds = |time: libc::timeval| time.tv_sec as f64 + time.tv_usec as f64 / 1e6;
    seconds(usage.ru_utime) + seconds(usage.ru_stime)
}
