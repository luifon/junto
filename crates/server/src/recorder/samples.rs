//! Media files for the recorder in memory (ADR 0904): what LiveKit Egress writes for a job,
//! generated with `ffmpeg`. A test that lets the server process a recording needs an
//! `ffmpeg` with the encoders `libopus` and `libx264` on its path.
//!
//! Each file is generated once in a process and kept in memory.

use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, OnceLock};

/// The length of the plain samples, in seconds. A test server reports a job that ran for no
/// time; a file of this length agrees with that within the tolerance of the check.
pub const SECONDS: u32 = 2;

/// The time by which the sound of [`late_composite`] is later than the sound of
/// [`bursts`], in milliseconds.
pub const LATE_MS: i64 = 1600;

/// The pieces of a composite as the recorder writes them.
pub type Pieces = Arc<Vec<Vec<u8>>>;

const TONE: &str = "sine=frequency=440:duration=2";
const PICTURE: &str = "testsrc2=size=320x180:rate=10:duration=2";

/// Runs ffmpeg in a new directory and returns the files that it wrote there, in the order
/// of their names.
fn generated(arguments: &[&str]) -> Vec<Vec<u8>> {
    let directory: PathBuf =
        std::env::temp_dir().join(format!("junto-samples-{}", uuid::Uuid::new_v4().simple()));
    let run = || -> std::io::Result<Vec<Vec<u8>>> {
        std::fs::create_dir_all(&directory)?;
        let program = std::env::var_os("JUNTO_FFMPEG").unwrap_or_else(|| "ffmpeg".into());
        let output = Command::new(program)
            .args(["-v", "error", "-nostdin", "-y"])
            .args(arguments)
            .current_dir(&directory)
            .output()?;
        if !output.status.success() {
            return Err(std::io::Error::other(
                String::from_utf8_lossy(&output.stderr).into_owned(),
            ));
        }
        let mut names: Vec<PathBuf> = std::fs::read_dir(&directory)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<std::io::Result<_>>()?;
        names.sort();
        names.iter().map(std::fs::read).collect()
    };
    let files = run();
    let _ = std::fs::remove_dir_all(&directory);
    match files {
        Ok(files) if !files.is_empty() => files,
        Ok(_) => panic!("ffmpeg wrote no sample file"),
        Err(error) => panic!(
            "a sample file could not be generated. The tests of recordings need ffmpeg with \
             libopus and libx264: {error}"
        ),
    }
}

fn track_of(source: &str, more: &[&str]) -> Arc<Vec<u8>> {
    let mut arguments = vec!["-f", "lavfi", "-i", source, "-ac", "2", "-c:a", "libopus"];
    arguments.extend_from_slice(more);
    arguments.push("track.ogg");
    Arc::new(generated(&arguments).swap_remove(0))
}

/// The pieces of a composite with the picture `picture`, and with the sound `sound` when
/// there is one, in pieces of `piece` seconds.
fn pieces_of(picture: &str, sound: Option<&str>, piece: &str) -> Pieces {
    let mut arguments = vec!["-f", "lavfi", "-i", picture];
    if let Some(sound) = sound {
        arguments.extend_from_slice(&["-f", "lavfi", "-i", sound, "-c:a", "aac"]);
    }
    arguments.extend_from_slice(&[
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-pix_fmt",
        "yuv420p",
        "-g",
        "10",
        "-f",
        "hls",
        "-hls_time",
        piece,
        "-hls_segment_filename",
        "part_%05d.ts",
        "zz.m3u8",
    ]);
    let mut files = generated(&arguments);
    // The playlist is the last file by its name. The server does not read it.
    files.pop();
    Arc::new(files)
}

macro_rules! once {
    ($type:ty, $make:expr) => {{
        static KEPT: OnceLock<$type> = OnceLock::new();
        KEPT.get_or_init(|| $make).clone()
    }};
}

/// An audio track: two seconds of a tone, as Ogg with Opus in stereo.
pub fn track() -> Arc<Vec<u8>> {
    once!(Arc<Vec<u8>>, track_of(TONE, &[]))
}

