use junto_media::audio::Format;
use junto_media::audio::synthetic::tone_level;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;

#[derive(Default)]
struct Samples {
    data: Vec<f32>,
    format: Option<Format>,
    frames: u64,
    failure: Option<String>,
}

#[derive(Default)]
pub struct Probe {
    samples: Mutex<Samples>,
    changed: Notify,
}

impl Probe {
    pub fn frames(&self) -> u64 {
        self.samples.lock().unwrap().frames
    }
    pub fn record(&self, data: &[f32], format: Format) {
        let mut held = self.samples.lock().unwrap();
        if format.channels == 0
            || format.sample_rate == 0
            || !data.len().is_multiple_of(usize::from(format.channels))
        {
            held.failure = Some("invalid callback format".into());
        } else if held.format.is_some_and(|before| before != format) {
            held.failure = Some("format changed during measurement".into());
        } else {
            held.format = Some(format);
            held.frames += (data.len() / usize::from(format.channels)) as u64;
            let remaining = (format.sample_rate as usize * 6).saturating_sub(held.data.len());
            held.data.extend(
                data.chunks_exact(usize::from(format.channels))
                    .take(remaining)
                    .map(|frame| frame[0]),
            );
        }
        self.changed.notify_one();
    }

    pub async fn measure(&self, label: &str, present: &[f32], absent: &[f32]) -> Vec<f32> {
        self.samples.lock().unwrap().data.clear();
        let (data, rate, frames) = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let changed = self.changed.notified();
                {
                    let held = self.samples.lock().unwrap();
                    assert!(held.failure.is_none(), "{label}: {:?}", held.failure);
                    if let Some(format) = held.format {
                        let rate = format.sample_rate as usize;
                        if held.data.len() == rate * 6 {
                            break (held.data[rate..].to_vec(), format.sample_rate, held.frames);
                        }
                    }
                }
                changed.await;
            }
        })
        .await
        .expect("six seconds of native samples within ten seconds");
        assert!(
            data.iter()
                .all(|sample| sample.is_finite() && sample.abs() < 0.99),
            "{label}: clipping or invalid PCM"
        );
        let levels: Vec<_> = present
            .iter()
            .map(|hz| tone_level(&data, rate, *hz))
            .collect();
        for (hz, db) in present.iter().zip(&levels) {
            println!(
                "{label}: present {hz} Hz={db:.2} dBFS, measured={} frames, total={frames}, rate={rate}",
                data.len()
            );
            assert!(*db > -30.0, "{label}: missing {hz} Hz ({db} dBFS)");
        }
        for hz in absent {
            let db = tone_level(&data, rate, *hz);
            println!("{label}: absent {hz} Hz={db:.2} dBFS");
            assert!(db < -55.0, "{label}: unexpected {hz} Hz ({db} dBFS)");
        }
        levels
    }
}

pub struct TappedHost {
    pub inner: Arc<dyn junto_media::audio::AudioHost>,
    pub input: Arc<Probe>,
    pub output: Arc<Probe>,
}
impl junto_media::audio::AudioHost for TappedHost {
    fn devices(&self) -> Result<junto_media::audio::Devices, junto_media::audio::AudioError> {
        self.inner.devices()
    }
    fn open_input(
        &self,
        id: Option<&junto_media::audio::DeviceId>,
        mut deliver: junto_media::audio::host::Deliver,
        failed: junto_media::audio::host::Failed,
    ) -> Result<junto_media::audio::OpenStream, junto_media::audio::AudioError> {
        let probe = self.input.clone();
        self.inner.open_input(
            id,
            Box::new(move |data, format| {
                probe.record(data, format);
                deliver(data, format);
            }),
            failed,
        )
    }
    fn open_output(
        &self,
        id: Option<&junto_media::audio::DeviceId>,
        mut fill: junto_media::audio::host::Fill,
        failed: junto_media::audio::host::Failed,
    ) -> Result<junto_media::audio::OpenStream, junto_media::audio::AudioError> {
        let probe = self.output.clone();
        self.inner.open_output(
            id,
            Box::new(move |data, format| {
                fill(data, format);
                probe.record(data, format);
            }),
            failed,
        )
    }
}
