//! The rules of the check of a finished file, and the place of a file on the timeline of
//! its recording (ADR 0602, ADR 0606). Every function works on what the tools measured and
//! reads no file.

use junto_protocol::recording::FileProblem;
use time::{Duration, OffsetDateTime};

/// A gap between two packets of sound that is a hole in its timestamps, in seconds.
const SOUND_HOLE: f64 = 0.25;
/// The same for the picture.
const PICTURE_HOLE: f64 = 1.0;
/// A stream may begin this much after the file and end this much before it, in seconds.
const EDGE: f64 = 1.0;
/// Sound whose highest level is below this is digital silence, in dB.
const SILENT_BELOW: f64 = -60.0;
/// A key frame with an average brightness below this is dark.
const DARK_BELOW: f64 = 24.0;
/// A key frame whose brightness spans less than this has one colour.
const ONE_COLOUR_BELOW: f64 = 16.0;
/// The share of dark or one-colour key frames above which the picture is blank.
const BLANK_SHARE: f64 = 0.05;
/// The length of the media may differ from the length that the recorder reported by this
/// much, plus one thousandth of the reported length.
const LENGTH_TOLERANCE: Duration = Duration::seconds(3);
/// One continuous stretch longer than this, in which a microphone file has sound and the
/// composite has none, is sound that the composite lacks.
const MISSING_STRETCH: Duration = Duration::seconds(2);

/// An audio file is this much later than the start time that the recorder reports
/// (ADR 0602). The value belongs to the tested recorder version.
const TRACK_LATE: Duration = Duration::milliseconds(47);
/// The first sample of a composite is this much before its reported end minus its length.
const COMPOSITE_LATE: Duration = Duration::milliseconds(135);

/// One step of a loudness envelope, in samples at [`ENVELOPE_RATE`].
pub const ENVELOPE_STEP: usize = 32;
pub const ENVELOPE_RATE: u32 = 16_000;
/// The agreement below which two envelopes do not place a file.
const LEAST_AGREEMENT: f64 = 0.5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamKind {
    Sound,
    Picture,
    Other,
}

/// What the packets of one stream state.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamTiming {
    pub kind: StreamKind,
    pub size: Option<(u32, u32)>,
    pub packets: u64,
    /// The timestamp of the first packet, in seconds.
    pub first: f64,
    /// The end of the last packet, in seconds.
    pub end: f64,
    /// The largest gap between the timestamps of two packets that follow each other.
    pub largest_gap: f64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Probe {
    pub streams: Vec<StreamTiming>,
}

/// The level and the silent ranges of the sound of a file. A range without an end lasts to
/// the end of the file.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Sound {
    /// The highest level in dB. Absent when the sound has no sample above nothing.
    pub peak_db: Option<f64>,
    pub silences: Vec<(f64, Option<f64>)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct KeyFrames {
    pub total: u64,
    pub dark: u64,
    pub one_colour: u64,
}

impl KeyFrames {
    /// Counts one key frame with its average, lowest and highest brightness.
    pub fn count(&mut self, average: f64, lowest: f64, highest: f64) {
        self.total += 1;
        if average < DARK_BELOW {
            self.dark += 1;
        }
        if highest - lowest < ONE_COLOUR_BELOW {
            self.one_colour += 1;
        }
    }
}

/// What the timestamps of a file state.
#[derive(Debug, Clone, PartialEq)]
pub struct Timed {
    /// The timestamp at which the file begins, in seconds.
    pub origin: f64,
    pub media_ms: i64,
    pub size: Option<(u32, u32)>,
    pub has_sound: bool,
    pub has_picture: bool,
    pub problems: Vec<FileProblem>,
}

fn add(problems: &mut Vec<FileProblem>, problem: FileProblem) {
    if !problems.contains(&problem) {
        problems.push(problem);
    }
}

