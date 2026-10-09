use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Result};
use futures_util::StreamExt;
use livekit::prelude::*;
use livekit::webrtc::audio_stream::native::NativeAudioStream;
use livekit::webrtc::stats::RtcStats;
use livekit::webrtc::video_stream::native::NativeVideoStream;
use serde::Serialize;
use serde_json::json;
use tokio::time::{interval, sleep_until, Instant};

use crate::cli::SubscribeArgs;
use crate::events::{emit, unix_ms};
use crate::session::{self, source_name};
use crate::tone::{self, DetectedTone};

const ANALYSIS_SAMPLES: usize = (tone::SAMPLE_RATE / 5) as usize;
const EXPECTATION_TOLERANCE_HZ: u32 = 10;

#[derive(Clone, Default, Serialize)]
struct TrackReport {
    participant: String,
    track_sid: String,
    kind: &'static str,
    source: &'static str,
    mime: String,
    muted: bool,
    subscribed_unix_ms: u64,
    unsubscribed_unix_ms: Option<u64>,
    packets_received: u64,
    packets_lost: i64,
    bytes_received: u64,
    audio_frames: u64,
    video_frames: u64,
    width: u32,
    height: u32,
    level_dbfs: Option<f64>,
    tones_now: Vec<DetectedTone>,
    /// Seconds in which each tone frequency was detected.
    tone_seconds: BTreeMap<u32, u32>,
}

type Reports = Arc<Mutex<BTreeMap<String, TrackReport>>>;

struct Expectation {
    participant: String,
    source: String,
    hz: u32,
}

fn parse_expectation(text: &str) -> Result<Expectation> {
    let invalid = || anyhow!("--expect must be identity/source=hz, got {text}");
    let (target, hz) = text.split_once('=').ok_or_else(invalid)?;
    let (participant, source) = target.split_once('/').ok_or_else(invalid)?;
    Ok(Expectation {
        participant: participant.into(),
        source: source.into(),
        hz: hz.parse::<f64>().map_err(|_| invalid())?.round() as u32,
    })
}

/// Returns whether every expectation was met.
pub async fn run(args: SubscribeArgs) -> Result<bool> {
    let expectations: Vec<Expectation> =
        args.expectations.iter().map(|e| parse_expectation(e)).collect::<Result<_>>()?;
    let (room, mut events) = session::connect(&args.connect, true).await?;
    emit("joined", json!({ "room": room.name(), "identity": room.local_participant().identity().to_string() }));

    let reports: Reports = Arc::default();
    let mut tracks: BTreeMap<String, RemoteTrack> = BTreeMap::new();
    let started = Instant::now();
    let deadline = started + Duration::from_secs_f64(args.duration.unwrap_or(86_400.0 * 365.0));
    let mut report_timer = interval(Duration::from_secs_f64(args.report_every.max(0.5)));
    report_timer.tick().await;

    loop {
        tokio::select! {
            _ = sleep_until(deadline) => break,
            _ = tokio::signal::ctrl_c() => break,
            _ = report_timer.tick() => {
                refresh_stats(&tracks, &reports).await;
                emit("report", json!({ "t": started.elapsed().as_secs_f64(), "tracks": snapshot(&reports) }));
            }
            event = events.recv() => match event {
                Some(RoomEvent::TrackSubscribed { track, publication, participant }) => {
                    let sid = publication.sid().to_string();
                    let report = TrackReport {
                        participant: participant.identity().to_string(),
                        track_sid: sid.clone(),
                        kind: if matches!(track, RemoteTrack::Audio(_)) { "audio" } else { "video" },
                        source: source_name(publication.source()),
                        mime: publication.mime_type(),
                        muted: publication.is_muted(),
                        subscribed_unix_ms: unix_ms(),
                        ..Default::default()
                    };
                    emit("track_subscribed", json!({
                        "participant": report.participant, "track_sid": sid,
                        "source": report.source, "mime": report.mime,
                    }));
                    reports.lock().unwrap().insert(sid.clone(), report);
                    match &track {
                        RemoteTrack::Audio(audio) => {
                            tokio::spawn(read_audio(audio.clone(), sid.clone(), reports.clone()));
                        }
                        RemoteTrack::Video(video) => {
                            tokio::spawn(read_video(video.clone(), sid.clone(), reports.clone()));
                        }
                    }
                    tracks.insert(sid, track);
                }
                Some(RoomEvent::TrackUnsubscribed { publication, participant, .. }) => {
                    let sid = publication.sid().to_string();
                    tracks.remove(&sid);
                    if let Some(report) = reports.lock().unwrap().get_mut(&sid) {
                        report.unsubscribed_unix_ms = Some(unix_ms());
                    }
                    emit("track_unsubscribed", json!({
                        "participant": participant.identity().to_string(), "track_sid": sid,
                    }));
                }
                Some(RoomEvent::TrackMuted { publication, .. }) => set_muted(&reports, &publication, true),
                Some(RoomEvent::TrackUnmuted { publication, .. }) => set_muted(&reports, &publication, false),
                Some(RoomEvent::Disconnected { reason }) => {
                    emit("disconnected", json!({ "reason": format!("{reason:?}") }));
                    break;
                }
                Some(_) => {}
                None => break,
            },
        }
    }

    refresh_stats(&tracks, &reports).await;
    let final_reports = snapshot(&reports);
    let mut failures = Vec::new();
    for expectation in &expectations {
        if let Err(reason) = check(expectation, &final_reports) {
            failures.push(reason);
        }
    }
    emit(
        "summary",
        json!({
            "t": started.elapsed().as_secs_f64(),
            "tracks": final_reports,
            "expectations": expectations.len(),
            "failures": failures,
        }),
    );
    room.close().await?;
    Ok(failures.is_empty())
}

