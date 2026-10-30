#![allow(clippy::unwrap_used, clippy::expect_used, clippy::disallowed_methods)]

#[cfg(target_os = "macos")]
mod native;

fn main() -> std::process::ExitCode {
    #[cfg(target_os = "macos")]
    {
        native::run()
    }
    #[cfg(not(target_os = "macos"))]
    {
        println!("native_macos_windows: not run on this system");
        std::process::ExitCode::SUCCESS
    }
}