/// Checks that every stream exists, covers the length of the file and has no hole in its
/// timestamps. `picture` states whether the file is a composite.
pub fn timing(probe: &Probe, picture: bool) -> Timed {
    let mut problems = Vec::new();
    let of = |kind: StreamKind| {
        probe
            .streams
            .iter()
            .find(|stream| stream.kind == kind && stream.packets > 0)
    };
    let sound = of(StreamKind::Sound);
    let image = of(StreamKind::Picture);
    if sound.is_none() || (picture && image.is_none()) {
        add(&mut problems, FileProblem::StreamMissing);
    }
    let present: Vec<&StreamTiming> = sound.into_iter().chain(image.filter(|_| picture)).collect();
    let origin = present
        .iter()
        .map(|stream| stream.first)
        .fold(f64::INFINITY, f64::min);
    let end = present
        .iter()
        .map(|stream| stream.end)
        .fold(f64::NEG_INFINITY, f64::max);
    let (origin, length) = if present.is_empty() {
        (0.0, 0.0)
    } else {
        (origin, (end - origin).max(0.0))
    };
    for stream in &present {
        let hole = match stream.kind {
            StreamKind::Sound => SOUND_HOLE,
            StreamKind::Picture | StreamKind::Other => PICTURE_HOLE,
        };
        if stream.largest_gap > hole {
            add(&mut problems, FileProblem::TimestampHole);
        }
        if stream.first - origin > EDGE || end - stream.end > EDGE {
            add(&mut problems, FileProblem::StreamShort);
        }
    }
    Timed {
        origin,
        #[allow(clippy::cast_possible_truncation)]
        media_ms: (length * 1000.0).round() as i64,
        size: image.and_then(|stream| stream.size),
        has_sound: sound.is_some(),
        has_picture: image.is_some(),
        problems,
    }
}

/// True when the length of the media does not agree with the length that the recorder
/// reported for the file.
pub fn length_differs(media_ms: i64, reported_ms: i64) -> bool {
    let tolerance = LENGTH_TOLERANCE.whole_milliseconds() + i128::from(reported_ms / 1000);
    i128::from((media_ms - reported_ms).abs()) > tolerance
}

/// True when the sound of a whole file is digital silence.
pub fn is_silent(sound: &Sound) -> bool {
    sound.peak_db.is_none_or(|peak| peak < SILENT_BELOW)
}

pub fn picture_is_blank(frames: &KeyFrames) -> bool {
    #[allow(clippy::cast_precision_loss)]
    let most = BLANK_SHARE * frames.total as f64;
    #[allow(clippy::cast_precision_loss)]
    let (dark, one_colour) = (frames.dark as f64, frames.one_colour as f64);
    frames.total == 0 || dark > most || one_colour > most
}

/// The first sample of an audio file on the timeline.
pub fn track_start(reported_start: OffsetDateTime) -> OffsetDateTime {
    reported_start - TRACK_LATE
}

/// The first sample of a composite on the timeline, from its reported end and its length.
pub fn composite_start(reported_end: OffsetDateTime, media_ms: i64) -> OffsetDateTime {
    reported_end - Duration::milliseconds(media_ms) - COMPOSITE_LATE
}

/// The first sample of a composite whose recorder reported no end, from the shift at which
/// its sound agrees with a microphone file. `shift` is the time by which the sound is later
/// in the composite than the reported start times of the two files state.
pub fn composite_start_by_sound(reported_start: OffsetDateTime, shift: Duration) -> OffsetDateTime {
    reported_start - shift - TRACK_LATE
}

/// The sound of a file on the timeline: its span and the ranges in which it is silent.
#[derive(Debug, Clone, PartialEq)]
pub struct Heard {
    pub from: OffsetDateTime,
    pub until: OffsetDateTime,
    pub silences: Vec<(OffsetDateTime, OffsetDateTime)>,
}

fn seconds(length: f64) -> Duration {
    Duration::seconds_f64(length)
}

impl Heard {
    /// Places the sound of a file whose first sample is at `starts_at`.
    pub fn of(starts_at: OffsetDateTime, timed: &Timed, sound: &Sound) -> Self {
        let until = starts_at + Duration::milliseconds(timed.media_ms);
        let at = |position: f64| starts_at + seconds(position - timed.origin);
        let silences = sound
            .silences
            .iter()
            .map(|(start, end)| (at(*start), end.map_or(until, at)))
            .collect();
        Self {
            from: starts_at,
            until,
            silences,
        }
    }
}

/// How far a microphone file lies from where the timeline places it in the composite, at
/// moments along the recording: the two files drift apart by tens of parts per million, so
/// one offset for the whole recording leaves them a fraction of a second apart at the edge
/// of each pause (ADR 0606).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Drift {
    /// Moments of the timeline, in order, each with the time by which the sound of the
    /// microphone file is later in the composite than the timeline states.
    points: Vec<(OffsetDateTime, Duration)>,
}

