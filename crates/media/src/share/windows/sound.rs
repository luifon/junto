//! The sound of a share with process loopback of the Windows audio session API (ADR 0706).
//!
//! Process loopback captures the sound that one process and its child processes play, or
//! all sound except that of one process and its child processes. The capture of one
//! application includes the tree of its process; all system sound excludes the tree of the
//! client's process, which plays the sound of a call.

use std::mem::ManuallyDrop;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK, AUDIOCLIENT_ACTIVATION_PARAMS,
    AUDIOCLIENT_ACTIVATION_PARAMS_0, AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
    AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS, ActivateAudioInterfaceAsync,
    IActivateAudioInterfaceAsyncOperation, IActivateAudioInterfaceCompletionHandler,
    IActivateAudioInterfaceCompletionHandler_Impl, IAudioCaptureClient, IAudioClient,
    PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE,
    PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE, VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
    WAVEFORMATEX,
};
use windows::Win32::Media::Multimedia::WAVE_FORMAT_IEEE_FLOAT;
use windows::Win32::System::Com::BLOB;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::Win32::System::Variant::VT_BLOB;
use windows::core::{HRESULT, IUnknown, Interface, Ref, implement};

use super::rules::{SOUND_CHANNELS, SOUND_RATE, packet_frames};
use crate::share::CaptureError;
use crate::share::delivery::Life;

/// How long the system has to activate the capture.
const ACTIVATION_LIMIT: Duration = Duration::from_secs(10);
/// The buffer of the capture at the system, in units of 100 nanoseconds: 200 ms.
const BUFFER: i64 = 2_000_000;
/// How long the capture thread waits for a packet before it looks at the state of the
/// capture.
const LOOK: u32 = 100;

fn failed(error: windows::core::Error) -> CaptureError {
    CaptureError::Failed(error.message())
}

/// Whose sound the capture takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Loopback {
    /// One process and its child processes.
    Include(u32),
    /// Every process except one and its child processes.
    Exclude(u32),
}

#[implement(IActivateAudioInterfaceCompletionHandler)]
struct Activated(mpsc::SyncSender<()>);

impl IActivateAudioInterfaceCompletionHandler_Impl for Activated_Impl {
    fn ActivateCompleted(
        &self,
        _: Ref<IActivateAudioInterfaceAsyncOperation>,
    ) -> windows::core::Result<()> {
        let _ = self.0.try_send(());
        Ok(())
    }
}

/// An event of the system, closed when dropped.
struct Event(HANDLE);

