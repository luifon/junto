//! Media Foundation sources and an asynchronous reader; all control stays on one thread.
use ::windows::Devices::Enumeration::{DeviceAccessInformation, DeviceAccessStatus, DeviceClass};
use ::windows::Win32::Foundation::E_ACCESSDENIED;
use ::windows::Win32::Media::MediaFoundation::*;
use ::windows::Win32::System::Com::{CoIncrementMTAUsage, CoTaskMemFree};
use ::windows::core::{GUID, HRESULT, Interface, Ref, implement};
use std::sync::{OnceLock, mpsc};
use std::time::{Duration, Instant};

use super::format::Mode;
use super::matrix::Tag;
use super::raw::{Layout, Pixel};
use super::{worker, *};

const VIDEO: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;

fn error(error: ::windows::core::Error) -> CameraError {
    match error.code() {
        E_ACCESSDENIED => CameraError::NoPermission(CameraAccess::Refused),
        MF_E_VIDEO_RECORDING_DEVICE_INVALIDATED => CameraError::DeviceGone,
        MF_E_VIDEO_RECORDING_DEVICE_PREEMPTED | MF_E_SHUTDOWN => CameraError::Interrupted,
        _ => CameraError::Failed(error.message()),
    }
}

fn initialize() -> Result<(), CameraError> {
    static READY: OnceLock<Result<(), CameraError>> = OnceLock::new();
    READY
        .get_or_init(|| {
            // SAFETY: COM and MF stay initialized until process exit, including late callbacks.
            unsafe {
                CoIncrementMTAUsage().map_err(error)?;
                MFStartup(MF_VERSION, MFSTARTUP_FULL).map_err(error)
            }
        })
        .clone()
}

fn access() -> CameraAccess {
    match DeviceAccessInformation::CreateFromDeviceClass(DeviceClass::VideoCapture)
        .and_then(|info| info.CurrentStatus())
    {
        Ok(DeviceAccessStatus::DeniedByUser) => CameraAccess::Refused,
        Ok(DeviceAccessStatus::DeniedBySystem) => CameraAccess::Restricted,
        _ => CameraAccess::Allowed,
    }
}

fn attributes() -> Result<IMFAttributes, CameraError> {
    let mut attributes = None;
    // SAFETY: MF initializes the out parameter with one owned interface.
    unsafe {
        MFCreateAttributes(&mut attributes, 4).map_err(error)?;
    }
    attributes.ok_or(CameraError::Unsupported)
}

fn string(attributes: &IMFAttributes, key: &GUID) -> Result<String, CameraError> {
    // SAFETY: the allocated slice holds the reported UTF-16 string plus its terminator.
    unsafe {
        let length = attributes.GetStringLength(key).map_err(error)? as usize;
        if length > 32768 {
            return Err(CameraError::Unsupported);
        }
        let mut text = vec![0; length + 1];
        attributes.GetString(key, &mut text, None).map_err(error)?;
        Ok(String::from_utf16_lossy(&text[..length]))
    }
}

fn enumerate() -> Result<Vec<(CameraDevice, IMFActivate)>, CameraError> {
    initialize()?;
    let attributes = attributes()?;
    // SAFETY: MF returns an array of owned COM references allocated with CoTaskMemAlloc.
    let activations = unsafe {
        attributes
            .SetGUID(
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
            )
            .map_err(error)?;
        let mut pointer = std::ptr::null_mut();
        let mut count = 0;
        MFEnumDeviceSources(&attributes, &mut pointer, &mut count).map_err(error)?;
        let mut owned = Vec::new();
        if count != 0 {
            if pointer.is_null() {
                return Err(CameraError::Unsupported);
            }
            for item in std::slice::from_raw_parts_mut(pointer, count as usize) {
                if let Some(item) = item.take() {
                    owned.push(item);
                }
            }
        }
        CoTaskMemFree(Some(pointer.cast()));
        owned
    };
    listed(activations)
}

