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
    }
}
