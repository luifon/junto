use crate::common::{connect, roundtrip, AnyResult, Options, RATE};
use crate::tone_stream::{self, ToneSpec};
use std::sync::atomic::Ordering;
use std::time::Duration;

pub fn run(options: &Options) -> AnyResult<()> {
    let frequency: f64 = options.number("freq", 440.0)?;
    let amplitude: f32 = options.number("amp", 0.2)?;
    let seconds: f64 = options.number("seconds", 3.0)?;
    let lead_silence: f64 = options.number("lead-silence", 0.0)?;
    let cycles: u32 = options.number("cycles", 1)?;
    let gap_seconds: f64 = options.number("gap-seconds", 1.0)?;
    let target = options.required("target")?.to_string();
    let application_name = options.text("app-name").unwrap_or("junto-spike-player");
    let node_name = options.text("node-name").unwrap_or("junto-spike-player").to_string();

    let (mainloop, _context, core) = connect(application_name)?;
    println!("PLAYER pid={} app_name={application_name} freq={frequency}", std::process::id());

    for cycle in 0..cycles {
        let tone = tone_stream::start(
            &core,
            ToneSpec {
                frequency,
                amplitude,
                lead_silence_frames: (lead_silence * RATE as f64) as u64,
                tone_frames: Some((seconds * RATE as f64) as u64),
                target: target.clone(),
                node_name: node_name.clone(),
                latency: options.text("latency").map(str::to_string),
            },
        )?;
        let timer = mainloop.loop_().add_timer({
            let report = tone.report.clone();
            let mainloop = mainloop.downgrade();
            move |_| {
                if report.finished.load(Ordering::SeqCst) {
                    if let Some(mainloop) = mainloop.upgrade() {
                        mainloop.quit();
                    }
                }
            }
        });
        timer
            .update_timer(Some(Duration::from_millis(20)), Some(Duration::from_millis(20)))
            .into_result()?;
        mainloop.run();

        let report = &tone.report;
        println!(
            "PLAYER cycle={cycle} node_id={} failed={} onset_ns={} onset_frame_in_block={} onset_block_frames={} onset_delay_ticks={} callbacks={}",
            tone.stream.node_id(),
            report.failed.load(Ordering::SeqCst),
            report.onset_ns.load(Ordering::SeqCst),
            report.onset_frame_in_block.load(Ordering::SeqCst),
            report.onset_block_frames.load(Ordering::SeqCst),
            report.onset_delay_ticks.load(Ordering::SeqCst),
            report.callbacks.load(Ordering::Relaxed),
        );
        let failed = report.failed.load(Ordering::SeqCst);
        drop(timer);
        drop(tone);
        roundtrip(&mainloop, &core)?;
        if failed {
            return Err("tone stream failed".into());
        }
        if cycle + 1 < cycles {
            std::thread::sleep(Duration::from_secs_f64(gap_seconds));
        }
    }
    Ok(())
}
