use std::path::{Path, PathBuf};

pub fn idle(value: Option<u32>) -> bool {
    value == Some(0)
}

/// One CoreAudio client process. `None` is a value that CoreAudio or the system did not report.
#[derive(Debug)]
pub struct Client {
    pub pid: Option<i32>,
    pub executable: Option<PathBuf>,
    pub devices: Option<Vec<u32>>,
}

/// Accepts the end of a stage only when each client that uses the device is this process or a
/// helper that runs this executable. CoreAudio lists every process that has read one of its
/// properties, and this process reads them before the check, so a list without this process
/// (including an empty list) is a list that cannot be trusted.
pub fn exclusive(clients: &[Client], device: u32, own: (i32, &Path)) -> Result<(), String> {
    if !clients.iter().any(|client| client.pid == Some(own.0)) {
        return Err(format!(
            "the process list does not contain this process: {clients:?}"
        ));
    }
    for client in clients {
        let devices = client
            .devices
            .as_ref()
            .ok_or_else(|| format!("unknown devices of {client:?}"))?;
        if !devices.contains(&device) || client.pid == Some(own.0) {
            continue;
        }
        if client.pid.is_none() || client.executable.as_deref() != Some(own.1) {
            return Err(format!("another process used the device: {client:?}"));
        }
    }
    Ok(())
}
