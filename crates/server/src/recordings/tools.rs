//! The programs `ffprobe` and `ffmpeg` as the server uses them (ADR 0606): the timing of the
//! packets of a file, the level and the silent ranges of its sound, the brightness of its
//! key frames, its loudness, and the join of the pieces of a composite into one MP4 file.
//!
//! Each call is one child process with a time limit. The child gets no variable of the
//! environment of the server except `PATH`. It reads a file that the server opened as its
//! standard input, with the form of the file stated, and gets no path of an input.

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use crate::recordings::check::{
    ENVELOPE_RATE, KeyFrames, Probe, Sound, StreamKind, StreamTiming, envelope,
};

/// The most text of the messages of a child that the server keeps.
const KEPT_MESSAGES: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolError {
    /// The program read the file and could not use it: the file is not the media that the
    /// server expects.
    Unreadable(String),
    /// The program did not run to its end: it did not start, it reached the time limit, or
    /// it was ended from outside.
    Failed(String),
}

impl fmt::Display for ToolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable(reason) => write!(formatter, "the file cannot be used: {reason}"),
            Self::Failed(reason) => write!(formatter, "the program did not finish: {reason}"),
        }
    }
}

impl std::error::Error for ToolError {}

pub type ToolResult<T> = Result<T, ToolError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tools {
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
    time_limit: std::time::Duration,
}

/// What one stream of a file adds up to while its packets are read.
#[derive(Default)]
struct Counted {
    packets: u64,
    first: Option<f64>,
    latest: f64,
    end: f64,
    largest_gap: f64,
    kind: Option<StreamKind>,
    width: Option<u32>,
    height: Option<u32>,
}

/// The fields of one line that ffprobe writes as `section,key=value,key=value`.
fn fields(line: &str) -> (&str, impl Iterator<Item = (&str, &str)>) {
    let mut parts = line.trim_end().split(',');
    let section = parts.next().unwrap_or_default();
    (section, parts.filter_map(|part| part.split_once('=')))
}

fn read_probe_line(line: &str, streams: &mut BTreeMap<u32, Counted>) {
    let (section, pairs) = fields(line);
    match section {
        "packet" => {
            let (mut index, mut shown, mut decoded, mut length) = (None, None, None, 0.0);
            for (key, value) in pairs {
                match key {
                    "stream_index" => index = value.parse::<u32>().ok(),
                    "pts_time" => shown = value.parse::<f64>().ok(),
                    "dts_time" => decoded = value.parse::<f64>().ok(),
                    "duration_time" => length = value.parse::<f64>().unwrap_or(0.0),
                    _ => {}
                }
            }
            let (Some(index), Some(time)) = (index, shown.or(decoded)) else {
                return;
            };
            let stream = streams.entry(index).or_default();
            if stream.packets > 0 {
                stream.largest_gap = stream.largest_gap.max(time - stream.latest);
            }
            stream.packets += 1;
            stream.first = Some(stream.first.map_or(time, |first| first.min(time)));
            stream.latest = if stream.packets == 1 {
                time
            } else {
                stream.latest.max(time)
            };
            stream.end = if stream.packets == 1 {
                time + length
            } else {
                stream.end.max(time + length)
            };
        }
        "stream" => {
            let (mut index, mut kind, mut width, mut height) = (None, None, None, None);
            for (key, value) in pairs {
                match key {
                    "index" => index = value.parse::<u32>().ok(),
                    "codec_type" => {
                        kind = Some(match value {
                            "audio" => StreamKind::Sound,
                            "video" => StreamKind::Picture,
                            _ => StreamKind::Other,
                        });
                    }
                    "width" => width = value.parse::<u32>().ok().filter(|width| *width > 0),
                    "height" => height = value.parse::<u32>().ok().filter(|height| *height > 0),
                    _ => {}
                }
            }
            if let Some(index) = index {
                let stream = streams.entry(index).or_default();
                (stream.kind, stream.width, stream.height) = (kind, width, height);
            }
        }
        _ => {}
    }
}

fn probe_of(streams: BTreeMap<u32, Counted>) -> Probe {
    let streams = streams
        .into_values()
        .map(|stream| StreamTiming {
            kind: stream.kind.unwrap_or(StreamKind::Other),
            size: stream.width.zip(stream.height),
            packets: stream.packets,
            first: stream.first.unwrap_or(0.0),
            end: stream.end,
            largest_gap: stream.largest_gap,
        })
        .collect();
    Probe { streams }
}

