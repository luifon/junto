use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{OutputCallbackInfo, SampleFormat, Stream, StreamConfig};
use std::f64::consts::PI;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn unix_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

pub struct TonePlayer {
    _stream: Stream,
    pub sample_rate: u32,
    pub channels: u16,
    pub first_tone_callback_unix_nanos: Arc<AtomicU64>,
    pub output_latency_nanos: Arc<AtomicU64>,
    pub first_tone_frame_offset: Arc<AtomicU64>,
}

pub fn play(frequency: f64, amplitude: f64, lead_silence_seconds: f64) -> Result<TonePlayer, String> {
    let device = cpal::default_host()
        .default_output_device()
        .ok_or("no default output device")?;
    let supported = device.default_output_config().map_err(|e| e.to_string())?;
    if supported.sample_format() != SampleFormat::F32 {
        return Err(format!("output format is {}, expected f32", supported.sample_format()));
    }
    let config: StreamConfig = supported.into();
    let sample_rate = config.sample_rate;
    let channels = config.channels;
    let lead_frames = (lead_silence_seconds * f64::from(sample_rate)) as u64;
    let first_tone = Arc::new(AtomicU64::new(0));
    let latency = Arc::new(AtomicU64::new(0));
    let offset = Arc::new(AtomicU64::new(0));
    let offset_in_callback = offset.clone();
    let first_tone_in_callback = first_tone.clone();
    let latency_in_callback = latency.clone();
    let mut frame_index = 0u64;
    let step = 2.0 * PI * frequency / f64::from(sample_rate);

    let stream = device
        .build_output_stream(
            config,
            move |data: &mut [f32], info: &OutputCallbackInfo| {
                let callback_started = unix_nanos();
                for (position, frame) in data.chunks_mut(channels as usize).enumerate() {
                    let value = if frame_index >= lead_frames {
                        if first_tone_in_callback.load(Ordering::Relaxed) == 0 {
                            first_tone_in_callback.store(callback_started, Ordering::Relaxed);
                            offset_in_callback.store(position as u64, Ordering::Relaxed);
                            let stamp = info.timestamp();
                            let ahead = stamp.playback.duration_since(stamp.callback);
                            latency_in_callback.store(ahead.as_nanos() as u64, Ordering::Relaxed);
                        }
                        (amplitude * (step * (frame_index - lead_frames) as f64).sin()) as f32
                    } else {
                        0.0
                    };
                    for sample in frame.iter_mut() {
                        *sample = value;
                    }
                    frame_index += 1;
                }
            },
            |error| eprintln!("tone stream error: {error}"),
            None,
        )
        .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;

    Ok(TonePlayer {
        _stream: stream,
        sample_rate,
        channels,
        first_tone_callback_unix_nanos: first_tone,
        output_latency_nanos: latency,
        first_tone_frame_offset: offset,
    })
}
