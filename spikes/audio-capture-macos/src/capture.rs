use crate::analysis::{dbfs, first_index_above, hann_goertzel_amplitude, rms, verdict};
use crate::tone::{self, unix_nanos};
use crate::wav;
use screencapturekit::cm::CMClock;
use screencapturekit::prelude::*;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    System,
    App,
    Window,
}

pub struct Options {
    pub mode: Mode,
    pub pid: Option<i32>,
    pub bundle: Option<String>,
    pub exclude_bundles: Vec<String>,
    pub seconds: f64,
    pub own_tone: Option<f64>,
    pub exclude_self: bool,
    pub frequencies: Vec<f64>,
    pub out: Option<PathBuf>,
    pub sample_rate: i32,
    pub channel_count: i32,
}

#[derive(Clone, Debug)]
struct Format {
    sample_rate: f64,
    channels: u32,
    bits_per_channel: u32,
    bytes_per_frame: u32,
    flags: u32,
    is_float: bool,
    is_big_endian: bool,
    subtype: String,
    buffers_in_list: usize,
    channels_per_buffer: u32,
}

struct BufferRecord {
    arrival_unix_nanos: u64,
    host_clock_seconds: f64,
    presentation_seconds: f64,
    frames: usize,
    first_frame: usize,
}

#[derive(Default)]
struct Captured {
    channels: Vec<Vec<f32>>,
    buffers: Vec<BufferRecord>,
    format: Option<Format>,
    callback_thread: Option<String>,
    unsupported: Option<String>,
}