/// The number after `label` in a line of ffmpeg, up to the next space.
fn number_after(line: &str, label: &str) -> Option<f64> {
    let rest = &line[line.find(label)? + label.len()..];
    rest.split_whitespace().next()?.parse().ok()
}

fn read_sound_line(line: &str, sound: &mut Sound) {
    if let Some(start) = number_after(line, "silence_start: ") {
        sound.silences.push((start, None));
    } else if let Some(end) = number_after(line, "silence_end: ") {
        if let Some(open) = sound.silences.last_mut().filter(|open| open.1.is_none()) {
            open.1 = Some(end);
        }
    } else if line.contains("max_volume: ") {
        sound.peak_db = number_after(line, "max_volume: ").filter(|peak| peak.is_finite());
    }
}

/// Collects the brightness values of the key frames that ffmpeg prints one frame after the
/// other.
#[derive(Default)]
struct Brightness {
    frames: KeyFrames,
    open: bool,
    average: Option<f64>,
    lowest: Option<f64>,
    highest: Option<f64>,
}

impl Brightness {
    fn close(&mut self) {
        if let (true, Some(average), Some(lowest), Some(highest)) =
            (self.open, self.average, self.lowest, self.highest)
        {
            self.frames.count(average, lowest, highest);
        }
        (self.average, self.lowest, self.highest) = (None, None, None);
        self.open = false;
    }

    fn read(&mut self, line: &str) {
        let value = |key: &str| {
            line.strip_prefix("lavfi.signalstats.")?
                .strip_prefix(key)?
                .strip_prefix('=')?
                .trim()
                .parse::<f64>()
                .ok()
        };
        if line.starts_with("frame:") {
            self.close();
            self.open = true;
        } else if let Some(average) = value("YAVG") {
            self.average = Some(average);
        } else if let Some(lowest) = value("YMIN") {
            self.lowest = Some(lowest);
        } else if let Some(highest) = value("YMAX") {
            self.highest = Some(highest);
        }
    }
}

async fn each_line<R: AsyncRead + Unpin>(reader: R, mut each: impl FnMut(&str)) {
    let mut reader = BufReader::new(reader);
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line).await {
            Ok(0) | Err(_) => return,
            Ok(_) => each(&String::from_utf8_lossy(&line)),
        }
    }
}

/// Reads a stream to its end and keeps the first part of it as text.
async fn messages<R: AsyncRead + Unpin>(mut reader: R) -> String {
    let mut kept = Vec::new();
    let mut buffer = [0u8; 1024];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                let room = KEPT_MESSAGES.saturating_sub(kept.len());
                kept.extend_from_slice(&buffer[..read.min(room)]);
            }
        }
    }
    String::from_utf8_lossy(&kept).trim().to_owned()
}

/// The form of a file that the server gives to a program. The program gets the form with
/// the file and does not find it out from the content, so that a file of another form, for
/// example a playlist that names further files, is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    /// An audio track: Ogg.
    Track,
    /// The video that the server joined: MP4.
    Video,
}

impl Container {
    const fn name(self) -> &'static str {
        match self {
            Self::Track => "ogg",
            Self::Video => "mp4",
        }
    }
}

/// The input of a program: the file that the server opened, as the standard input of the
/// child. No program gets a path of an input.
const INPUT: &str = "pipe:0";

/// Which output of a child carries what the server reads.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Reads {
    Output,
    Messages,
}

impl Tools {
    pub fn new(ffmpeg: PathBuf, ffprobe: PathBuf, time_limit: time::Duration) -> Self {
        Self {
            ffmpeg,
            ffprobe,
            time_limit: time_limit.unsigned_abs(),
        }
    }

    fn command(program: &Path) -> Command {
        let mut command = Command::new(program);
        command.env_clear().kill_on_drop(true);
        if let Some(path) = std::env::var_os("PATH") {
            command.env("PATH", path);
        }
        command
    }

