//! The test-support factory has a type that cannot open a native camera.
#[cfg(feature = "test-support")]
#[test]
fn test_support_camera_factory_cannot_open_hardware() {
    let _: std::sync::Arc<junto_media::camera::NoCamera> = junto_media::camera::system();
}
