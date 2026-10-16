#[allow(dead_code)]
#[path = "native/camera_progress.rs"]
mod camera_progress;

#[test]
fn a_wrong_frame_cannot_be_hidden_by_a_later_correct_frame() {
    for configured_first in [true, false] {
        let mut progress = camera_progress::Progress::default();
        if configured_first {
            progress.configure((640, 480));
        }
        progress.picture((800, 600), true);
        progress.picture((640, 480), true);
        progress.configure((640, 480));
        assert!(progress.invalid);
    }
}
