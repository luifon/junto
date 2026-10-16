use super::guard::Guard;
use super::measure::{Probe, TappedHost};
use crate::scene::{Person, Room, account};

struct ReadPermission;
impl MicrophonePermission for ReadPermission {
    fn state(&self) -> MicrophoneAccess {
        SystemPermission.state()
    }
    fn request(&self) -> junto_media::permission::Answer {
        Box::pin(std::future::ready(SystemPermission.state()))
    }
}
use junto_media::audio::{AudioHost, OpenStream, Processing};
use junto_media::calls::{CallsView, MediaOptions};
use junto_media::engine::{MicrophoneState, VoiceConfig};
use junto_media::permission::{
    FixedPermission, MicrophoneAccess, MicrophonePermission, SystemPermission,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

pub fn person(
    host: Arc<dyn AudioHost>,
    permission: Arc<dyn MicrophonePermission>,
    config: VoiceConfig,
) -> Person {
    let mut person = Person::new(0.0);
    person.calls = junto_media::Calls::new(
        person.device.client.clone(),
        MediaOptions {
            host,
            permission,
            camera: Arc::new(junto_media::camera::NoCamera),
            capture: Arc::new(junto_media::share::NoCapture),
            timing: Default::default(),
        },
    );
    person.calls.set_voice(config);
    person
}

pub async fn state(person: &Person, condition: impl FnMut(&CallsView) -> bool) {
    let mut watch = person.calls.watch();
    tokio::time::timeout(Duration::from_secs(10), watch.wait_for(condition))
        .await
        .unwrap()
        .unwrap();
}

fn config(host: &Guard) -> VoiceConfig {
    VoiceConfig {
        input_device: Some(host.id().clone()),
        output_device: Some(host.id().clone()),
        processing: Processing::OFF,
        push_to_talk: false,
    }
}

pub struct Tone {
    hz: Arc<AtomicU32>,
    stream: Option<OpenStream>,
}
impl Tone {
    pub fn new(host: &Guard) -> Self {
        let hz = Arc::new(AtomicU32::new(0));
        let choice = hz.clone();
        let mut phase = 0.0f64;
        let stream = host
            .open_output(
                Some(host.id()),
                Box::new(move |samples, format| {
                    let hz = f64::from(choice.load(Ordering::SeqCst));
                    for frame in samples.chunks_exact_mut(usize::from(format.channels)) {
                        frame.fill(if hz == 0.0 {
                            0.0
                        } else {
                            0.2 * phase.sin() as f32
                        });
                        phase = (phase
                            + std::f64::consts::TAU * hz / f64::from(format.sample_rate))
                            % std::f64::consts::TAU;
                    }
                }),
                Box::new(|error| eprintln!("tone stream failed: {error}")),
            )
            .unwrap();
        Self {
            hz,
            stream: Some(stream),
        }
    }
    pub fn set(&self, hz: u32) {
        self.hz.store(hz, Ordering::SeqCst);
    }
}
impl Drop for Tone {
    fn drop(&mut self) {
        self.set(0);
        self.stream.take();
    }
}

pub async fn capture() {
    assert_eq!(SystemPermission.state(), MicrophoneAccess::Allowed);
    let host = super::audio::host().unwrap();
    let raw = Arc::new(Probe::default());
    let tap = Arc::new(TappedHost {
        inner: host.clone(),
        input: raw.clone(),
        output: Arc::default(),
    });
    let native = person(tap, Arc::new(ReadPermission), config(&host));
    let sink = junto_media::audio::synthetic::SyntheticHost::with_tone(0.0);
    let received = Arc::new(Probe::default());
    let peer = person(
        Arc::new(TappedHost {
            inner: Arc::new(sink),
            input: Arc::default(),
            output: received.clone(),
        }),
        Arc::new(FixedPermission::allowed()),
        VoiceConfig {
            processing: Processing::OFF,
            ..Default::default()
        },
    );
    peer.calls.set_muted(true);
    let (room, owner) = Room::new("Native capture", &native, "Native").await;
    room.member(&owner, &peer, "Peer").await;
    native.enter(room.target).await;
    peer.enter(room.target).await;
    state(&native, |v| {
        v.call
            .as_ref()
            .is_some_and(|c| c.microphone == MicrophoneState::Running)
    })
    .await;
    let tone = Tone::new(&host);
    for hz in [733, 1103] {
        tone.set(hz);
        let other = if hz == 733 { 1103. } else { 733. };
        let present = [hz as f32];
        let absent = [other];
        let (raw, _) = tokio::join!(
            raw.measure("A1 raw", &present, &absent),
            received.measure("A1 received", &present, &absent)
        );
        assert!((raw[0] - 20.0 * 0.2f32.log10()).abs() < 2.0);
    }
    tone.set(0);
    received
        .measure("A1 marker silence", &[], &[733., 1103.])
        .await;
    tone.set(733);
    native.calls.set_muted(true);
    received.measure("A1 muted", &[], &[733.]).await;
    native.calls.set_muted(false);
    received.measure("A1 unmuted", &[733.], &[1103.]).await;
    let mut voice = config(&host);
    voice.push_to_talk = true;
    native.calls.set_voice(voice);
    received
        .measure("A1 push-to-talk released", &[], &[733.])
        .await;
    native.calls.set_talking(true);
    received
        .measure("A1 push-to-talk pressed", &[733.], &[1103.])
        .await;
    native.calls.set_talking(false);
    received
        .measure("A1 push-to-talk released again", &[], &[733.])
        .await;
    drop(tone);
    native.calls.leave().await;
    peer.calls.leave().await;
    assert_eq!(host.active(), 0);
    assert!(!host.failed());
    println!("A1 passed");
}

pub async fn playout() {
    assert_eq!(SystemPermission.state(), MicrophoneAccess::Allowed);
    let host = super::audio::host().unwrap();
    let blocked = Arc::new(FixedPermission::new(
        MicrophoneAccess::Refused,
        MicrophoneAccess::Refused,
    ));
    let native = person(host.clone(), blocked, config(&host));
    let peer = Person::new(733.0);
    let (room, owner) = Room::new("Native playout", &native, "Native").await;
    let remote = room.member(&owner, &peer, "Peer").await;
    native.enter(room.target).await;
    peer.enter(room.target).await;
    state(&native, |v| {
        v.call
            .as_ref()
            .is_some_and(|c| c.microphone == MicrophoneState::NoPermission)
    })
    .await;
    let measured = Arc::new(Probe::default());
    let input = measured.clone();
    let probe = host
        .open_input(
            Some(host.id()),
            Box::new(move |data, format| input.record(data, format)),
            Box::new(|error| eprintln!("probe failed: {error}")),
        )
        .unwrap();
    let full = measured.measure("A2 full volume", &[733.], &[1103.]).await[0];
    native
        .calls
        .set_volume(room.target.server_id, account(&remote), 0.5);
    let half = measured.measure("A2 half volume", &[733.], &[1103.]).await[0];
    assert!(
        (full - half - 6.0206).abs() < 2.0,
        "gain full={full} half={half}"
    );
    native.calls.set_deafened(true);
    measured.measure("A2 sound off", &[], &[733.]).await;
    native.calls.leave().await;
    assert_eq!(host.active(), 1, "only the independent input probe remains");
    measured.measure("A2 leave", &[], &[733.]).await;
    drop(probe);
    peer.calls.leave().await;
    assert_eq!(host.active(), 0);
    assert!(!host.failed());
    println!("A2 passed");
}

pub async fn lifecycle() {
    let host = super::audio::host().unwrap();
    let native = person(host.clone(), Arc::new(ReadPermission), config(&host));
    let (room, _) = Room::new("Native lifecycle", &native, "Native").await;
    for _ in 0..2 {
        native.enter(room.target).await;
        state(&native, |v| {
            v.call
                .as_ref()
                .is_some_and(|c| c.microphone == MicrophoneState::Running)
        })
        .await;
        assert_eq!(host.active(), 2);
        native.calls.leave().await;
        assert_eq!(host.active(), 0);
    }
    native.enter(room.target).await;
    state(&native, |v| {
        v.call
            .as_ref()
            .is_some_and(|c| c.microphone == MicrophoneState::Running)
    })
    .await;
    let mut choice = config(&host);
    choice.output_device = Some(junto_media::audio::DeviceId("deliberately-missing".into()));
    native.calls.set_voice(choice);
    state(&native, |v| {
        v.call
            .as_ref()
            .is_some_and(|c| matches!(c.output, junto_media::engine::OutputState::Failed(_)))
    })
    .await;
    assert!(host.failed());
    native.calls.leave().await;
    assert_eq!(host.active(), 0);
    println!("A3 passed: stop/start, explicit route and failed missing-device fallback");
}
