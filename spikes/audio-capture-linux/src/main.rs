mod analyze;
mod capture;
mod common;
mod graph;
mod player;
mod tone_stream;
mod wav;

const USAGE: &str = "usage: junto-audio-spike <gen-wav|play|capture|analyze> [--option value]...";

fn main() {
    let mut args = std::env::args().skip(1);
    let command = args.next().unwrap_or_default();
    let options = common::Options::parse(args);
    let result = match command.as_str() {
        "gen-wav" => wav::run(&options),
        "play" => player::run(&options),
        "capture" => capture::run(&options),
        "analyze" => analyze::run(&options),
        _ => Err(USAGE.into()),
    };
    if let Err(error) = result {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}