fn snapshot(reports: &Reports) -> Vec<TrackReport> {
    reports.lock().unwrap().values().cloned().collect()
}

fn set_muted(reports: &Reports, publication: &TrackPublication, muted: bool) {
    let sid = publication.sid().to_string();
    if let Some(report) = reports.lock().unwrap().get_mut(&sid) {
        report.muted = muted;
    }
    emit(if muted { "track_muted" } else { "track_unmuted" }, json!({ "track_sid": sid }));
}

fn check(expectation: &Expectation, reports: &[TrackReport]) -> Result<(), String> {
    let name = format!("{}/{}", expectation.participant, expectation.source);
    let matching: Vec<&TrackReport> = reports
        .iter()
        .filter(|r| r.participant == expectation.participant && r.source == expectation.source)
        .collect();
    if matching.is_empty() {
        return Err(format!("{name}: no such track was received"));
    }
    let dominant: Vec<u32> = matching
        .iter()
        .filter_map(|r| r.tone_seconds.iter().max_by_key(|(_, &seconds)| seconds).map(|(&hz, _)| hz))
        .collect();
    if dominant.iter().any(|hz| hz.abs_diff(expectation.hz) <= EXPECTATION_TOLERANCE_HZ) {
        Ok(())
    } else {
        Err(format!("{name}: expected {} Hz, dominant tones were {dominant:?}", expectation.hz))
    }
}

async fn refresh_stats(tracks: &BTreeMap<String, RemoteTrack>, reports: &Reports) {
    for (sid, track) in tracks {
        let Ok(stats) = track.get_stats().await else { continue };
        for stat in stats {
            if let RtcStats::InboundRtp(inbound) = stat {
                if let Some(report) = reports.lock().unwrap().get_mut(sid) {
                    report.packets_received = inbound.received.packets_received;
                    report.packets_lost = inbound.received.packets_lost;
                    report.bytes_received = inbound.inbound.bytes_received;
                }
            }
        }
    }
}

async fn read_audio(track: RemoteAudioTrack, sid: String, reports: Reports) {
    let mut stream = NativeAudioStream::new(track.rtc_track(), tone::SAMPLE_RATE as i32, 1);
    let mut second: Vec<i16> = Vec::with_capacity(tone::SAMPLE_RATE as usize);
    while let Some(frame) = stream.next().await {
        second.extend_from_slice(&frame.data);
        let analysed = if second.len() >= tone::SAMPLE_RATE as usize {
            let window = &second[second.len() - ANALYSIS_SAMPLES..];
            let result = (tone::detect_tones(window, tone::SAMPLE_RATE), tone::rms_dbfs(&second));
            second.clear();
            Some(result)
        } else {
            None
        };
        let mut guard = reports.lock().unwrap();
        let Some(report) = guard.get_mut(&sid) else { return };
        report.audio_frames += 1;
        if let Some((tones, level)) = analysed {
            for tone in &tones {
                *report.tone_seconds.entry(tone.hz).or_default() += 1;
            }
            report.tones_now = tones;
            report.level_dbfs = Some(level);
        }
    }
}

async fn read_video(track: RemoteVideoTrack, sid: String, reports: Reports) {
    let mut stream = NativeVideoStream::new(track.rtc_track());
    while let Some(frame) = stream.next().await {
        let mut guard = reports.lock().unwrap();
        let Some(report) = guard.get_mut(&sid) else { return };
        report.video_frames += 1;
        report.width = frame.buffer.width();
        report.height = frame.buffer.height();
    }
}