    /// Runs a child to its end within the time limit. `work` reads its outputs.
    async fn finish<T>(
        &self,
        program: &Path,
        work: impl Future<Output = io::Result<(std::process::ExitStatus, String, T)>>,
    ) -> ToolResult<T> {
        let name = program.display();
        match tokio::time::timeout(self.time_limit, work).await {
            Err(_) => Err(ToolError::Failed(format!("{name} reached the time limit"))),
            Ok(Err(error)) => Err(ToolError::Failed(format!("{name}: {error}"))),
            Ok(Ok((status, _, value))) if status.success() => Ok(value),
            Ok(Ok((status, said, _))) if status.code().is_some() => {
                let said = said.lines().next().unwrap_or_default().to_owned();
                Err(ToolError::Unreadable(format!("{name}: {status}: {said}")))
            }
            Ok(Ok((status, _, _))) => Err(ToolError::Failed(format!("{name}: {status}"))),
        }
    }

    /// Runs a child and gives each line of one of its outputs to `each`. The child reads
    /// `input` as its standard input.
    async fn lines(
        &self,
        program: &Path,
        arguments: Vec<std::ffi::OsString>,
        input: Option<std::fs::File>,
        reads: Reads,
        each: impl FnMut(&str),
    ) -> ToolResult<()> {
        let work = async {
            let mut child = Self::command(program)
                .args(arguments)
                .stdin(input.map_or_else(Stdio::null, Stdio::from))
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()?;
            let (output, errors) = (child.stdout.take(), child.stderr.take());
            let (Some(output), Some(errors)) = (output, errors) else {
                return Err(io::Error::other("the child has no output"));
            };
            let said = match reads {
                Reads::Output => tokio::join!(each_line(output, each), messages(errors)).1,
                Reads::Messages => {
                    tokio::join!(each_line(errors, each), messages(output));
                    String::new()
                }
            };
            Ok((child.wait().await?, said, ()))
        };
        self.finish(program, work).await
    }

