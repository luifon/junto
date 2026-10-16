fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
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
            let info = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/native/Usage.plist");
            println!("cargo:rerun-if-changed={}", info.display());
            println!(
                "cargo:rustc-link-arg-examples=-Wl,-sectcreate,__TEXT,__info_plist,{}",
                info.display()
            );
        }
    }
}
