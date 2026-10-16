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

#[test]
fn a_native_frame_outside_601_must_have_been_converted() {
    let before = camera_progress::Progress::default();
    let mut progress = before;
    for delivery in [(true, None), (false, Some(true)), (true, None)] {
        progress.picture((640, 480), true);
        progress.delivered(delivery.0, delivery.1);
    }
    assert_eq!(progress.conversions(&before), (2, 2));
    // A 709 buffer delivered without conversion.
    progress.picture((640, 480), true);
    progress.delivered(false, Some(false));
    assert_eq!(progress.conversions(&before), (3, 2));
}