fn store(sample: &CMSampleBuffer, shared: &Mutex<Captured>, host_clock: &CMClock) {
    let arrival_unix_nanos = unix_nanos();
    let host_clock_seconds = host_clock.time().as_seconds().unwrap_or(f64::NAN);
    let presentation_seconds = sample.presentation_timestamp().as_seconds().unwrap_or(f64::NAN);
    let Ok(list) = sample.audio_buffer_list() else {
        return;
    };
    let mut captured = shared.lock().unwrap();

    if captured.format.is_none() {
        let thread = std::thread::current();
        captured.callback_thread = Some(format!(
            "name={:?} id={:?}",
            thread.name().unwrap_or("<unnamed>"),
            thread.id()
        ));
        if let Some(description) = sample.format_description() {
            captured.format = Some(Format {
                sample_rate: description.audio_sample_rate().unwrap_or(0.0),
                channels: description.audio_channel_count().unwrap_or(0),
                bits_per_channel: description.audio_bits_per_channel().unwrap_or(0),
                bytes_per_frame: description.audio_bytes_per_frame().unwrap_or(0),
                flags: description.audio_format_flags().unwrap_or(0),
                is_float: description.audio_is_float(),
                is_big_endian: description.audio_is_big_endian(),
                subtype: description.media_subtype_string(),
                buffers_in_list: list.num_buffers(),
                channels_per_buffer: list.get(0).map_or(0, |b| b.number_channels),
            });
        }
    }

    let Some(format) = captured.format.clone() else {
        return;
    };
    if !format.is_float || format.bits_per_channel != 32 || format.is_big_endian {
        captured.unsupported = Some(format!("{format:?}"));
        return;
    }

    let first_frame = captured.channels.first().map_or(0, Vec::len);
    let mut channel_index = 0usize;
    let mut frames = 0usize;
    for buffer in list.iter() {
        let interleaved = buffer.number_channels.max(1) as usize;
        let samples: Vec<f32> = buffer
            .data()
            .chunks_exact(4)
            .map(|bytes| f32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
            .collect();
        frames = samples.len() / interleaved;
        for offset in 0..interleaved {
            while captured.channels.len() <= channel_index {
                captured.channels.push(Vec::new());
            }
            captured.channels[channel_index]
                .extend(samples.iter().skip(offset).step_by(interleaved).copied());
            channel_index += 1;
        }
    }
    captured.buffers.push(BufferRecord {
        arrival_unix_nanos,
        host_clock_seconds,
        presentation_seconds,
        frames,
        first_frame,
    });
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGMainDisplayID() -> u32;
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

pub fn request(wait_seconds: f64) -> Result<(), String> {
    println!("pid={} CGPreflightScreenCaptureAccess={}", std::process::id(), unsafe {
        CGPreflightScreenCaptureAccess()
    });
    println!("CGRequestScreenCaptureAccess={}", unsafe { CGRequestScreenCaptureAccess() });
    match SCShareableContent::get() {
        Ok(content) => println!("SCShareableContent ok applications={}", content.applications().len()),
        Err(error) => println!("SCShareableContent error: {error:?}"),
    }
    std::thread::sleep(Duration::from_secs_f64(wait_seconds));
    println!("after {wait_seconds} s CGPreflightScreenCaptureAccess={}", unsafe {
        CGPreflightScreenCaptureAccess()
    });
    Ok(())
}

pub fn preflight() -> Result<(), String> {
    println!("pid={} CGPreflightScreenCaptureAccess={}", std::process::id(), unsafe {
        CGPreflightScreenCaptureAccess()
    });
    Ok(())
}

fn connect_to_window_server() {
    unsafe {
        CGMainDisplayID();
    }
}

fn find_application(
    content: &SCShareableContent,
    options: &Options,
) -> Result<SCRunningApplication, String> {
    let applications = content.applications();
    if let Some(bundle) = &options.bundle {
        return applications
            .into_iter()
            .find(|a| &a.bundle_identifier() == bundle)
            .ok_or(format!("bundle {bundle} is not in SCShareableContent.applications"));
    }
    let pid = options.pid.ok_or("--pid or --bundle is required")?;
    applications
        .into_iter()
        .find(|a| a.process_id() == pid)
        .ok_or(format!("pid {pid} is not in SCShareableContent.applications"))
}

fn build_filter(options: &Options) -> Result<SCContentFilter, String> {
    connect_to_window_server();
    let content = SCShareableContent::create()
        .with_on_screen_windows_only(false)
        .get()
        .map_err(|e| format!("SCShareableContent failed: {e:?}"))?;
    let display = content.displays().into_iter().next().ok_or("no display")?;

    match options.mode {
        Mode::System if options.exclude_bundles.is_empty() => {
            println!("filter=SCContentFilter(display:excludingWindows:[])");
            SCContentFilter::create()
                .with_display(&display)
                .with_excluding_windows(&[])
                .build()
                .map_err(|e| format!("{e:?}"))
        }
        Mode::System => {
            let excluded: Vec<SCRunningApplication> = content
                .applications()
                .into_iter()
                .filter(|a| options.exclude_bundles.contains(&a.bundle_identifier()))
                .collect();
            if let Some(missing) = options
                .exclude_bundles
                .iter()
                .find(|bundle| !excluded.iter().any(|a| &a.bundle_identifier() == *bundle))
            {
                return Err(format!("bundle {missing} is not in SCShareableContent.applications"));
            }
            println!(
                "filter=SCContentFilter(display:excludingApplications:[{} application(s)]:exceptingWindows:[])",
                excluded.len()
            );
            let references: Vec<&SCRunningApplication> = excluded.iter().collect();
            SCContentFilter::create()
                .with_display(&display)
                .with_excluding_applications(&references, &[])
                .build()
                .map_err(|e| format!("{e:?}"))
        }
        Mode::App => {
            let application = find_application(&content, options)?;
            println!(
                "filter=SCContentFilter(display:includingApplications:[application]:exceptingWindows:[]) application_pid={} bundle={:?}",
                application.process_id(),
                application.bundle_identifier()
            );
            SCContentFilter::create()
                .with_display(&display)
                .with_including_applications(&[&application], &[])
                .build()
                .map_err(|e| format!("{e:?}"))
        }
        Mode::Window => {
            let application = find_application(&content, options)?;
            let pid = application.process_id();
            let window = content
                .windows()
                .into_iter()
                .filter(|w| w.owning_application().is_some_and(|a| a.process_id() == pid))
                .max_by_key(|w| {
                    let frame = w.frame();
                    (frame.size.width * frame.size.height) as i64
                })
                .ok_or(format!("pid {pid} owns no window in SCShareableContent.windows"))?;
            println!(
                "filter=SCContentFilter(desktopIndependentWindow:) window_id={} on_screen={} owner_pid={pid} owner_bundle={:?}",
                window.window_id(),
                window.is_on_screen(),
                application.bundle_identifier()
            );
            SCContentFilter::create()
                .with_window(&window)
                .build()
                .map_err(|e| format!("{e:?}"))
        }
    }
}

pub fn run(options: &Options) -> Result<(), String> {
    let filter = build_filter(options)?;

    let configuration = SCStreamConfiguration::new()
        .with_width(16)
        .with_height(16)
        .with_fps(1)
        .with_captures_audio(true)
        .with_sample_rate(options.sample_rate)
        .with_channel_count(options.channel_count)
        .with_excludes_current_process_audio(options.exclude_self);
    println!(
        "configuration capturesAudio={} sampleRate={} channelCount={} excludesCurrentProcessAudio={}",
        configuration.captures_audio(),
        configuration.sample_rate(),
        configuration.channel_count(),
        configuration.excludes_current_process_audio()
    );

    let shared = Arc::new(Mutex::new(Captured::default()));
    let shared_in_handler = shared.clone();
    let host_clock = CMClock::host_time_clock();

    let mut stream = SCStream::new(&filter, &configuration).map_err(|e| format!("SCStream::new: {e:?}"))?;
    stream
        .add_output_handler(
            move |sample: CMSampleBuffer, _kind: SCStreamOutputType| {
                store(&sample, &shared_in_handler, &host_clock);
            },
            SCStreamOutputType::Audio,
        )
        .map_err(|e| format!("add_output_handler: {e:?}"))?;

    let start_requested = unix_nanos();
    stream.start_capture().map_err(|e| format!("start_capture: {e:?}"))?;
    println!("start_capture_returned_after_ms={:.1}", (unix_nanos() - start_requested) as f64 / 1e6);

    let own_player = match options.own_tone {
        Some(frequency) => Some(tone::play(frequency, 0.2, 0.0)?),
        None => None,
    };

    std::thread::sleep(Duration::from_secs_f64(options.seconds));
    stream.stop_capture().map_err(|e| format!("stop_capture: {e:?}"))?;
    if let Some(player) = &own_player {
        println!("own_tone_output sample_rate={} channels={}", player.sample_rate, player.channels);
    }
    drop(own_player);

    let captured = shared.lock().unwrap();
    report(&captured, options, start_requested)
}

fn report(captured: &Captured, options: &Options, start_requested: u64) -> Result<(), String> {
    if let Some(unsupported) = &captured.unsupported {
        return Err(format!("unsupported sample format: {unsupported}"));
    }
    let Some(format) = &captured.format else {
        println!("audio_buffers=0");
        return Err("no audio buffer was delivered".into());
    };
    println!(
        "format sample_rate={} channels={} bits_per_channel={} bytes_per_frame={} format_flags=0x{:x} float={} big_endian={} subtype={:?} buffers_in_list={} channels_per_buffer={}",
        format.sample_rate, format.channels, format.bits_per_channel, format.bytes_per_frame,
        format.flags, format.is_float, format.is_big_endian, format.subtype,
        format.buffers_in_list, format.channels_per_buffer
    );
    println!("callback_thread {}", captured.callback_thread.as_deref().unwrap_or("?"));

    let buffers = &captured.buffers;
    let frame_counts: Vec<usize> = buffers.iter().map(|b| b.frames).collect();
    let min_frames = frame_counts.iter().min().copied().unwrap_or(0);
    let max_frames = frame_counts.iter().max().copied().unwrap_or(0);
    println!(
        "audio_buffers={} frames_per_buffer_min={} frames_per_buffer_max={} buffer_ms={:.2}",
        buffers.len(), min_frames, max_frames, max_frames as f64 * 1000.0 / format.sample_rate
    );
    if let Some(first) = buffers.first() {
        println!("first_buffer_after_start_ms={:.1}", (first.arrival_unix_nanos as f64 - start_requested as f64) / 1e6);
    }

    let delivery: Vec<f64> = buffers
        .iter()
        .map(|b| (b.host_clock_seconds - b.presentation_seconds) * 1000.0)
        .filter(|d| d.is_finite())
        .collect();
    if !delivery.is_empty() {
        let mean = delivery.iter().sum::<f64>() / delivery.len() as f64;
        let min = delivery.iter().copied().fold(f64::INFINITY, f64::min);
        let max = delivery.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        println!("host_clock_minus_buffer_pts_ms mean={mean:.2} min={min:.2} max={max:.2}");
    }

    let frames = captured.channels.iter().map(Vec::len).min().unwrap_or(0);
    let mono: Vec<f32> = (0..frames)
        .map(|i| captured.channels.iter().map(|c| c[i]).sum::<f32>() / captured.channels.len() as f32)
        .collect();
    println!("captured_frames={} captured_seconds={:.3} rms_dbfs={:.1}", frames, frames as f64 / format.sample_rate, dbfs(rms(&mono)));
    for (index, channel) in captured.channels.iter().enumerate() {
        println!("channel[{index}] rms_dbfs={:.1}", dbfs(rms(&channel[..frames])));
    }

    for frequency in &options.frequencies {
        let level = dbfs(hann_goertzel_amplitude(&mono, format.sample_rate, *frequency));
        println!("tone hz={frequency} level_dbfs={level:.1} verdict={}", verdict(level));
    }

    if let Some(onset) = first_index_above(&mono, 0.01) {
        if let Some(buffer) = buffers.iter().find(|b| onset >= b.first_frame && onset < b.first_frame + b.frames) {
            let offset = onset - buffer.first_frame;
            println!(
                "onset frame={} buffer_arrival_unix_nanos={} offset_in_buffer_frames={} buffer_frames={} onset_pts_seconds={:.6} arrival_host_clock_seconds={:.6}",
                onset, buffer.arrival_unix_nanos, offset, buffer.frames,
                buffer.presentation_seconds + offset as f64 / format.sample_rate,
                buffer.host_clock_seconds
            );
        }
    } else {
        println!("onset none");
    }

    if let Some(path) = &options.out {
        let trimmed: Vec<Vec<f32>> = captured.channels.iter().map(|c| c[..frames].to_vec()).collect();
        wav::write_float32(path, format.sample_rate as u32, &trimmed).map_err(|e| e.to_string())?;
        println!("wav={}", path.display());
    }
    Ok(())
}

pub fn list() -> Result<(), String> {
    let content = SCShareableContent::create()
        .with_on_screen_windows_only(false)
        .get()
        .map_err(|e| format!("SCShareableContent failed: {e:?}"))?;
    let windows = content.windows();
    for display in content.displays() {
        println!("display id={} {}x{}", display.display_id(), display.width(), display.height());
    }
    for application in content.applications() {
        let pid = application.process_id();
        let window_count = windows
            .iter()
            .filter(|w| w.owning_application().is_some_and(|a| a.process_id() == pid))
            .count();
        println!(
            "application pid={} windows={} bundle={:?} name={:?}",
            pid,
            window_count,
            application.bundle_identifier(),
            application.application_name()
        );
    }
    Ok(())
}