/// The cameras of an enumeration, each with its activation object. An entry without a
/// symbolic link cannot be chosen again and is left out; one without a name is named
/// "Camera".
fn listed<T: Interface>(items: Vec<T>) -> Result<Vec<(CameraDevice, T)>, CameraError> {
    let mut devices = Vec::new();
    for item in items {
        let attributes: IMFAttributes = item.cast().map_err(error)?;
        let Ok(id) = string(
            &attributes,
            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK,
        ) else {
            continue;
        };
        let name = string(&attributes, &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME)
            .ok()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| "Camera".into());
        devices.push((CameraDevice { id, name }, item));
    }
    Ok(devices)
}

pub(super) struct SystemCamera;
impl CameraCapture for SystemCamera {
    fn access(&self) -> CameraAccess {
        access()
    }
    fn request_access(&self) -> Pending<CameraAccess> {
        Box::pin(async { access() })
    }
    fn devices(&self) -> Result<Vec<CameraDevice>, CameraError> {
        std::thread::Builder::new()
            .name("camera-devices".into())
            .spawn(|| Ok(enumerate()?.into_iter().map(|(device, _)| device).collect()))
            .map_err(|e| CameraError::Failed(e.to_string()))?
            .join()
            .map_err(|_| CameraError::Failed("camera enumeration stopped".into()))?
    }
    fn start(
        &self,
        chosen: Option<String>,
        sink: Arc<dyn CameraSink>,
    ) -> Pending<Result<CaptureStream, CameraError>> {
        Box::pin(worker::start(sink, move |delivery, ready| {
            let mut ready = Some(ready);
            if let Err(error) = capture(chosen, &delivery, &mut ready) {
                if let Some(ready) = ready.take() {
                    let _ = ready.send(Err(error));
                } else {
                    delivery.ended(error);
                }
            }
        }))
    }
}

struct Read {
    status: HRESULT,
    flags: u32,
    /// The time stamp of the sample in units of 100 nanoseconds.
    timestamp: i64,
    sample: Option<IMFSample>,
}

#[implement(IMFSourceReaderCallback)]
struct Callback(mpsc::Sender<Read>);
impl IMFSourceReaderCallback_Impl for Callback_Impl {
    fn OnReadSample(
        &self,
        status: HRESULT,
        _: u32,
        flags: u32,
        timestamp: i64,
        sample: Ref<IMFSample>,
    ) -> ::windows::core::Result<()> {
        let _ = self.0.send(Read {
            status,
            flags,
            timestamp,
            sample: sample.as_ref().cloned(),
        });
        Ok(())
    }
    fn OnFlush(&self, _: u32) -> ::windows::core::Result<()> {
        Ok(())
    }
    fn OnEvent(&self, _: u32, event: Ref<IMFMediaEvent>) -> ::windows::core::Result<()> {
        if let Some(event) = event.as_ref() {
            // SAFETY: the callback owns a valid event for this invocation.
            if let Ok(status) = unsafe { event.GetStatus() }
                && status.is_err()
            {
                let _ = self.0.send(Read {
                    status,
                    flags: 0,
                    timestamp: 0,
                    sample: None,
                });
            }
        }
        Ok(())
    }
}

struct Source {
    activation: IMFActivate,
    source: IMFMediaSource,
}
impl Drop for Source {
    fn drop(&mut self) {
        // SAFETY: the control thread ends the source; callbacks own only their sender.
        unsafe {
            let _ = self.source.Shutdown();
            let _ = self.activation.ShutdownObject();
        }
    }
}

fn pair(value: u64) -> (u32, u32) {
    ((value >> 32) as u32, value as u32)
}