/// An audio track of digital silence, as a muted microphone gives it.
pub fn silent_track() -> Arc<Vec<u8>> {
    once!(
        Arc<Vec<u8>>,
        track_of("anullsrc=r=48000:cl=stereo:d=2", &[])
    )
}

/// An audio track of three seconds whose packets between the first and the second second
/// are missing: a hole in its timestamps.
pub fn holed_track() -> Arc<Vec<u8>> {
    once!(Arc<Vec<u8>>, {
        let whole = track_of("sine=frequency=440:duration=3", &[]);
        let directory =
            std::env::temp_dir().join(format!("junto-samples-{}", uuid::Uuid::new_v4().simple()));
        let holed = (|| -> std::io::Result<Vec<u8>> {
            std::fs::create_dir_all(&directory)?;
            std::fs::write(directory.join("whole.ogg"), whole.as_slice())?;
            let program = std::env::var_os("JUNTO_FFMPEG").unwrap_or_else(|| "ffmpeg".into());
            let output = Command::new(program)
                .args([
                    "-v",
                    "error",
                    "-nostdin",
                    "-y",
                    "-i",
                    "whole.ogg",
                    "-c",
                    "copy",
                ])
                .args(["-bsf:a", "noise=drop=between(pts*tb\\,1\\,2)", "holed.ogg"])
                .current_dir(&directory)
                .output()?;
            if !output.status.success() {
                return Err(std::io::Error::other(
                    String::from_utf8_lossy(&output.stderr).into_owned(),
                ));
            }
            std::fs::read(directory.join("holed.ogg"))
        })();
        let _ = std::fs::remove_dir_all(&directory);
        match holed {
            Ok(holed) => Arc::new(holed),
            Err(error) => panic!("the sample with a hole could not be generated: {error}"),
        }
    })
}

/// The pieces of a composite: two seconds of a test picture with a tone, in two pieces.
pub fn composite() -> Pieces {
    once!(Pieces, pieces_of(PICTURE, Some(TONE), "1"))
}

/// A composite whose picture is black.
pub fn dark_composite() -> Pieces {
    once!(
        Pieces,
        pieces_of(
            "color=c=black:size=320x180:rate=10:duration=2",
            Some(TONE),
            "1"
        )
    )
}

/// A composite whose sound is digital silence, as a layout page that plays no track gives
/// it.
pub fn mute_composite() -> Pieces {
    once!(
        Pieces,
        pieces_of(PICTURE, Some("anullsrc=r=48000:cl=stereo:d=2"), "1")
    )
}

/// A composite without any sound.
pub fn soundless_composite() -> Pieces {
    once!(Pieces, pieces_of(PICTURE, None, "1"))
}

/// Short bursts of a tone at irregular times in twelve seconds, `late` seconds late.
fn burst_source(late: f64) -> String {
    let bursts: Vec<String> = [0.4, 1.3, 2.9, 4.1, 6.6, 7.2, 9.5]
        .iter()
        .map(|start| format!("between(t-{late},{start},{})", start + 0.25))
        .collect();
    format!(
        "aevalsrc='0.5*sin(440*2*PI*t)*({})':d=12:s=48000",
        bursts.join("+")
    )
}

/// An audio track of twelve seconds with bursts of a tone, as a person who speaks gives it.
pub fn bursts() -> Arc<Vec<u8>> {
    once!(Arc<Vec<u8>>, track_of(&burst_source(0.0), &[]))
}

/// A composite of twelve seconds whose sound is the sound of [`bursts`], later by
/// [`LATE_MS`]: the delay of the sound inside a composite (ADR 0602).
pub fn late_composite() -> Pieces {
    once!(Pieces, {
        #[allow(clippy::cast_precision_loss)]
        let late = LATE_MS as f64 / 1000.0;
        pieces_of(
            "testsrc2=size=320x180:rate=10:duration=12",
            Some(&burst_source(late)),
            "2",
        )
    })
}
