//! DirectShow video input devices, for a source that only DirectShow lists, such as a virtual
//! camera that registers a DirectShow source filter (ADR 0770). The pictures reach a sink
//! filter of this module with one input pin, in the same layouts as the Media Foundation path.
use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant};

use ::windows::Win32::Foundation::{E_NOTIMPL, E_POINTER, E_UNEXPECTED, S_FALSE, S_OK};
use ::windows::Win32::Media::DirectShow::*;
use ::windows::Win32::Media::IReferenceClock;
use ::windows::Win32::Media::MediaFoundation::{
    AM_MEDIA_TYPE, CLSID_CaptureGraphBuilder2, CLSID_FilterGraph, CLSID_SystemDeviceEnum,
    CLSID_VideoInputDeviceCategory, FORMAT_VideoInfo, FORMAT_VideoInfo2, MEDIASUBTYPE_ARGB32,
    MEDIASUBTYPE_I420, MEDIASUBTYPE_IYUV, MEDIASUBTYPE_NV12, MEDIASUBTYPE_RGB24,
    MEDIASUBTYPE_RGB32, MEDIASUBTYPE_UYVY, MEDIASUBTYPE_YUY2, MEDIATYPE_Video,
    PIN_CATEGORY_CAPTURE, VIDEOINFOHEADER, VIDEOINFOHEADER2,
};
use ::windows::Win32::System::Com::StructuredStorage::IPropertyBag;
use ::windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemAlloc, CoTaskMemFree, IMoniker, IPersist_Impl,
};
use ::windows::Win32::System::Variant::{VARIANT, VT_BSTR, VariantClear};
use ::windows::core::{
    BOOL, ComObject, GUID, HRESULT, Interface, PCWSTR, PWSTR, Ref, implement, w,
};

use super::super::bitmap::{self, Subtype, Video};
use super::super::format::{self, Mode};
use super::super::raw::Layout;
use super::super::{CameraDevice, CameraError, worker};
use super::{Reading, error};

/// The largest picture that a sample may carry: four bytes per pixel at the largest size that
/// capture takes (`format::takes`) fit.
const LARGEST_SAMPLE: usize = 32 * 1024 * 1024;

/// A DirectShow video input device: its entry of the camera list, the device path of a device
/// that has one, and its moniker.
pub(super) struct Device {
    pub(super) device: CameraDevice,
    pub(super) path: Option<String>,
    moniker: IMoniker,
}

/// The devices of the video input category of DirectShow. A device is named by its device path
/// when it has one, and otherwise by the display name of its moniker, which names the
/// registered filter of a virtual camera.
pub(super) fn enumerate() -> Result<Vec<Device>, CameraError> {
    // SAFETY: the system device enumerator returns owned COM references; each moniker and
    // property bag is used on this thread while it is held.
    unsafe {
        let devices: ICreateDevEnum =
            CoCreateInstance(&CLSID_SystemDeviceEnum, None, CLSCTX_INPROC_SERVER).map_err(error)?;
        let mut monikers = None;
        devices
            .CreateClassEnumerator(&CLSID_VideoInputDeviceCategory, &mut monikers, 0)
            .map_err(error)?;
        // An empty category gives S_FALSE and no enumerator.
        let Some(monikers) = monikers else {
            return Ok(Vec::new());
        };
        let mut listed = Vec::new();
        loop {
            let mut item = [None];
            let mut fetched = 0;
            if monikers.Next(&mut item, Some(&mut fetched)) != S_OK || fetched == 0 {
                break;
            }
            let Some(moniker) = item[0].take() else {
                break;
            };
            let Ok(bag) = moniker.BindToStorage::<_, _, IPropertyBag>(None, None) else {
                continue;
            };
            let path = text(&bag, w!("DevicePath")).filter(|path| !path.is_empty());
            let id = match &path {
                Some(path) => path.clone(),
                None => match display_name(&moniker) {
                    Some(name) => name,
                    None => continue,
                },
            };
            let name = text(&bag, w!("FriendlyName"))
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| "Camera".into());
            listed.push(Device {
                device: CameraDevice { id, name },
                path,
                moniker,
            });
        }
        Ok(listed)
    }
}

/// A text property of a device.
fn text(bag: &IPropertyBag, name: PCWSTR) -> Option<String> {
    let mut value = VARIANT::default();
    // SAFETY: the variant is initialized empty, read only as the type that it states, and
    // cleared once.
    unsafe {
        bag.Read(name, &mut value, None).ok()?;
        let read = (value.Anonymous.Anonymous.vt == VT_BSTR)
            .then(|| value.Anonymous.Anonymous.Anonymous.bstrVal.to_string());
        let _ = VariantClear(&mut value);
        read
    }
}

/// The display name of a moniker.
fn display_name(moniker: &IMoniker) -> Option<String> {
    // SAFETY: the moniker returns a string allocated with CoTaskMemAlloc, which is read once
    // and freed.
    unsafe {
        let name = moniker.GetDisplayName(None, None).ok()?;
        if name.is_null() {
            return None;
        }
        let read = name.to_string().ok();
        CoTaskMemFree(Some(name.0.cast()));
        read.filter(|name| !name.is_empty())
    }
}

/// A media type with its own copy of the format block.
#[derive(Clone)]
struct Owned {
    major: GUID,
    sub: GUID,
    fixed: bool,
    temporal: bool,
    sample_size: u32,
    format_type: GUID,
    format: Vec<u8>,
}

