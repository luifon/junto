use std::time::Duration;

use anyhow::{anyhow, Result};
use livekit::options::{AudioEncoding, TrackPublishOptions, VideoCodec};
use livekit::prelude::*;
use livekit::webrtc::audio_source::native::NativeAudioSource;
use livekit::webrtc::audio_source::{AudioSourceOptions, RtcAudioSource};
use livekit::webrtc::video_source::native::NativeVideoSource;
use livekit::webrtc::video_source::{RtcVideoSource, VideoResolution};
use serde_json::json;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::{sleep, sleep_until, Instant};

use crate::cli::{PublishArgs, VideoCodecArg};
use crate::events::emit;
use crate::pattern::{self, Pattern, PatternKind};
use crate::session::{self, source_name};
use crate::tone::{self, FileAudio, Marker, ToneSpec};

const SHARE_AUDIO_BITRATE: u64 = 128_000;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Action {
    StartScreen,
    StopScreen,
    Mute,
    Unmute,
    Reconnect,
    Leave,
}

struct Published {
    publication: LocalTrackPublication,
    generator: JoinHandle<()>,
}

impl Drop for Published {
    fn drop(&mut self) {
        self.generator.abort();
    }
}

struct Session {
    room: Room,
    events: UnboundedReceiver<RoomEvent>,
    mic: Option<Published>,
    _camera: Option<Published>,
    screen: Vec<Published>,
}

/// The files that replace the tones, and the signal that starts them.
struct Files {
    mic: Option<FileAudio>,
    screen_audio: Option<FileAudio>,
}

impl Files {
    fn load(args: &PublishArgs) -> Result<Self> {
        if args.screen_audio_file.is_some() && !args.screen {
            return Err(anyhow!("--screen-audio-file needs --screen"));
        }
        let (start, play) = watch::channel(!args.play_on_stdin);
        if args.play_on_stdin {
            std::thread::spawn(move || {
                for line in std::io::stdin().lines() {
                    match line {
                        Ok(line) if line.trim() == "play" => {
                            let _ = start.send(true);
                        }
                        Ok(_) => {}
                        Err(_) => break,
                    }
                }
            });
        }
        let file = |path: &Option<std::path::PathBuf>| -> Result<Option<FileAudio>> {
            path.as_deref()
                .map(|path| Ok(FileAudio { samples: tone::read_wav(path)?, play: play.clone() }))
                .transpose()
        };
        Ok(Self { mic: file(&args.mic_file)?, screen_audio: file(&args.screen_audio_file)? })
    }
}

pub async fn run(args: PublishArgs) -> Result<()> {
    let started = Instant::now();
    let files = Files::load(&args)?;
    let mut timeline: Vec<(f64, Action)> = Vec::new();
    if args.screen && args.screen_after > 0.0 {
        timeline.push((args.screen_after, Action::StartScreen));
    }
    let optional = [
        (args.screen_stop_after, Action::StopScreen),
        (args.mute_after, Action::Mute),
        (args.unmute_after, Action::Unmute),
        (args.reconnect_after, Action::Reconnect),
        (args.leave_after, Action::Leave),
    ];
    timeline.extend(optional.into_iter().filter_map(|(at, action)| at.map(|at| (at, action))));
    timeline.sort_by(|a, b| a.0.total_cmp(&b.0));

    let mut sharing = args.screen && args.screen_after <= 0.0;
    let mut muted = false;
    let mut session = join(&args, &files, sharing, muted).await?;

    for (at, action) in timeline {
        if !wait_until(started + Duration::from_secs_f64(at), &mut session).await {
            return leave(session).await;
        }
        emit("action", json!({ "action": format!("{action:?}"), "t": started.elapsed().as_secs_f64() }));
        match action {
            Action::StartScreen => {
                sharing = true;
                session.screen = publish_screen(&session.room, &args, &files).await?;
            }
            Action::StopScreen => {
                sharing = false;
                for track in session.screen.drain(..) {
                    session.room.local_participant().unpublish_track(&track.publication.sid()).await?;
                }
            }
            Action::Mute | Action::Unmute => {
                muted = action == Action::Mute;
                if let Some(mic) = &session.mic {
                    if muted {
                        mic.publication.mute()
                    } else {
                        mic.publication.unmute()
                    }
                }
            }
            Action::Reconnect => {
                leave(session).await?;
                sleep(Duration::from_secs_f64(args.reconnect_gap)).await;
                session = join(&args, &files, sharing, muted).await?;
            }
            Action::Leave => return leave(session).await,
        }
    }
    wait_until(started + Duration::from_secs(86_400 * 365), &mut session).await;
    leave(session).await
}

/// Returns false when the process was interrupted or the server closed the connection.
async fn wait_until(deadline: Instant, session: &mut Session) -> bool {
    loop {
        tokio::select! {
            _ = sleep_until(deadline) => return true,
            _ = tokio::signal::ctrl_c() => return false,
            event = session.events.recv() => match event {
                Some(RoomEvent::Disconnected { reason }) => {
                    emit("disconnected", json!({ "reason": format!("{reason:?}") }));
                    return false;
                }
                Some(RoomEvent::Reconnecting) => emit("reconnecting", json!({})),
                Some(RoomEvent::Reconnected) => emit("reconnected", json!({})),
                Some(_) => {}
                None => return false,
            },
        }
    }
}

async fn leave(session: Session) -> Result<()> {
    let identity = session.room.local_participant().identity().to_string();
    let Session { room, mic, _camera, screen, .. } = session;
    drop((mic, _camera, screen));
    room.close().await?;
    emit("left", json!({ "identity": identity }));
    Ok(())
}