    /// Checks that both programs start. The server asks at its start, so that a missing
    /// program is an error of the configuration and not of a recording.
    pub async fn available(&self) -> Result<(), String> {
        for program in [&self.ffmpeg, &self.ffprobe] {
            let version = vec!["-version".into()];
            self.lines(program, version, None, Reads::Output, |_| {})
                .await
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    /// The timing of the packets of every stream of a file. No packet is decoded.
    pub async fn probe(&self, file: std::fs::File, container: Container) -> ToolResult<Probe> {
        let arguments = vec![
            "-v".into(),
            "error".into(),
            "-show_entries".into(),
            "packet=stream_index,pts_time,dts_time,duration_time:\
             stream=index,codec_type,width,height"
                .into(),
            "-of".into(),
            "csv=p=1:nk=0".into(),
            "-f".into(),
            container.name().into(),
            "-i".into(),
            INPUT.into(),
        ];
        let mut streams = BTreeMap::new();
        self.lines(
            &self.ffprobe,
            arguments,
            Some(file),
            Reads::Output,
            |line| {
                read_probe_line(line, &mut streams);
            },
        )
        .await?;
        Ok(probe_of(streams))
    }

    /// The highest level of the sound of a file and the ranges of at least one second in
    /// which it is below -70 dB.
    pub async fn sound(&self, file: std::fs::File, container: Container) -> ToolResult<Sound> {
        let arguments = vec![
            "-v".into(),
            "info".into(),
            "-nostdin".into(),
            "-nostats".into(),
            "-f".into(),
            container.name().into(),
            "-i".into(),
            INPUT.into(),
            "-vn".into(),
            "-af".into(),
            "silencedetect=noise=-70dB:d=1,volumedetect".into(),
            "-f".into(),
            "null".into(),
            "-".into(),
        ];
        let mut sound = Sound::default();
        self.lines(
            &self.ffmpeg,
            arguments,
            Some(file),
            Reads::Messages,
            |line| {
                read_sound_line(line, &mut sound);
            },
        )
        .await?;
        Ok(sound)
    }

    /// The brightness of the key frames of the picture of a video. Only key frames are
    /// decoded.
    pub async fn key_frames(&self, video: std::fs::File) -> ToolResult<KeyFrames> {
        let arguments = vec![
            "-v".into(),
            "error".into(),
            "-nostdin".into(),
            "-skip_frame".into(),
            "nokey".into(),
            "-f".into(),
            Container::Video.name().into(),
            "-i".into(),
            INPUT.into(),
            "-an".into(),
            "-fps_mode".into(),
            "passthrough".into(),
            "-vf".into(),
            "scale=160:90,signalstats,metadata=mode=print:file=-".into(),
            "-f".into(),
            "null".into(),
            "-".into(),
        ];
        let mut brightness = Brightness::default();
        self.lines(
            &self.ffmpeg,
            arguments,
            Some(video),
            Reads::Output,
            |line| {
                brightness.read(line);
            },
        )
        .await?;
        brightness.close();
        Ok(brightness.frames)
    }

    /// The loudness envelope of a part of the sound of a file (ADR 0602).
    pub async fn loudness(
        &self,
        file: std::fs::File,
        container: Container,
        from: f64,
        seconds: f64,
    ) -> ToolResult<Vec<f32>> {
        let work = async {
            let mut child = Self::command(&self.ffmpeg)
                .args(["-v", "error", "-nostdin", "-ss", &format!("{from:.3}")])
                .args(["-f", container.name(), "-i", INPUT])
                .args(["-t", &format!("{seconds:.3}"), "-map", "0:a:0", "-ac", "1"])
                .arg("-af")
                .arg(format!("aresample={ENVELOPE_RATE}:async=1"))
                .args(["-f", "s16le", "-"])
                .stdin(Stdio::from(file))
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()?;
            let (output, errors) = (child.stdout.take(), child.stderr.take());
            let (Some(mut output), Some(errors)) = (output, errors) else {
                return Err(io::Error::other("the child has no output"));
            };
            let mut raw = Vec::new();
            let (read, said) = tokio::join!(output.read_to_end(&mut raw), messages(errors));
            read?;
            Ok((child.wait().await?, said, raw))
        };
        let raw = self.finish(&self.ffmpeg, work).await?;
        let samples: Vec<i16> = raw
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| i16::from_le_bytes(*pair))
            .collect();
        Ok(envelope(&samples))
    }

    /// Joins the pieces of a composite into one MP4 file without encoding again. `open`
    /// opens the piece of each position; the bytes of the pieces are given to ffmpeg as one
    /// transport stream, as its reader of a closed playlist does.
    pub async fn join(
        &self,
        pieces: usize,
        open: impl Fn(usize) -> io::Result<std::fs::File>,
        output: &Path,
    ) -> ToolResult<()> {
        let work = async {
            let mut child = Self::command(&self.ffmpeg)
                .args([
                    "-v", "error", "-nostdin", "-y", "-f", "mpegts", "-i", "pipe:0",
                ])
                .args(["-c", "copy", "-movflags", "+faststart", "-f", "mp4"])
                .arg(output)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()?;
            let (input, errors) = (child.stdin.take(), child.stderr.take());
            let (Some(mut input), Some(errors)) = (input, errors) else {
                return Err(io::Error::other("the child has no input"));
            };
            let feed = async move {
                for position in 0..pieces {
                    let mut piece = tokio::fs::File::from_std(open(position)?);
                    tokio::io::copy(&mut piece, &mut input).await?;
                }
                input.shutdown().await
            };
            let (fed, said) = tokio::join!(feed, messages(errors));
            let status = child.wait().await?;
            // A feed that failed while ffmpeg ended with success gave ffmpeg a part of the
            // pieces only.
            if status.success() {
                fed?;
            }
            Ok((status, said, ()))
        };
        self.finish(&self.ffmpeg, work).await
    }
}

#[cfg(test)]
mod tests {
    use junto_protocol::recording::FileProblem;
    use time::Duration;

    use super::*;
    use crate::recorder::samples;
    use crate::recordings::check;