impl Owned {
    /// A copy of a media type of DirectShow.
    ///
    /// # Safety
    /// `media` is null or points to a valid media type whose format block has `cbFormat`
    /// bytes.
    unsafe fn copy(media: *const AM_MEDIA_TYPE) -> Option<Self> {
        // SAFETY: the caller states that the pointer is null or valid.
        let media = unsafe { media.as_ref()? };
        let format = if media.pbFormat.is_null() || media.cbFormat == 0 {
            Vec::new()
        } else {
            // SAFETY: the format block holds `cbFormat` bytes.
            unsafe { std::slice::from_raw_parts(media.pbFormat, media.cbFormat as usize) }.to_vec()
        };
        Some(Self {
            major: media.majortype,
            sub: media.subtype,
            fixed: media.bFixedSizeSamples.as_bool(),
            temporal: media.bTemporalCompression.as_bool(),
            sample_size: media.lSampleSize,
            format_type: media.formattype,
            format,
        })
    }

    /// Writes this type into a media type that the caller frees, with a format block
    /// allocated with CoTaskMemAlloc.
    ///
    /// # Safety
    /// `to` points to writable memory for one media type.
    unsafe fn fill(&self, to: *mut AM_MEDIA_TYPE) -> ::windows::core::Result<()> {
        let format = if self.format.is_empty() {
            std::ptr::null_mut()
        } else {
            // SAFETY: the allocation has the length of the copied block.
            let block = unsafe { CoTaskMemAlloc(self.format.len()) }.cast::<u8>();
            if block.is_null() {
                return Err(::windows::Win32::Foundation::E_OUTOFMEMORY.into());
            }
            // SAFETY: both regions have the length of the block and do not overlap.
            unsafe {
                std::ptr::copy_nonoverlapping(self.format.as_ptr(), block, self.format.len())
            };
            block
        };
        // SAFETY: the caller states that `to` is writable; the previous content is not read.
        unsafe {
            to.write(AM_MEDIA_TYPE {
                majortype: self.major,
                subtype: self.sub,
                bFixedSizeSamples: self.fixed.into(),
                bTemporalCompression: self.temporal.into(),
                lSampleSize: self.sample_size,
                formattype: self.format_type,
                pUnk: std::mem::ManuallyDrop::new(None),
                cbFormat: self.format.len() as u32,
                pbFormat: format,
            })
        };
        Ok(())
    }

    /// A media type of DirectShow that points into this copy, for a call that reads it.
    fn borrowed(&mut self) -> AM_MEDIA_TYPE {
        AM_MEDIA_TYPE {
            majortype: self.major,
            subtype: self.sub,
            bFixedSizeSamples: self.fixed.into(),
            bTemporalCompression: self.temporal.into(),
            lSampleSize: self.sample_size,
            formattype: self.format_type,
            pUnk: std::mem::ManuallyDrop::new(None),
            cbFormat: self.format.len() as u32,
            pbFormat: if self.format.is_empty() {
                std::ptr::null_mut()
            } else {
                self.format.as_mut_ptr()
            },
        }
    }

    /// The width, the height, the frame time in units of 100 nanoseconds, the visible width,
    /// and the control flags of a video format block.
    fn video(&self) -> Option<Video> {
        if self.major != MEDIATYPE_Video {
            return None;
        }
        if self.format_type == FORMAT_VideoInfo
            && self.format.len() >= std::mem::size_of::<VIDEOINFOHEADER>()
        {
            // SAFETY: the block holds a whole header; it is read without alignment.
            let header =
                unsafe { std::ptr::read_unaligned(self.format.as_ptr().cast::<VIDEOINFOHEADER>()) };
            return Some(Video {
                width: header.bmiHeader.biWidth,
                height: header.bmiHeader.biHeight,
                visible: header.rcTarget.right - header.rcTarget.left,
                frame_time: header.AvgTimePerFrame,
                flags: None,
            });
        }
        if self.format_type == FORMAT_VideoInfo2
            && self.format.len() >= std::mem::size_of::<VIDEOINFOHEADER2>()
        {
            // SAFETY: the block holds a whole header; it is read without alignment, and the
            // union holds the control flags in every form.
            let header = unsafe {
                std::ptr::read_unaligned(self.format.as_ptr().cast::<VIDEOINFOHEADER2>())
            };
            // SAFETY: both members of the union are a 32-bit value.
            let flags = unsafe { header.Anonymous.dwControlFlags };
            return Some(Video {
                width: header.bmiHeader.biWidth,
                height: header.bmiHeader.biHeight,
                visible: header.rcTarget.right - header.rcTarget.left,
                frame_time: header.AvgTimePerFrame,
                flags: Some(flags),
            });
        }
        None
    }

    /// The uncompressed subtype of this type that capture reads.
    fn subtype(&self) -> Option<Subtype> {
        let sub = self.sub;
        Some(if sub == MEDIASUBTYPE_NV12 {
            Subtype::Nv12
        } else if sub == MEDIASUBTYPE_I420 || sub == MEDIASUBTYPE_IYUV {
            Subtype::I420
        } else if sub == MEDIASUBTYPE_YUY2 {
            Subtype::Yuy2
        } else if sub == MEDIASUBTYPE_UYVY {
            Subtype::Uyvy
        } else if sub == MEDIASUBTYPE_RGB24 {
            Subtype::Rgb24
        } else if sub == MEDIASUBTYPE_RGB32 || sub == MEDIASUBTYPE_ARGB32 {
            Subtype::Rgb32
        } else {
            return None;
        })
    }

    /// The layout of the pictures of an uncompressed type that capture can read.
    fn layout(&self) -> Option<Layout> {
        bitmap::layout_of(self.subtype()?, &self.video()?)
    }

