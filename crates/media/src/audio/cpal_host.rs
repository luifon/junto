//! The sound devices of the machine, through `cpal`: CoreAudio on macOS, WASAPI on Windows,
//! and PipeWire on Linux (ADR 0733). On Linux the client plays the sound of a call through
//! its own PipeWire connection, so that a capture of system sound can leave it out
//! (ADR 0502).

use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};

use super::host::{
    AudioError, AudioHost, Deliver, DeviceId, DeviceInfo, Devices, Failed, Fill, Format, OpenStream,
};
use super::rate;

/// How long the system has to start a stream.
const OPEN_TIMEOUT: Duration = Duration::from_secs(5);

#[cfg(test)]
thread_local! {
    pub(super) static HOSTS_CREATED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

pub struct CpalHost {
    host: Result<cpal::Host, AudioError>,
}

impl Default for CpalHost {
    fn default() -> Self {
        Self::new()
    }
}

impl CpalHost {
    pub fn new() -> Self {
        #[cfg(test)]
        HOSTS_CREATED.with(|count| count.set(count.get() + 1));
        Self { host: host() }
    }

    fn host(&self) -> Result<&cpal::Host, AudioError> {
        self.host.as_ref().map_err(Clone::clone)
    }

    fn device(&self, chosen: Option<&DeviceId>, input: bool) -> Result<cpal::Device, AudioError> {
        let device = match chosen {
            Some(id) => {
                let id: cpal::DeviceId = id.0.parse().map_err(|_| AudioError::NoDevice)?;
                self.host()?.device_by_id(&id)
            }
            None if input => self.host()?.default_input_device(),
            None => self.host()?.default_output_device(),
        };
        device.ok_or(AudioError::NoDevice)
    }

    fn info(&self, device: &cpal::Device, input: bool) -> Result<DeviceInfo, AudioError> {
        let id = device.id().map_err(error_of)?;
        let default = if input {
            self.host()?.default_input_device()
        } else {
            self.host()?.default_output_device()
        };
        Ok(DeviceInfo {
            name: device.description().map_or_else(
                |_| device.to_string(),
                |description| description.name().to_owned(),
            ),
            is_default: default.and_then(|default| default.id().ok()).as_ref() == Some(&id),
            id: DeviceId(id.to_string()),
        })
    }
}

#[cfg(target_os = "linux")]
fn host() -> Result<cpal::Host, AudioError> {
    super::linux::host()
}

#[cfg(not(target_os = "linux"))]
fn host() -> Result<cpal::Host, AudioError> {
    Ok(cpal::default_host())
}

fn error_of(error: cpal::Error) -> AudioError {
    match error.kind() {
        cpal::ErrorKind::DeviceNotAvailable => AudioError::NoDevice,
        cpal::ErrorKind::PermissionDenied => AudioError::Refused,
        cpal::ErrorKind::UnsupportedConfig => AudioError::Unsupported(error.to_string()),
        _ => AudioError::Failed(error.to_string()),
    }
}

/// A fault that does not end a stream: the sound had a gap, or the system did not give
/// the thread of the stream its priority. For an output stream also a change of the
/// default output of the system: the stream follows it by itself and goes on. An input
/// stream that reports another device has failed: the microphone of a call is not
/// replaced without the person (ADR 0794).
fn passes(error: &cpal::Error, output: bool) -> bool {
    match error.kind() {
        cpal::ErrorKind::Xrun | cpal::ErrorKind::RealtimeDenied => true,
        cpal::ErrorKind::DeviceChanged => output,
        _ => false,
    }
}

fn report(failed: Shared, output: bool) -> impl FnMut(cpal::Error) + Send + 'static {
    move |error| {
        if passes(&error, output) {
            tracing::debug!(%error, "a sound stream had a fault that passes");
        } else {
            tell(&failed, error_of(error));
        }
    }
}

/// The failure callback of a stream, which its error callback and its data callback share.
type Shared = std::sync::Arc<std::sync::Mutex<Failed>>;

fn tell(failed: &Shared, error: AudioError) {
    (failed
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner))(error);
}

/// The data callback of a stream runs on a thread of the system (a CoreAudio callback on
/// macOS). After a panic in it, the stream delivers nothing more and is reported as failed
/// (ADR 0757).
fn panicked() -> AudioError {
    AudioError::Failed("the sound callback panicked".into())
}

/// The format that a stream of a device is opened with: the default of the device, or
/// another one that the device offers when the pipeline does not convert the default rate.
/// The channels and the sample type of the default are kept where the device offers them
/// at the new rate.
fn usable_config(
    default: cpal::SupportedStreamConfig,
    offered: &[cpal::SupportedStreamConfigRange],
) -> Result<cpal::SupportedStreamConfig, AudioError> {
    if rate::converts(default.sample_rate()) {
        return Ok(default);
    }
    rate::PREFERRED_RATES
        .iter()
        .find_map(|rate| {
            offered
                .iter()
                .filter(|range| (range.min_sample_rate()..=range.max_sample_rate()).contains(rate))
                .max_by_key(|range| {
                    (
                        range.channels() == default.channels(),
                        range.sample_format() == default.sample_format(),
                    )
                })
                .and_then(|range| (*range).try_with_sample_rate(*rate))
        })
        .ok_or_else(|| AudioError::Unsupported(format!("{} Hz", default.sample_rate())))
}

