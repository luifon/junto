//! Raw video on the restricted connection supplied by the ScreenCast portal.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use pipewire as pw;
use pw::spa;

use super::connection::Connection;
use super::pixels::{Crop, Format, Frame};
use super::portal::Video;
use super::{Life, error};
use crate::share::{CaptureError, ShareQuality};

type Ready = tokio::sync::oneshot::Sender<Result<(u32, u32), CaptureError>>;

fn pod(object: spa::pod::Object) -> Result<Vec<u8>, CaptureError> {
    spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(object),
    )
    .map(|result| result.0.into_inner())
    .map_err(error)
}

fn parameters() -> Result<Vec<Vec<u8>>, CaptureError> {
    use spa::pod::{ChoiceValue, Property, Value};
    use spa::utils::{Choice, ChoiceEnum, ChoiceFlags, Id, SpaTypes};
    let mask = (1 << spa::sys::SPA_DATA_MemPtr) | (1 << spa::sys::SPA_DATA_MemFd);
    let buffers = spa::pod::Object {
        type_: SpaTypes::ObjectParamBuffers.as_raw(),
        id: spa::param::ParamType::Buffers.as_raw(),
        properties: vec![Property::new(
            spa::sys::SPA_PARAM_BUFFERS_dataType,
            Value::Choice(ChoiceValue::Int(Choice(
                ChoiceFlags::empty(),
                ChoiceEnum::Flags {
                    default: mask,
                    flags: vec![mask],
                },
            ))),
        )],
    };
    let crop = spa::pod::Object {
        type_: SpaTypes::ObjectParamMeta.as_raw(),
        id: spa::param::ParamType::Meta.as_raw(),
        properties: vec![
            Property::new(
                spa::sys::SPA_PARAM_META_type,
                Value::Id(Id(spa::sys::SPA_META_VideoCrop)),
            ),
            Property::new(
                spa::sys::SPA_PARAM_META_size,
                Value::Int(std::mem::size_of::<spa::buffer::meta::MetaVideoCrop>() as i32),
            ),
        ],
    };
    Ok(vec![pod(buffers)?, pod(crop)?])
}

fn due(next: &mut Option<Instant>, now: Instant, frame_rate: u32) -> bool {
    let period = Duration::from_secs_f64(1.0 / f64::from(frame_rate));
    // Allow callback jitter around a deadline without halving a source at the requested rate.
    if next.is_some_and(|deadline| now + period / 20 < deadline) {
        return false;
    }
    let deadline = next.unwrap_or(now) + period;
    *next = Some(if deadline < now {
        now + period
    } else {
        deadline
    });
    true
}

struct Data {
    format: Option<((u32, u32), Format)>,
    ready: Option<Ready>,
    next: Option<Instant>,
}

pub(super) fn run(
    life: Arc<Life>,
    source: Video,
    quality: ShareQuality,
    frames: mpsc::SyncSender<Frame>,
    ready: Ready,
) {
    let result = capture(life.clone(), source, quality, frames, ready);
    if let Err(failure) = result {
        life.fail(failure);
    }
}

