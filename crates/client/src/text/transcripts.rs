//! The text of the changes of a transcript, of its details, of the parts of a recording,
//! and of the language of a new recording (ADR 0616).

use junto_client_core::protocol::recording::{RecordingSegment, SegmentFailure, SegmentState};
use junto_client_core::protocol::transcript::{
    SegmentReason, SegmentTranscription, SpeakerSeparation, SpeakersReason, TranscriptSegment,
};

// A line of a transcript.
pub const CORRECT: &str = "Correct";
pub const CORRECT_LINE: &str = "Correct the text of this line";
pub const CORRECTED_TEXT: &str = "Corrected text";
pub const ORIGINAL: &str = "Original";
pub const SHOW_ORIGINAL: &str = "Show the text that the speech model wrote and who corrected it";
pub const CORRECTION_EMPTY: &str = "Type the corrected text.";
pub const CORRECTION_LONG: &str = "A correction has at most 2,000 characters.";
pub const CONTROL_CHARACTER: &str = "The text holds a control character, such as a line break.";
pub const CORRECTIONS_FULL: &str =
    "This line has 100 corrections, which is the most that the server keeps.";
pub const CORRECTION_REFUSED: &str =
    "The server did not accept the text. A correction has 1 to 2,000 characters and no line break.";

// A voice of shared sound.
pub const VOICES: &str = "Voices of shared sound";
pub const NAME_VOICE: &str = "Name";
pub const RENAME_VOICE: &str = "Rename";
pub const NAME_VOICE_HELP: &str = "Give this voice a name in this recording";
pub const VOICE_NAME: &str = "Name of the voice";
pub const NAME_EMPTY: &str = "Type a name.";
pub const NAME_LONG: &str = "A name has at most 64 characters.";
pub const NAME_REFUSED: &str =
    "The server did not accept the name. A name has 1 to 64 characters and no line break.";

// Refusals of a change.
pub const ARCHIVED: &str =
    "The channel of this recording is archived. Its transcript takes no change.";
pub const CHANGE_NOT_ALLOWED: &str = "Your account may not change this transcript.";
pub const CHANGE_GONE: &str = "The server holds this line or this voice no longer.";
pub const CHANGED_MEANWHILE: &str = "This changed on the server after the form was opened. \
     The form shows the new value when it arrives; save again to replace it.";
pub const CHANGED_TAKEN: &str =
    "This changed on the server meanwhile. The form holds the new text.";

/// The line or the name changed on the server while the person edited it: by another person,
/// or by the same account on another device.
pub fn changed_kept(now: &str) -> String {
    format!("This changed on the server meanwhile to “{now}”. Save replaces it with your text.")
}
pub const REQUEST_NOT_ALLOWED: &str = "You do not hold “manage recordings” in this channel. The server did not ask for a \
     transcript.";
pub const REQUEST_CONFLICT: &str = "The server does not transcribe this recording now: it has \
     no transcription worker, the recording has no audio file with a place on its timeline, \
     or a transcript was asked for already.";

// The details of a transcript.
pub const DETAILS: &str = "Details";
pub const DETAILS_HELP: &str =
    "The glossary terms, the voices and the result of each audio file of this transcript";
pub const GLOSSARY: &str = "Glossary";
pub const GLOSSARY_EMPTY: &str = "The speech model received no glossary term.";
pub const AUDIO_FILES: &str = "Audio files";
pub const UNKNOWN_FILE: &str = "An audio file that the recording does not list";

// A new request for a transcript.
pub const REQUEST: &str = "Transcribe";
pub const REQUEST_AGAIN: &str = "Transcribe again";
pub const REQUEST_HELP: &str = "Ask the server to write the transcript of this recording";
pub const REQUESTING: &str = "Asking…";
pub const REQUEST_NEEDS: &str = "You do not hold “manage recordings” in this workspace.";

// The parts of a recording.
pub const PARTS: &str = "Parts";
pub const PARTS_HELP: &str = "Every file of this recording with the time range that it holds";

