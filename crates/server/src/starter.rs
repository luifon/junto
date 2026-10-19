//! The end of the process that started the server (ADR 0008, ADR 0915). A server started with
//! `JUNTO_STOP_WITH_PROCESS` stops when that process ends, also when it is killed.

#[cfg(windows)]
mod windows_process;

/// Resolves when the process with the number `starter` has ended, at once when it is not the
/// process that started this one.
pub async fn ended(starter: u32) {
    let (done, end) = tokio::sync::oneshot::channel::<()>();
    std::thread::spawn(move || {
        wait(starter);
        let _ = done.send(());
    });
    let _ = end.await;
}

/// The interval at which the parent is read on Unix systems.
#[cfg(unix)]
const PARENT_CHECK: std::time::Duration = std::time::Duration::from_millis(100);

/// When a process ends, the system gives each of its children a new parent (the first process,
/// or a process that collects orphans), so the parent of this process changes. Two processes
/// that run at the same time never have the same number, so the new parent cannot have the
/// number of the ended one. No descriptor that another process inherited delays this.
#[cfg(unix)]
fn wait(starter: u32) {
    let Ok(starter) = i32::try_from(starter) else {
        return;
    };
    while rustix::process::Pid::as_raw(rustix::process::getppid()) == starter {
        std::thread::sleep(PARENT_CHECK);
    }
}

#[cfg(windows)]
fn wait(starter: u32) {
    windows_process::wait_for_end(starter);
}
