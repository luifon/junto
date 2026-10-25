//! Raw video on the restricted connection supplied by the ScreenCast portal.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use pipewire as pw;
use pw::spa;

use super::connection::Connection;
use super::latest::Latest;
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
    let meta = |kind: u32, size: usize| spa::pod::Object {
        type_: SpaTypes::ObjectParamMeta.as_raw(),
        id: spa::param::ParamType::Meta.as_raw(),
        properties: vec![
            Property::new(spa::sys::SPA_PARAM_META_type, Value::Id(Id(kind))),
            Property::new(spa::sys::SPA_PARAM_META_size, Value::Int(size as i32)),
        ],
    };
    // A producer attaches only the metadata that the consumer asks for. The header states
    // that a buffer is corrupted.
    Ok(vec![
        pod(buffers)?,
        pod(meta(
            spa::sys::SPA_META_VideoCrop,
            std::mem::size_of::<spa::buffer::meta::MetaVideoCrop>(),
        ))?,
        pod(meta(
            spa::sys::SPA_META_Header,
            std::mem::size_of::<spa::buffer::meta::MetaHeader>(),
        ))?,
    ])
}

/// Whether a buffer holds no picture to send: its producer marked it, or it is empty.
fn unusable(
    header: Option<spa::buffer::meta::MetaHeaderFlags>,
    chunk: spa::buffer::ChunkFlags,
    size: u32,
) -> bool {
    use spa::buffer::meta::MetaHeaderFlags;
    header.is_some_and(|flags| flags.intersects(MetaHeaderFlags::CORRUPTED | MetaHeaderFlags::GAP))
        || chunk.contains(spa::buffer::ChunkFlags::CORRUPTED)
        || size == 0
}

/// The answer to the start of the share. The stream listener and the end of the thread
/// both hold it: whichever has the first picture or the first failure sends it.
type Answer = Rc<Cell<Option<Ready>>>;

struct Data {
    format: Option<((u32, u32), Format)>,
    ready: Answer,
}

pub(super) fn run(
    life: Arc<Life>,
    source: Video,
    quality: ShareQuality,
    pictures: Arc<Latest<Frame>>,
    ready: Ready,
) {
    let ready: Answer = Rc::new(Cell::new(Some(ready)));
    let result = capture(life.clone(), source, quality, pictures, ready.clone());
    finish(&life, &ready, result);
}

/// A failure is sent as the answer before the answer can be dropped without one, so the
/// start never reads a closed channel in place of the reason.
fn finish(life: &Life, ready: &Cell<Option<Ready>>, result: Result<(), CaptureError>) {
    if let Err(failure) = result {
        if let Some(ready) = ready.take() {
            let _ = ready.send(Err(failure.clone()));
        }
        life.fail(failure);
    }
}

fn capture(
    life: Arc<Life>,
    source: Video,
    quality: ShareQuality,
    pictures: Arc<Latest<Frame>>,
    ready: Answer,
) -> Result<(), CaptureError> {
    let connection = Connection::new(Some(source.fd), life.clone())?;
    let registry = connection.core.get_registry_rc().map_err(error)?;
    let target = Rc::new(Cell::new(source.serial.is_none().then_some(source.node)));
    let _registry_listener = registry
        .add_listener_local()
        .global({
            let target = target.clone();
            move |global| {
                super::connection::in_callback("PipeWire video global", || {
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
                })
            }
        })
        .global_remove({
            let life = life.clone();
            move |id| {
                super::connection::in_callback("PipeWire video global_remove", || {
                    if target.get() == Some(id) {
                        life.fail(CaptureError::SourceGone);
                    }
                })
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
            ready,
        })
        .state_changed({
            let life = life.clone();
            move |_, data, _, state| {
                super::connection::in_callback("PipeWire video state_changed", || {
                    if let pw::stream::StreamState::Error(message) = state {
                        let failure = error(message);
                        if let Some(ready) = data.ready.take() {
                            let _ = ready.send(Err(failure.clone()));
                        }
                        life.fail(failure);
                    }
                })
            }
        })
        .param_changed({
            let life = life.clone();
            move |stream, data, id, param| {
                super::connection::in_callback("PipeWire video param_changed", || {
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
                            _ => {
                                return Err(error(
                                    "the portal selected an unsupported video format",
                                ));
                            }
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
                })
            }
        })
        .process({
            let life = life.clone();
            move |stream, data| {
                super::connection::in_callback("PipeWire video process", || {
                    let Some(mut buffer) = stream.dequeue_buffer() else {
                        return;
                    };
                    if life.stopped() {
                        return;
                    }
                    let header = buffer
                        .find_meta::<spa::buffer::meta::MetaHeader>()
                        .map(|header| header.flags());
                    if buffer.datas_mut().first().is_some_and(|plane| {
                        unusable(header, plane.chunk().flags(), plane.chunk().size())
                    }) {
                        return;
                    }
                    let Some((dimensions, format)) = data.format else {
                        return;
                    };
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
                            pictures.put(frame);
                            if let Some(ready) = data.ready.take() {
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
                })
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
        ),
        // A source with a variable frame rate sends no more pictures than this.
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoMaxFramerate,
            Choice,
            Range,
            Fraction,
            spa::utils::Fraction {
                num: quality.frame_rate,
                denom: 1
            },
            spa::utils::Fraction { num: 1, denom: 1 },
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
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn a_failure_before_the_first_picture_is_the_answer_to_the_start() {
        let life = Life::default();
        let (ready, mut first_picture) = tokio::sync::oneshot::channel();
        let ready = Cell::new(Some(ready));
        finish(&life, &ready, Err(CaptureError::SourceGone));
        assert!(matches!(
            first_picture.try_recv(),
            Ok(Err(CaptureError::SourceGone))
        ));
        assert!(matches!(life.result(), Err(CaptureError::SourceGone)));
    }

    #[test]
    fn the_capture_asks_for_the_header_and_skips_a_buffer_that_its_producer_marked() {
        use spa::buffer::ChunkFlags;
        use spa::buffer::meta::MetaHeaderFlags;
        // The kinds of metadata that the parameters ask a producer for.
        let asked: Vec<u32> = parameters()
            .unwrap()
            .iter()
            .filter_map(|bytes| {
                let (_, value) =
                    spa::pod::deserialize::PodDeserializer::deserialize_any_from(bytes).ok()?;
                let spa::pod::Value::Object(object) = value else {
                    return None;
                };
                object.properties.iter().find_map(|property| {
                    match (property.key, &property.value) {
                        (spa::sys::SPA_PARAM_META_type, spa::pod::Value::Id(kind)) => Some(kind.0),
                        _ => None,
                    }
                })
            })
            .collect();
        assert_eq!(
            asked,
            [spa::sys::SPA_META_VideoCrop, spa::sys::SPA_META_Header]
        );
        assert!(!unusable(None, ChunkFlags::empty(), 64));
        assert!(!unusable(
            Some(MetaHeaderFlags::empty()),
            ChunkFlags::empty(),
            64
        ));
        assert!(unusable(
            Some(MetaHeaderFlags::CORRUPTED),
            ChunkFlags::empty(),
            64
        ));
        assert!(unusable(
            Some(MetaHeaderFlags::GAP),
            ChunkFlags::empty(),
            64
        ));
        assert!(unusable(None, ChunkFlags::CORRUPTED, 64));
        assert!(unusable(None, ChunkFlags::empty(), 0));
    }
}
