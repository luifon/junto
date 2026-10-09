use crate::common::{monotonic_ns, stereo_f32_format_pod, AnyResult, Sine, BYTES_PER_FRAME, CHANNELS};
use pipewire as pw;
use pw::spa;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;

pub struct ToneSpec {
    pub frequency: f64,
    pub amplitude: f32,
    pub lead_silence_frames: u64,
    pub tone_frames: Option<u64>,
    pub target: String,
    pub node_name: String,
    pub latency: Option<String>,
}

#[derive(Default)]
pub struct ToneReport {
    pub finished: AtomicBool,
    pub failed: AtomicBool,
    pub onset_ns: AtomicU64,
    pub onset_frame_in_block: AtomicU64,
    pub onset_block_frames: AtomicU64,
    pub onset_delay_ticks: AtomicI64,
    pub callbacks: AtomicU64,
}

struct ToneState {
    sine: Sine,
    position: u64,
    lead_silence_frames: u64,
    end_frame: Option<u64>,
    onset_recorded: bool,
    report: Arc<ToneReport>,
}

pub struct ToneStream {
    pub stream: pw::stream::StreamRc,
    pub report: Arc<ToneReport>,
    _listener: pw::stream::StreamListener<ToneState>,
}

pub fn start(core: &pw::core::CoreRc, spec: ToneSpec) -> AnyResult<ToneStream> {
    let mut properties = pw::properties::properties! {
        "media.type" => "Audio",
        "media.category" => "Playback",
        "media.role" => "Music",
        "node.name" => spec.node_name.clone(),
        "target.object" => spec.target.clone(),
        "node.dont-fallback" => "true",
        "node.dont-reconnect" => "true",
        "state.restore-props" => "false",
        "state.restore-target" => "false",
    };
    if let Some(latency) = &spec.latency {
        properties.insert("node.latency", latency.clone());
    }
    let stream = pw::stream::StreamRc::new(core.clone(), &spec.node_name, properties)?;
    let report = Arc::new(ToneReport::default());
    let state = ToneState {
        sine: Sine::new(spec.frequency, spec.amplitude),
        position: 0,
        lead_silence_frames: spec.lead_silence_frames,
        end_frame: spec.tone_frames.map(|frames| spec.lead_silence_frames + frames),
        onset_recorded: false,
        report: report.clone(),
    };

    let listener = stream
        .add_local_listener_with_user_data(state)
        .state_changed({
            let report = report.clone();
            move |_, _, _, new| {
                if let pw::stream::StreamState::Error(message) = new {
                    eprintln!("tone stream error: {message}");
                    report.failed.store(true, Ordering::SeqCst);
                    report.finished.store(true, Ordering::SeqCst);
                }
            }
        })
        .process(|stream, state| {
            let now = monotonic_ns();
            let Some(mut buffer) = stream.dequeue_buffer() else { return };
            let requested = buffer.requested() as usize;
            let datas = buffer.datas_mut();
            if datas.is_empty() {
                return;
            }
            let data = &mut datas[0];
            let mut frames = 0;
            if let Some(bytes) = data.data() {
                let capacity = bytes.len() / BYTES_PER_FRAME;
                frames = if requested > 0 { requested.min(capacity) } else { capacity };
                for frame in 0..frames {
                    let in_tone = state.position >= state.lead_silence_frames
                        && state.end_frame.map_or(true, |end| state.position < end);
                    let sample = if in_tone { state.sine.next_sample() } else { 0.0 };
                    if in_tone && !state.onset_recorded {
                        state.onset_recorded = true;
                        state.report.onset_ns.store(now, Ordering::SeqCst);
                        state.report.onset_frame_in_block.store(frame as u64, Ordering::SeqCst);
                        state.report.onset_block_frames.store(frames as u64, Ordering::SeqCst);
                        if let Ok(time) = stream.time() {
                            state.report.onset_delay_ticks.store(time.delay(), Ordering::SeqCst);
                        }
                    }
                    for channel in 0..CHANNELS {
                        let start = frame * BYTES_PER_FRAME + channel * 4;
                        bytes[start..start + 4].copy_from_slice(&sample.to_le_bytes());
                    }
                    state.position += 1;
                }
            }
            let chunk = data.chunk_mut();
            *chunk.offset_mut() = 0;
            *chunk.stride_mut() = BYTES_PER_FRAME as i32;
            *chunk.size_mut() = (frames * BYTES_PER_FRAME) as u32;
            state.report.callbacks.fetch_add(1, Ordering::Relaxed);
            if state.end_frame.is_some_and(|end| state.position >= end) {
                state.report.finished.store(true, Ordering::SeqCst);
            }
        })
        .register()?;

    let format = stereo_f32_format_pod();
    let mut params = [spa::pod::Pod::from_bytes(&format).ok_or("format pod")?];
    stream.connect(
        spa::utils::Direction::Output,
        None,
        pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS | pw::stream::StreamFlags::RT_PROCESS,
        &mut params,
    )?;
    Ok(ToneStream { stream, report, _listener: listener })
}
