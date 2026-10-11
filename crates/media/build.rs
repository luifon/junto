fn main() {
    // The WebRTC library holds Objective-C categories that the linker drops on macOS unless
    // it is told to keep them. A binary without them stops at its start.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg=-ObjC");
    }
}
