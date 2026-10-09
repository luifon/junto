mod analysis;
mod capture;
mod tone;
mod wav;

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::Ordering;
use std::time::Duration;

const USAGE: &str = "usage:
  spike list
  spike preflight
  spike request [--wait S]
  spike wav --hz F --seconds S --out FILE
  spike tone --hz F --seconds S [--lead S]
  spike capture --mode system|app|window [--pid P | --bundle ID] --seconds S
                [--own-tone F] [--exclude-self true|false]
                [--exclude-bundles ID,ID] [--freqs F,F,F] [--out FILE]
                [--rate 48000] [--channels 2]";

fn flags(arguments: &[String]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let mut index = 0;
    while index + 1 < arguments.len() {
        if let Some(name) = arguments[index].strip_prefix("--") {
            map.insert(name.to_string(), arguments[index + 1].clone());
        }
        index += 2;
    }
    map
}

fn number(map: &HashMap<String, String>, name: &str, default: f64) -> f64 {
    map.get(name).and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn frequencies(map: &HashMap<String, String>) -> Vec<f64> {
    map.get("freqs")
        .map(|list| list.split(',').filter_map(|f| f.parse().ok()).collect())
        .unwrap_or_default()
}

fn run_tone(map: &HashMap<String, String>) -> Result<(), String> {
    let hz = number(map, "hz", 1000.0);
    let seconds = number(map, "seconds", 5.0);
    let lead = number(map, "lead", 0.0);
    let player = tone::play(hz, 0.2, lead)?;
    println!(
        "tone pid={} hz={hz} sample_rate={} channels={}",
        std::process::id(),
        player.sample_rate,
        player.channels
    );
    std::thread::sleep(Duration::from_secs_f64(seconds + lead));
    println!(
        "tone first_tone_callback_unix_nanos={} first_tone_frame_offset={} output_latency_nanos={}",
        player.first_tone_callback_unix_nanos.load(Ordering::Relaxed),
        player.first_tone_frame_offset.load(Ordering::Relaxed),
        player.output_latency_nanos.load(Ordering::Relaxed)
    );
    Ok(())
}

fn run_capture(map: &HashMap<String, String>) -> Result<(), String> {
    let mode = match map.get("mode").map(String::as_str) {
        Some("system") => capture::Mode::System,
        Some("app") => capture::Mode::App,
        Some("window") => capture::Mode::Window,
        other => return Err(format!("unknown mode {other:?}")),
    };
    capture::run(&capture::Options {
        mode,
        pid: map.get("pid").and_then(|v| v.parse().ok()),
        bundle: map.get("bundle").cloned(),
        exclude_bundles: map
            .get("exclude-bundles")
            .map(|list| list.split(',').map(str::to_string).collect())
            .unwrap_or_default(),
        seconds: number(map, "seconds", 4.0),
        own_tone: map.get("own-tone").and_then(|v| v.parse().ok()),
        exclude_self: map.get("exclude-self").map_or(true, |v| v == "true"),
        frequencies: frequencies(map),
        out: map.get("out").map(PathBuf::from),
        sample_rate: number(map, "rate", 48000.0) as i32,
        channel_count: number(map, "channels", 2.0) as i32,
    })
}

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = arguments.first() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let map = flags(&arguments[1..]);
    let result = match command.as_str() {
        "list" => capture::list(),
        "preflight" => capture::preflight(),
        "request" => capture::request(number(&map, "wait", 20.0)),
        "wav" => wav::write_sine(
            &PathBuf::from(map.get("out").cloned().unwrap_or_default()),
            48000,
            number(&map, "hz", 1000.0),
            0.2,
            number(&map, "seconds", 5.0),
        )
        .map_err(|e| e.to_string()),
        "tone" => run_tone(&map),
        "capture" => run_capture(&map),
        _ => Err(USAGE.to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}
