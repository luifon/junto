//! The media source of the virtual cameras of the Windows device check (ADR 0913). The Frame
//! Server of Windows loads this library as a COM server after the check has registered it. The
//! source generates its pictures in memory at the rate of the chosen media type: a brightness
//! ramp that moves with each picture and four areas of different colour.

#[path = "virtual_profiles.rs"]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod profiles;

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod source {
    use super::profiles;
    use std::ffi::c_void;
    use std::sync::{Mutex, PoisonError, mpsc};
    use std::time::{Duration, Instant};

    use windows::Win32::Foundation::{
        CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_OUTOFMEMORY, E_POINTER,
        ERROR_SET_NOT_FOUND, ERROR_SUCCESS, S_FALSE,
    };
    use windows::Win32::Graphics::Imaging::{
        CLSID_WICImagingFactory, GUID_ContainerFormatJpeg, GUID_WICPixelFormat24bppBGR,
        IWICImagingFactory, WICBitmapEncoderNoCache,
    };
    use windows::Win32::Media::KernelStreaming::{
        IKsControl, IKsControl_Impl, KSIDENTIFIER, PINNAME_VIDEO_CAPTURE,
    };
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
        IClassFactory, IClassFactory_Impl, STATFLAG_NONAME, STATSTG, STREAM_SEEK_SET,
    };
    use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
    use windows::Win32::UI::Shell::SHCreateMemStream;
    use windows::core::{
        BOOL, Error, GUID, HRESULT, HSTRING, IUnknown, Interface, PCWSTR, PWSTR, Ref, Result,
        implement,
    };

    /// A COM object that Media Foundation uses from any thread.
    struct Free<T>(T);
    // SAFETY: Media Foundation event queues, samples and tokens are free-threaded.
    unsafe impl<T> Send for Free<T> {}

    #[derive(Clone, Copy, PartialEq)]
    struct Kind {
        subtype: GUID,
        width: u32,
        height: u32,
        rate: u32,
    }

    /// The profile that the check stored under the key of `class`.
    fn profile(class: &GUID) -> Option<String> {
        let key = HSTRING::from(format!("SOFTWARE\\Classes\\CLSID\\{{{class:?}}}"));
        let mut buffer = [0u16; 64];
        let mut size = std::mem::size_of_val(&buffer) as u32;
        // SAFETY: the buffer is writable for `size` bytes.
        let status = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                &key,
                &HSTRING::from(profiles::PROFILE),
                RRF_RT_REG_SZ,
                None,
                Some(buffer.as_mut_ptr().cast()),
                Some(&mut size),
            )
        };
        if status != ERROR_SUCCESS {
            return None;
        }
        let length = (size as usize / 2).saturating_sub(1).min(buffer.len());
        Some(String::from_utf16_lossy(&buffer[..length]))
    }

    fn kinds(class: &GUID) -> Option<Vec<Kind>> {
        let kind = |subtype, width, height, rate| Kind {
            subtype,
            width,
            height,
            rate,
        };
        let profile = profile(class)?;
        if profile == profiles::UNCOMPRESSED {
            Some(vec![
                kind(MFVideoFormat_NV12, 1280, 720, 30),
                kind(MFVideoFormat_YUY2, 640, 480, 30),
            ])
        } else if profile == profiles::COMPRESSED {
            Some(vec![
                kind(MFVideoFormat_YUY2, 1280, 720, 10),
                kind(MFVideoFormat_MJPG, 1280, 720, 30),
            ])
        } else {
            None
        }
    }

    fn pair(high: u32, low: u32) -> u64 {
        (u64::from(high) << 32) | u64::from(low)
    }

    fn line(kind: &Kind) -> u32 {
        if kind.subtype == MFVideoFormat_YUY2 {
            kind.width * 2
        } else {
            kind.width
        }
    }

    fn media_type(kind: &Kind) -> Result<IMFMediaType> {
        // SAFETY: the new media type is owned here.
        unsafe {
            let media = MFCreateMediaType()?;
            media.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            media.SetGUID(&MF_MT_SUBTYPE, &kind.subtype)?;
            media.SetUINT64(&MF_MT_FRAME_SIZE, pair(kind.width, kind.height))?;
            media.SetUINT64(&MF_MT_FRAME_RATE, pair(kind.rate, 1))?;
            media.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pair(1, 1))?;
            media.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            media.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)?;
            if kind.subtype != MFVideoFormat_MJPG {
                media.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32)?;
                media.SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT601.0 as u32)?;
                media.SetUINT32(&MF_MT_DEFAULT_STRIDE, line(kind))?;
                media.SetUINT32(&MF_MT_FIXED_SIZE_SAMPLES, 1)?;
                media.SetUINT32(&MF_MT_SAMPLE_SIZE, size(kind) as u32)?;
            }
            Ok(media)
        }
    }

    fn size(kind: &Kind) -> usize {
        let (w, h) = (kind.width as usize, kind.height as usize);
        if kind.subtype == MFVideoFormat_YUY2 {
            w * h * 2
        } else {
            w * h * 3 / 2
        }
    }

    /// The limited-range 601 values (Y, Cb, Cr) of the four quarters, top left, top right,
    /// bottom left, bottom right. Each is inside RGB by at least 19 levels, so that the JPEG
    /// pictures, which are encoded from RGB, keep them; a decoder that leaves full range or
    /// uses another matrix moves Cb and Cr by about 8 levels or more.
    const QUARTERS: [(u8, u8, u8); 4] = [
        (150, 63, 63),
        (130, 73, 183),
        (110, 183, 73),
        (100, 193, 193),
    ];

    /// The values of one pixel of picture `index`: the colour of its quarter, and in the top
    /// eighth a brightness ramp that moves with each picture.
    fn pixel(kind: &Kind, x: u32, y: u32, index: u32) -> (u8, u8, u8) {
        if y < kind.height / 8 {
            let ramp = (x + index * 8) % kind.width;
            return ((64 + ramp * 112 / kind.width) as u8, 128, 128);
        }
        let quarter = usize::from(x >= kind.width / 2) + 2 * usize::from(y >= kind.height / 2);
        QUARTERS[quarter]
    }

    fn uncompressed(kind: &Kind, index: u32) -> Vec<u8> {
        let (w, h) = (kind.width, kind.height);
        let mut bytes = Vec::with_capacity(size(kind));
        if kind.subtype == MFVideoFormat_YUY2 {
            for y in 0..h {
                for x in (0..w).step_by(2) {
                    let (y0, cb, cr) = pixel(kind, x, y, index);
                    let (y1, _, _) = pixel(kind, x + 1, y, index);
                    bytes.extend_from_slice(&[y0, cb, y1, cr]);
                }
            }
            return bytes;
        }
        for y in 0..h {
            for x in 0..w {
                bytes.push(pixel(kind, x, y, index).0);
            }
        }
        for y in (0..h).step_by(2) {
            for x in (0..w).step_by(2) {
                let (_, cb, cr) = pixel(kind, x, y, index);
                bytes.extend_from_slice(&[cb, cr]);
            }
        }
        bytes
    }

    /// Picture `index` as a JPEG file, encoded by the imaging component of Windows.
    fn jpeg(kind: &Kind, index: u32) -> Result<Vec<u8>> {
        let (w, h) = (kind.width, kind.height);
        let mut bgr = Vec::with_capacity((w * h * 3) as usize);
        for y in 0..h {
            for x in 0..w {
                let (luma, cb, cr) = pixel(kind, x, y, index);
                let (l, b, r) = (
                    1.164 * (f32::from(luma) - 16.0),
                    f32::from(cb) - 128.0,
                    f32::from(cr) - 128.0,
                );
                let channel = |value: f32| value.clamp(0.0, 255.0) as u8;
                bgr.extend_from_slice(&[
                    channel(l + 2.017 * b),
                    channel(l - 0.392 * b - 0.813 * r),
                    channel(l + 1.596 * r),
                ]);
            }
        }
        // SAFETY: every object is owned here; the pixel slice outlives the write.
        unsafe {
            let factory: IWICImagingFactory =
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
            let stream = SHCreateMemStream(None).ok_or_else(|| Error::from(E_OUTOFMEMORY))?;
            let encoder = factory.CreateEncoder(&GUID_ContainerFormatJpeg, std::ptr::null())?;
            encoder.Initialize(&stream, WICBitmapEncoderNoCache)?;
            let mut frame = None;
            encoder.CreateNewFrame(&mut frame, std::ptr::null_mut())?;
            let frame = frame.ok_or_else(|| Error::from(E_POINTER))?;
            frame.Initialize(None)?;
            frame.SetSize(w, h)?;
            let mut format = GUID_WICPixelFormat24bppBGR;
            frame.SetPixelFormat(&mut format)?;
            if format != GUID_WICPixelFormat24bppBGR {
                return Err(Error::from(E_POINTER));
            }
            frame.WritePixels(h, w * 3, &bgr)?;
            frame.Commit()?;
            encoder.Commit()?;
            let mut stat = STATSTG::default();
            stream.Stat(&mut stat, STATFLAG_NONAME)?;
            stream.Seek(0, STREAM_SEEK_SET, None)?;
            let mut bytes = vec![0u8; stat.cbSize as usize];
            let mut read = 0;
            stream
                .Read(
                    bytes.as_mut_ptr().cast(),
                    bytes.len() as u32,
                    Some(&mut read),
                )
                .ok()?;
            bytes.truncate(read as usize);
            Ok(bytes)
        }
    }

    /// The pictures that the stream sends in turn.
    fn pictures(kind: &Kind) -> Result<Vec<Vec<u8>>> {
        let count = kind.rate;
        (0..count)
            .map(|index| {
                if kind.subtype == MFVideoFormat_MJPG {
                    jpeg(kind, index * (kind.width / 8 / count).max(1))
                } else {
                    Ok(uncompressed(kind, index * (kind.width / 8 / count).max(1)))
                }
            })
            .collect()
    }

    fn sample(
        bytes: &[u8],
        time: i64,
        duration: i64,
        token: Option<&IUnknown>,
    ) -> Result<IMFSample> {
        // SAFETY: the buffer is locked for the copy of at most its length.
        unsafe {
            let buffer = MFCreateMemoryBuffer(bytes.len() as u32)?;
            let mut pointer = std::ptr::null_mut();
            buffer.Lock(&mut pointer, None, None)?;
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer, bytes.len());
            buffer.Unlock()?;
            buffer.SetCurrentLength(bytes.len() as u32)?;
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(time)?;
            sample.SetSampleDuration(duration)?;
            if let Some(token) = token {
                sample.SetUnknown(&MFSampleExtension_Token, token)?;
            }
            Ok(sample)
        }
    }

    /// Sends one sample for each request, each at its time at the rate of the media type.
    fn deliver(
        queue: Free<IMFMediaEventQueue>,
        kind: Kind,
        requests: mpsc::Receiver<Free<Option<IUnknown>>>,
    ) {
        // SAFETY: this thread joins the multithreaded apartment for the imaging component.
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let queue = queue.0;
        let Ok(pictures) = pictures(&kind) else {
            // SAFETY: the queue is free-threaded.
            let _ = unsafe {
                queue.QueueEventParamVar(
                    MEError.0 as u32,
                    &GUID::zeroed(),
                    E_POINTER,
                    std::ptr::null(),
                )
            };
            return;
        };
        let interval = Duration::from_secs(1) / kind.rate;
        let duration = (10_000_000 / kind.rate) as i64;
        let start = Instant::now();
        let mut index = 0u32;
        while let Ok(token) = requests.recv() {
            let due = start + interval * index;
            if let Some(wait) = due.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
            // SAFETY: the time of the system clock in units of 100 nanoseconds.
            let time = unsafe { MFGetSystemTime() };
            let picture = &pictures[index as usize % pictures.len()];
            index += 1;
            let Ok(sample) = sample(picture, time, duration, token.0.as_ref()) else {
                break;
            };
            // SAFETY: the queue is free-threaded and takes its own reference to the sample.
            if unsafe {
                queue.QueueEventParamUnk(
                    MEMediaSample.0 as u32,
                    &GUID::zeroed(),
                    HRESULT(0),
                    &sample,
                )
            }
            .is_err()
            {
                break;
            }
        }
    }

    struct Delivery {
        requests: mpsc::Sender<Free<Option<IUnknown>>>,
        thread: std::thread::JoinHandle<()>,
    }

    struct StreamState {
        state: MF_STREAM_STATE,
        delivery: Option<Delivery>,
        source: Option<IMFMediaSource>,
    }

    #[implement(IMFMediaStream2, IKsControl)]
    struct Stream {
        queue: IMFMediaEventQueue,
        descriptor: IMFStreamDescriptor,
        kinds: Vec<Kind>,
        state: Mutex<StreamState>,
    }

    impl Stream {
        fn lock(&self) -> std::sync::MutexGuard<'_, StreamState> {
            self.state.lock().unwrap_or_else(PoisonError::into_inner)
        }

        /// The kind that the client set on the stream descriptor, or the first.
        fn current(&self) -> Kind {
            // SAFETY: reading the current type of the descriptor's handler.
            let current = unsafe {
                self.descriptor
                    .GetMediaTypeHandler()
                    .and_then(|handler| handler.GetCurrentMediaType())
            };
            let found = current.ok().and_then(|media| {
                // SAFETY: reading attributes of the type.
                let (subtype, size, rate) = unsafe {
                    (
                        media.GetGUID(&MF_MT_SUBTYPE).ok()?,
                        media.GetUINT64(&MF_MT_FRAME_SIZE).ok()?,
                        media.GetUINT64(&MF_MT_FRAME_RATE).ok()?,
                    )
                };
                self.kinds.iter().copied().find(|kind| {
                    kind.subtype == subtype
                        && pair(kind.width, kind.height) == size
                        && pair(kind.rate, 1) == rate
                })
            });
            found.unwrap_or(self.kinds[0])
        }

        fn start(&self, position: *const PROPVARIANT) -> Result<()> {
            let kind = self.current();
            let (requests, receive) = mpsc::channel();
            let queue = Free(self.queue.clone());
            let thread = std::thread::Builder::new()
                .name("virtual-camera".into())
                .spawn(move || deliver(queue, kind, receive))
                .map_err(|_| Error::from(E_OUTOFMEMORY))?;
            let mut state = self.lock();
            if let Some(old) = state.delivery.replace(Delivery { requests, thread }) {
                drop(old.requests);
                let _ = old.thread.join();
            }
            state.state = MF_STREAM_STATE_RUNNING;
            drop(state);
            // SAFETY: the queue copies the start position.
            unsafe {
                self.queue.QueueEventParamVar(
                    MEStreamStarted.0 as u32,
                    &GUID::zeroed(),
                    HRESULT(0),
                    position,
                )
            }
        }

        fn stop(&self) -> Result<()> {
            let delivery = {
                let mut state = self.lock();
                state.state = MF_STREAM_STATE_STOPPED;
                state.delivery.take()
            };
            if let Some(delivery) = delivery {
                drop(delivery.requests);
                let _ = delivery.thread.join();
            }
            // SAFETY: an event without a value.
            unsafe {
                self.queue.QueueEventParamVar(
                    MEStreamStopped.0 as u32,
                    &GUID::zeroed(),
                    HRESULT(0),
                    std::ptr::null(),
                )
            }
        }

        fn shutdown(&self) {
            let delivery = {
                let mut state = self.lock();
                state.source = None;
                state.delivery.take()
            };
            if let Some(delivery) = delivery {
                drop(delivery.requests);
                let _ = delivery.thread.join();
            }
            // SAFETY: shutting down the stream's own queue.
            let _ = unsafe { self.queue.Shutdown() };
        }
    }

    impl IMFMediaEventGenerator_Impl for Stream_Impl {
        fn GetEvent(&self, flags: MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS) -> Result<IMFMediaEvent> {
            // SAFETY: delegated to the event queue.
            unsafe { self.queue.GetEvent(flags.0) }
        }
        fn BeginGetEvent(
            &self,
            callback: Ref<IMFAsyncCallback>,
            state: Ref<IUnknown>,
        ) -> Result<()> {
            // SAFETY: delegated to the event queue.
            unsafe { self.queue.BeginGetEvent(callback.as_ref(), state.as_ref()) }
        }
        fn EndGetEvent(&self, result: Ref<IMFAsyncResult>) -> Result<IMFMediaEvent> {
            // SAFETY: delegated to the event queue.
            unsafe { self.queue.EndGetEvent(result.as_ref()) }
        }
        fn QueueEvent(
            &self,
            met: u32,
            extended: *const GUID,
            status: HRESULT,
            value: *const PROPVARIANT,
        ) -> Result<()> {
            // SAFETY: delegated to the event queue.
            unsafe { self.queue.QueueEventParamVar(met, extended, status, value) }
        }
    }

    impl IMFMediaStream_Impl for Stream_Impl {
        fn GetMediaSource(&self) -> Result<IMFMediaSource> {
            self.lock()
                .source
                .clone()
                .ok_or_else(|| Error::from(MF_E_SHUTDOWN))
        }
        fn GetStreamDescriptor(&self) -> Result<IMFStreamDescriptor> {
            Ok(self.descriptor.clone())
        }
        fn RequestSample(&self, token: Ref<IUnknown>) -> Result<()> {
            let state = self.lock();
            let delivery = state
                .delivery
                .as_ref()
                .ok_or_else(|| Error::from(MF_E_INVALIDREQUEST))?;
            delivery
                .requests
                .send(Free(token.as_ref().cloned()))
                .map_err(|_| Error::from(MF_E_SHUTDOWN))
        }
    }

    impl IMFMediaStream2_Impl for Stream_Impl {
        fn SetStreamState(&self, value: MF_STREAM_STATE) -> Result<()> {
            self.lock().state = value;
            Ok(())
        }
        fn GetStreamState(&self) -> Result<MF_STREAM_STATE> {
            Ok(self.lock().state)
        }
    }

    fn no_property() -> Result<()> {
        Err(Error::from(HRESULT::from_win32(ERROR_SET_NOT_FOUND.0)))
    }

    impl IKsControl_Impl for Stream_Impl {
        fn KsProperty(
            &self,
            _: *const KSIDENTIFIER,
            _: u32,
            _: *mut c_void,
            _: u32,
            _: *mut u32,
        ) -> Result<()> {
            no_property()
        }
        fn KsMethod(
            &self,
            _: *const KSIDENTIFIER,
            _: u32,
            _: *mut c_void,
            _: u32,
            _: *mut u32,
        ) -> Result<()> {
            no_property()
        }
        fn KsEvent(
            &self,
            _: *const KSIDENTIFIER,
            _: u32,
            _: *mut c_void,
            _: u32,
            _: *mut u32,
        ) -> Result<()> {
            no_property()
        }
    }

    #[implement(IMFMediaSourceEx, IMFGetService, IKsControl)]
    struct Source {
        queue: IMFMediaEventQueue,
        attributes: IMFAttributes,
        descriptor: IMFPresentationDescriptor,
        stream: windows::core::ComObject<Stream>,
        started: Mutex<bool>,
    }

    fn source(kinds: Vec<Kind>) -> Result<IMFMediaSourceEx> {
        // SAFETY: every object is created and owned here.
        unsafe {
            let types: Vec<Option<IMFMediaType>> = kinds
                .iter()
                .map(|kind| media_type(kind).map(Some))
                .collect::<Result<_>>()?;
            let stream_descriptor = MFCreateStreamDescriptor(0, &types)?;
            stream_descriptor.SetGUID(&MF_DEVICESTREAM_STREAM_CATEGORY, &PINNAME_VIDEO_CAPTURE)?;
            stream_descriptor.SetUINT32(&MF_DEVICESTREAM_STREAM_ID, 0)?;
            stream_descriptor.SetUINT32(&MF_DEVICESTREAM_FRAMESERVER_SHARED, 1)?;
            stream_descriptor.SetUINT32(
                &MF_DEVICESTREAM_ATTRIBUTE_FRAMESOURCE_TYPES,
                MFFrameSourceTypes_Color.0 as u32,
            )?;
            stream_descriptor
                .GetMediaTypeHandler()?
                .SetCurrentMediaType(types[0].as_ref())?;
            let descriptor =
                MFCreatePresentationDescriptor(Some(&[Some(stream_descriptor.clone())]))?;
            descriptor.SelectStream(0)?;
            let mut attributes = None;
            MFCreateAttributes(&mut attributes, 4)?;
            let attributes = attributes.ok_or_else(|| Error::from(E_OUTOFMEMORY))?;
            let stream = windows::core::ComObject::new(Stream {
                queue: MFCreateEventQueue()?,
                descriptor: stream_descriptor,
                kinds,
                state: Mutex::new(StreamState {
                    state: MF_STREAM_STATE_STOPPED,
                    delivery: None,
                    source: None,
                }),
            });
            let source = windows::core::ComObject::new(Source {
                queue: MFCreateEventQueue()?,
                attributes,
                descriptor,
                stream: stream.clone(),
                started: Mutex::new(false),
            });
            let source: IMFMediaSourceEx = source.to_interface();
            stream.lock().source = Some(source.cast()?);
            Ok(source)
        }
    }

    impl IMFMediaEventGenerator_Impl for Source_Impl {
        fn GetEvent(&self, flags: MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS) -> Result<IMFMediaEvent> {
            // SAFETY: delegated to the event queue.
            unsafe { self.queue.GetEvent(flags.0) }
        }
        fn BeginGetEvent(
            &self,
            callback: Ref<IMFAsyncCallback>,
            state: Ref<IUnknown>,
        ) -> Result<()> {
            // SAFETY: delegated to the event queue.
            unsafe { self.queue.BeginGetEvent(callback.as_ref(), state.as_ref()) }
        }
        fn EndGetEvent(&self, result: Ref<IMFAsyncResult>) -> Result<IMFMediaEvent> {
            // SAFETY: delegated to the event queue.
            unsafe { self.queue.EndGetEvent(result.as_ref()) }
        }
        fn QueueEvent(
            &self,
            met: u32,
            extended: *const GUID,
            status: HRESULT,
            value: *const PROPVARIANT,
        ) -> Result<()> {
            // SAFETY: delegated to the event queue.
            unsafe { self.queue.QueueEventParamVar(met, extended, status, value) }
        }
    }

    impl IMFMediaSource_Impl for Source_Impl {
        fn GetCharacteristics(&self) -> Result<u32> {
            Ok(MFMEDIASOURCE_IS_LIVE.0 as u32)
        }
        fn CreatePresentationDescriptor(&self) -> Result<IMFPresentationDescriptor> {
            // SAFETY: a copy of the source's descriptor.
            unsafe { self.descriptor.Clone() }
        }
        fn Start(
            &self,
            descriptor: Ref<IMFPresentationDescriptor>,
            _: *const GUID,
            position: *const PROPVARIANT,
        ) -> Result<()> {
            let descriptor = descriptor.ok()?;
            let mut selected = BOOL(0);
            let mut stream_descriptor = None;
            // SAFETY: out parameters of the call.
            unsafe {
                descriptor.GetStreamDescriptorByIndex(0, &mut selected, &mut stream_descriptor)?
            };
            if !selected.as_bool() {
                return Err(Error::from(MF_E_INVALIDREQUEST));
            }
            let first = !std::mem::replace(
                &mut *self.started.lock().unwrap_or_else(PoisonError::into_inner),
                true,
            );
            let stream: IUnknown = self.stream.to_interface();
            let event = if first { MENewStream } else { MEUpdatedStream };
            // SAFETY: the queue takes its own reference to the stream.
            unsafe {
                self.queue.QueueEventParamUnk(
                    event.0 as u32,
                    &GUID::zeroed(),
                    HRESULT(0),
                    &stream,
                )?
            };
            self.stream.start(position)?;
            // SAFETY: the queue copies the start position.
            unsafe {
                self.queue.QueueEventParamVar(
                    MESourceStarted.0 as u32,
                    &GUID::zeroed(),
                    HRESULT(0),
                    position,
                )
            }
        }
        fn Stop(&self) -> Result<()> {
            self.stream.stop()?;
            // SAFETY: an event without a value.
            unsafe {
                self.queue.QueueEventParamVar(
                    MESourceStopped.0 as u32,
                    &GUID::zeroed(),
                    HRESULT(0),
                    std::ptr::null(),
                )
            }
        }
        fn Pause(&self) -> Result<()> {
            Err(Error::from(MF_E_INVALID_STATE_TRANSITION))
        }
        fn Shutdown(&self) -> Result<()> {
            self.stream.shutdown();
            // SAFETY: shutting down the source's own queue.
            unsafe { self.queue.Shutdown() }
        }
    }

    impl IMFMediaSourceEx_Impl for Source_Impl {
        fn GetSourceAttributes(&self) -> Result<IMFAttributes> {
            Ok(self.attributes.clone())
        }
        fn GetStreamAttributes(&self, identifier: u32) -> Result<IMFAttributes> {
            if identifier != 0 {
                return Err(Error::from(MF_E_INVALIDSTREAMNUMBER));
            }
            self.stream.descriptor.cast()
        }
        fn SetD3DManager(&self, _: Ref<IUnknown>) -> Result<()> {
            Ok(())
        }
    }

    impl IMFGetService_Impl for Source_Impl {
        fn GetService(&self, _: *const GUID, _: *const GUID, _: *mut *mut c_void) -> Result<()> {
            Err(Error::from(MF_E_UNSUPPORTED_SERVICE))
        }
    }

    impl IKsControl_Impl for Source_Impl {
        fn KsProperty(
            &self,
            _: *const KSIDENTIFIER,
            _: u32,
            _: *mut c_void,
            _: u32,
            _: *mut u32,
        ) -> Result<()> {
            no_property()
        }
        fn KsMethod(
            &self,
            _: *const KSIDENTIFIER,
            _: u32,
            _: *mut c_void,
            _: u32,
            _: *mut u32,
        ) -> Result<()> {
            no_property()
        }
        fn KsEvent(
            &self,
            _: *const KSIDENTIFIER,
            _: u32,
            _: *mut c_void,
            _: u32,
            _: *mut u32,
        ) -> Result<()> {
            no_property()
        }
    }

    /// The object that the Frame Server creates from the class identifier; its attributes are
    /// an attribute store of Media Foundation.
    #[implement(IMFActivate)]
    struct Activate {
        attributes: IMFAttributes,
        kinds: Vec<Kind>,
        source: Mutex<Option<IMFMediaSourceEx>>,
    }

    impl IMFActivate_Impl for Activate_Impl {
        fn ActivateObject(&self, riid: *const GUID, ppv: *mut *mut c_void) -> Result<()> {
            let mut held = self.source.lock().unwrap_or_else(PoisonError::into_inner);
            if held.is_none() {
                *held = Some(source(self.kinds.clone())?);
            }
            let source = held.as_ref().ok_or_else(|| Error::from(E_POINTER))?;
            // SAFETY: the caller gives a valid interface identifier and out parameter.
            unsafe { source.query(riid, ppv).ok() }
        }
        fn ShutdownObject(&self) -> Result<()> {
            if let Some(source) = self
                .source
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take()
            {
                // SAFETY: the source ends its stream and queues.
                unsafe { source.Shutdown()? };
            }
            Ok(())
        }
        fn DetachObject(&self) -> Result<()> {
            self.source
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
            Ok(())
        }
    }

    // SAFETY (every method): the arguments come from the caller of the COM method and are
    // passed on unchanged to the attribute store, which states the same contract.
    impl IMFAttributes_Impl for Activate_Impl {
        fn GetItem(&self, key: *const GUID, value: *mut PROPVARIANT) -> Result<()> {
            unsafe {
                self.attributes
                    .GetItem(key, (!value.is_null()).then_some(value))
            }
        }
        fn GetItemType(&self, key: *const GUID) -> Result<MF_ATTRIBUTE_TYPE> {
            unsafe { self.attributes.GetItemType(key) }
        }
        fn CompareItem(&self, key: *const GUID, value: *const PROPVARIANT) -> Result<BOOL> {
            unsafe { self.attributes.CompareItem(key, value) }
        }
        fn Compare(
            &self,
            theirs: Ref<IMFAttributes>,
            kind: MF_ATTRIBUTES_MATCH_TYPE,
        ) -> Result<BOOL> {
            unsafe { self.attributes.Compare(theirs.as_ref(), kind) }
        }
        fn GetUINT32(&self, key: *const GUID) -> Result<u32> {
            unsafe { self.attributes.GetUINT32(key) }
        }
        fn GetUINT64(&self, key: *const GUID) -> Result<u64> {
            unsafe { self.attributes.GetUINT64(key) }
        }
        fn GetDouble(&self, key: *const GUID) -> Result<f64> {
            unsafe { self.attributes.GetDouble(key) }
        }
        fn GetGUID(&self, key: *const GUID) -> Result<GUID> {
            unsafe { self.attributes.GetGUID(key) }
        }
        fn GetStringLength(&self, key: *const GUID) -> Result<u32> {
            unsafe { self.attributes.GetStringLength(key) }
        }
        fn GetString(
            &self,
            key: *const GUID,
            value: PWSTR,
            size: u32,
            length: *mut u32,
        ) -> Result<()> {
            unsafe {
                let buffer = std::slice::from_raw_parts_mut(value.0, size as usize);
                self.attributes
                    .GetString(key, buffer, (!length.is_null()).then_some(length))
            }
        }
        fn GetAllocatedString(
            &self,
            key: *const GUID,
            value: *mut PWSTR,
            length: *mut u32,
        ) -> Result<()> {
            unsafe { self.attributes.GetAllocatedString(key, value, length) }
        }
        fn GetBlobSize(&self, key: *const GUID) -> Result<u32> {
            unsafe { self.attributes.GetBlobSize(key) }
        }
        fn GetBlob(
            &self,
            key: *const GUID,
            buffer: *mut u8,
            size: u32,
            written: *mut u32,
        ) -> Result<()> {
            unsafe {
                let buffer = std::slice::from_raw_parts_mut(buffer, size as usize);
                self.attributes
                    .GetBlob(key, buffer, (!written.is_null()).then_some(written))
            }
        }
        fn GetAllocatedBlob(
            &self,
            key: *const GUID,
            buffer: *mut *mut u8,
            size: *mut u32,
        ) -> Result<()> {
            unsafe { self.attributes.GetAllocatedBlob(key, buffer, size) }
        }
        fn GetUnknown(
            &self,
            key: *const GUID,
            riid: *const GUID,
            ppv: *mut *mut c_void,
        ) -> Result<()> {
            unsafe {
                (Interface::vtable(&self.attributes).GetUnknown)(
                    Interface::as_raw(&self.attributes),
                    key,
                    riid,
                    ppv,
                )
                .ok()
            }
        }
        fn SetItem(&self, key: *const GUID, value: *const PROPVARIANT) -> Result<()> {
            unsafe { self.attributes.SetItem(key, value) }
        }
        fn DeleteItem(&self, key: *const GUID) -> Result<()> {
            unsafe { self.attributes.DeleteItem(key) }
        }
        fn DeleteAllItems(&self) -> Result<()> {
            unsafe { self.attributes.DeleteAllItems() }
        }
        fn SetUINT32(&self, key: *const GUID, value: u32) -> Result<()> {
            unsafe { self.attributes.SetUINT32(key, value) }
        }
        fn SetUINT64(&self, key: *const GUID, value: u64) -> Result<()> {
            unsafe { self.attributes.SetUINT64(key, value) }
        }
        fn SetDouble(&self, key: *const GUID, value: f64) -> Result<()> {
            unsafe { self.attributes.SetDouble(key, value) }
        }
        fn SetGUID(&self, key: *const GUID, value: *const GUID) -> Result<()> {
            unsafe { self.attributes.SetGUID(key, value) }
        }
        fn SetString(&self, key: *const GUID, value: &PCWSTR) -> Result<()> {
            unsafe { self.attributes.SetString(key, *value) }
        }
        fn SetBlob(&self, key: *const GUID, buffer: *const u8, size: u32) -> Result<()> {
            unsafe {
                self.attributes
                    .SetBlob(key, std::slice::from_raw_parts(buffer, size as usize))
            }
        }
        fn SetUnknown(&self, key: *const GUID, value: Ref<IUnknown>) -> Result<()> {
            unsafe { self.attributes.SetUnknown(key, value.as_ref()) }
        }
        fn LockStore(&self) -> Result<()> {
            unsafe { self.attributes.LockStore() }
        }
        fn UnlockStore(&self) -> Result<()> {
            unsafe { self.attributes.UnlockStore() }
        }
        fn GetCount(&self) -> Result<u32> {
            unsafe { self.attributes.GetCount() }
        }
        fn GetItemByIndex(
            &self,
            index: u32,
            key: *mut GUID,
            value: *mut PROPVARIANT,
        ) -> Result<()> {
            unsafe {
                self.attributes
                    .GetItemByIndex(index, key, (!value.is_null()).then_some(value))
            }
        }
        fn CopyAllItems(&self, destination: Ref<IMFAttributes>) -> Result<()> {
            unsafe { self.attributes.CopyAllItems(destination.as_ref()) }
        }
    }

    #[implement(IClassFactory)]
    struct Factory(Vec<Kind>);

    impl IClassFactory_Impl for Factory_Impl {
        fn CreateInstance(
            &self,
            outer: Ref<IUnknown>,
            riid: *const GUID,
            ppv: *mut *mut c_void,
        ) -> Result<()> {
            if outer.is_some() {
                return Err(Error::from(CLASS_E_NOAGGREGATION));
            }
            let mut attributes = None;
            // SAFETY: an out parameter; the store is owned by the new object.
            unsafe { MFCreateAttributes(&mut attributes, 4)? };
            let activate: IMFActivate = Activate {
                attributes: attributes.ok_or_else(|| Error::from(E_OUTOFMEMORY))?,
                kinds: self.0.clone(),
                source: Mutex::new(None),
            }
            .into();
            // SAFETY: the caller gives a valid interface identifier and out parameter.
            unsafe { activate.query(riid, ppv).ok() }
        }
        fn LockServer(&self, _: BOOL) -> Result<()> {
            Ok(())
        }
    }

    /// The entry of a COM server: the class factory of one of the two sources.
    ///
    /// # Safety
    ///
    /// COM calls it with valid pointers.
    #[unsafe(no_mangle)]
    pub unsafe extern "system" fn DllGetClassObject(
        class: *const GUID,
        riid: *const GUID,
        ppv: *mut *mut c_void,
    ) -> HRESULT {
        if class.is_null() || riid.is_null() || ppv.is_null() {
            return E_POINTER;
        }
        // SAFETY: the pointers were checked above.
        unsafe {
            *ppv = std::ptr::null_mut();
            let Some(kinds) = kinds(&*class) else {
                return CLASS_E_CLASSNOTAVAILABLE;
            };
            if let Err(error) = MFStartup(MF_VERSION, MFSTARTUP_LITE) {
                return error.code();
            }
            let factory: IClassFactory = Factory(kinds).into();
            factory.query(riid, ppv)
        }
    }

    /// The library stays loaded: a source can outlive the last reference that COM counts.
    #[unsafe(no_mangle)]
    pub extern "system" fn DllCanUnloadNow() -> HRESULT {
        S_FALSE
    }
}
