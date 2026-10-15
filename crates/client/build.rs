fn main() -> Result<(), Box<dyn std::error::Error>> {
    // The WebRTC library holds Objective-C categories that the linker drops on macOS unless
    // it is told to keep them. A binary without them stops at its start.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg=-ObjC");
        let info = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?)
            .join("resources/macos/Usage.plist");
        println!("cargo:rerun-if-changed={}", info.display());
        println!(
            "cargo:rustc-link-arg-bin=junto=-Wl,-sectcreate,__TEXT,__info_plist,{}",
            info.display()
        );
    }
    // The Linux packages carry the X11 keyboard libraries in `lib` beside the program
    // (ADR 0754). An old-style search path (DT_RPATH) also applies to the libraries
    // that those libraries load. A build outside a package has no such directory, and the
    // loader then takes the libraries of the system.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo:rustc-link-arg-bin=junto=-Wl,--disable-new-dtags,-rpath,$ORIGIN/lib");
    }
    Ok(())
}