    /// A directory of one test with the sample files that the test writes into it.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let name = format!("junto-tools-{}", uuid::Uuid::new_v4().simple());
            let path = std::env::temp_dir().join(name);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, bytes).unwrap();
            path
        }

        /// Joins the pieces of a sample composite into a video.
        async fn video(&self, name: &str, pieces: &samples::Pieces) -> PathBuf {
            let parts: Vec<PathBuf> = pieces
                .iter()
                .enumerate()
                .map(|(number, piece)| self.file(&format!("{name}_{number:05}.ts"), piece))
                .collect();
            let video = self.0.join(format!("{name}.mp4"));
            tools()
                .join(
                    parts.len(),
                    |position| std::fs::File::open(&parts[position]),
                    &video,
                )
                .await
                .unwrap();
            video
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn open(path: &Path) -> std::fs::File {
        std::fs::File::open(path).unwrap()
    }

    fn tools() -> Tools {
        Tools::new("ffmpeg".into(), "ffprobe".into(), Duration::minutes(10))
    }

    fn near(value: i64, expected: i64, within: i64) -> bool {
        (value - expected).abs() <= within
    }

    #[tokio::test]
    async fn both_programs_are_found_and_a_missing_one_is_told() {
        tools().available().await.unwrap();
        let absent = Tools::new(
            "/nonexistent/ffmpeg".into(),
            "ffprobe".into(),
            Duration::minutes(1),
        );
        assert!(absent.available().await.is_err());
        let scratch = Scratch::new();
        let track = scratch.file("track.ogg", &samples::track());
        assert!(matches!(
            absent.sound(open(&track), Container::Track).await,
            Err(ToolError::Failed(_))
        ));
    }

    #[tokio::test]
    async fn a_track_file_is_measured() {
        let scratch = Scratch::new();
        let track = scratch.file("track.ogg", &samples::track());
        let probe = tools().probe(open(&track), Container::Track).await.unwrap();
        assert_eq!(probe.streams.len(), 1);
        assert_eq!(probe.streams[0].kind, StreamKind::Sound);
        assert!(probe.streams[0].packets > 50);
        let timed = check::timing(&probe, false);
        assert_eq!(timed.problems, []);
        assert!(near(timed.media_ms, 2000, 60), "{}", timed.media_ms);
        assert!(timed.has_sound && !timed.has_picture);

        let sound = tools().sound(open(&track), Container::Track).await.unwrap();
        assert!(sound.peak_db.is_some_and(|peak| peak > -40.0), "{sound:?}");
        assert!(sound.silences.is_empty());
        assert!(!check::is_silent(&sound));
    }

    #[tokio::test]
    async fn digital_silence_and_a_hole_in_the_timestamps_are_measured() {
        let scratch = Scratch::new();
        let silent = scratch.file("silent.ogg", &samples::silent_track());
        let sound = tools()
            .sound(open(&silent), Container::Track)
            .await
            .unwrap();
        assert!(check::is_silent(&sound), "{sound:?}");
        assert_eq!(sound.silences.len(), 1);
        let timed = check::timing(
            &tools()
                .probe(open(&silent), Container::Track)
                .await
                .unwrap(),
            false,
        );
        assert_eq!(timed.problems, []);

        let holed = scratch.file("holed.ogg", &samples::holed_track());
        let probe = tools().probe(open(&holed), Container::Track).await.unwrap();
        assert!(
            (0.9..1.1).contains(&probe.streams[0].largest_gap),
            "{probe:?}"
        );
        let timed = check::timing(&probe, false);
        assert_eq!(timed.problems, [FileProblem::TimestampHole]);
        assert!(near(timed.media_ms, 3000, 60), "{}", timed.media_ms);
    }

    #[tokio::test]
    async fn the_pieces_of_a_composite_are_joined_into_a_video_that_passes_the_check() {
        let scratch = Scratch::new();
        let video = scratch.video("composite", &samples::composite()).await;
        let probe = tools().probe(open(&video), Container::Video).await.unwrap();
        let timed = check::timing(&probe, true);
        assert_eq!(timed.problems, [], "{probe:?}");
        assert_eq!(timed.size, Some((320, 180)));
        assert!(timed.has_sound && timed.has_picture);
        assert!(near(timed.media_ms, 2000, 150), "{}", timed.media_ms);
        // The video holds every frame of both pieces.
        let picture = probe
            .streams
            .iter()
            .find(|stream| stream.kind == StreamKind::Picture)
            .unwrap();
        assert_eq!(picture.packets, 20);

        let frames = tools().key_frames(open(&video)).await.unwrap();
        assert_eq!((frames.total, frames.dark, frames.one_colour), (2, 0, 0));
        assert!(!check::picture_is_blank(&frames));
        let sound = tools().sound(open(&video), Container::Video).await.unwrap();
        assert!(!check::is_silent(&sound));
    }

    #[tokio::test]
    async fn a_dark_picture_a_silent_video_and_a_video_without_sound_are_measured() {
        let scratch = Scratch::new();
        let dark = scratch.video("dark", &samples::dark_composite()).await;
        let frames = tools().key_frames(open(&dark)).await.unwrap();
        assert_eq!((frames.total, frames.dark, frames.one_colour), (2, 2, 2));
        assert!(check::picture_is_blank(&frames));

        let mute = scratch.video("mute", &samples::mute_composite()).await;
        assert!(check::is_silent(
            &tools().sound(open(&mute), Container::Video).await.unwrap()
        ));
        let timed = check::timing(
            &tools().probe(open(&mute), Container::Video).await.unwrap(),
            true,
        );
        assert_eq!(timed.problems, []);

        let soundless = scratch
            .video("soundless", &samples::soundless_composite())
            .await;
        let timed = check::timing(
            &tools()
                .probe(open(&soundless), Container::Video)
                .await
                .unwrap(),
            true,
        );
        assert_eq!(timed.problems, [FileProblem::StreamMissing]);
        assert!(!timed.has_sound && timed.has_picture);
    }

    #[tokio::test]
    async fn a_file_that_is_no_media_is_unreadable_and_a_time_limit_is_a_failure() {
        let scratch = Scratch::new();
        let garbage = scratch.file("garbage.ogg", b"this is no sound");
        assert!(matches!(
            tools().probe(open(&garbage), Container::Track).await,
            Err(ToolError::Unreadable(_))
        ));
        // A file of another form than the stated one is not read by its content: a video in
        // the place of a track, and a playlist that names another file.
        let video = scratch.video("composite", &samples::composite()).await;
        let as_track = scratch.file("video.ogg", &std::fs::read(&video).unwrap());
        assert!(matches!(
            tools().probe(open(&as_track), Container::Track).await,
            Err(ToolError::Unreadable(_))
        ));
        let playlist = "#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1.0,\ncomposite_00000.ts\n\
                        #EXT-X-ENDLIST\n";
        let named = scratch.file("playlist.ogg", playlist.as_bytes());
        for container in [Container::Track, Container::Video] {
            assert!(matches!(
                tools().probe(open(&named), container).await,
                Err(ToolError::Unreadable(_))
            ));
        }
        let track = scratch.file("track.ogg", &samples::track());
        let hasty = Tools::new("ffmpeg".into(), "ffprobe".into(), Duration::ZERO);
        match hasty.probe(open(&track), Container::Track).await {
            Err(ToolError::Failed(reason)) => assert!(reason.contains("time limit"), "{reason}"),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn a_join_fails_when_a_piece_cannot_be_opened_or_is_no_media() {
        let scratch = Scratch::new();
        let pieces = samples::composite();
        let first = scratch.file("part_00000.ts", &pieces[0]);
        let video = scratch.0.join("video.mp4");
        let open = |position: usize| match position {
            0 => std::fs::File::open(&first),
            _ => Err(io::Error::from(io::ErrorKind::NotFound)),
        };
        assert!(tools().join(2, open, &video).await.is_err());

        let garbage = scratch.file("garbage.ts", b"this is no transport stream");
        let joined = tools()
            .join(1, |_| std::fs::File::open(&garbage), &video)
            .await;
        assert!(
            matches!(joined, Err(ToolError::Unreadable(_))),
            "{joined:?}"
        );
    }

    #[tokio::test]
    async fn the_sound_of_a_composite_is_placed_against_a_track_by_its_loudness() {
        let scratch = Scratch::new();
        let track = scratch.file("bursts.ogg", &samples::bursts());
        let video = scratch.video("late", &samples::late_composite()).await;
        let of_video = tools()
            .loudness(open(&video), Container::Video, 0.0, 12.0)
            .await
            .unwrap();
        let of_track = tools()
            .loudness(open(&track), Container::Track, 0.0, 12.0)
            .await
            .unwrap();
        assert!(of_track.len() > 5500 && of_video.len() > 5500);
        let shift = check::best_shift(&of_video, &of_track, Duration::seconds(6)).unwrap();
        let late = Duration::milliseconds(samples::LATE_MS);
        assert!((shift - late).abs() < Duration::milliseconds(30), "{shift}");
    }

    #[test]
    fn probe_lines_add_up_per_stream() {
        let mut streams = BTreeMap::new();
        for line in [
            "packet,stream_index=1,pts_time=0.000000,dts_time=0.000000,duration_time=0.023220,",
            "packet,stream_index=0,pts_time=0.100000,dts_time=0.000000,duration_time=0.033000",
            "packet,stream_index=0,pts_time=0.033000,dts_time=0.033000,duration_time=0.033000",
            "packet,stream_index=1,pts_time=0.023220,dts_time=0.023220,duration_time=0.023197",
            "packet,stream_index=1,pts_time=2.523220,dts_time=2.523220,duration_time=0.023197",
            "packet,stream_index=2,pts_time=N/A,dts_time=N/A,duration_time=N/A",
            "packet,stream_index=0,pts_time=N/A,dts_time=1.200000,duration_time=N/A",
            "stream,index=0,codec_type=video,width=1920,height=1080",
            "stream,index=1,codec_type=audio",
            "stream,index=2,codec_type=data",
            "format,duration=2.5",
            "",
        ] {
            read_probe_line(line, &mut streams);
        }
        let probe = probe_of(streams);
        assert_eq!(probe.streams.len(), 3);
        let picture = &probe.streams[0];
        assert_eq!(picture.kind, StreamKind::Picture);
        assert_eq!(picture.size, Some((1920, 1080)));
        assert_eq!(picture.packets, 3);
        assert!((picture.first - 0.033).abs() < 1e-9);
        assert!((picture.end - 1.2).abs() < 1e-9);
        assert!((picture.largest_gap - 1.1).abs() < 1e-9);
        let sound = &probe.streams[1];
        assert_eq!(
            (sound.kind, sound.size, sound.packets),
            (StreamKind::Sound, None, 3)
        );
        assert!((sound.largest_gap - 2.5).abs() < 1e-9);
        assert!((sound.end - 2.546_417).abs() < 1e-6);
        assert_eq!(
            (probe.streams[2].kind, probe.streams[2].packets),
            (StreamKind::Other, 0)
        );
    }

    #[test]
    fn sound_lines_give_the_peak_and_the_silent_ranges() {
        let mut sound = Sound::default();
        for line in [
            "[Parsed_volumedetect_1 @ 0xb33028540] n_samples: 0",
            "[Parsed_silencedetect_0 @ 0xb330289c0] silence_start: 2.034375",
            "[Parsed_silencedetect_0 @ 0xb330289c0] silence_end: 4.999896 | silence_duration: 2.965521",
            "[Parsed_silencedetect_0 @ 0xb330289c0] silence_start: -0.0065",
            "[Parsed_silencedetect_0 @ 0xb330289c0] silence_end: 1 | silence_duration: 1.0065",
            "[Parsed_silencedetect_0 @ 0xb330289c0] silence_start: 9",
            "[Parsed_volumedetect_1 @ 0xb33028a80] mean_volume: -27.1 dB",
            "[Parsed_volumedetect_1 @ 0xb33028a80] max_volume: -20.9 dB",
        ] {
            read_sound_line(line, &mut sound);
        }
        assert_eq!(sound.peak_db, Some(-20.9));
        assert_eq!(
            sound.silences,
            [
                (2.034_375, Some(4.999_896)),
                (-0.0065, Some(1.0)),
                (9.0, None)
            ]
        );
        let mut silent = Sound::default();
        read_sound_line(
            "[Parsed_volumedetect_1 @ 0x1] max_volume: -inf dB",
            &mut silent,
        );
        assert_eq!(silent.peak_db, None);
        read_sound_line(
            "[Parsed_volumedetect_1 @ 0x1] max_volume: -91.0 dB",
            &mut silent,
        );
        assert_eq!(silent.peak_db, Some(-91.0));
    }

    #[test]
    fn key_frame_lines_are_counted_per_frame() {
        let mut brightness = Brightness::default();
        for line in [
            "lavfi.signalstats.YAVG=1",
            "frame:0    pts:2090    pts_time:0.0232222",
            "lavfi.signalstats.YMIN=24",
            "lavfi.signalstats.YAVG=120.781",
            "lavfi.signalstats.YMAX=214",
            "lavfi.signalstats.YDIF=0",
            "frame:1    pts:92090   pts_time:1.023222",
            "lavfi.signalstats.YMIN=16",
            "lavfi.signalstats.YAVG=16",
            "lavfi.signalstats.YMAX=16",
            "frame:2    pts:182090   pts_time:2.023222",
            "lavfi.signalstats.YMIN=16",
        ] {
            brightness.read(line);
        }
        brightness.close();
        let frames = brightness.frames;
        assert_eq!((frames.total, frames.dark, frames.one_colour), (2, 1, 1));
    }
}
