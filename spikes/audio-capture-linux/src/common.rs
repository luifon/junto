use pipewire as pw;
use pw::spa;
use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::str::FromStr;

pub type AnyResult<T> = Result<T, Box<dyn std::error::Error>>;

pub const RATE: u32 = 48_000;
pub const CHANNELS: usize = 2;
pub const BYTES_PER_FRAME: usize = CHANNELS * std::mem::size_of::<f32>();

pub struct Options(HashMap<String, String>);

impl Options {
    pub fn parse(args: impl Iterator<Item = String>) -> Self {
        let mut values = HashMap::new();
        let mut pending: Option<String> = None;
        for arg in args {
            match arg.strip_prefix("--") {
                Some(key) => {
                    if let Some(flag) = pending.replace(key.to_string()) {
                        values.insert(flag, "true".to_string());
                    }
                }
                None => {
                    if let Some(key) = pending.take() {
                        values.insert(key, arg);
                    }
                }
            }
        }
        if let Some(flag) = pending {
            values.insert(flag, "true".to_string());
        }
        Options(values)
    }

    pub fn text(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }

    pub fn required(&self, key: &str) -> AnyResult<&str> {
        self.text(key).ok_or_else(|| format!("missing --{key}").into())
    }

    pub fn number<T: FromStr>(&self, key: &str, default: T) -> AnyResult<T> {
        match self.text(key) {
            Some(text) => text.parse().map_err(|_| format!("bad value for --{key}: {text}").into()),
            None => Ok(default),
        }
    }

    pub fn optional_number<T: FromStr>(&self, key: &str) -> AnyResult<Option<T>> {
        match self.text(key) {
            Some(text) => text.parse().map(Some).map_err(|_| format!("bad value for --{key}: {text}").into()),
            None => Ok(None),
        }
    }

    pub fn flag(&self, key: &str) -> bool {
        self.text(key) == Some("true")
    }
}

pub fn monotonic_ns() -> u64 {
    let mut time = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) };
    time.tv_sec as u64 * 1_000_000_000 + time.tv_nsec as u64
}

pub struct Sine {
    phase: f64,
    step: f64,
    amplitude: f32,
}

impl Sine {
    pub fn new(frequency: f64, amplitude: f32) -> Self {
        Sine { phase: 0.0, step: std::f64::consts::TAU * frequency / RATE as f64, amplitude }
    }

    pub fn next_sample(&mut self) -> f32 {
        let value = self.phase.sin() as f32 * self.amplitude;
        self.phase += self.step;
        if self.phase >= std::f64::consts::TAU {
            self.phase -= std::f64::consts::TAU;
        }
        value
    }
}

pub fn stereo_f32_format_pod() -> Vec<u8> {
    let mut info = spa::param::audio::AudioInfoRaw::new();
    info.set_format(spa::param::audio::AudioFormat::F32LE);
    info.set_rate(RATE);
    info.set_channels(CHANNELS as u32);
    let mut position = [0; spa::param::audio::MAX_CHANNELS];
    position[0] = spa::sys::SPA_AUDIO_CHANNEL_FL;
    position[1] = spa::sys::SPA_AUDIO_CHANNEL_FR;
    info.set_position(position);
    let object = spa::pod::Object {
        type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: spa::param::ParamType::EnumFormat.as_raw(),
        properties: info.into(),
    };
    spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(object),
    )
    .expect("format pod")
    .0
    .into_inner()
}

pub fn roundtrip(mainloop: &pw::main_loop::MainLoopRc, core: &pw::core::CoreRc) -> AnyResult<()> {
    let done = Rc::new(Cell::new(false));
    let pending = core.sync(0)?;
    let _listener = core
        .add_listener_local()
        .done({
            let done = done.clone();
            let mainloop = mainloop.clone();
            move |id, seq| {
                if id == pw::core::PW_ID_CORE && seq == pending {
                    done.set(true);
                    mainloop.quit();
                }
            }
        })
        .register();
    while !done.get() {
        mainloop.run();
    }
    Ok(())
}

pub fn connect(application_name: &str) -> AnyResult<(pw::main_loop::MainLoopRc, pw::context::ContextRc, pw::core::CoreRc)> {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(
        &mainloop,
        Some(pw::properties::properties! { "application.name" => application_name }),
    )?;
    let core = context.connect_rc(None)?;
    Ok((mainloop, context, core))
}