// The language of a new recording.
pub const LANGUAGE_MENU: &str = "Choose the spoken language to start recording";
pub const DETECT: &str = "Detect the language of each sentence";
pub const DETECT_HELP: &str = "For speech in more than one language";
pub const ONE_LANGUAGE: &str = "One language for the whole recording";
pub const CANCEL_RECORDING: &str = "Cancel";
pub const CANCEL_RECORDING_HELP: &str = "Cancel the recording before it starts";

pub fn changed_by(by: &str, at: &str) -> String {
    if at.is_empty() {
        format!("Corrected by {by}")
    } else {
        format!("Corrected by {by}, {at}")
    }
}

pub fn named_by(by: &str, at: &str) -> String {
    if at.is_empty() {
        format!("Named by {by}")
    } else {
        format!("Named by {by}, {at}")
    }
}

/// A voice and whose shared sound holds it: "Speaker 2 · shared sound of Ana".
pub fn voice_of(label: &str, of: &str) -> String {
    format!("{label} · shared sound of {of}")
}

pub fn glossary_used(terms: &[String]) -> String {
    format!("Used ({}): {}", terms.len(), terms.join(", "))
}

pub fn glossary_cut(terms: &[String]) -> String {
    format!(
        "Not used, over the limit of 200 tokens ({}): {}",
        terms.len(),
        terms.join(", ")
    )
}

/// The count of the parts of a recording on their button.
pub fn parts(count: usize) -> String {
    format!("{PARTS} ({count})")
}

/// The time range of a part: "09:41:02 – 09:42:30".
pub fn part_span(started: Option<&str>, ended: Option<&str>) -> String {
    match (started, ended) {
        (Some(started), Some(ended)) => format!("{started} – {ended}"),
        (Some(started), None) => format!("from {started}, no end reported"),
        (None, Some(ended)) => format!("until {ended}, no start reported"),
        (None, None) => "no time reported".to_owned(),
    }
}

/// The state of a part of a recording. `processed`: the server has checked the files of
/// the recording.
pub fn part_state(segment: &RecordingSegment, processed: bool) -> String {
    use junto_client_core::protocol::recording::FileCheck;
    match (segment.state, &segment.file) {
        (SegmentState::Starting, _) => "starting…".to_owned(),
        (SegmentState::Recording, _) => "being recorded".to_owned(),
        (SegmentState::Failed, file) => {
            let reason = match segment.failure {
                Some(SegmentFailure::RecorderUnavailable) => "no recorder was available",
                Some(SegmentFailure::StartRefused) => "the recorder refused to start it",
                Some(SegmentFailure::StartInterrupted) => {
                    "the server stopped before the recorder started it"
                }
                Some(SegmentFailure::RecorderLost) => {
                    "the recorder lost its connection while recording"
                }
                Some(SegmentFailure::RecorderError) => "the recorder ended it with an error",
                Some(SegmentFailure::Unknown) | None => {
                    "for a reason that this version does not know"
                }
            };
            if file.is_some() {
                format!("failed, {reason}; the saved part is kept")
            } else {
                format!("not saved: {reason}")
            }
        }
        (SegmentState::Ended, Some(file)) if file.check == FileCheck::Failed => format!(
            "saved, failed the check: {}",
            crate::text::file_problems(&file.problems)
        ),
        (SegmentState::Ended, Some(_)) => "saved".to_owned(),
        (SegmentState::Ended, None) if processed => "no file was found".to_owned(),
        (SegmentState::Ended, None) => "ended; the server is checking its file".to_owned(),
    }
}