fn input_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut deliver: Deliver,
    failed: Failed,
) -> Result<cpal::Stream, cpal::Error>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let format = Format {
        sample_rate: config.sample_rate,
        channels: config.channels,
    };
    let mut converted: Vec<f32> = Vec::new();
    let failed: Shared = std::sync::Arc::new(std::sync::Mutex::new(failed));
    let told = failed.clone();
    let mut broken = false;
    device.build_input_stream(
        config,
        move |samples: &[T], _: &cpal::InputCallbackInfo| {
            if broken {
                return;
            }
            broken = crate::callback::stop_panic(
                "microphone",
                || true,
                || {
                    converted.clear();
                    converted.extend(samples.iter().map(|sample| f32::from_sample(*sample)));
                    deliver(&converted, format);
                    false
                },
            );
            if broken {
                crate::callback::after_a_panic("microphone failure", || tell(&told, panicked()));
            }
        },
        report(failed, false),
        Some(OPEN_TIMEOUT),
    )
}

fn output_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut fill: Fill,
    failed: Failed,
) -> Result<cpal::Stream, cpal::Error>
where
    T: SizedSample + FromSample<f32>,
{
    let format = Format {
        sample_rate: config.sample_rate,
        channels: config.channels,
    };
    let mut filled: Vec<f32> = Vec::new();
    let failed: Shared = std::sync::Arc::new(std::sync::Mutex::new(failed));
    let told = failed.clone();
    let mut broken = false;
    device.build_output_stream(
        config,
        move |samples: &mut [T], _: &cpal::OutputCallbackInfo| {
            if !broken {
                broken = crate::callback::stop_panic(
                    "sound output",
                    || true,
                    || {
                        filled.clear();
                        filled.resize(samples.len(), 0.0);
                        fill(&mut filled, format);
                        false
                    },
                );
                if broken {
                    crate::callback::after_a_panic("sound output failure", || {
                        tell(&told, panicked())
                    });
                }
            }
            if broken {
                filled.clear();
                filled.resize(samples.len(), 0.0);
            }
            for (sample, value) in samples.iter_mut().zip(&filled) {
                *sample = T::from_sample(*value);
            }
        },
        report(failed, true),
        Some(OPEN_TIMEOUT),
    )
}

impl AudioHost for CpalHost {
    fn devices(&self) -> Result<Devices, AudioError> {
        let list = |input: bool| -> Result<Vec<DeviceInfo>, AudioError> {
            let devices: Vec<cpal::Device> = if input {
                self.host()?.input_devices().map_err(error_of)?.collect()
            } else {
                self.host()?.output_devices().map_err(error_of)?.collect()
            };
            Ok(devices
                .iter()
                .filter_map(|device| self.info(device, input).ok())
                .collect())
        };
        Ok(Devices {
            inputs: list(true)?,
            outputs: list(false)?,
        })
    }

    fn open_input(
        &self,
        device: Option<&DeviceId>,
        deliver: Deliver,
        failed: Failed,
    ) -> Result<OpenStream, AudioError> {
        let device = self.device(device, true)?;
        let info = self.info(&device, true)?;
        let default = device.default_input_config().map_err(error_of)?;
        let offered: Vec<_> = if rate::converts(default.sample_rate()) {
            Vec::new()
        } else {
            device
                .supported_input_configs()
                .map_err(error_of)?
                .collect()
        };
        let supported = usable_config(default, &offered)?;
        let config = supported.config();
        let stream = match supported.sample_format() {
            SampleFormat::F32 => input_stream::<f32>(&device, config, deliver, failed),
            SampleFormat::I16 => input_stream::<i16>(&device, config, deliver, failed),
            SampleFormat::U16 => input_stream::<u16>(&device, config, deliver, failed),
            SampleFormat::I32 => input_stream::<i32>(&device, config, deliver, failed),
            SampleFormat::I8 => input_stream::<i8>(&device, config, deliver, failed),
            SampleFormat::U8 => input_stream::<u8>(&device, config, deliver, failed),
            other => return Err(AudioError::Unsupported(other.to_string())),
        }
        .map_err(error_of)?;
        stream.play().map_err(error_of)?;
        Ok(OpenStream::new(info, stream))
    }

