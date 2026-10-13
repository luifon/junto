//! Test statistics sampled at the boundaries of a measured interval.

use std::time::Duration;

use livekit::rtc_engine::SessionStats;
use tokio::sync::oneshot;

use super::{Actor, Command, Controls, Engine, Publication};

pub(crate) type Reply = oneshot::Sender<Result<SessionStats, String>>;
pub(crate) type SoundReply = oneshot::Sender<SoundCheck>;

/// The time that the lines of a [`SoundCheck`] cover.
const SPAN: Duration = Duration::from_millis(600);

/// What an engine knows of its sound at one moment, for a live test that failed. Each
/// line has one character for each 10 ms of the last 600 ms, the oldest first: a point for
/// no frame, a digit for the number of frames.
#[derive(Debug)]
pub struct SoundCheck {
    /// The time since the media connection was made.
    pub connected_for: Option<Duration>,
    pub connection: super::Connection,
    pub microphone: super::MicrophoneState,
    pub output: super::OutputState,
    pub controls: Controls,
    /// The microphone track of this device as the room holds it: published, and muted.
    pub own_track: Option<bool>,
    /// The frames of the microphone are sent, and not silence in their place.
    pub gate_open: bool,
    /// Each remote audio track: the identity, the source, subscribed, muted.
    pub remote_tracks: Vec<(String, String, bool, bool)>,
    /// The frames that the thread of the microphone made.
    pub captured: String,
    /// The frames that the media connection took with sound, and as silence.
    pub sent: String,
    pub sent_as_silence: String,
    /// Frames of the microphone that the media connection did not take in time, in all.
    pub capture_dropped: u64,
    /// The frames that the remote tracks gave.
    pub received: String,
    /// The frames that the output device took with the sound of a track, and without.
    pub played: String,
    pub played_as_silence: String,
    /// How often the queue of a played track ran empty, and how often one was cut, in all.
    pub queues_ran_empty: u64,
    pub queues_cut: u64,
    /// Each queue of a track: played now, and the frames that wait.
    pub queues: Vec<(bool, usize)>,
}

impl std::fmt::Display for SoundCheck {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            formatter,
            "  connected for {:?}; {:?}; microphone {:?}; output {:?}",
            self.connected_for, self.connection, self.microphone, self.output
        )?;
        writeln!(
            formatter,
            "  {:?}; own track muted: {:?}; gate open: {}",
            self.controls, self.own_track, self.gate_open
        )?;
        writeln!(
            formatter,
            "  remote audio tracks (identity, source, subscribed, muted): {:?}",
            self.remote_tracks
        )?;
        writeln!(formatter, "  captured          {}", self.captured)?;
        writeln!(formatter, "  sent              {}", self.sent)?;
        writeln!(formatter, "  sent as silence   {}", self.sent_as_silence)?;
        writeln!(formatter, "  received          {}", self.received)?;
        writeln!(formatter, "  played            {}", self.played)?;
        writeln!(formatter, "  played as silence {}", self.played_as_silence)?;
        write!(
            formatter,
            "  in all: {} frames not taken from the microphone, {} empty queues, {} cut queues; queues now (played, frames): {:?}",
            self.capture_dropped, self.queues_ran_empty, self.queues_cut, self.queues
        )
    }
}

impl Engine {
    pub(crate) fn statistics(&self, reply: Reply) {
        let _ = self.commands.send(Command::Statistics(reply));
    }

    pub(crate) fn sound_check(&self, reply: SoundReply) {
        let _ = self.commands.send(Command::SoundCheck(reply));
    }
}

impl Actor {
    pub(super) async fn statistics(&mut self, reply: Reply) {
        let Some(room) = self.room.as_ref() else {
            let _ = reply.send(Err("no media connection".to_owned()));
            return;
        };
        let stats = match tokio::time::timeout(Duration::from_secs(3), room.get_stats()).await {
            Ok(stats) => stats.map_err(|error| error.to_string()),
            Err(_) => Err("statistics request timed out".to_owned()),
        };
        let _ = reply.send(stats);
    }
}

impl Actor {
    pub(super) fn sound_check(&self, reply: SoundReply) {
        let state = self.state.borrow().clone();
        let remote_tracks = self
            .room
            .iter()
            .flat_map(|room| room.remote_participants().into_values())
            .flat_map(|participant| {
                let identity = participant.identity().as_str().to_owned();
                participant
                    .track_publications()
                    .into_values()
                    .filter(|publication| publication.kind() == livekit::track::TrackKind::Audio)
                    .map(move |publication| {
                        (
                            identity.clone(),
                            format!("{:?}", publication.source()),
                            publication.is_subscribed(),
                            publication.is_muted(),
                        )
                    })
            })
            .collect();
        let passed = &self.mixer.passed;
        let order = std::sync::atomic::Ordering::Relaxed;
        let _ = reply.send(SoundCheck {
            connected_for: self.connected_at.map(|at| at.elapsed()),
            connection: state.connection,
            microphone: state.microphone,
            output: state.output,
            controls: self.controls,
            own_track: match &self.publication {
                Publication::Published(publication) => Some(publication.is_muted()),
                _ => None,
            },
            gate_open: self.link.is_open(),
            remote_tracks,
            captured: self.link.captured.line(SPAN),
            sent: self.link.taken.sound.line(SPAN),
            sent_as_silence: self.link.taken.silence.line(SPAN),
            capture_dropped: self.link.dropped(),
            received: passed.received.line(SPAN),
            played: passed.played.line(SPAN),
            played_as_silence: passed.silent.line(SPAN),
            queues_ran_empty: passed.ran_empty.load(order),
            queues_cut: passed.cut.load(order),
            queues: self.mixer.queues(),
        });
    }
}