/// A measured point further than this from the line that fits the points is a wrong
/// match of the envelopes, such as a phrase matched with the next one: the drift between
/// two windows is a few milliseconds (ADR 0606).
const DRIFT_OUTLIER: Duration = Duration::seconds(1);

impl Drift {
    pub fn of(mut points: Vec<(OffsetDateTime, Duration)>) -> Self {
        points.sort_by_key(|(at, _)| *at);
        // The drift is close to a straight line over a recording: the point furthest from the
        // line that fits the points is dropped while it is further than [`DRIFT_OUTLIER`],
        // one at a time, so that a wrong point does not move the line for the others.
        while points.len() >= 3 {
            let origin = points[0].0;
            let x = |at: OffsetDateTime| (at - origin).as_seconds_f64();
            let count = points.len() as f64;
            let mean_x = points.iter().map(|(at, _)| x(*at)).sum::<f64>() / count;
            let mean_y = points
                .iter()
                .map(|(_, shift)| shift.as_seconds_f64())
                .sum::<f64>()
                / count;
            let (mut covariance, mut variance) = (0.0, 0.0);
            for (at, shift) in &points {
                let dx = x(*at) - mean_x;
                covariance += dx * (shift.as_seconds_f64() - mean_y);
                variance += dx * dx;
            }
            let slope = if variance > f64::EPSILON {
                covariance / variance
            } else {
                0.0
            };
            let residual = |(at, shift): &(OffsetDateTime, Duration)| {
                (shift.as_seconds_f64() - (mean_y + slope * (x(*at) - mean_x))).abs()
            };
            let Some((worst, off)) = points
                .iter()
                .enumerate()
                .map(|(index, point)| (index, residual(point)))
                .max_by(|first, second| first.1.total_cmp(&second.1))
            else {
                break;
            };
            if off <= DRIFT_OUTLIER.as_seconds_f64() {
                break;
            }
            points.remove(worst);
        }
        Self { points }
    }

    /// The shift at a moment: between two points on the line through them, before the first
    /// and after the last the shift of that point.
    pub fn at(&self, at: OffsetDateTime) -> Duration {
        let Some(first) = self.points.first() else {
            return Duration::ZERO;
        };
        if at <= first.0 {
            return first.1;
        }
        for pair in self.points.windows(2) {
            let ((from, early), (until, late)) = (pair[0], pair[1]);
            if at <= until {
                let span = (until - from).as_seconds_f64();
                if span <= f64::EPSILON {
                    return late;
                }
                let part = (at - from).as_seconds_f64() / span;
                return early + (late - early) * part;
            }
        }
        self.points
            .last()
            .map_or(Duration::ZERO, |(_, shift)| *shift)
    }
}

impl Heard {
    /// The same sound where the composite holds it: each moment moved by the drift there.
    pub fn drifted(&self, drift: &Drift) -> Self {
        let moved = |at: OffsetDateTime| at + drift.at(at);
        Self {
            from: moved(self.from),
            until: moved(self.until),
            silences: self
                .silences
                .iter()
                .map(|(start, end)| (moved(*start), moved(*end)))
                .collect(),
        }
    }
}

/// The longest continuous stretch in which the composite is silent and the microphone file
/// `reference` has sound, when it is longer than [`MISSING_STRETCH`]. Short pieces at the
/// edges of pauses, which follow from small differences of timing, are not added up.
pub fn missing_stretch(
    composite: &Heard,
    reference: &Heard,
) -> Option<(OffsetDateTime, OffsetDateTime)> {
    let mut longest: Option<(OffsetDateTime, OffsetDateTime)> = None;
    for silence in &composite.silences {
        let from = silence.0.max(reference.from);
        let until = silence.1.min(reference.until);
        if until <= from {
            continue;
        }
        // The parts of the silence of the composite in which the microphone is not silent.
        let mut quiet: Vec<(OffsetDateTime, OffsetDateTime)> = reference
            .silences
            .iter()
            .filter(|(start, end)| *end > from && *start < until)
            .map(|(start, end)| ((*start).max(from), (*end).min(until)))
            .collect();
        quiet.sort_by_key(|(start, _)| *start);
        let mut cursor = from;
        let mut pieces = Vec::new();
        for (start, end) in quiet {
            if start > cursor {
                pieces.push((cursor, start));
            }
            cursor = cursor.max(end);
        }
        if cursor < until {
            pieces.push((cursor, until));
        }
        for piece in pieces {
            if longest.is_none_or(|held| piece.1 - piece.0 > held.1 - held.0) {
                longest = Some(piece);
            }
        }
    }
    longest.filter(|(from, until)| *until - *from > MISSING_STRETCH)
}

