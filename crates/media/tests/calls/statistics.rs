//! Failure-only diagnostics for the measured video frame rate.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use junto_media::Calls;
use livekit::webrtc::stats::RtcStats;

use crate::scene::Person;

fn load_average() -> String {
    if let Ok(load) = std::fs::read_to_string("/proc/loadavg") {
        return load
            .split_whitespace()
            .next()
            .unwrap_or("unavailable")
            .to_owned();
    }
    let Ok(output) = Command::new("sysctl").args(["-n", "vm.loadavg"]).output() else {
        return "unavailable".to_owned();
    };
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .find(|value| value.parse::<f64>().is_ok())
        .unwrap_or("unavailable")
        .to_owned()
}

fn video(stat: &RtcStats) -> bool {
    match stat {
        RtcStats::OutboundRtp(stat) => stat.stream.kind == "video",
        RtcStats::InboundRtp(stat) => stat.stream.kind == "video",
        RtcStats::RemoteInboundRtp(stat) => stat.stream.kind == "video",
        RtcStats::MediaSource(stat) => stat.source.kind == "video",
        _ => false,
    }
}

async fn connection(label: &str, calls: &Calls) {
    // Diagnostics must still finish if the failed connection no longer answers.
    match tokio::time::timeout(Duration::from_secs(5), calls.statistics()).await {
        Ok(Ok(stats)) => {
            for stat in stats.publisher_stats.iter().filter(|stat| video(stat)) {
                eprintln!("{label} publisher: {stat:?}");
            }
            for stat in stats.subscriber_stats.iter().filter(|stat| video(stat)) {
                eprintln!("{label} subscriber: {stat:?}");
            }
        }
        Ok(Err(error)) => eprintln!("{label} statistics unavailable: {error}"),
        Err(_) => eprintln!("{label} statistics did not answer within five seconds"),
    }
}

pub async fn frame_rate(sender: &Person, receiver: &Person, pictures: usize) {
    let at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let load = load_average();
    let host_load = if Path::new("/.dockerenv").exists() {
        "not exposed to the container"
    } else {
        &load
    };
    eprintln!(
        "frame-rate failure: unix_ms={at}, pictures={pictures}, load_1m={load}, host_load_1m={host_load}"
    );
    let picture = receiver.calls.main_picture();
    eprintln!(
        "selected picture: {:?}",
        picture
            .borrow()
            .as_ref()
            .map(|picture| (picture.share, picture.frame.source))
    );
    let view = receiver.calls.view();
    eprintln!(
        "selected share and received layers: {:?}",
        view.call.as_ref().map(|call| (&call.watched, &call.shares))
    );
    tokio::join!(
        connection("sender", &sender.calls),
        connection("receiver", &receiver.calls)
    );
}
