pub const CHECK: &str = "Check devices";
pub const TITLE: &str = "Check devices before joining";
pub const MICROPHONE: &str = "Test microphone";
pub const OUTPUT: &str = "Test output";
pub const CAMERA: &str = "Preview camera";
pub const STOP: &str = "Stop test";
pub const STOP_CAMERA: &str = "Stop preview";
pub const REFRESH: &str = "Refresh devices";
pub const LOCAL: &str = "These tests stay on this device. Use headphones for the microphone test.";
pub const IN_CALL: &str = "A device that you choose here is used by your call at once. The \
     input level is the level of your call's microphone, and the preview shows the picture that \
     your call sends.";
pub const CALL_CAMERA_OFF: &str = "Your camera is off in the call. Turn it on in the call to see \
     the picture that it sends.";
pub const STARTING: &str = "Opening device…";
pub const PLAYING: &str = "The test is playing through your selected output.";
pub const CAMERA_OFF: &str = "Your camera preview appears here. It is not sent to anyone.";
pub const DISCONNECTED: &str = "A saved device is disconnected. Audio uses the system default until it returns; choose another camera to preview.";
pub fn audio_failed(error: &junto_media::audio::AudioError) -> String {
    format!("The audio test stopped: {error}. Check the device and its permission, then try again.")
}
pub fn camera_failed(error: &junto_media::camera::CameraError) -> String {
    format!(
        "The camera preview stopped: {error}. Check the device and its permission, then try again."
    )
}

pub fn destination(call: &str, server: &str) -> String {
    format!("{call} on {server}")
}
