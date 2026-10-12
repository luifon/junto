//! A native capture with explicitly selected playback links and bounded sample storage.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use pipewire as pw;
use pw::spa;

use super::connection::Connection;
use super::graph::{CHANNELS, stereo};
use super::registry::Registry;
use super::{Life, error};
use crate::share::{ApplicationSource, CaptureError, SoundChoice};

pub(super) const CAPACITY: usize = 48_000 / 5;
const POSITIONS: [u32; 9] = [
    spa::sys::SPA_AUDIO_CHANNEL_FL,
    spa::sys::SPA_AUDIO_CHANNEL_FR,
    spa::sys::SPA_AUDIO_CHANNEL_FC,
    spa::sys::SPA_AUDIO_CHANNEL_LFE,
    spa::sys::SPA_AUDIO_CHANNEL_RL,
    spa::sys::SPA_AUDIO_CHANNEL_RR,
    spa::sys::SPA_AUDIO_CHANNEL_SL,
    spa::sys::SPA_AUDIO_CHANNEL_SR,
    spa::sys::SPA_AUDIO_CHANNEL_MONO,
];

pub(super) fn sources() -> Result<(bool, Vec<ApplicationSource>), CaptureError> {
    let life = Arc::new(Life::default());
    let connection = Connection::new(None, life.clone())?;
    let registry = Registry::new(
        connection.core.clone(),
        connection.nonce.clone(),
        SoundChoice::None,
        life.clone(),
    )?;
    connection.roundtrip(life.clone())?;
    connection.roundtrip(life)?;
    let graph = registry.graph.borrow();
    if !graph.ready() {
        return Err(error("PipeWire did not identify the capture connection"));
    }
    Ok((graph.system_available(), graph.applications()))
}

fn format() -> Result<Vec<u8>, CaptureError> {
    let mut info = spa::param::audio::AudioInfoRaw::new();
    info.set_format(spa::param::audio::AudioFormat::F32LE);
    info.set_rate(48_000);
    info.set_channels(CHANNELS.len() as u32);
    let mut positions = [0; spa::param::audio::MAX_CHANNELS];
    positions[..9].copy_from_slice(&POSITIONS);
    info.set_position(positions);
    let object = spa::pod::Object {
        type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: spa::param::ParamType::EnumFormat.as_raw(),
        properties: info.into(),
    };
    spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(object),
    )
    .map(|result| result.0.into_inner())
    .map_err(error)
}

pub(super) fn run(
    life: Arc<Life>,
    choice: SoundChoice,
    samples: rtrb::Producer<[f32; 2]>,
    ready: tokio::sync::oneshot::Sender<Result<(), CaptureError>>,
) {
    let mut ready = Some(ready);
    let result = capture(life.clone(), choice, samples, &mut ready);
    if let Err(failure) = result {
        if let Some(ready) = ready.take() {
            let _ = ready.send(Err(failure.clone()));
        }
        life.fail(failure);
    }
}