    fn open_output(
        &self,
        device: Option<&DeviceId>,
        fill: Fill,
        failed: Failed,
    ) -> Result<OpenStream, AudioError> {
        let device = self.device(device, false)?;
        let info = self.info(&device, false)?;
        let default = device.default_output_config().map_err(error_of)?;
        let offered: Vec<_> = if rate::converts(default.sample_rate()) {
            Vec::new()
        } else {
            device
                .supported_output_configs()
                .map_err(error_of)?
                .collect()
        };
        let supported = usable_config(default, &offered)?;
        let config = supported.config();
        let stream = match supported.sample_format() {
            SampleFormat::F32 => output_stream::<f32>(&device, config, fill, failed),
            SampleFormat::I16 => output_stream::<i16>(&device, config, fill, failed),
            SampleFormat::U16 => output_stream::<u16>(&device, config, fill, failed),
            SampleFormat::I32 => output_stream::<i32>(&device, config, fill, failed),
            SampleFormat::I8 => output_stream::<i8>(&device, config, fill, failed),
            SampleFormat::U8 => output_stream::<u8>(&device, config, fill, failed),
            other => return Err(AudioError::Unsupported(other.to_string())),
        }
        .map_err(error_of)?;
        stream.play().map_err(error_of)?;
        Ok(OpenStream::new(info, stream))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads the device lists of the machine. No device is opened: the test uses no
    /// microphone and plays no sound, and passes on a machine without sound devices.
    #[test]
    fn the_devices_of_the_machine_are_listed_with_at_most_one_default_of_each_kind() {
        let host = CpalHost::new();
        let devices = match host.devices() {
            Ok(devices) => devices,
            // A machine without a sound system.
            Err(AudioError::Failed(_) | AudioError::NoDevice) => return,
            Err(other) => panic!("{other}"),
        };
        for list in [&devices.inputs, &devices.outputs] {
            assert!(list.iter().filter(|device| device.is_default).count() <= 1);
            for device in list {
                assert!(!device.id.0.is_empty());
                // The identifier is the one that opens the device again.
                assert!(
                    device.id.0.parse::<cpal::DeviceId>().is_ok(),
                    "{}",
                    device.id
                );
            }
        }
    }

    /// The calls of the host leave the thread of their caller in the apartment of COM that it
    /// had, and the objects that `cpal` keeps serve a later call from another thread, after
    /// the thread that made them has ended (ADR 0733).
    #[cfg(target_os = "windows")]
    #[test]
    fn a_call_leaves_the_apartment_of_the_calling_thread_as_it_was() {
        use crate::com::apartment;
        let host = std::sync::Arc::new(CpalHost::new());
        let mut lists = Vec::new();
        for _ in 0..2 {
            let host = host.clone();
            lists.push(
                std::thread::spawn(move || {
                    assert!(!apartment().entered());
                    let devices = host.devices();
                    assert!(!apartment().entered(), "the thread is in {:?}", apartment());
                    devices
                })
                .join()
                .unwrap(),
            );
        }
        assert_eq!(lists[0], lists[1]);
    }

    #[test]
    fn a_changed_default_output_is_not_a_failure_of_the_stream() {
        let changed = cpal::Error::new(cpal::ErrorKind::DeviceChanged);
        assert!(passes(&changed, true));
        assert!(!passes(&changed, false));
        let gone = cpal::Error::new(cpal::ErrorKind::DeviceNotAvailable);
        assert!(!passes(&gone, true) && !passes(&gone, false));
        let gap = cpal::Error::new(cpal::ErrorKind::Xrun);
        assert!(passes(&gap, true) && passes(&gap, false));
    }

    #[test]
    fn a_device_is_opened_at_a_rate_that_the_pipeline_converts() {
        use cpal::{SupportedBufferSize, SupportedStreamConfig, SupportedStreamConfigRange};
        let size = SupportedBufferSize::Unknown;
        let default = |rate| SupportedStreamConfig::new(2, rate, size, SampleFormat::F32);
        let range = |channels, low, high, format| {
            SupportedStreamConfigRange::new(channels, low, high, size, format)
        };
        // The default of the device is kept when the pipeline converts it.
        let kept = usable_config(default(44_100), &[]).unwrap();
        assert_eq!((kept.sample_rate(), kept.channels()), (44_100, 2));
        let kept = usable_config(default(22_050), &[]).unwrap();
        assert_eq!(kept.sample_rate(), 22_050);

        // A default of 384 kHz: the best other rate that the device offers, with the
        // channels and the sample type of the default where they are offered.
        let offered = [
            range(1, 8_000, 96_000, SampleFormat::I16),
            range(2, 44_100, 44_100, SampleFormat::F32),
            range(2, 48_000, 48_000, SampleFormat::I16),
            range(2, 48_000, 384_000, SampleFormat::F32),
        ];
        let chosen = usable_config(default(384_000), &offered).unwrap();
        assert_eq!(
            (
                chosen.sample_rate(),
                chosen.channels(),
                chosen.sample_format()
            ),
            (48_000, 2, SampleFormat::F32)
        );
        let chosen = usable_config(default(384_000), &offered[..2]).unwrap();
        assert_eq!((chosen.sample_rate(), chosen.channels()), (48_000, 1));

        // A device that offers no such rate is not opened.
        let only = [range(2, 352_800, 384_000, SampleFormat::F32)];
        assert_eq!(
            usable_config(default(384_000), &only).unwrap_err(),
            AudioError::Unsupported("384000 Hz".to_owned())
        );
    }
}