fn layout(media: &IMFMediaType) -> Result<Layout, CameraError> {
    // SAFETY: attributes are read from the reader's current media type.
    unsafe {
        let (width, height) = pair(media.GetUINT64(&MF_MT_FRAME_SIZE).map_err(error)?);
        let subtype = media.GetGUID(&MF_MT_SUBTYPE).map_err(error)?;
        let pixel = if subtype == MFVideoFormat_NV12 {
            Pixel::Nv12
        } else if subtype == MFVideoFormat_I420 || subtype == MFVideoFormat_IYUV {
            Pixel::I420
        } else if subtype == MFVideoFormat_YUY2 {
            Pixel::Yuy2
        } else {
            return Err(CameraError::Unsupported);
        };
        let interlace = media.GetUINT32(&MF_MT_INTERLACE_MODE).unwrap_or(0);
        if interlace != 0 && interlace != MFVideoInterlace_Progressive.0 as u32 {
            return Err(CameraError::Unsupported);
        }
        let range = media.GetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE).unwrap_or(0);
        if range != 0 && range != MFNominalRange_16_235.0 as u32 {
            return Err(CameraError::Unsupported);
        }
        let matrix = match media.GetUINT32(&MF_MT_YUV_MATRIX).unwrap_or(0) {
            0 => Tag::Missing,
            1 => Tag::Bt709,
            2 => Tag::Bt601,
            3 => Tag::Smpte240m,
            4 | 5 => Tag::Bt2020,
            _ => Tag::Other,
        };
        let tight = if pixel == Pixel::Yuy2 {
            width.checked_mul(2).ok_or(CameraError::Unsupported)?
        } else {
            width
        };
        let stride = media.GetUINT32(&MF_MT_DEFAULT_STRIDE).unwrap_or(tight) as usize;
        let layout = Layout {
            width,
            height,
            stride,
            pixel,
            matrix,
        };
        layout.length().ok_or(CameraError::Unsupported)?;
        Ok(layout)
    }
}

/// The capture mode of a native type that capture can read: a progressive uncompressed type
/// that `layout` accepts, or a progressive MJPEG type, which the reader decodes.
fn mode(media: &IMFMediaType) -> Option<Mode> {
    // SAFETY: reading attributes does not mutate the type.
    unsafe {
        let (n, d) = pair(media.GetUINT64(&MF_MT_FRAME_RATE).ok()?);
        let rate = (n > 0 && d > 0).then_some((n, d));
        if media.GetGUID(&MF_MT_SUBTYPE).ok()? == MFVideoFormat_MJPG {
            let interlace = media.GetUINT32(&MF_MT_INTERLACE_MODE).unwrap_or(0);
            if interlace != 0 && interlace != MFVideoInterlace_Progressive.0 as u32 {
                return None;
            }
            let (width, height) = pair(media.GetUINT64(&MF_MT_FRAME_SIZE).ok()?);
            return Some(Mode {
                width,
                height,
                rate,
                compressed: true,
            });
        }
        let layout = layout(media).ok()?;
        Some(Mode {
            width: layout.width,
            height: layout.height,
            rate,
            compressed: false,
        })
    }
}

/// The index of the native type to capture with (`format::select`).
fn choose_type(types: &[IMFMediaType]) -> Option<usize> {
    let modes: Vec<(usize, Mode)> = types
        .iter()
        .enumerate()
        .filter_map(|(index, media)| Some((index, mode(media)?)))
        .collect();
    let listed: Vec<Mode> = modes.iter().map(|(_, mode)| *mode).collect();
    super::format::select(&listed).map(|chosen| modes[chosen].0)
}

/// Sets the chosen native type. An MJPEG type is decoded by the reader's video processing to
/// limited-range 601 NV12 of the same size and rate.
fn configure(reader: &IMFSourceReader) -> Result<(Layout, Option<(u32, u32)>), CameraError> {
    let mut types = Vec::new();
    for index in 0..1024 {
        // SAFETY: the reader owns the source and returns a retained type.
        match unsafe { reader.GetNativeMediaType(VIDEO, index) } {
            Ok(media) => types.push(media),
            Err(e) if e.code() == MF_E_NO_MORE_TYPES => break,
            Err(e) => return Err(error(e)),
        }
    }
    let media = &types[choose_type(&types).ok_or(CameraError::Unsupported)?];
    let rate = mode(media).and_then(|mode| mode.rate);
    // SAFETY: the source has advertised the type; there is no pending read yet. The output
    // type is owned here and copied by the reader.
    unsafe {
        if media.GetGUID(&MF_MT_SUBTYPE).map_err(error)? == MFVideoFormat_MJPG {
            let extended: IMFSourceReaderEx = reader.cast().map_err(error)?;
            extended.SetNativeMediaType(VIDEO, media).map_err(error)?;
            let output = MFCreateMediaType().map_err(error)?;
            output
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(error)?;
            output
                .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)
                .map_err(error)?;
            for key in [&MF_MT_FRAME_SIZE, &MF_MT_FRAME_RATE] {
                output
                    .SetUINT64(key, media.GetUINT64(key).map_err(error)?)
                    .map_err(error)?;
            }
            output
                .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
                .map_err(error)?;
            output
                .SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32)
                .map_err(error)?;
            output
                .SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT601.0 as u32)
                .map_err(error)?;
            reader
                .SetCurrentMediaType(VIDEO, None, &output)
                .map_err(error)?;
        } else {
            reader
                .SetCurrentMediaType(VIDEO, None, media)
                .map_err(error)?;
        }
        Ok((
            layout(&reader.GetCurrentMediaType(VIDEO).map_err(error)?)?,
            rate,
        ))
    }
}

