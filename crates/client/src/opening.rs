//! Links and files that the client opens with the applications of the system. On Windows the
//! shell starts those applications, outside the process tree of the client, whose sound a
//! share with all system sound leaves out (ADR 0706).

use std::path::Path;

use gpui::App;

/// Opens an address with the application of the system for it.
pub fn open_url(address: &str, cx: &App) {
    #[cfg(target_os = "windows")]
    {
        let _ = cx;
        open_by_the_shell(address.into());
    }
    #[cfg(not(target_os = "windows"))]
    cx.open_url(address);
}

/// Opens a file with the application of the system for it.
pub fn open_with_system(path: &Path, cx: &App) {
    #[cfg(target_os = "windows")]
    {
        let _ = cx;
        open_by_the_shell(path.as_os_str().to_owned());
    }
    #[cfg(not(target_os = "windows"))]
    cx.open_with_system(path);
}

/// The request goes to another process and can wait for it, so it runs on a thread of its
/// own.
#[cfg(target_os = "windows")]
fn open_by_the_shell(target: std::ffi::OsString) {
    let started = std::thread::Builder::new()
        .name("open".into())
        .spawn(move || match junto_media::share::windows::open(&target) {
            Ok(junto_media::share::windows::Opened::ByTheShell) => {}
            Ok(junto_media::share::windows::Opened::ByTheClient) => tracing::warn!(
                "the client started the application itself: a share with all system sound \
                 leaves out its sound"
            ),
            Err(reason) => tracing::warn!(%reason, "the target was not opened"),
        });
    if let Err(error) = started {
        tracing::warn!(%error, "the target was not opened");
    }
}