fn segment_reason(reason: Option<SegmentReason>) -> &'static str {
    match reason {
        Some(SegmentReason::FileMissing) => "the file is missing",
        Some(SegmentReason::NotARegularFile) => "the file is not a regular file",
        Some(SegmentReason::DecodeFailed) => "the sound could not be decoded",
        Some(SegmentReason::NotOneAudioStream) => "the file does not hold one sound stream",
        Some(SegmentReason::DecodeIncomplete) => "the sound could be decoded only in part",
        Some(SegmentReason::TimestampGap) => "the times of the sound have a gap",
        Some(SegmentReason::TimestampBackward) => "the times of the sound go back",
        Some(SegmentReason::SegmentDurationLimit) => "the file is longer than the worker reads",
        Some(SegmentReason::JobDurationLimit) => "the recording is longer than the worker reads",
        Some(SegmentReason::TimeLimit) => "the worker reached its time limit",
        Some(SegmentReason::ProcessingFailed) => "the speech model failed",
        Some(SegmentReason::ShorterThanFile) => "the worker read less sound than the file holds",
        Some(SegmentReason::NoFile) => "the server found no file",
        Some(SegmentReason::NoStartTime) => "the recorder reported no start time for the file",
        Some(SegmentReason::Unknown) | None => "for a reason that this version does not know",
    }
}

fn speakers_reason(reason: Option<SpeakersReason>) -> &'static str {
    match reason {
        Some(SpeakersReason::SegmentFailed) => "the file was not transcribed",
        Some(SpeakersReason::EmbeddingFailed) => "the voice model failed",
        Some(SpeakersReason::ClusteringFailed) => "the grouping of the voices failed",
        Some(SpeakersReason::NoVoiceWindow) => "the speech of a line was too short",
        Some(SpeakersReason::SpeakerWindowLimit) => "the limit of voice samples was reached",
        Some(SpeakersReason::TimeLimit) => "the time limit was reached",
        Some(SpeakersReason::Unknown) | None => "for a reason that this version does not know",
    }
}

fn clock(milliseconds: u64) -> String {
    crate::text::clock_of(milliseconds)
}

/// What the transcript holds of one audio file.
pub fn segment_result(result: &TranscriptSegment) -> String {
    let lines = match result.lines {
        1 => "1 line".to_owned(),
        lines => format!("{lines} lines"),
    };
    let mut sentence = match result.status {
        SegmentTranscription::Ok => format!("transcribed, {lines}"),
        SegmentTranscription::Empty => "transcribed, no speech".to_owned(),
        SegmentTranscription::Damaged => match result.processed_ms {
            Some(processed) => format!(
                "transcribed up to {} of the file, {lines}: {}",
                clock(processed),
                segment_reason(result.reason)
            ),
            None => format!(
                "transcribed in part, {lines}: {}",
                segment_reason(result.reason)
            ),
        },
        SegmentTranscription::Failed => {
            format!("not transcribed: {}", segment_reason(result.reason))
        }
        SegmentTranscription::NotTranscribed => format!(
            "not given to the speech model: {}",
            segment_reason(result.reason)
        ),
    };
    let voices = match result.speakers {
        SpeakerSeparation::NotApplicable => None,
        SpeakerSeparation::NotRequested => Some("voices not separated".to_owned()),
        SpeakerSeparation::Separated => Some("voices separated".to_owned()),
        SpeakerSeparation::Unassigned => Some(format!(
            "some lines have no voice: {}",
            speakers_reason(result.speakers_reason)
        )),
        SpeakerSeparation::Failed => Some(format!(
            "voices not separated: {}",
            speakers_reason(result.speakers_reason)
        )),
    };
    if let Some(voices) = voices {
        sentence = format!("{sentence}; {voices}");
    }
    sentence
}