    /// The capture mode of an uncompressed type that `layout` accepts. An MJPEG type gives no
    /// mode: the MJPEG decoder of DirectShow is a filter between the source and the sink,
    /// which the direct connection of the capture does not insert (ADR 0770).
    fn mode(&self) -> Option<Mode> {
        let video = self.video()?;
        let layout = self.layout()?;
        Some(Mode {
            width: layout.width,
            height: layout.height,
            rate: video.rate(),
            compressed: false,
        })
    }

    /// The subtype, the size and the rate of this type, for the log.
    fn describe(&self) -> String {
        let video = self.video();
        format::describe(
            &bitmap::subtype_name(self.sub.to_u128()),
            video.map(|video| (video.width.unsigned_abs(), video.height.unsigned_abs())),
            video.and_then(|video| video.rate()),
        )
    }
}

/// What the input pin hands to the capture thread.
enum Delivered {
    /// The bytes of one picture, and its start time in units of 100 nanoseconds.
    Picture(Vec<u8>, Option<i64>),
    /// The source changed the type of its pictures.
    Changed,
    /// The stream of the source ended.
    Ended,
}

#[derive(Default)]
struct PinState {
    connected: Option<(IPin, Owned)>,
}

/// The input pin of the sink. It holds the filter until the capture ends the graph.
#[implement(IPin, IMemInputPin)]
struct Input {
    filter: Mutex<Option<IBaseFilter>>,
    state: Mutex<PinState>,
    delivered: mpsc::SyncSender<Delivered>,
    /// The source changed the type of its pictures. The capture thread reads it before each
    /// picture, so a change is not lost when the queue is full.
    changed: std::sync::atomic::AtomicBool,
    /// The stream of the source ended (`EndOfStream` of the pin).
    ended: std::sync::atomic::AtomicBool,
    /// The identifier of the device, for the counts of the device check.
    device: String,
}

impl Input {
    fn accepts(media: &Owned) -> bool {
        media.layout().is_some()
    }

    fn layout(&self) -> Option<Layout> {
        let state = self.state.lock().ok()?;
        state.connected.as_ref()?.1.layout()
    }
}

const PIN_NAME: PCWSTR = w!("Input");

fn name_into(name: &str, into: &mut [u16; 128]) {
    for (place, unit) in into.iter_mut().zip(name.encode_utf16().take(127)) {
        *place = unit;
    }
}

impl IPin_Impl for Input_Impl {
    fn Connect(&self, _: Ref<IPin>, _: *const AM_MEDIA_TYPE) -> ::windows::core::Result<()> {
        Err(E_UNEXPECTED.into())
    }

    fn ReceiveConnection(
        &self,
        connector: Ref<IPin>,
        media: *const AM_MEDIA_TYPE,
    ) -> ::windows::core::Result<()> {
        let connector = connector.ok()?;
        // SAFETY: the connecting pin passes a valid media type for this call.
        let media = unsafe { Owned::copy(media) }.ok_or(E_POINTER)?;
        let mut state = self.state.lock().map_err(|_| E_UNEXPECTED)?;
        if state.connected.is_some() {
            return Err(VFW_E_ALREADY_CONNECTED.into());
        }
        if !Input::accepts(&media) {
            return Err(VFW_E_TYPE_NOT_ACCEPTED.into());
        }
        state.connected = Some((connector.clone(), media));
        Ok(())
    }

    fn Disconnect(&self) -> ::windows::core::Result<()> {
        let mut state = self.state.lock().map_err(|_| E_UNEXPECTED)?;
        match state.connected.take() {
            Some(_) => Ok(()),
            None => Err(::windows::core::Error::from_hresult(S_FALSE)),
        }
    }

    fn ConnectedTo(&self) -> ::windows::core::Result<IPin> {
        let state = self.state.lock().map_err(|_| E_UNEXPECTED)?;
        state
            .connected
            .as_ref()
            .map(|(pin, _)| pin.clone())
            .ok_or_else(|| VFW_E_NOT_CONNECTED.into())
    }

    fn ConnectionMediaType(&self, media: *mut AM_MEDIA_TYPE) -> ::windows::core::Result<()> {
        if media.is_null() {
            return Err(E_POINTER.into());
        }
        let state = self.state.lock().map_err(|_| E_UNEXPECTED)?;
        let (_, owned) = state.connected.as_ref().ok_or(VFW_E_NOT_CONNECTED)?;
        // SAFETY: the caller passes memory for one media type, which it frees.
        unsafe { owned.fill(media) }
    }

    fn QueryPinInfo(&self, info: *mut PIN_INFO) -> ::windows::core::Result<()> {
        if info.is_null() {
            return Err(E_POINTER.into());
        }
        let filter = self.filter.lock().map_err(|_| E_UNEXPECTED)?.clone();
        let mut name = [0; 128];
        name_into("Input", &mut name);
        // SAFETY: the caller passes memory for one pin description and releases its filter.
        unsafe {
            info.write(PIN_INFO {
                pFilter: std::mem::ManuallyDrop::new(filter),
                dir: PINDIR_INPUT,
                achName: name,
            })
        };
        Ok(())
    }

    fn QueryDirection(&self) -> ::windows::core::Result<PIN_DIRECTION> {
        Ok(PINDIR_INPUT)
    }

    fn QueryId(&self) -> ::windows::core::Result<PWSTR> {
        let name: Vec<u16> = "Input".encode_utf16().chain([0]).collect();
        // SAFETY: the allocation holds the name and its terminator; the caller frees it.
        unsafe {
            let block = CoTaskMemAlloc(name.len() * 2).cast::<u16>();
            if block.is_null() {
                return Err(::windows::Win32::Foundation::E_OUTOFMEMORY.into());
            }
            std::ptr::copy_nonoverlapping(name.as_ptr(), block, name.len());
            Ok(PWSTR(block))
        }
    }

