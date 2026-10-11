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
/// The composite may lack this much of the sound of one microphone file.
const MISSING_SOUND: Duration = Duration::seconds(1);

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

fn overlap(a: (OffsetDateTime, OffsetDateTime), b: (OffsetDateTime, OffsetDateTime)) -> Duration {
    (a.1.min(b.1) - a.0.max(b.0)).max(Duration::ZERO)
}

/// True when the composite is silent, for more than the allowed time, in ranges in which
/// the microphone file `reference` has sound.
pub fn lacks_sound_of(composite: &Heard, reference: &Heard) -> bool {
    let span = (reference.from, reference.until);
    let mut missing = Duration::ZERO;
    for silence in &composite.silences {
        let inside = overlap(*silence, span);
        if inside.is_zero() {
            continue;
        }
        let clamped = (silence.0.max(span.0), silence.1.min(span.1));
        let covered = reference
            .silences
            .iter()
            .fold(Duration::ZERO, |sum, quiet| sum + overlap(clamped, *quiet));
        missing += (inside - covered).max(Duration::ZERO);
    }
    missing > MISSING_SOUND
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
    fn the_composite_lacks_sound_where_it_is_silent_and_a_microphone_is_not() {
        let composite = heard(0.0, 60.0, &[(10.0, 18.0)]);
        assert!(lacks_sound_of(&composite, &heard(0.0, 60.0, &[])));
        // The microphone is silent in the same range, also when it is placed half a second
        // off.
        assert!(!lacks_sound_of(
            &composite,
            &heard(0.0, 60.0, &[(9.5, 17.5)])
        ));
        // Only the part inside the span of the microphone file counts.
        assert!(!lacks_sound_of(&composite, &heard(17.5, 60.0, &[])));
        assert!(!lacks_sound_of(&composite, &heard(0.0, 10.9, &[])));
        assert!(lacks_sound_of(&composite, &heard(0.0, 11.5, &[])));
        // Several short ranges add up.
        let patchy = heard(0.0, 60.0, &[(5.0, 5.6), (20.0, 20.6)]);
        assert!(lacks_sound_of(&patchy, &heard(0.0, 60.0, &[])));
        assert!(!lacks_sound_of(
            &heard(0.0, 60.0, &[]),
            &heard(0.0, 60.0, &[])
        ));
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