fn frame(
    sample: &IMFSample,
    mut layout: Layout,
) -> Result<livekit::webrtc::video_frame::I420Buffer, CameraError> {
    // SAFETY: the sample and its buffers remain retained for the copy.
    unsafe {
        let buffer = if sample.GetBufferCount().map_err(error)? == 1 {
            sample.GetBufferByIndex(0).map_err(error)?
        } else {
            sample.ConvertToContiguousBuffer().map_err(error)?
        };
        if let Ok(two_d) = buffer.cast::<IMF2DBuffer>() {
            layout.stride = layout.width as usize * if layout.pixel == Pixel::Yuy2 { 2 } else { 1 };
            let mut bytes = vec![0; layout.length().ok_or(CameraError::Unsupported)?];
            if two_d.GetContiguousLength().map_err(error)? as usize != bytes.len() {
                return Err(CameraError::Unsupported);
            }
            two_d.ContiguousCopyTo(&mut bytes).map_err(error)?;
            return layout.convert(&bytes);
        }
        let mut pointer = std::ptr::null_mut();
        let mut length = 0;
        buffer
            .Lock(&mut pointer, None, Some(&mut length))
            .map_err(error)?;
        let result = if pointer.is_null() || length as usize > 32 * 1024 * 1024 {
            Err(CameraError::Unsupported)
        } else {
            layout.convert(std::slice::from_raw_parts(pointer, length as usize))
        };
        let unlock = buffer.Unlock().map_err(error);
        unlock?;
        result
    }
}

