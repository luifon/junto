//! Links and files that the client opens with the applications of the system. On Windows the
//! shell starts those applications, outside the process tree of the client, whose sound a
//! share with all system sound leaves out (ADR 0706).
//!
//! A link reaches the system only in the form of ADR 0740, which holds no quote and no
//! space, and each system receives it as one value: macOS as a URL object, Linux as one
//! argument of the opening program or as the value of the desktop portal, Windows as the
//! file of the shell's call. No command line is built here.

use std::path::Path;

use gpui::App;
use junto_client_core::link::Link;
#[cfg(target_os = "windows")]
use junto_media::share::windows::Target;

/// Opens a link with the application of the system for its scheme.
pub fn open_link(link: &Link, cx: &App) {
    #[cfg(target_os = "windows")]
    {
        let _ = cx;
        open_by_the_shell(Target::Link(link.clone()));
    }
    #[cfg(not(target_os = "windows"))]
    cx.open_url(link.as_str());
}

/// Opens a file with the application of the system for it.
pub fn open_with_system(path: &Path, cx: &App) {
    #[cfg(target_os = "windows")]
    {
        let _ = cx;
        open_by_the_shell(Target::File(path.to_owned()));
    }
    #[cfg(not(target_os = "windows"))]
    cx.open_with_system(path);
}

/// The request goes to another process and can wait for it, so it runs on a thread of its
/// own.
#[cfg(target_os = "windows")]
fn open_by_the_shell(target: Target) {
    use junto_media::share::windows::{Opened, open};
    let started = std::thread::Builder::new()
        .name("open".into())
        .spawn(move || match open(&target) {
            Ok(Opened::ByTheShell) => {}
            Ok(Opened::ByTheClient) => tracing::warn!(
                "the client started the application itself: a share with all system sound \
                 leaves out its sound"
            ),
            Err(reason) => tracing::warn!(%reason, "the target was not opened"),
        });
    if let Err(error) = started {
        tracing::warn!(%error, "the target was not opened");
    }
}
