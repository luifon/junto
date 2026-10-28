use super::{
    fixture::Fixture,
    measure::{Probe, TappedHost},
    protocol::Geometry,
};
use crate::scene::{Person, Room, account};
use junto_media::audio::{AudioHost, Format};
use junto_media::engine::{ShareId, ShareState};
use junto_media::share::picture::PictureFrame;
use junto_media::share::{
    CaptureError, CaptureRequest, CaptureSink, CaptureStream, Pending, PictureSource, Resolution,
    ScreenAccess, ScreenCapture, ShareQuality, ShareSources, SoundChoice,
};
use livekit::webrtc::video_frame::VideoBuffer;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::watch;

struct Observed {
    native: Arc<dyn ScreenCapture>,
    pictures: watch::Sender<Option<Arc<PictureFrame>>>,
    stopped: watch::Sender<usize>,
    audio: Arc<Probe>,
}
struct Sink {
    inner: Arc<dyn CaptureSink>,
    pictures: watch::Sender<Option<Arc<PictureFrame>>>,
    stopped: watch::Sender<usize>,
    audio: Arc<Probe>,
}
impl CaptureSink for Sink {
    fn picture(&self, buffer: Arc<dyn VideoBuffer>) {
        let picture = junto_media::share::picture::convert(&*buffer, (4096, 4096))
            .expect("native screen buffer converts");
        self.pictures.send_replace(Some(Arc::new(picture)));
        self.inner.picture(buffer);
    }
    fn sound(&self, data: &[f32], format: Format) {
        self.audio.record(data, format);
        self.inner.sound(data, format);
    }
    fn ended(&self, error: CaptureError) {
        self.inner.ended(error);
    }
}
impl Drop for Sink {
    fn drop(&mut self) {
        self.stopped.send_modify(|count| *count += 1);
    }
}
impl ScreenCapture for Observed {
    fn access(&self) -> ScreenAccess {
        self.native.access()
    }
    fn request_access(&self) {}
    fn open_settings(&self) {}
    fn sources(&self) -> Pending<Result<ShareSources, CaptureError>> {
        self.native.sources()
    }
    fn start(
        &self,
        request: CaptureRequest,
        sink: Arc<dyn CaptureSink>,
    ) -> Pending<Result<CaptureStream, CaptureError>> {
        self.native.start(
            request,
            Arc::new(Sink {
                inner: sink,
                pictures: self.pictures.clone(),
                stopped: self.stopped.clone(),
                audio: self.audio.clone(),
            }),
        )
    }
}

fn matches(frame: &PictureFrame, geometry: &Geometry, display: bool, marker: u32) -> bool {
    let (width, height, x, y) = if display {
        (
            geometry.display_width,
            geometry.display_height,
            geometry.x,
            geometry.y,
        )
    } else {
        (geometry.width, geometry.height, 0, 0)
    };
    if (u64::from(frame.width) * u64::from(height))
        .abs_diff(u64::from(frame.height) * u64::from(width))
        > 4 * u64::from(width.max(height))
    {
        return false;
    }
    let pixel = |px: f64, py: f64| {
        frame.pixel(
            ((f64::from(x) + px * f64::from(geometry.width)) * f64::from(frame.width)
                / f64::from(width)) as u32,
            ((f64::from(y) + py * f64::from(geometry.height)) * f64::from(frame.height)
                / f64::from(height)) as u32,
        )
    };
    for bit in 0..32 {
        let expected: u8 = if marker & (1 << bit) == 0 { 0 } else { 255 };
        if !pixel((f64::from(bit) + 0.5) / 32.0, 0.05).is_some_and(|p| {
            p.0.abs_diff(expected) < 55
                && p.1.abs_diff(expected) < 55
                && p.2.abs_diff(expected) < 55
        }) {
            return false;
        }
    }
    for (index, expected) in [
        (204u8, 26u8, 26u8),
        (26, 204, 26),
        (26, 26, 204),
        (204, 204, 26),
    ]
    .into_iter()
    .enumerate()
    {
        if !pixel(
            if index % 2 == 0 { 0.25 } else { 0.75 },
            if index < 2 { 0.325 } else { 0.775 },
        )
        .is_some_and(|p| super::colors::matches(p, expected))
        {
            return false;
        }
    }
    true
}