    fn QueryAccept(&self, media: *const AM_MEDIA_TYPE) -> HRESULT {
        // SAFETY: the caller passes a valid media type for this call.
        match unsafe { Owned::copy(media) } {
            Some(media) if Input::accepts(&media) => S_OK,
            _ => S_FALSE,
        }
    }

    fn EnumMediaTypes(&self) -> ::windows::core::Result<IEnumMediaTypes> {
        Ok(NoTypes.into())
    }

    fn QueryInternalConnections(
        &self,
        _: ::windows::core::OutRef<IPin>,
        _: *mut u32,
    ) -> ::windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn EndOfStream(&self) -> ::windows::core::Result<()> {
        self.ended.store(true, std::sync::atomic::Ordering::Release);
        let _ = self.delivered.try_send(Delivered::Ended);
        Ok(())
    }

    fn BeginFlush(&self) -> ::windows::core::Result<()> {
        Ok(())
    }

    fn EndFlush(&self) -> ::windows::core::Result<()> {
        Ok(())
    }

    fn NewSegment(&self, _: i64, _: i64, _: f64) -> ::windows::core::Result<()> {
        Ok(())
    }
}

impl IMemInputPin_Impl for Input_Impl {
    fn GetAllocator(&self) -> ::windows::core::Result<IMemAllocator> {
        Err(VFW_E_NO_ALLOCATOR.into())
    }

    fn NotifyAllocator(&self, _: Ref<IMemAllocator>, _: BOOL) -> ::windows::core::Result<()> {
        Ok(())
    }

    fn GetAllocatorRequirements(&self) -> ::windows::core::Result<ALLOCATOR_PROPERTIES> {
        Err(E_NOTIMPL.into())
    }

    fn Receive(&self, sample: Ref<IMediaSample>) -> ::windows::core::Result<()> {
        // A panic cannot unwind into DirectShow (ADR 0757).
        crate::callback::stop_panic(
            "camera sample",
            || Ok(()),
            || {
                let sample = sample.ok()?;
                // SAFETY: the sample and its buffer are valid for this call; the bytes are
                // copied before it returns, and a changed type is freed as DirectShow states.
                unsafe {
                    if let Ok(changed) = sample.GetMediaType()
                        && !changed.is_null()
                    {
                        free_media_type(changed);
                        self.changed
                            .store(true, std::sync::atomic::Ordering::Release);
                        let _ = self.delivered.try_send(Delivered::Changed);
                        return Ok(());
                    }
                    let length = usize::try_from(sample.GetActualDataLength()).unwrap_or(0);
                    let pointer = sample.GetPointer()?;
                    if pointer.is_null() || length == 0 || length > LARGEST_SAMPLE {
                        return Ok(());
                    }
                    let bytes = std::slice::from_raw_parts(pointer, length).to_vec();
                    let (mut start, mut end) = (0, 0);
                    let at = sample.GetTime(&mut start, &mut end).ok().map(|_| start);
                    // A full queue drops the picture, and the device check counts it as lost.
                    let dropped = self
                        .delivered
                        .try_send(Delivered::Picture(bytes, at))
                        .is_err();
                    worker::count(&self.device, 1, Some(u32::from(dropped)));
                }
                Ok(())
            },
        )
    }

    fn ReceiveMultiple(
        &self,
        samples: *const Option<IMediaSample>,
        count: i32,
    ) -> ::windows::core::Result<i32> {
        let count = usize::try_from(count).unwrap_or(0);
        if samples.is_null() || count == 0 {
            return Ok(0);
        }
        // SAFETY: the caller passes `count` samples that stay valid for this call.
        let samples = unsafe { std::slice::from_raw_parts(samples, count) };
        for sample in samples {
            IMemInputPin_Impl::Receive(self, Ref::from(sample))?;
        }
        Ok(count as i32)
    }

    fn ReceiveCanBlock(&self) -> ::windows::core::Result<()> {
        Err(::windows::core::Error::from_hresult(S_FALSE))
    }
}

/// Frees a media type that DirectShow allocated, with its format block.
///
/// # Safety
/// `media` was allocated with CoTaskMemAlloc and is not used afterwards.
unsafe fn free_media_type(media: *mut AM_MEDIA_TYPE) {
    // SAFETY: the caller hands over the allocation and its format block.
    unsafe {
        if let Some(owned) = media.as_mut() {
            if !owned.pbFormat.is_null() {
                CoTaskMemFree(Some(owned.pbFormat.cast()));
            }
            std::mem::ManuallyDrop::drop(&mut owned.pUnk);
        }
        CoTaskMemFree(Some(media.cast()));
    }
}

/// The media types that the pin prefers: none, the source chooses.
#[implement(IEnumMediaTypes)]
struct NoTypes;

impl IEnumMediaTypes_Impl for NoTypes_Impl {
    fn Next(&self, _: u32, _: *mut *mut AM_MEDIA_TYPE, fetched: *mut u32) -> HRESULT {
        if !fetched.is_null() {
            // SAFETY: the caller passes a writable count when it passes one.
            unsafe { fetched.write(0) };
        }
        S_FALSE
    }

    fn Skip(&self, _: u32) -> ::windows::core::Result<()> {
        Err(::windows::core::Error::from_hresult(S_FALSE))
    }

    fn Reset(&self) -> ::windows::core::Result<()> {
        Ok(())
    }

    fn Clone(&self) -> ::windows::core::Result<IEnumMediaTypes> {
        Ok(NoTypes.into())
    }
}