async fn join(args: &PublishArgs, files: &Files, sharing: bool, muted: bool) -> Result<Session> {
    let (room, events) = session::connect(&args.connect, false).await?;
    let local = room.local_participant();
    emit(
        "joined",
        json!({
            "room": room.name(),
            "identity": local.identity().to_string(),
            "participant_sid": local.sid().to_string(),
        }),
    );

    let mic = if args.no_mic {
        None
    } else {
        let spec = ToneSpec { hz: args.mic_hz, level: args.level, marker: marker(args) };
        let options = TrackPublishOptions { source: TrackSource::Microphone, ..Default::default() };
        Some(publish_tone(&room, "microphone", spec, files.mic.clone(), options).await?)
    };
    if muted {
        if let Some(mic) = &mic {
            mic.publication.mute();
        }
    }

    let camera = if args.camera {
        let (width, height) = pattern::parse_size(&args.camera_size)?;
        let pattern =
            Pattern::new(PatternKind::Camera, (width, height), &label(args), color(args)?, marker(args));
        let options = video_options(TrackSource::Camera, args);
        Some(publish_pattern(&room, "camera", pattern, (width, height), args.camera_fps, options).await?)
    } else {
        None
    };

    let screen = if sharing { publish_screen(&room, args, files).await? } else { Vec::new() };
    Ok(Session { room, events, mic, _camera: camera, screen })
}

async fn publish_screen(room: &Room, args: &PublishArgs, files: &Files) -> Result<Vec<Published>> {
    let (width, height) = pattern::parse_size(&args.screen_size)?;
    let pattern =
        Pattern::new(PatternKind::Screen, (width, height), &label(args), color(args)?, marker(args));
    let options = video_options(TrackSource::Screenshare, args);
    let mut tracks =
        vec![publish_pattern(room, "screen", pattern, (width, height), args.screen_fps, options).await?];
    if args.screen_audio_hz.is_some() || files.screen_audio.is_some() {
        let hz = args.screen_audio_hz.unwrap_or(0.0);
        let options = TrackPublishOptions {
            source: TrackSource::ScreenshareAudio,
            dtx: false,
            red: false,
            audio_encoding: Some(AudioEncoding { max_bitrate: SHARE_AUDIO_BITRATE }),
            ..Default::default()
        };
        let spec = ToneSpec { hz, level: args.level, marker: None };
        tracks.push(publish_tone(room, "screen-audio", spec, files.screen_audio.clone(), options).await?);
    }
    Ok(tracks)
}

fn marker(args: &PublishArgs) -> Option<Marker> {
    args.marker_period_ms.map(|period_ms| Marker { period_ms, gap_ms: args.marker_gap_ms })
}

fn label(args: &PublishArgs) -> String {
    args.label.clone().or_else(|| args.connect.identity.clone()).unwrap_or_else(|| "participant".into())
}

fn color(args: &PublishArgs) -> Result<(u8, u8, u8)> {
    match &args.screen_color {
        Some(text) => pattern::parse_color(text).map_err(|e| anyhow!("--screen-color: {e}")),
        None => Ok(pattern::color_for(&label(args))),
    }
}

fn video_options(source: TrackSource, args: &PublishArgs) -> TrackPublishOptions {
    let video_codec = match args.video_codec {
        VideoCodecArg::Vp8 => VideoCodec::VP8,
        VideoCodecArg::H264 => VideoCodec::H264,
    };
    TrackPublishOptions { source, simulcast: args.simulcast, video_codec, ..Default::default() }
}

async fn publish_tone(
    room: &Room,
    name: &str,
    spec: ToneSpec,
    file: Option<FileAudio>,
    options: TrackPublishOptions,
) -> Result<Published> {
    let processing =
        AudioSourceOptions { echo_cancellation: false, noise_suppression: false, auto_gain_control: false };
    let source = NativeAudioSource::new(processing, tone::SAMPLE_RATE, 1, 0);
    let track = LocalAudioTrack::create_audio_track(name, RtcAudioSource::Native(source.clone()));
    let publication = room.local_participant().publish_track(LocalTrack::Audio(track), options).await?;
    announce(&publication, json!({ "tone_hz": spec.hz, "file": file.is_some() }));
    let generator = tokio::spawn(tone::run_tone(source, name.to_owned(), spec, file));
    Ok(Published { publication, generator })
}

async fn publish_pattern(
    room: &Room,
    name: &str,
    pattern: Pattern,
    (width, height): (u32, u32),
    fps: u32,
    options: TrackPublishOptions,
) -> Result<Published> {
    let is_screencast = options.source == TrackSource::Screenshare;
    let source = NativeVideoSource::new(VideoResolution { width, height }, is_screencast);
    let track = LocalVideoTrack::create_video_track(name, RtcVideoSource::Native(source.clone()));
    let publication = room.local_participant().publish_track(LocalTrack::Video(track), options).await?;
    announce(&publication, json!({ "width": width, "height": height, "fps": fps }));
    Ok(Published { publication, generator: tokio::spawn(pattern::run_pattern(source, pattern, fps)) })
}

fn announce(publication: &LocalTrackPublication, mut details: serde_json::Value) {
    let map = details.as_object_mut().expect("details is an object");
    map.insert("track_sid".into(), json!(publication.sid().to_string()));
    map.insert("source".into(), json!(source_name(publication.source())));
    map.insert("name".into(), json!(publication.name()));
    emit("published", details);
}