fn diagnostic(frame: &PictureFrame, g: &Geometry) -> String {
    let mut samples = Vec::new();
    for y in [0.05, 0.325, 0.775] {
        for x in [0.25, 0.75] {
            let x = ((f64::from(g.x) + x * f64::from(g.width)) * f64::from(frame.width)
                / f64::from(g.display_width)) as u32;
            let y = ((f64::from(g.y) + y * f64::from(g.height)) * f64::from(frame.height)
                / f64::from(g.display_height)) as u32;
            samples.push((x, y, frame.pixel(x, y)));
        }
    }
    format!(
        "size={}x{} source={:?} fixture samples={samples:?}",
        frame.width, frame.height, frame.source
    )
}

pub async fn capture(audio_only: bool) {
    let started = Instant::now();
    let system = junto_media::share::system();
    assert_eq!(
        system.access(),
        ScreenAccess::Allowed,
        "screen access must already be allowed"
    );
    let mut fixture = Fixture::launch("A").await;
    let sources = system.sources().await.unwrap();
    let window = sources
        .windows
        .iter()
        .find(|window| window.id == fixture.geometry.window)
        .expect("fixture is a shareable external window");
    let application = window.application.clone();
    assert!(application.bundle_id.ends_with(".A"));
    let display = sources
        .displays
        .iter()
        .find(|display| display.id == fixture.geometry.display)
        .unwrap();
    assert_eq!(
        (display.width, display.height),
        (
            fixture.geometry.display_width,
            fixture.geometry.display_height
        )
    );
    let (pictures, mut native_pictures) = watch::channel(None);
    let (stopped, mut stops) = watch::channel(0);
    let native_audio = Arc::new(Probe::default());
    let mut local = Person::new(0.0);
    local.calls = junto_media::Calls::new(
        local.device.client.clone(),
        junto_media::MediaOptions {
            host: Arc::new(local.host.clone()),
            permission: Arc::new(junto_media::permission::FixedPermission::allowed()),
            camera: Arc::new(junto_media::camera::NoCamera),
            capture: Arc::new(Observed {
                native: system,
                pictures,
                stopped,
                audio: native_audio.clone(),
            }),
            timing: Default::default(),
        },
    );
    local.calls.set_voice(crate::scene::plain_voice());
    let received_audio = Arc::new(Probe::default());
    let peer = super::calls::person(
        Arc::new(TappedHost {
            inner: Arc::new(junto_media::audio::synthetic::SyntheticHost::with_tone(0.0)),
            input: Arc::default(),
            output: received_audio.clone(),
        }),
        Arc::new(junto_media::permission::FixedPermission::allowed()),
        crate::scene::plain_voice(),
    );
    let (room, owner) = Room::new("Native share", &local, "Native").await;
    room.member(&owner, &peer, "Peer").await;
    local.enter(room.target).await;
    peer.enter(room.target).await;
    let share = ShareId::of(account(&owner), 0);
    peer.calls.set_view_size(3840, 2160);
    let mut remote = peer.calls.main_picture();
    let quality = ShareQuality {
        resolution: Resolution::Source,
        frame_rate: 30,
    };
    if !audio_only {
        let marker = u32::from_le_bytes(uuid::Uuid::new_v4().as_bytes()[..4].try_into().unwrap());
        for (index, picture) in [
            PictureSource::Display(display.id),
            PictureSource::Window(window.id),
        ]
        .into_iter()
        .enumerate()
        {
            let display = index == 0;
            local.calls.start_share(CaptureRequest {
                picture,
                sound: SoundChoice::None,
                quality,
                dialog_parent: None,
            });
            super::calls::state(&local, |v| {
                v.call
                    .as_ref()
                    .is_some_and(|c| matches!(c.own_shares.get(&0), Some(ShareState::Running(_))))
            })
            .await;
            peer.calls.watch_share(share);
            for marker in [marker, marker ^ 0x5a5a5a5a] {
                fixture.pattern(marker);
                let found = tokio::time::timeout(
                    Duration::from_secs(10),
                    native_pictures.wait_for(|picture| {
                        picture
                            .as_ref()
                            .is_some_and(|frame| matches(frame, &fixture.geometry, display, marker))
                    }),
                )
                .await
                .is_ok_and(|result| result.is_ok());
                if !found {
                    panic!(
                        "fresh native fixture marker={marker}: {:?}",
                        native_pictures
                            .borrow()
                            .as_ref()
                            .map(|frame| diagnostic(frame, &fixture.geometry))
                    );
                }
                tokio::time::timeout(
                    Duration::from_secs(10),
                    remote.wait_for(|picture| {
                        picture.as_ref().is_some_and(|picture| {
                            picture.share == share
                                && (picture.frame.picture()).is_some_and(|frame| {
                                    matches(frame, &fixture.geometry, display, marker)
                                })
                        })
                    }),
                )
                .await
                .expect("fresh remote fixture marker")
                .unwrap();
                println!(
                    "S{} fresh native and remote marker={marker}, elapsed={:?}",
                    index + 1,
                    started.elapsed()
                );
            }
            if display {
                local.calls.stop_share(None);
            } else {
                fixture.close();
                super::calls::state(&local, |v| {
                    v.call.as_ref().is_some_and(|c| c.share_ended.is_some())
                })
                .await;
            }
            tokio::time::timeout(
                Duration::from_secs(2),
                stops.wait_for(|count| *count == index + 1),
            )
            .await
            .expect("native capture sinks released")
            .unwrap();
        }
        println!(
            "S1/S2 passed content, close and sink release; system stop completion is not exposed by the capture interface"
        );
    } else {
        let mut second = Fixture::launch("B").await;
        let host = super::audio::host().unwrap();
        let own = super::calls::Tone::new(&host);
        let loopback = Arc::new(Probe::default());
        let input = loopback.clone();
        let probe = host
            .open_input(
                Some(host.id()),
                Box::new(move |data, format| input.record(data, format)),
                Box::new(|error| eprintln!("fixture probe: {error}")),
            )
            .unwrap();
        fixture.tone(733);
        second.tone(1103);
        own.set(1559);
        loopback
            .measure("S3 fixture playback", &[733., 1103., 1559.], &[])
            .await;
        for (index, sound) in [
            SoundChoice::Application(application),
            SoundChoice::System,
            SoundChoice::None,
        ]
        .into_iter()
        .enumerate()
        {
            let native_before = native_audio.frames();
            let reported = sound.reported();
            local.calls.start_share(CaptureRequest {
                picture: PictureSource::Window(window.id),
                sound,
                quality,
                dialog_parent: None,
            });
            super::calls::state(&local, |v| {
                v.call.as_ref().is_some_and(
                    |c| matches!(c.own_shares.get(&0), Some(ShareState::Running(info)) if info.sound == reported),
                )
            })
            .await;
            peer.calls.watch_share(share);
            let (present, absent): (&[f32], &[f32]) = match index {
                0 => (&[733.], &[1103., 1559.]),
                1 => (&[733., 1103.], &[1559.]),
                _ => (&[], &[733., 1103., 1559.]),
            };
            if index < 2 {
                tokio::join!(
                    native_audio.measure("S3 native", present, absent),
                    received_audio.measure("S3 remote", present, absent)
                );
                let check = peer.calls.sound_check().await.unwrap();
                let source = format!("{:?}", livekit::track::TrackSource::ScreenshareAudio);
                assert!(
                    check
                        .remote_tracks
                        .iter()
                        .any(|(_, kind, _, _)| kind == &source),
                    "shared audio must be published"
                );
            } else {
                received_audio.measure("S3 None", present, absent).await;
                assert_eq!(
                    native_audio.frames(),
                    native_before,
                    "None must have no native audio callbacks"
                );
                let check = peer.calls.sound_check().await.unwrap();
                let source = format!("{:?}", livekit::track::TrackSource::ScreenshareAudio);
                assert!(
                    check
                        .remote_tracks
                        .iter()
                        .all(|(_, kind, _, _)| kind != &source),
                    "None must have no shared-audio publication"
                );
            }
            local.calls.stop_share(None);
            tokio::time::timeout(
                Duration::from_secs(2),
                stops.wait_for(|count| *count == index + 1),
            )
            .await
            .expect("native share sinks released")
            .unwrap();
        }
        fixture.tone(0);
        second.tone(0);
        drop(own);
        drop(probe);
        assert!(!host.failed());
        assert_eq!(host.active(), 0);
        second.close();
        println!("S3 passed");
    }
    local.calls.leave().await;
    peer.calls.leave().await;
    fixture.close();
}