/// The one pin of the sink.
#[implement(IEnumPins)]
struct Pins {
    pin: IPin,
    taken: Mutex<bool>,
}

impl IEnumPins_Impl for Pins_Impl {
    fn Next(&self, count: u32, pins: *mut Option<IPin>, fetched: *mut u32) -> HRESULT {
        let Ok(mut taken) = self.taken.lock() else {
            return E_UNEXPECTED;
        };
        let given = u32::from(count > 0 && !*taken && !pins.is_null());
        if given == 1 {
            // SAFETY: the caller passes room for `count` pins.
            unsafe { pins.write(Some(self.pin.clone())) };
            *taken = true;
        }
        if !fetched.is_null() {
            // SAFETY: the caller passes a writable count when it passes one.
            unsafe { fetched.write(given) };
        }
        if given == count { S_OK } else { S_FALSE }
    }

    fn Skip(&self, count: u32) -> ::windows::core::Result<()> {
        let mut taken = self.taken.lock().map_err(|_| E_UNEXPECTED)?;
        if count == 0 {
            return Ok(());
        }
        let skipped = !*taken;
        *taken = true;
        if skipped && count == 1 {
            Ok(())
        } else {
            Err(::windows::core::Error::from_hresult(S_FALSE))
        }
    }

    fn Reset(&self) -> ::windows::core::Result<()> {
        *self.taken.lock().map_err(|_| E_UNEXPECTED)? = false;
        Ok(())
    }

    fn Clone(&self) -> ::windows::core::Result<IEnumPins> {
        let taken = *self.taken.lock().map_err(|_| E_UNEXPECTED)?;
        Ok(Pins {
            pin: self.pin.clone(),
            taken: Mutex::new(taken),
        }
        .into())
    }
}

struct SinkState {
    state: FILTER_STATE,
    clock: Option<IReferenceClock>,
    /// The graph that the sink joined. The graph checks it before it connects a pin of the
    /// sink, and it leaves the graph when the capture removes the sink (`Graph`).
    graph: Option<IFilterGraph>,
}

/// The filter that receives the pictures of the source through its one input pin.
#[implement(IBaseFilter)]
struct Sink {
    pin: IPin,
    state: Mutex<SinkState>,
}

/// The class identifier that the sink states; no class is registered under it.
const SINK_CLASS: GUID = GUID::from_u128(0x6a3c_2f0e_4b8d_4c71_9f5e_1d2b_7a90_c4e3);

impl IPersist_Impl for Sink_Impl {
    fn GetClassID(&self) -> ::windows::core::Result<GUID> {
        Ok(SINK_CLASS)
    }
}

impl IMediaFilter_Impl for Sink_Impl {
    fn Stop(&self) -> ::windows::core::Result<()> {
        self.state.lock().map_err(|_| E_UNEXPECTED)?.state = State_Stopped;
        Ok(())
    }

    fn Pause(&self) -> ::windows::core::Result<()> {
        self.state.lock().map_err(|_| E_UNEXPECTED)?.state = State_Paused;
        Ok(())
    }

    fn Run(&self, _: i64) -> ::windows::core::Result<()> {
        self.state.lock().map_err(|_| E_UNEXPECTED)?.state = State_Running;
        Ok(())
    }

    fn GetState(&self, _: u32) -> ::windows::core::Result<FILTER_STATE> {
        Ok(self.state.lock().map_err(|_| E_UNEXPECTED)?.state)
    }

    fn SetSyncSource(&self, clock: Ref<IReferenceClock>) -> ::windows::core::Result<()> {
        self.state.lock().map_err(|_| E_UNEXPECTED)?.clock = clock.cloned();
        Ok(())
    }

    fn GetSyncSource(&self) -> ::windows::core::Result<IReferenceClock> {
        self.state
            .lock()
            .map_err(|_| E_UNEXPECTED)?
            .clock
            .clone()
            .ok_or_else(|| ::windows::core::Error::from_hresult(S_FALSE))
    }
}

impl IBaseFilter_Impl for Sink_Impl {
    fn EnumPins(&self) -> ::windows::core::Result<IEnumPins> {
        Ok(Pins {
            pin: self.pin.clone(),
            taken: Mutex::new(false),
        }
        .into())
    }

    fn FindPin(&self, id: &PCWSTR) -> ::windows::core::Result<IPin> {
        // SAFETY: the caller passes a terminated string for this call.
        let wanted = unsafe { id.to_string() }.map_err(|_| E_POINTER)?;
        // SAFETY: the name of the pin is a terminated constant.
        let own = unsafe { PIN_NAME.to_string() }.map_err(|_| E_UNEXPECTED)?;
        if wanted == own {
            Ok(self.pin.clone())
        } else {
            Err(VFW_E_NOT_FOUND.into())
        }
    }

    fn QueryFilterInfo(&self, info: *mut FILTER_INFO) -> ::windows::core::Result<()> {
        if info.is_null() {
            return Err(E_POINTER.into());
        }
        let graph = self.state.lock().map_err(|_| E_UNEXPECTED)?.graph.clone();
        let mut name = [0; 128];
        name_into("Junto", &mut name);
        // SAFETY: the caller passes memory for one filter description and releases the graph
        // that it holds.
        unsafe {
            info.write(FILTER_INFO {
                achName: name,
                pGraph: std::mem::ManuallyDrop::new(graph),
            })
        };
        Ok(())
    }

    fn JoinFilterGraph(&self, graph: Ref<IFilterGraph>, _: &PCWSTR) -> ::windows::core::Result<()> {
        self.state.lock().map_err(|_| E_UNEXPECTED)?.graph = graph.cloned();
        Ok(())
    }