/// The loudness of mono samples in steps of [`ENVELOPE_STEP`], smoothed over five steps,
/// with its mean removed.
pub fn envelope(samples: &[i16]) -> Vec<f32> {
    let level: Vec<f32> = samples
        .as_chunks::<ENVELOPE_STEP>()
        .0
        .iter()
        .map(|frame| {
            let energy: f32 = frame
                .iter()
                .map(|sample| {
                    let value = f32::from(*sample) / f32::from(i16::MAX);
                    value * value
                })
                .sum();
            #[allow(clippy::cast_precision_loss)]
            (energy / ENVELOPE_STEP as f32).sqrt()
        })
        .collect();
    let smooth: Vec<f32> = (0..level.len())
        .map(|index| {
            let from = index.saturating_sub(2);
            let until = (index + 3).min(level.len());
            level[from..until].iter().sum::<f32>() / 5.0
        })
        .collect();
    #[allow(clippy::cast_precision_loss)]
    let mean = smooth.iter().sum::<f32>() / smooth.len().max(1) as f32;
    smooth.into_iter().map(|value| value - mean).collect()
}

/// The time by which `other` must be delayed to agree best with `reference`, searched up to
/// `most` in both directions. `None` when the two do not agree clearly.
pub fn best_shift(reference: &[f32], other: &[f32], most: Duration) -> Option<Duration> {
    let length = reference.len().min(other.len());
    let (reference, other) = (&reference[..length], &other[..length]);
    let step = f64::from(u32::try_from(ENVELOPE_STEP).ok()?) / f64::from(ENVELOPE_RATE);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let most_steps = ((most.as_seconds_f64() / step) as usize).min(length / 2);
    if most_steps == 0 {
        return None;
    }
    let energy = |values: &[f32]| {
        values
            .iter()
            .map(|value| f64::from(*value).powi(2))
            .sum::<f64>()
    };
    let norm = (energy(reference) * energy(other)).sqrt();
    if norm <= f64::EPSILON {
        return None;
    }
    let agreement = |lag: isize| -> f64 {
        let (from, until) = if lag >= 0 {
            (lag.unsigned_abs(), length)
        } else {
            (0, length - lag.unsigned_abs())
        };
        (from..until)
            .map(|index| {
                let delayed = index.wrapping_add_signed(-lag);
                f64::from(reference[index]) * f64::from(other[delayed])
            })
            .sum()
    };
    let most_lag = isize::try_from(most_steps).ok()?;
    let values: Vec<f64> = (-most_lag..=most_lag).map(agreement).collect();
    let (best, peak) = values
        .iter()
        .copied()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(&b.1))?;
    if peak / norm < LEAST_AGREEMENT {
        return None;
    }
    #[allow(clippy::cast_precision_loss)]
    let mut shift = best as f64 - most_steps as f64;
    if best > 0 && best + 1 < values.len() {
        let (before, after) = (values[best - 1], values[best + 1]);
        let curve = before - 2.0 * peak + after;
        if curve.abs() > f64::EPSILON {
            shift += 0.5 * (before - after) / curve;
        }
    }
    Some(seconds(shift * step))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(kind: StreamKind, first: f64, end: f64, largest_gap: f64) -> StreamTiming {
        StreamTiming {
            kind,
            size: (kind == StreamKind::Picture).then_some((1920, 1080)),
            packets: 100,
            first,
            end,
            largest_gap,
        }
    }

    fn at(second: f64) -> OffsetDateTime {
        OffsetDateTime::UNIX_EPOCH + seconds(second)
    }

    #[test]
    fn a_good_file_has_its_streams_from_its_start_to_its_end() {
        let probe = Probe {
            streams: vec![
                stream(StreamKind::Picture, 0.023, 120.0, 0.034),
                stream(StreamKind::Sound, 0.0, 120.02, 0.021),
            ],
        };
        let timed = timing(&probe, true);
        assert_eq!(timed.problems, []);
        assert_eq!(timed.media_ms, 120_020);
        assert_eq!(timed.size, Some((1920, 1080)));
        assert!(timed.has_sound);

        let track = Probe {
            streams: vec![stream(StreamKind::Sound, -0.0065, 60.0, 0.02)],
        };
        let timed = timing(&track, false);
        assert_eq!(timed.problems, []);
        assert_eq!(timed.media_ms, 60_007);
        assert_eq!(timed.size, None);
    }

    #[test]
    fn a_missing_stream_a_short_stream_and_a_hole_are_problems() {
        let without_picture = Probe {
            streams: vec![stream(StreamKind::Sound, 0.0, 30.0, 0.02)],
        };
        assert_eq!(
            timing(&without_picture, true).problems,
            [FileProblem::StreamMissing]
        );
        assert_eq!(timing(&without_picture, false).problems, []);
        assert_eq!(
            timing(&Probe::default(), false).problems,
            [FileProblem::StreamMissing]
        );
        let empty = Probe {
            streams: vec![StreamTiming {
                packets: 0,
                ..stream(StreamKind::Sound, 0.0, 0.0, 0.0)
            }],
        };
        assert_eq!(timing(&empty, false).problems, [FileProblem::StreamMissing]);

        let picture_ends_early = Probe {
            streams: vec![
                stream(StreamKind::Picture, 0.0, 15.0, 0.034),
                stream(StreamKind::Sound, 0.0, 36.3, 0.021),
            ],
        };
        assert_eq!(
            timing(&picture_ends_early, true).problems,
            [FileProblem::StreamShort]
        );
        let sound_begins_late = Probe {
            streams: vec![
                stream(StreamKind::Picture, 0.0, 36.0, 0.034),
                stream(StreamKind::Sound, 1.5, 36.0, 0.021),
            ],
        };
        assert_eq!(
            timing(&sound_begins_late, true).problems,
            [FileProblem::StreamShort]
        );

        let holes = Probe {
            streams: vec![
                stream(StreamKind::Picture, 0.0, 36.0, 0.9),
                stream(StreamKind::Sound, 0.0, 36.0, 0.26),
            ],
        };
        assert_eq!(timing(&holes, true).problems, [FileProblem::TimestampHole]);
        let at_the_bounds = Probe {
            streams: vec![
                stream(StreamKind::Picture, 0.0, 36.0, 1.0),
                stream(StreamKind::Sound, 1.0, 35.0, 0.25),
            ],
        };
        assert_eq!(timing(&at_the_bounds, true).problems, []);
    }

    #[test]
    fn the_length_agrees_within_three_seconds_and_a_thousandth() {
        assert!(!length_differs(32_200, 35_200));
        assert!(length_differs(32_200, 40_000));
        assert!(!length_differs(3_000, 0));
        assert!(length_differs(3_001, 0));
        let three_hours = 3 * 3600 * 1000;
        assert!(!length_differs(three_hours + 13_800, three_hours));
        assert!(length_differs(three_hours + 13_801, three_hours));
        assert!(length_differs(three_hours - 13_801, three_hours));
    }

    #[test]
    fn digital_silence_and_blank_pictures_are_told() {
        let with_peak = |peak_db| Sound {
            peak_db,
            silences: Vec::new(),
        };
        assert!(is_silent(&with_peak(None)));
        assert!(is_silent(&with_peak(Some(-91.0))));
        assert!(!is_silent(&with_peak(Some(-60.0))));
        assert!(!is_silent(&with_peak(Some(-20.9))));

        let mut frames = KeyFrames::default();
        assert!(picture_is_blank(&frames));
        for _ in 0..95 {
            frames.count(120.0, 24.0, 214.0);
        }
        for _ in 0..5 {
            frames.count(16.0, 16.0, 16.0);
        }
        assert_eq!((frames.total, frames.dark, frames.one_colour), (100, 5, 5));
        assert!(!picture_is_blank(&frames));
        frames.count(23.9, 0.0, 200.0);
        assert_eq!((frames.dark, frames.one_colour), (6, 5));
        assert!(picture_is_blank(&frames));

        let mut flat = KeyFrames::default();
        for _ in 0..4 {
            flat.count(128.0, 120.0, 135.9);
        }
        assert!(picture_is_blank(&flat));
    }

    #[test]
    fn files_are_placed_by_the_measured_constants() {
        assert_eq!(track_start(at(100.0)), at(99.953));
        assert_eq!(composite_start(at(100.0), 40_000), at(59.865));
        assert_eq!(
            composite_start_by_sound(at(100.0), seconds(1.6)),
            at(98.353)
        );
    }

    fn heard(from: f64, until: f64, silences: &[(f64, f64)]) -> Heard {
        Heard {
            from: at(from),
            until: at(until),
            silences: silences
                .iter()
                .map(|(start, end)| (at(*start), at(*end)))
                .collect(),
        }
    }

    #[test]
    fn the_composite_lacks_one_stretch_where_it_is_silent_and_a_microphone_is_not() {
        let composite = heard(0.0, 60.0, &[(10.0, 18.0)]);
        assert_eq!(
            missing_stretch(&composite, &heard(0.0, 60.0, &[])),
            Some((at(10.0), at(18.0)))
        );
        // The microphone is silent in the same range, also when it is placed half a second
        // off: the pieces at the edges are shorter than the limit.
        assert_eq!(
            missing_stretch(&composite, &heard(0.0, 60.0, &[(9.5, 17.5)])),
            None
        );
        // Only the part inside the span of the microphone file counts.
        assert_eq!(missing_stretch(&composite, &heard(17.5, 60.0, &[])), None);
        assert_eq!(missing_stretch(&composite, &heard(0.0, 11.9, &[])), None);
        assert_eq!(
            missing_stretch(&composite, &heard(0.0, 12.5, &[])),
            Some((at(10.0), at(12.5)))
        );
        // Many short pieces do not add up.
        let patchy: Vec<(f64, f64)> = (0..40)
            .map(|index| (f64::from(index) * 1.5, f64::from(index) * 1.5 + 0.3))
            .collect();
        assert_eq!(
            missing_stretch(&heard(0.0, 60.0, &patchy), &heard(0.0, 60.0, &[])),
            None
        );
    }

    /// A window whose envelopes matched the wrong phrase gives a point far from the line of
    /// its neighbours; the drift does not follow it.
    #[test]
    fn a_wrong_match_of_one_window_is_not_followed() {
        let start = OffsetDateTime::UNIX_EPOCH;
        let at = |seconds: i64| start + Duration::seconds(seconds);
        let shift = |millis: i64| Duration::milliseconds(millis);
        // A drift of 35 ms per second, measured every 15 seconds; the window at 97 s matched
        // the next phrase, 6 seconds away.
        let drift = Drift::of(vec![
            (at(7), shift(118)),
            (at(22), shift(1_701)),
            (at(37), shift(2_144)),
            (at(52), shift(2_676)),
            (at(67), shift(3_235)),
            (at(82), shift(3_702)),
            (at(97), shift(-2_051)),
            (at(112), shift(4_718)),
        ]);
        let at_97 = drift.at(at(97));
        assert!(at_97 > shift(3_900) && at_97 < shift(4_500), "{at_97:?}");
        assert_eq!(drift.at(at(112)), shift(4_718));
    }

    /// A recording of an hour whose microphone file drifts by 90 parts per million and has
    /// a pause every 20 seconds: placed with one offset the edges of the pauses disagree,
    /// and followed along the drift nothing is missing; a removed stretch of 3 seconds is
    /// found with its place.
    #[test]
    fn a_drift_between_the_files_is_followed_and_a_removed_stretch_is_found() {
        let length = 3600.0;
        let drift_rate = 0.000_09;
        // The microphone pauses for 2 seconds every 20 seconds; the composite holds the
        // same pauses later by the drift of that moment, and 1 second late at the start.
        let pauses: Vec<(f64, f64)> = (1..180)
            .map(|index| f64::from(index) * 20.0)
            .map(|start| (start, start + 2.0))
            .collect();
        let shift = |time: f64| 1.0 - drift_rate * time;
        let microphone = heard(0.0, length, &pauses);
        let in_composite: Vec<(f64, f64)> = pauses
            .iter()
            .map(|(start, end)| (start + shift(*start), end + shift(*end)))
            .collect();
        let composite = heard(0.0, length + 1.0, &in_composite);
        // With the offset of the start only, the pauses at the end are 0.3 seconds off: no
        // stretch is longer than the limit, but the pieces are many.
        let fixed = Drift::of(vec![(at(0.0), seconds(1.0))]);
        assert_eq!(
            missing_stretch(&composite, &microphone.drifted(&fixed)),
            None
        );
        // Followed every 5 minutes: nothing is missing.
        let points: Vec<(OffsetDateTime, Duration)> = (0..=12)
            .map(|index| f64::from(index) * 300.0)
            .map(|time| (at(time), seconds(shift(time))))
            .collect();
        let drift = Drift::of(points);
        assert_eq!(
            missing_stretch(&composite, &microphone.drifted(&drift)),
            None
        );
        // A stretch of 3 seconds of speech that the composite lost at 30 minutes is found
        // where the composite holds it.
        let mut lost = in_composite.clone();
        let place = 1805.0 + shift(1805.0);
        lost.push((place, place + 3.0));
        lost.sort_by(|a, b| a.0.total_cmp(&b.0));
        let found = missing_stretch(
            &heard(0.0, length + 1.0, &lost),
            &microphone.drifted(&drift),
        )
        .unwrap();
        assert!(
            (found.0 - at(place)).abs() < Duration::milliseconds(50),
            "{found:?}"
        );
        assert!((found.1 - found.0 - seconds(3.0)).abs() < Duration::milliseconds(50));
    }

    #[test]
    fn silent_ranges_are_placed_from_the_first_timestamp_of_the_file() {
        let timed = Timed {
            origin: 0.5,
            media_ms: 10_000,
            size: None,
            has_sound: true,
            has_picture: false,
            problems: Vec::new(),
        };
        let sound = Sound {
            peak_db: Some(-12.0),
            silences: vec![(2.5, Some(4.5)), (8.5, None)],
        };
        assert_eq!(
            Heard::of(at(100.0), &timed, &sound),
            heard(100.0, 110.0, &[(102.0, 104.0), (108.0, 110.0)])
        );
    }

    /// Bursts of a tone at irregular times, as samples.
    fn bursts(seconds_long: usize, delayed_by: f64) -> Vec<i16> {
        let rate = f64::from(ENVELOPE_RATE);
        (0..seconds_long * ENVELOPE_RATE as usize)
            .map(|index| {
                #[allow(clippy::cast_precision_loss)]
                let time = index as f64 / rate - delayed_by;
                let on = time >= 0.0
                    && [0.4, 1.3, 2.9, 4.1, 6.6, 7.2, 9.5]
                        .iter()
                        .any(|start| (*start..*start + 0.25).contains(&time));
                #[allow(clippy::cast_possible_truncation)]
                let sample = if on {
                    ((time * 440.0 * std::f64::consts::TAU).sin() * 8000.0) as i16
                } else {
                    0
                };
                sample
            })
            .collect()
    }

    #[test]
    fn the_shift_between_two_envelopes_is_found() {
        let reference = envelope(&bursts(12, 1.6));
        let other = envelope(&bursts(12, 0.0));
        assert_eq!(reference.len(), 12 * 500);
        let shift = best_shift(&reference, &other, Duration::seconds(3)).unwrap();
        assert!((shift.as_seconds_f64() - 1.6).abs() < 0.004, "{shift}");
        let back = best_shift(&other, &reference, Duration::seconds(3)).unwrap();
        assert!((back.as_seconds_f64() + 1.6).abs() < 0.004, "{back}");
        let none = best_shift(&reference, &reference, Duration::seconds(3)).unwrap();
        assert!(none.abs() < Duration::milliseconds(1), "{none}");
    }

    #[test]
    fn silence_and_unrelated_sound_place_nothing() {
        let sound = envelope(&bursts(12, 0.0));
        let silence = envelope(&vec![0; 12 * ENVELOPE_RATE as usize]);
        assert_eq!(best_shift(&sound, &silence, Duration::seconds(3)), None);
        assert_eq!(best_shift(&silence, &silence, Duration::seconds(3)), None);
        assert_eq!(best_shift(&sound, &[], Duration::seconds(3)), None);
        // A shift outside the searched range is not found as a clear agreement.
        let far = envelope(&bursts(12, 5.5));
        assert_eq!(best_shift(&far, &sound, Duration::seconds(1)), None);
    }
}