fn capture(
    life: Arc<Life>,
    source: Video,
    quality: ShareQuality,
    frames: mpsc::SyncSender<Frame>,
    ready: Ready,
) -> Result<(), CaptureError> {
    let connection = Connection::new(Some(source.fd), life.clone())?;
    let registry = connection.core.get_registry_rc().map_err(error)?;
    let target = Rc::new(Cell::new(source.serial.is_none().then_some(source.node)));
    let _registry_listener = registry
        .add_listener_local()
        .global({
            let target = target.clone();
            move |global| {
                if global.type_ == pw::types::ObjectType::Node
                    && let Some(serial) = source.serial
                    && global
                        .props
                        .and_then(|p| p.get("object.serial"))
                        .and_then(|v| v.parse::<u64>().ok())
                        == Some(serial)
                {
                    target.set(Some(global.id));
                }
            }
        })
        .global_remove({
            let life = life.clone();
            move |id| {
                if target.get() == Some(id) {
                    life.fail(CaptureError::SourceGone);
                }
            }
        })
        .register();
    let mut properties = pw::properties::properties! {
        "media.type" => "Video", "media.category" => "Capture", "media.role" => "Screen",
        "node.name" => "junto-shared-picture", "node.dont-reconnect" => "true", "node.dont-fallback" => "true",
    };
    if let Some(serial) = source.serial {
        properties.insert("target.object", serial.to_string());
    }
    let stream =
        pw::stream::StreamRc::new(connection.core.clone(), "Junto shared picture", properties)
            .map_err(error)?;
    let listener = stream
        .add_local_listener_with_user_data(Data {
            format: None,
            ready: Some(ready),
            next: None,
        })
        .state_changed({
            let life = life.clone();
            move |_, data, _, state| {
                if let pw::stream::StreamState::Error(message) = state {
                    let failure = error(message);
                    if let Some(ready) = data.ready.take() {
                        let _ = ready.send(Err(failure.clone()));
                    }
                    life.fail(failure);
                }
            }
        })
        .param_changed({
            let life = life.clone();
            move |stream, data, id, param| {
                if id != spa::param::ParamType::Format.as_raw() {
                    return;
                }
                let Some(param) = param else {
                    return;
                };
                let mut info = spa::param::video::VideoInfoRaw::default();
                let result = (|| {
                    info.parse(param).map_err(error)?;
                    let format = match info.format() {
                        spa::param::video::VideoFormat::BGRA => Format::Bgra,
                        spa::param::video::VideoFormat::BGRx => Format::Bgrx,
                        spa::param::video::VideoFormat::RGBA => Format::Rgba,
                        spa::param::video::VideoFormat::RGBx => Format::Rgbx,
                        _ => return Err(error("the portal selected an unsupported video format")),
                    };
                    data.format = Some(((info.size().width, info.size().height), format));
                    let values = parameters()?;
                    let mut params: Vec<_> = values
                        .iter()
                        .map(|bytes| {
                            spa::pod::Pod::from_bytes(bytes)
                                .ok_or_else(|| error("invalid video buffer parameters"))
                        })
                        .collect::<Result<_, _>>()?;
                    stream.update_params(&mut params).map_err(error)
                })();
                if let Err(failure) = result {
                    life.fail(failure);
                }
            }
        })
        .process({
            let life = life.clone();
            move |stream, data| {
                let Some(mut buffer) = stream.dequeue_buffer() else {
                    return;
                };
                if life.stopped() {
                    return;
                }
                if buffer
                    .find_meta::<spa::buffer::meta::MetaHeader>()
                    .is_some_and(|header| {
                        header.flags().intersects(
                            spa::buffer::meta::MetaHeaderFlags::CORRUPTED
                                | spa::buffer::meta::MetaHeaderFlags::GAP,
                        )
                    })
                {
                    return;
                }
                if buffer
                    .datas_mut()
                    .first()
                    .is_some_and(|plane| plane.chunk().size() == 0)
                {
                    return;
                }
                let Some((dimensions, format)) = data.format else {
                    return;
                };
                let now = Instant::now();
                if !due(&mut data.next, now, quality.frame_rate) {
                    return;
                }
                let crop = buffer
                    .find_meta::<spa::buffer::meta::MetaVideoCrop>()
                    .and_then(|crop| {
                        let crop = crop.meta_region();
                        crop.is_valid().then(|| Crop {
                            x: crop.position().x,
                            y: crop.position().y,
                            width: crop.size().width,
                            height: crop.size().height,
                        })
                    });
                let result = (|| {
                    let plane = buffer
                        .datas_mut()
                        .first_mut()
                        .ok_or_else(|| error("the portal delivered no pixel plane"))?;
                    if plane.type_() != spa::buffer::DataType::MemPtr
                        && plane.type_() != spa::buffer::DataType::MemFd
                    {
                        return Err(error("the portal did not supply shared-memory pixels"));
                    }
                    let offset = plane.chunk().offset() as usize;
                    let length = plane.chunk().size() as usize;
                    let stride = plane.chunk().stride();
                    let bytes = plane
                        .data()
                        .ok_or_else(|| error("the portal supplied an unmapped pixel buffer"))?;
                    let end = offset
                        .checked_add(length)
                        .filter(|end| *end <= bytes.len())
                        .ok_or_else(|| error("the portal supplied invalid pixel boundaries"))?;
                    Frame::copy(&bytes[offset..end], dimensions, stride, crop, format)
                })();
                match result {
                    Ok(frame) => {
                        let size = quality.resolution.fit(frame.width, frame.height);
                        if frames.try_send(frame).is_ok()
                            && let Some(ready) = data.ready.take()
                        {
                            let _ = ready.send(Ok(size));
                        }
                    }
                    Err(failure) => {
                        if let Some(ready) = data.ready.take() {
                            let _ = ready.send(Err(failure.clone()));
                        }
                        life.fail(failure);
                    }
                }
            }
        })
        .register()
        .map_err(error)?;
    let format = pod(spa::pod::object!(
        spa::utils::SpaTypes::ObjectParamFormat,
        spa::param::ParamType::EnumFormat,
        spa::pod::property!(
            spa::param::format::FormatProperties::MediaType,
            Id,
            spa::param::format::MediaType::Video
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::MediaSubtype,
            Id,
            spa::param::format::MediaSubtype::Raw
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            spa::param::video::VideoFormat::BGRx,
            spa::param::video::VideoFormat::BGRx,
            spa::param::video::VideoFormat::BGRA,
            spa::param::video::VideoFormat::RGBx,
            spa::param::video::VideoFormat::RGBA
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            spa::utils::Fraction {
                num: quality.frame_rate,
                denom: 1
            },
            spa::utils::Fraction { num: 0, denom: 1 },
            spa::utils::Fraction {
                num: quality.frame_rate,
                denom: 1
            }
        )
    ))?;
    let mut params = [spa::pod::Pod::from_bytes(&format)
        .ok_or_else(|| error("invalid requested video format"))?];
    stream
        .connect(
            spa::utils::Direction::Input,
            source.serial.is_none().then_some(source.node),
            pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .map_err(error)?;
    connection.drive(life, None, || false)?;
    stream.disconnect().map_err(error)?;
    drop(listener);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn callback_jitter_does_not_halve_the_selected_picture_rate() {
        let start = Instant::now();
        let mut next = None;
        let mut accepted = 0;
        for index in 0..300_u64 {
            let jitter = if index.is_multiple_of(2) { 0 } else { 200_000 };
            if due(
                &mut next,
                start + Duration::from_nanos(index * 1_000_000_000 / 30 + jitter),
                30,
            ) {
                accepted += 1;
            }
        }
        assert_eq!(accepted, 300);
        let mut next = None;
        let accepted = (0..600_u64)
            .filter(|index| {
                due(
                    &mut next,
                    start + Duration::from_nanos(index * 1_000_000_000 / 60),
                    15,
                )
            })
            .count();
        assert_eq!(accepted, 150);
    }
}