    fn QueryVendorInfo(&self) -> ::windows::core::Result<PWSTR> {
        Err(E_NOTIMPL.into())
    }
}

/// The graph of one capture: the source, the sink and the controls. Dropping it stops the
/// graph, removes both filters and breaks the reference between the pin and the sink.
struct Graph<'a> {
    delivery: &'a worker::Delivery,
    graph: IGraphBuilder,
    control: IMediaControl,
    source: IBaseFilter,
    sink: IBaseFilter,
    input: ComObject<Input>,
    /// The filters that joined the graph: only these are removed at the end.
    source_added: bool,
    sink_added: bool,
}

impl Drop for Graph<'_> {
    fn drop(&mut self) {
        // SAFETY: the capture thread owns the graph; the filters stay alive until removed.
        unsafe {
            if let Err(error) = self.control.Stop() {
                self.delivery
                    .cleanup_failed(format!("camera graph stop: {error}"));
            }
            for (filter, added, operation) in [
                (&self.sink, self.sink_added, "camera sink removal"),
                (&self.source, self.source_added, "camera source removal"),
            ] {
                if !added {
                    continue;
                }
                if let Err(error) = self.graph.RemoveFilter(filter) {
                    self.delivery
                        .cleanup_failed(format!("{operation}: {error}"));
                }
            }
        }
        if let Ok(mut filter) = self.input.filter.lock() {
            filter.take();
        }
    }
}

/// The error of a step of the capture, with the step named in a failure that has no error of
/// its own.
fn at(step: &'static str) -> impl Fn(::windows::core::Error) -> CameraError {
    move |failure| {
        let code = failure.code();
        match error(failure) {
            CameraError::Failed(message) => {
                CameraError::Failed(format!("{step}: {} ({code:?})", message.trim()))
            }
            other => other,
        }
    }
}

/// The output pin of the source that gives the pictures: the pin of the capture category, or,
/// for a source that states no category for its pins, such as the virtual camera of OBS
/// Studio, the first output pin with a stream configuration.
fn capture_pin(builder: &ICaptureGraphBuilder2, source: &IBaseFilter) -> Result<IPin, CameraError> {
    // SAFETY: the builder and the source belong to the graph of this thread; each pin of the
    // enumeration is an owned reference.
    unsafe {
        let found = builder.FindPin(
            source,
            PINDIR_OUTPUT,
            Some(&PIN_CATEGORY_CAPTURE),
            Some(&MEDIATYPE_Video),
            true,
            0,
        );
        let failure = match found {
            Ok(pin) => return Ok(pin),
            Err(failure) => failure,
        };
        let pins = source.EnumPins().map_err(at("the pins of the source"))?;
        loop {
            let mut item = [None];
            let mut fetched = 0;
            if pins.Next(&mut item, Some(&mut fetched)) != S_OK || fetched == 0 {
                break;
            }
            let Some(pin) = item[0].take() else {
                break;
            };
            if pin
                .QueryDirection()
                .is_ok_and(|direction| direction == PINDIR_OUTPUT)
                && pin.cast::<IAMStreamConfig>().is_ok()
            {
                return Ok(pin);
            }
        }
        Err(at("the capture pin")(failure))
    }
}

/// The types that the capture pin offers.
fn stream_types(config: &IAMStreamConfig) -> Result<Vec<Owned>, CameraError> {
    let (mut count, mut size) = (0, 0);
    // SAFETY: the counts are written by the pin.
    unsafe { config.GetNumberOfCapabilities(&mut count, &mut size) }
        .map_err(at("the capabilities of the capture pin"))?;
    let mut caps = vec![0u8; usize::try_from(size).unwrap_or(0).max(256)];
    let mut types = Vec::new();
    for index in 0..count.clamp(0, 1024) {
        let mut media = std::ptr::null_mut();
        // SAFETY: the buffer for the capabilities is at least the size that the pin stated;
        // the returned type is copied and freed.
        unsafe {
            if config
                .GetStreamCaps(index, &mut media, caps.as_mut_ptr())
                .is_err()
            {
                continue;
            }
            if let Some(owned) = Owned::copy(media) {
                types.push(owned);
            }
            if !media.is_null() {
                free_media_type(media);
            }
        }
    }
    Ok(types)
}

/// The media types that a pin offers in its own enumeration, for a pin whose stream
/// configuration states no capability.
fn pin_types(pin: &IPin) -> Vec<Owned> {
    let mut types = Vec::new();
    // SAFETY: each type of the enumeration is allocated by the pin, copied and freed once.
    unsafe {
        let Ok(offered) = pin.EnumMediaTypes() else {
            return types;
        };
        for _ in 0..1024 {
            let mut item = [std::ptr::null_mut()];
            let mut fetched = 0;
            if offered.Next(&mut item, Some(&mut fetched)) != S_OK || fetched == 0 {
                break;
            }
            if let Some(owned) = Owned::copy(item[0]) {
                types.push(owned);
            }
            if !item[0].is_null() {
                free_media_type(item[0]);
            }
        }
    }
    types
}

/// Asks the capture pin for the uncompressed type `media` and connects the pin directly to
/// the sink.
fn connect(
    graph: &IGraphBuilder,
    output: &IPin,
    input: &IPin,
    config: &IAMStreamConfig,
    media: &mut Owned,
) -> Result<(), CameraError> {
    let borrowed = media.borrowed();
    // SAFETY: the borrowed type points into `media`, which outlives both calls; the pins
    // belong to filters of this graph.
    // A pin that refuses the format as a request, such as one that offers its types only in
    // its own enumeration, can still take it in the connection.
    let result = unsafe {
        let _ = config.SetFormat(&borrowed);
        graph
            .ConnectDirect(output, input, Some(&borrowed))
            .map_err(at("the direct connection"))
    };
    if result.is_err() {
        // SAFETY: disconnecting pins of this graph that may be half connected.
        unsafe {
            let _ = graph.Disconnect(output);
            let _ = graph.Disconnect(input);
        }
    }
    result
}