fn capture(
    chosen: Option<String>,
    delivery: &worker::Delivery,
    ready: &mut Option<worker::Ready>,
) -> Result<(), CameraError> {
    initialize()?;
    let permission = access();
    if permission != CameraAccess::Allowed {
        return Err(CameraError::NoPermission(permission));
    }
    let devices = enumerate()?;
    let listed: Vec<_> = devices.iter().map(|(device, _)| device.clone()).collect();
    let chosen = choose(&listed, chosen.as_deref())?;
    let (_, activation) = devices
        .into_iter()
        .find(|(device, _)| device.id == chosen.id)
        .ok_or(CameraError::DeviceGone)?;
    // The claim is declared before the source, so that it is released after the shutdown.
    let Some(_claim) = delivery.claim(&chosen.id) else {
        return Ok(());
    };
    // SAFETY: activation is used only on this capture thread and shut down by Source.
    let source = Source {
        source: unsafe { activation.ActivateObject() }.map_err(error)?,
        activation,
    };
    let (send, receive) = mpsc::channel();
    let callback: IMFSourceReaderCallback = Callback(send).into();
    let attributes = attributes()?;
    // SAFETY: the reader retains the callback and its own reference to the source.
    let reader = unsafe {
        attributes
            .SetUnknown(&MF_SOURCE_READER_ASYNC_CALLBACK, &callback)
            .map_err(error)?;
        // The decoder of an MJPEG type and the conversion of its output (`configure`).
        attributes
            .SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1)
            .map_err(error)?;
        let reader =
            MFCreateSourceReaderFromMediaSource(&source.source, &attributes).map_err(error)?;
        reader
            .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
            .map_err(error)?;
        reader.SetStreamSelection(VIDEO, true).map_err(error)?;
        reader
    };
    let (layout, rate) = configure(&reader)?;
    // SAFETY: asynchronous mode requires null out parameters; only one read is outstanding.
    let read = || unsafe {
        reader
            .ReadSample(VIDEO, 0, None, None, None, None)
            .map_err(error)
    };
    read()?;
    let start = Instant::now();
    let mut last_picture = start;
    let mut pacer = worker::Pacer::default();
    while !delivery.stopped() {
        let event = match receive.recv_timeout(Duration::from_millis(100)) {
            Ok(event) => event,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if last_picture.elapsed() >= Duration::from_secs(5) {
                    return Err(CameraError::Interrupted);
                }
                continue;
            }
            Err(_) => return Err(CameraError::Interrupted),
        };
        event.status.ok().map_err(error)?;
        if event.flags & (MF_SOURCE_READERF_ERROR.0 | MF_SOURCE_READERF_ENDOFSTREAM.0) as u32 != 0 {
            return Err(CameraError::DeviceGone);
        }
        if event.flags
            & (MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.0
                | MF_SOURCE_READERF_NATIVEMEDIATYPECHANGED.0) as u32
            != 0
        {
            return Err(CameraError::Interrupted);
        }
        if let Some(sample) = event.sample {
            last_picture = Instant::now();
            let at = u64::try_from(event.timestamp)
                .map(|units| Duration::from_nanos(units.saturating_mul(100)))
                .unwrap_or(last_picture - start);
            if pacer.due(at) {
                let picture = frame(&sample, layout)?;
                if let Some(ready) = ready.take() {
                    let _ = ready.send(Ok(worker::Started {
                        width: layout.width,
                        height: layout.height,
                        rate,
                    }));
                }
                delivery.picture(Arc::new(picture));
            }
        }
        read()?;
    }
    // SAFETY: shutdown happens on this thread and the callback retains no source or reader.
    unsafe {
        let _ = reader.Flush(VIDEO);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_foundation_two_dimensional_pitch_is_not_transmitted() {
        initialize().unwrap();
        // SAFETY: the test owns the 2D buffer, keeps it locked for each write, and opens no device.
        unsafe {
            let buffer = MFCreate2DMediaBuffer(4, 2, u32::from_le_bytes(*b"NV12"), false).unwrap();
            let two_d: IMF2DBuffer = buffer.cast().unwrap();
            let mut pointer = std::ptr::null_mut();
            let mut pitch = 0;
            two_d.Lock2D(&mut pointer, &mut pitch).unwrap();
            assert!(pitch >= 4);
            for row in 0..2 {
                std::ptr::write_bytes(pointer.add(row * pitch as usize), 135, 4);
            }
            std::ptr::copy_nonoverlapping(
                [67, 57, 67, 57].as_ptr(),
                pointer.add(2 * pitch as usize),
                4,
            );
            two_d.Unlock2D().unwrap();
            let sample = MFCreateSample().unwrap();
            sample.AddBuffer(&buffer).unwrap();
            let picture = frame(
                &sample,
                Layout {
                    width: 4,
                    height: 2,
                    stride: 4,
                    pixel: Pixel::Nv12,
                    matrix: Tag::Bt601,
                },
            )
            .unwrap();
            assert_eq!(
                picture.data(),
                (&[135u8; 8][..], &[67u8; 2][..], &[57u8; 2][..])
            );
        }
    }

    #[test]
    fn media_foundation_padded_709_sample_reaches_the_shared_converter() {
        initialize().unwrap();
        // SAFETY: the test owns the media type, sample and writable buffer; no device opens.
        unsafe {
            let media = MFCreateMediaType().unwrap();
            media
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .unwrap();
            media.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12).unwrap();
            media
                .SetUINT64(&MF_MT_FRAME_SIZE, (4u64 << 32) | 2)
                .unwrap();
            media.SetUINT32(&MF_MT_DEFAULT_STRIDE, 8).unwrap();
            media
                .SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT709.0 as u32)
                .unwrap();
            let bytes = [
                157, 157, 157, 157, 0, 0, 0, 0, 157, 157, 157, 157, 0, 0, 0, 0, 58, 51, 58, 51, 0,
                0, 0, 0,
            ];
            let buffer = MFCreateMemoryBuffer(bytes.len() as u32).unwrap();
            let mut pointer = std::ptr::null_mut();
            buffer.Lock(&mut pointer, None, None).unwrap();
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer, bytes.len());
            buffer.Unlock().unwrap();
            buffer.SetCurrentLength(bytes.len() as u32).unwrap();
            let sample = MFCreateSample().unwrap();
            sample.AddBuffer(&buffer).unwrap();
            let picture = frame(&sample, layout(&media).unwrap()).unwrap();
            assert_eq!(
                picture.data(),
                (&[135u8; 8][..], &[67u8; 2][..], &[57u8; 2][..])
            );
            buffer.SetCurrentLength(15).unwrap();
            assert!(frame(&sample, layout(&media).unwrap()).is_err());
            media
                .SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_0_255.0 as u32)
                .unwrap();
            assert!(layout(&media).is_err());
        }
    }
    #[test]
    fn permission_refusal_and_unplug_are_preserved() {
        assert_eq!(
            error(E_ACCESSDENIED.into()),
            CameraError::NoPermission(CameraAccess::Refused)
        );
        assert_eq!(
            error(MF_E_VIDEO_RECORDING_DEVICE_INVALIDATED.into()),
            CameraError::DeviceGone
        );
        assert_eq!(
            error(MF_E_VIDEO_RECORDING_DEVICE_PREEMPTED.into()),
            CameraError::Interrupted
        );
    }

    fn video(subtype: &GUID, size: (u32, u32), rate: u32) -> IMFMediaType {
        // SAFETY: the test owns the new media type.
        unsafe {
            let media = MFCreateMediaType().unwrap();
            media
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .unwrap();
            media.SetGUID(&MF_MT_SUBTYPE, subtype).unwrap();
            media
                .SetUINT64(
                    &MF_MT_FRAME_SIZE,
                    (u64::from(size.0) << 32) | u64::from(size.1),
                )
                .unwrap();
            media
                .SetUINT64(&MF_MT_FRAME_RATE, (u64::from(rate) << 32) | 1)
                .unwrap();
            media
        }
    }

    #[test]
    fn a_compressed_type_is_chosen_when_only_it_reaches_24_fps_at_the_largest_size() {
        initialize().unwrap();
        let types = [
            video(&MFVideoFormat_YUY2, (1280, 720), 10),
            video(&MFVideoFormat_MJPG, (1280, 720), 30),
            video(&MFVideoFormat_YUY2, (640, 480), 30),
        ];
        assert_eq!(choose_type(&types), Some(1));
        let types = [
            video(&MFVideoFormat_MJPG, (1280, 720), 30),
            video(&MFVideoFormat_NV12, (1280, 720), 30),
        ];
        assert_eq!(choose_type(&types), Some(1));
        let types = [
            video(&MFVideoFormat_YUY2, (1280, 720), 10),
            video(&MFVideoFormat_YUY2, (640, 480), 30),
        ];
        assert_eq!(choose_type(&types), Some(1));
    }

    #[test]
    fn a_camera_without_a_name_is_listed_and_one_without_a_link_is_left_out() {
        initialize().unwrap();
        let entry = |link: Option<&str>, name: Option<&str>| {
            let attributes = attributes().unwrap();
            // SAFETY: the test owns the attributes and the strings outlive each call.
            unsafe {
                if let Some(link) = link {
                    attributes
                        .SetString(
                            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK,
                            &::windows::core::HSTRING::from(link),
                        )
                        .unwrap();
                }
                if let Some(name) = name {
                    attributes
                        .SetString(
                            &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME,
                            &::windows::core::HSTRING::from(name),
                        )
                        .unwrap();
                }
            }
            attributes
        };
        let devices: Vec<CameraDevice> = listed(vec![
            entry(Some("first-link"), Some("First camera")),
            entry(Some("second-link"), None),
            entry(None, Some("Camera without a link")),
        ])
        .unwrap()
        .into_iter()
        .map(|(device, _)| device)
        .collect();
        assert_eq!(
            devices,
            [
                CameraDevice {
                    id: "first-link".into(),
                    name: "First camera".into(),
                },
                CameraDevice {
                    id: "second-link".into(),
                    name: "Camera".into(),
                },
            ]
        );
    }
}
