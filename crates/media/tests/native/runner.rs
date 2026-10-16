#![allow(clippy::unwrap_used, clippy::expect_used)]

#[cfg(target_os = "macos")]
mod audio;
#[cfg(target_os = "macos")]
mod calls;
#[cfg(target_os = "macos")]
mod camera;
#[cfg(target_os = "macos")]
mod fixture;
#[cfg(target_os = "macos")]
mod guard;
#[cfg(target_os = "macos")]
mod measure;
#[cfg(target_os = "macos")]
mod protocol;
#[cfg(target_os = "macos")]
mod route;
#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../calls/scene.rs"]
mod scene;
#[cfg(target_os = "macos")]
mod screen;
#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../../../client-core/tests/e2e/support.rs"]
mod support;

#[cfg(target_os = "macos")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use junto_media::permission::{MicrophonePermission, SystemPermission};
    if std::env::var("JUNTO_TEST_NATIVE_DEVICES").as_deref() != Ok("1") {
        return Err(
            "NOT RUN: set JUNTO_TEST_NATIVE_DEVICES=1 for an explicitly selected stage".into(),
        );
    }
    let stage = std::env::args().nth(1).ok_or("a stage is required")?;
    if let Some(permission) = stage.strip_prefix("request-") {
        if std::env::var("JUNTO_TEST_PERMISSION_REQUEST").as_deref() != Ok(permission) {
            return Err(
                "NOT RUN: permission requests need a matching JUNTO_TEST_PERMISSION_REQUEST".into(),
            );
        }
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        match permission {
            "microphone" => println!(
                "microphone={:?}",
                runtime.block_on(SystemPermission.request())
            ),
            "camera" => println!(
                "camera={:?}",
                runtime.block_on(junto_media::camera::native_for_test().request_access())
            ),
            "screen" => {
                let capture = junto_media::share::system();
                println!("screen before={:?}", capture.access());
                capture.request_access();
                println!("screen after={:?}", capture.access());
            }
            _ => return Err("unknown permission".into()),
        }
        return Ok(());
    }
    if stage == "fixture-audio" {
        use std::io::{BufRead, Write};
        let host = audio::host()?;
        let tone = calls::Tone::new(&host);
        println!("ready {}", host.id());
        std::io::stdout().flush()?;
        for line in std::io::stdin().lock().lines() {
            let line = line?;
            let hz: u32 = line
                .strip_prefix("tone ")
                .ok_or("invalid tone command")?
                .parse()?;
            if hz > 4000 || host.failed() {
                return Err("fixture output refused".into());
            }
            tone.set(hz);
            println!("tone {hz}");
            std::io::stdout().flush()?;
        }
        return Ok(());
    }
    println!(
        "permissions microphone={:?} camera={:?} screen={:?}",
        SystemPermission.state(),
        junto_media::camera::native_for_test().access(),
        junto_media::share::system().access()
    );
    if matches!(stage.as_str(), "d0" | "a1" | "a2" | "a3" | "s3")
        && SystemPermission.state() != junto_media::permission::MicrophoneAccess::Allowed
    {
        return Err("NOT RUN: microphone permission must already be allowed".into());
    }
    match stage.as_str() {
        "inspect" => println!("verified BlackHole id={}", route::verify()?),
        "d0" => audio::devices()?,
        "a1" | "a2" | "a3" | "c1" | "s1-s2" | "s3" => {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(4)
                .enable_all()
                .build()?;
            runtime.block_on(async {
                match stage.as_str() {
                    "a1" => calls::capture().await,
                    "a2" => calls::playout().await,
                    "a3" => calls::lifecycle().await,
                    "c1" => camera::capture().await,
                    _ => screen::capture(stage == "s3").await,
                }
            });
        }
        _ => return Err("unknown stage".into()),
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("NOT RUN: native device tests require macOS");
    std::process::exit(2);
}