fn capture(
    life: Arc<Life>,
    choice: SoundChoice,
    mut samples: rtrb::Producer<[f32; 2]>,
    ready: &mut Option<tokio::sync::oneshot::Sender<Result<(), CaptureError>>>,
) -> Result<(), CaptureError> {
    let connection = Connection::new(None, life.clone())?;
    let registry = Rc::new(Registry::new(
        connection.core.clone(),
        connection.nonce.clone(),
        choice.clone(),
        life.clone(),
    )?);
    connection.roundtrip(life.clone())?;
    connection.roundtrip(life.clone())?;
    if !registry.graph.borrow().ready() {
        return Err(error("PipeWire did not identify the capture connection"));
    }
    if let SoundChoice::Application(chosen) = &choice
        && !registry
            .graph
            .borrow()
            .applications()
            .iter()
            .any(|app| &app.id == chosen)
    {
        return Err(CaptureError::SourceGone);
    }
    let stream = pw::stream::StreamRc::new(
        connection.core.clone(),
        "Junto shared sound",
        pw::properties::properties! {
            "media.type" => "Audio", "media.category" => "Capture", "media.role" => "Production",
            "node.name" => "junto-shared-sound", "node.autoconnect" => "false",
            "state.restore-props" => "false", "state.restore-target" => "false",
            "node.dont-fallback" => "true", "node.dont-reconnect" => "true",
        },
    )
    .map_err(error)?;
    let malformed = Arc::new(AtomicBool::new(false));
    let negotiated = Rc::new(Cell::new(false));
    let listener = stream
        .add_local_listener::<()>()
        .state_changed({
            let registry = Rc::downgrade(&registry);
            let life = life.clone();
            move |stream, _, _, state| {
                if let pw::stream::StreamState::Error(message) = state {
                    life.fail(error(message));
                }
                let id = stream.node_id();
                if id != pw::constants::ID_ANY
                    && let Some(registry) = registry.upgrade()
                {
                    registry.set_capture(id);
                }
            }
        })
        .param_changed({
            let negotiated = negotiated.clone();
            let life = life.clone();
            move |_, _, id, param| {
                if id != spa::param::ParamType::Format.as_raw() {
                    return;
                }
                let Some(param) = param else {
                    return;
                };
                let mut info = spa::param::audio::AudioInfoRaw::new();
                if info.parse(param).is_err()
                    || info.rate() != 48_000
                    || info.channels() != 9
                    || info.format() != spa::param::audio::AudioFormat::F32LE
                    || info.position()[..9] != POSITIONS
                {
                    life.fail(error(
                        "PipeWire selected an unsupported shared-sound format",
                    ));
                    return;
                }
                negotiated.set(true);
            }
        })
        .process({
            let malformed = malformed.clone();
            move |stream, _| {
                let Some(mut buffer) = stream.dequeue_buffer() else {
                    return;
                };
                let Some(data) = buffer.datas_mut().first_mut() else {
                    return;
                };
                let offset = data.chunk().offset() as usize;
                let size = data.chunk().size() as usize;
                let stride = data.chunk().stride();
                let Some(bytes) = data.data() else {
                    malformed.store(true, Ordering::Relaxed);
                    return;
                };
                let Some(end) = offset.checked_add(size).filter(|end| *end <= bytes.len()) else {
                    malformed.store(true, Ordering::Relaxed);
                    return;
                };
                if !size.is_multiple_of(36) || (stride != 0 && stride != 36) {
                    malformed.store(true, Ordering::Relaxed);
                    return;
                }
                for frame in bytes[offset..end].as_chunks::<36>().0 {
                    let mut channels = [0.0; 9];
                    for (sample, bytes) in channels.iter_mut().zip(frame.as_chunks::<4>().0) {
                        *sample = f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                    }
                    // The producer never waits for the sender or allocates in the audio callback.
                    let _ = samples.push(stereo(channels));
                }
            }
        })
        .register()
        .map_err(error)?;
    let format = format()?;
    let mut params =
        [spa::pod::Pod::from_bytes(&format)
            .ok_or_else(|| error("invalid sound format request"))?];
    stream
        .connect(
            spa::utils::Direction::Input,
            None,
            pw::stream::StreamFlags::MAP_BUFFERS | pw::stream::StreamFlags::RT_PROCESS,
            &mut params,
        )
        .map_err(error)?;
    connection.drive(life.clone(), Some(Duration::from_secs(15)), {
        let registry = registry.clone();
        move || {
            let graph = registry.graph.borrow();
            negotiated.get()
                && graph.capture.is_some_and(|node| {
                    CHANNELS.iter().all(|channel| {
                        graph.ports.values().any(|port| {
                            port.node == node && !port.output && port.channel == *channel
                        })
                    })
                })
        }
    })?;
    if let Some(ready) = ready.take() {
        let _ = ready.send(Ok(()));
    }
    connection.drive(life.clone(), None, {
        let life = life.clone();
        move || {
            if malformed.load(Ordering::Relaxed) {
                life.fail(error("PipeWire delivered an invalid sound buffer"));
            }
            false
        }
    })?;
    stream.disconnect().map_err(error)?;
    drop(listener);
    Ok(())
}

#[cfg(test)]
mod tests;
