//! The level of the microphone outside a call: the settings show it under the threshold of
//! voice activation (ADR 0733). Nothing is sent; the frames end at the meter.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, watch};

use super::capture::{CaptureChain, CaptureLink};
use super::host::usable;
use super::processing::Processor;
use super::{AudioError, AudioHost, DeviceId, OpenStream, Processing, level};
use crate::engine::Levels;

/// How often the level is told to the window.
const LEVEL_PERIOD: Duration = Duration::from_millis(50);

/// An open input device whose level is written to `levels` until it is dropped.
pub struct InputTest {
    _stream: OpenStream,
    publisher: tokio::task::JoinHandle<()>,
}

impl InputTest {
    /// Opens the device. The call can wait for the sound system: it runs on a thread of the
    /// runtime, never on the thread of a window.
    pub fn open(
        host: &dyn AudioHost,
        device: Option<&DeviceId>,
        processing: Processing,
        levels: watch::Sender<Levels>,
        runtime: &tokio::runtime::Handle,
    ) -> Result<Self, AudioError> {
        let link = Arc::new(CaptureLink::default());
        let (frames, taken) = mpsc::channel(1);
        drop(taken);
        let connected = host.devices().unwrap_or_default();
        let chosen = usable(device, &connected.inputs);
        let mut chain =
            CaptureChain::new(link.clone(), Arc::new(Processor::new(processing)), frames);
        let stream = host.open_input(
            chosen,
            Box::new(move |samples, format| chain.deliver(samples, format)),
            Box::new(|_| {}),
        )?;
        let publisher = runtime.spawn(async move {
            let mut every = tokio::time::interval(LEVEL_PERIOD);
            loop {
                every.tick().await;
                let input = level::meter(link.input.read());
                levels.send_if_modified(|held| {
                    let changed = held.input != input;
                    held.input = input;
                    changed
                });
            }
        });
        Ok(Self {
            _stream: stream,
            publisher,
        })
    }
}

impl Drop for InputTest {
    fn drop(&mut self) {
        self.publisher.abort();
    }
}