impl Drop for Event {
    fn drop(&mut self) {
        // SAFETY: the handle was created by `CreateEventW` and is closed once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// A started capture of sound.
pub(super) struct Capture {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    event: Event,
}

impl Capture {
    /// Activates and starts the capture. The calling thread is in the multithreaded
    /// apartment.
    pub(super) fn start(loopback: Loopback) -> Result<Self, CaptureError> {
        let (process_id, mode) = match loopback {
            Loopback::Include(process) => {
                (process, PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE)
            }
            Loopback::Exclude(process) => {
                (process, PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE)
            }
        };
        let parameters = AUDIOCLIENT_ACTIVATION_PARAMS {
            ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
            Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                    TargetProcessId: process_id,
                    ProcessLoopbackMode: mode,
                },
            },
        };
        // The drop of a variant clears it, which frees the memory of a blob. This blob
        // points to the parameters on the stack, so the variant is never dropped.
        let mut variant = ManuallyDrop::new(PROPVARIANT::default());
        // SAFETY: the variant is a blob that points to the parameters, which outlive the
        // activation; the system copies them during the call.
        unsafe {
            let inner = &mut *variant.Anonymous.Anonymous;
            inner.vt = VT_BLOB;
            inner.Anonymous.blob = BLOB {
                cbSize: std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
                pBlobData: (&parameters as *const AUDIOCLIENT_ACTIVATION_PARAMS)
                    .cast_mut()
                    .cast(),
            };
        }
        let (done, completed) = mpsc::sync_channel(1);
        let handler: IActivateAudioInterfaceCompletionHandler = Activated(done).into();
        // SAFETY: the variant and the handler outlive the call; the system calls the handler
        // once, on a thread of its own.
        let operation = unsafe {
            ActivateAudioInterfaceAsync(
                VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
                &IAudioClient::IID,
                Some(&*variant),
                &handler,
            )
        }
        .map_err(failed)?;
        completed.recv_timeout(ACTIVATION_LIMIT).map_err(|_| {
            CaptureError::Failed("the system did not activate the sound capture".to_owned())
        })?;
        let mut result = HRESULT(0);
        let mut activated: Option<IUnknown> = None;
        // SAFETY: the activation has completed; the call writes its result and interface.
        unsafe { operation.GetActivateResult(&mut result, &mut activated) }.map_err(failed)?;
        result.ok().map_err(failed)?;
        let client: IAudioClient = activated
            .ok_or_else(|| CaptureError::Failed("no sound capture".to_owned()))?
            .cast()
            .map_err(failed)?;

        let block_align = (SOUND_CHANNELS * 4) as u16;
        let format = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_IEEE_FLOAT as u16,
            nChannels: SOUND_CHANNELS as u16,
            nSamplesPerSec: SOUND_RATE,
            nAvgBytesPerSec: SOUND_RATE * u32::from(block_align),
            nBlockAlign: block_align,
            wBitsPerSample: 32,
            cbSize: 0,
        };
        // SAFETY: the format is complete and outlives the call. The event handle is valid
        // until `Event` closes it, after the client has stopped.
        unsafe {
            client
                .Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    AUDCLNT_STREAMFLAGS_LOOPBACK
                        | AUDCLNT_STREAMFLAGS_EVENTCALLBACK
                        | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
                    BUFFER,
                    0,
                    &format,
                    None,
                )
                .map_err(failed)?;
            let event = Event(CreateEventW(None, false, false, None).map_err(failed)?);
            client.SetEventHandle(event.0).map_err(failed)?;
            let capture: IAudioCaptureClient = client.GetService().map_err(failed)?;
            client.Start().map_err(failed)?;
            Ok(Self {
                client,
                capture,
                event,
            })
        }
    }

    /// Gives the frames of the capture to `push` until the capture stops. Returns the
    /// failure that stopped it.
    pub(super) fn run(
        &self,
        life: &Arc<Life>,
        mut push: impl FnMut([f32; 2]),
    ) -> Result<(), CaptureError> {
        while !life.stopped() {
            // SAFETY: the event handle is valid while `self` lives.
            if unsafe { WaitForSingleObject(self.event.0, LOOK) } != WAIT_OBJECT_0 {
                continue;
            }
            loop {
                // SAFETY: each buffer that `GetBuffer` gives is read before `ReleaseBuffer`
                // returns it, and holds `frames` frames of the format of the client.
                unsafe {
                    if self.capture.GetNextPacketSize().map_err(failed)? == 0 {
                        break;
                    }
                    let mut data = std::ptr::null_mut();
                    let mut frames = 0u32;
                    let mut flags = 0u32;
                    self.capture
                        .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
                        .map_err(failed)?;
                    let length = frames as usize * SOUND_CHANNELS * 4;
                    let bytes = (!data.is_null()).then(|| std::slice::from_raw_parts(data, length));
                    let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
                    let read = packet_frames(bytes, frames as usize, silent, &mut push);
                    self.capture.ReleaseBuffer(frames).map_err(failed)?;
                    if !read {
                        return Err(CaptureError::Failed(
                            "the system gave sound in another form than the one asked for"
                                .to_owned(),
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        // SAFETY: the client was started; stopping a stopped client changes nothing.
        let _ = unsafe { self.client.Stop() };
    }
}