/// Captures a DirectShow device: the first type of the order of `format::order` that connects,
/// with the same pictures, sizes and errors as the Media Foundation path.
pub(super) fn capture(
    device: Device,
    chosen: &CameraDevice,
    delivery: &worker::Delivery,
    ready: &mut Option<worker::Ready>,
    reading: &mut Reading,
) -> Result<(), CameraError> {
    reading.step = "the source filter and the filter graph";
    let (send, receive) = mpsc::sync_channel(2);
    // SAFETY: the objects are created and used on this capture thread; `Graph` ends them.
    let (graph, builder, control, events, source) = unsafe {
        let source: IBaseFilter = device
            .moniker
            .BindToObject(None, None)
            .map_err(at("the source filter"))?;
        let graph: IGraphBuilder = CoCreateInstance(&CLSID_FilterGraph, None, CLSCTX_INPROC_SERVER)
            .map_err(at("the filter graph"))?;
        let builder: ICaptureGraphBuilder2 =
            CoCreateInstance(&CLSID_CaptureGraphBuilder2, None, CLSCTX_INPROC_SERVER)
                .map_err(at("the capture graph builder"))?;
        builder
            .SetFiltergraph(&graph)
            .map_err(at("the graph of the builder"))?;
        let control: IMediaControl = graph.cast().map_err(at("the control of the graph"))?;
        let events: IMediaEvent = graph.cast().map_err(at("the events of the graph"))?;
        (graph, builder, control, events, source)
    };
    let input = ComObject::new(Input {
        filter: Mutex::new(None),
        state: Mutex::new(PinState::default()),
        delivered: send,
        changed: std::sync::atomic::AtomicBool::new(false),
        ended: std::sync::atomic::AtomicBool::new(false),
        device: chosen.id.clone(),
    });
    let pin: IPin = input.to_interface();
    let sink: IBaseFilter = Sink {
        pin: pin.clone(),
        state: Mutex::new(SinkState {
            state: State_Stopped,
            clock: None,
            graph: None,
        }),
    }
    .into();
    if let Ok(mut filter) = input.filter.lock() {
        *filter = Some(sink.clone());
    }
    let mut graph = Graph {
        delivery,
        graph,
        control,
        source,
        sink,
        input,
        source_added: false,
        sink_added: false,
    };
    reading.step = "the capture pin";
    // SAFETY: both filters join the graph of this thread.
    let output = unsafe {
        graph
            .graph
            .AddFilter(&graph.source, w!("Camera"))
            .map_err(at("the source in the graph"))?;
        graph.source_added = true;
        graph
            .graph
            .AddFilter(&graph.sink, w!("Junto"))
            .map_err(at("the sink in the graph"))?;
        graph.sink_added = true;
        capture_pin(&builder, &graph.source)?
    };
    let config: IAMStreamConfig = output
        .cast()
        .map_err(at("the stream configuration of the capture pin"))?;
    reading.step = "the types of the capture pin";
    let mut types = stream_types(&config)?;
    if types.is_empty() {
        types = pin_types(&output);
    }
    reading.offered = Some(
        types
            .iter()
            .map(Owned::describe)
            .collect::<Vec<_>>()
            .join(", "),
    );
    reading.step = "a type of the capture pin that capture reads";
    let modes: Vec<(usize, Mode)> = types
        .iter()
        .enumerate()
        .filter_map(|(index, media)| Some((index, media.mode()?)))
        .collect();
    let listed: Vec<Mode> = modes.iter().map(|(_, mode)| *mode).collect();
    let mut failure = CameraError::Unsupported;
    let mut connected = None;
    for chosen_mode in format::order(&listed) {
        reading.step = "the direct connection of a type";
        let (index, mode) = modes[chosen_mode];
        match connect(&graph.graph, &output, &pin, &config, &mut types[index]) {
            Ok(()) => {
                connected = Some(mode.rate);
                break;
            }
            Err(error) => failure = error,
        }
    }
    let rate = connected.ok_or(failure)?;
    reading.step = "the layout of the connected type";
    let layout = graph.input.layout().ok_or(CameraError::Unsupported)?;
    reading.connected = graph
        .input
        .state
        .lock()
        .ok()
        .and_then(|state| state.connected.as_ref().map(|(_, media)| media.describe()));
    reading.step = "the start of the graph";
    // SAFETY: the graph of this thread is connected; `Graph` stops it.
    unsafe { graph.control.Run() }.map_err(at("the start of the graph"))?;
    reading.step = "the first picture";
    let start = Instant::now();
    let mut last_picture = start;
    let mut pacer = worker::Pacer::default();
    while !delivery.stopped() {
        lost(&events)?;
        // A change of the type and the end of the stream are read before each wait, so they
        // count also when the queue had no room for their message.
        if graph
            .input
            .changed
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return Err(CameraError::Interrupted);
        }
        // The end of the stream of a source ends the capture as the end of stream of Media
        // Foundation does: as a device that is gone.
        if graph.input.ended.load(std::sync::atomic::Ordering::Acquire) {
            return Err(CameraError::DeviceGone);
        }
        let delivered = match receive.recv_timeout(Duration::from_millis(100)) {
            Ok(delivered) => delivered,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if last_picture.elapsed() >= Duration::from_secs(5) {
                    return Err(CameraError::Interrupted);
                }
                continue;
            }
            Err(_) => return Err(CameraError::Interrupted),
        };
        if graph
            .input
            .changed
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return Err(CameraError::Interrupted);
        }
        let (bytes, at) = match delivered {
            Delivered::Picture(bytes, at) => (bytes, at),
            Delivered::Changed => return Err(CameraError::Interrupted),
            Delivered::Ended => return Err(CameraError::DeviceGone),
        };
        last_picture = Instant::now();
        let at = at
            .and_then(|units| u64::try_from(units).ok())
            .map(|units| Duration::from_nanos(units.saturating_mul(100)))
            .unwrap_or(last_picture - start);
        if pacer.due(at) {
            let picture = layout.convert(&bytes)?;
            if let Some(ready) = ready.take() {
                let (width, height) = layout.published();
                let _ = ready.send(Ok(worker::Started {
                    width,
                    height,
                    rate,
                }));
            }
            delivery.picture(std::sync::Arc::new(picture));
        }
    }
    Ok(())
}