/// The languages that the speech model knows, by their codes (ADR 0613), in the order of
/// their English names. The worker refuses a code that its model does not know.
pub const LANGUAGES: &[(&str, &str)] = &[
    ("af", "Afrikaans"),
    ("sq", "Albanian"),
    ("am", "Amharic"),
    ("ar", "Arabic"),
    ("hy", "Armenian"),
    ("as", "Assamese"),
    ("az", "Azerbaijani"),
    ("ba", "Bashkir"),
    ("eu", "Basque"),
    ("be", "Belarusian"),
    ("bn", "Bengali"),
    ("bs", "Bosnian"),
    ("br", "Breton"),
    ("bg", "Bulgarian"),
    ("yue", "Cantonese"),
    ("ca", "Catalan"),
    ("zh", "Chinese"),
    ("hr", "Croatian"),
    ("cs", "Czech"),
    ("da", "Danish"),
    ("nl", "Dutch"),
    ("en", "English"),
    ("et", "Estonian"),
    ("fo", "Faroese"),
    ("fi", "Finnish"),
    ("fr", "French"),
    ("gl", "Galician"),
    ("ka", "Georgian"),
    ("de", "German"),
    ("el", "Greek"),
    ("gu", "Gujarati"),
    ("ht", "Haitian Creole"),
    ("ha", "Hausa"),
    ("haw", "Hawaiian"),
    ("he", "Hebrew"),
    ("hi", "Hindi"),
    ("hu", "Hungarian"),
    ("is", "Icelandic"),
    ("id", "Indonesian"),
    ("it", "Italian"),
    ("ja", "Japanese"),
    ("jw", "Javanese"),
    ("kn", "Kannada"),
    ("kk", "Kazakh"),
    ("km", "Khmer"),
    ("ko", "Korean"),
    ("lo", "Lao"),
    ("la", "Latin"),
    ("lv", "Latvian"),
    ("ln", "Lingala"),
    ("lt", "Lithuanian"),
    ("lb", "Luxembourgish"),
    ("mk", "Macedonian"),
    ("mg", "Malagasy"),
    ("ms", "Malay"),
    ("ml", "Malayalam"),
    ("mt", "Maltese"),
    ("mi", "Maori"),
    ("mr", "Marathi"),
    ("mn", "Mongolian"),
    ("my", "Myanmar"),
    ("ne", "Nepali"),
    ("no", "Norwegian"),
    ("nn", "Nynorsk"),
    ("oc", "Occitan"),
    ("ps", "Pashto"),
    ("fa", "Persian"),
    ("pl", "Polish"),
    ("pt", "Portuguese"),
    ("pa", "Punjabi"),
    ("ro", "Romanian"),
    ("ru", "Russian"),
    ("sa", "Sanskrit"),
    ("sr", "Serbian"),
    ("sn", "Shona"),
    ("sd", "Sindhi"),
    ("si", "Sinhala"),
    ("sk", "Slovak"),
    ("sl", "Slovenian"),
    ("so", "Somali"),
    ("es", "Spanish"),
    ("su", "Sundanese"),
    ("sw", "Swahili"),
    ("sv", "Swedish"),
    ("tl", "Tagalog"),
    ("tg", "Tajik"),
    ("ta", "Tamil"),
    ("tt", "Tatar"),
    ("te", "Telugu"),
    ("th", "Thai"),
    ("bo", "Tibetan"),
    ("tr", "Turkish"),
    ("tk", "Turkmen"),
    ("uk", "Ukrainian"),
    ("ur", "Urdu"),
    ("uz", "Uzbek"),
    ("vi", "Vietnamese"),
    ("cy", "Welsh"),
    ("yi", "Yiddish"),
    ("yo", "Yoruba"),
];

/// The English name of a language code, or the code itself.
pub fn language_name(code: &str) -> Option<&'static str> {
    LANGUAGES
        .iter()
        .find(|(known, _)| *known == code)
        .map(|(_, name)| *name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_languages_are_the_codes_that_the_server_takes_once_each_in_the_order_of_their_names() {
        assert_eq!(LANGUAGES.len(), 100);
        for (code, _) in LANGUAGES {
            assert!(
                (2..=3).contains(&code.len()) && code.bytes().all(|byte| byte.is_ascii_lowercase()),
                "{code}"
            );
            assert_ne!(*code, "auto");
        }
        let mut codes: Vec<&str> = LANGUAGES.iter().map(|(code, _)| *code).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), LANGUAGES.len());
        assert!(LANGUAGES.windows(2).all(|pair| pair[0].1 < pair[1].1));
        assert_eq!(language_name("pt"), Some("Portuguese"));
        assert_eq!(language_name("xx"), None);
    }
}
pub const CHANGE_SAVED: &str = "The change is saved.";
