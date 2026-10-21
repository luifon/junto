fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo:rerun-if-changed=src/camera/v4l2.c");
        cc::Build::new()
            .file("src/camera/v4l2.c")
            .compile("junto_camera");
        println!("cargo:rerun-if-changed=src/audio/linux/pipewire.cc");
        cc::Build::new()
            .cpp(true)
            .cpp_link_stdlib(None)
            .file("src/audio/linux/pipewire.cc")
            .compile("junto_pipewire");
    }
    // The WebRTC library holds Objective-C categories that the linker drops on macOS unless
    // it is told to keep them. A binary without them stops at its start.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg=-ObjC");
        if std::env::var_os("CARGO_FEATURE_NATIVE_DEVICE_TESTS").is_some() {
            // Cargo sets the directory of the package for each run of the build script; a path
            // compiled into it can name another checkout whose build the target directory reused.
            let info = std::env::var_os("CARGO_MANIFEST_DIR")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| panic!("Cargo sets CARGO_MANIFEST_DIR for a build script"))
                .join("tests/native/Usage.plist");
            println!("cargo:rerun-if-changed={}", info.display());
            println!(
                "cargo:rustc-link-arg-examples=-Wl,-sectcreate,__TEXT,__info_plist,{}",
                info.display()
            );
        }
    }
}