/// The error of an event of the graph that ends the capture: the device was removed, or the
/// graph stopped on an error.
fn lost(events: &IMediaEvent) -> Result<(), CameraError> {
    loop {
        let (mut code, mut first, mut second) = (0, 0, 0);
        // SAFETY: the event parameters are freed after each event, as the graph states.
        let result = unsafe {
            if events
                .GetEvent(&mut code, &mut first, &mut second, 0)
                .is_err()
            {
                return Ok(());
            }
            let _ = events.FreeEventParams(code, first, second);
            match code as u32 {
                EC_DEVICE_LOST if second == 0 => Err(CameraError::DeviceGone),
                EC_ERRORABORT => Err(error(::windows::core::Error::from_hresult(HRESULT(
                    first as i32,
                )))),
                _ => Ok(()),
            }
        };
        result?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::raw::Pixel;
    use ::windows::Win32::Media::MediaFoundation::MEDIASUBTYPE_MJPG;

    fn video_type(subtype: GUID, width: i32, height: i32, frame_time: i64) -> Owned {
        let mut header = VIDEOINFOHEADER {
            AvgTimePerFrame: frame_time,
            ..Default::default()
        };
        header.bmiHeader.biWidth = width;
        header.bmiHeader.biHeight = height;
        // SAFETY: the header is plain data of its own size.
        let format = unsafe {
            std::slice::from_raw_parts(
                (&header as *const VIDEOINFOHEADER).cast::<u8>(),
                std::mem::size_of::<VIDEOINFOHEADER>(),
            )
        }
        .to_vec();
        Owned {
            major: MEDIATYPE_Video,
            sub: subtype,
            fixed: true,
            temporal: false,
            sample_size: 0,
            format_type: FORMAT_VideoInfo,
            format,
        }
    }

    #[test]
    fn a_direct_show_type_gives_the_mode_and_layout_of_the_media_foundation_path() {
        let yuy2 = video_type(MEDIASUBTYPE_YUY2, 1280, 720, 333_333);
        assert_eq!(
            yuy2.mode(),
            Some(Mode {
                width: 1280,
                height: 720,
                rate: Some((10_000_000, 333_333)),
                compressed: false,
            })
        );
        let layout = yuy2.layout().unwrap();
        assert_eq!(
            (layout.width, layout.height, layout.stride),
            (1280, 720, 2560)
        );
        let nv12 = video_type(MEDIASUBTYPE_NV12, 640, -480, 0);
        let layout = nv12.layout().unwrap();
        assert_eq!(
            (layout.height, layout.stride, layout.pixel),
            (480, 640, Pixel::Nv12)
        );
        assert_eq!(nv12.mode().unwrap().rate, None);
        // The MJPEG decoder of DirectShow is not inserted by the direct connection.
        let mjpeg = video_type(MEDIASUBTYPE_MJPG, 1280, 720, 333_333);
        assert!(mjpeg.mode().is_none());
        assert!(mjpeg.layout().is_none());
        assert_eq!(mjpeg.describe(), "MJPG 1280x720 30.00 fps");
        // The one size of a virtual camera above the published size is taken.
        let large = video_type(MEDIASUBTYPE_NV12, 1920, 1080, 166_666);
        assert_eq!(large.layout().unwrap().published(), (1280, 720));
        for (subtype, pixel) in [
            (MEDIASUBTYPE_UYVY, Pixel::Uyvy),
            (MEDIASUBTYPE_RGB24, Pixel::Bgr24 { bottom_up: true }),
            (MEDIASUBTYPE_RGB32, Pixel::Bgrx { bottom_up: true }),
            (MEDIASUBTYPE_ARGB32, Pixel::Bgrx { bottom_up: true }),
        ] {
            let media = video_type(subtype, 1920, 1080, 333_333);
            assert_eq!(media.layout().unwrap().pixel, pixel);
            assert!(media.mode().is_some());
        }
    }

    #[test]
    fn a_media_type_is_copied_out_and_freed_with_its_format_block() {
        let owned = video_type(MEDIASUBTYPE_NV12, 640, 480, 333_333);
        // SAFETY: the test allocates one media type, fills it, copies it back and frees it.
        unsafe {
            let media =
                CoTaskMemAlloc(std::mem::size_of::<AM_MEDIA_TYPE>()).cast::<AM_MEDIA_TYPE>();
            assert!(!media.is_null());
            owned.fill(media).unwrap();
            let copied = Owned::copy(media).unwrap();
            assert_eq!(copied.format, owned.format);
            assert_eq!(copied.sub, MEDIASUBTYPE_NV12);
            free_media_type(media);
        }
    }
}
