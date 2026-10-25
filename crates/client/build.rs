// The build script writes files of the build; it is not code of the window (ADR 0730).
#[allow(clippy::disallowed_methods)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    // The script reads no file of the package other than those it names below; the icon is
    // a build dependency, and a change of it runs the script again.
    println!("cargo:rerun-if-changed=build.rs");
    // The native target `endurance` bounds the work on the thread of the windows; the bound
    // holds for an optimized build, so the target reads the level of the build (ADR 0902).
    println!(
        "cargo:rustc-env=JUNTO_OPT_LEVEL={}",
        std::env::var("OPT_LEVEL")?
    );
    let target = std::env::var("CARGO_CFG_TARGET_OS")?;
    let output = std::path::PathBuf::from(std::env::var("OUT_DIR")?);
    // The program carries the icon (ADR 0756). On Windows it is the icon group 1 of the
    // program, which Explorer shows and which gpui gives to the windows; the MSVC linker
    // reads it from a compiled resource file. On Linux it is the picture that the windows
    // give to an X11 window manager.
    if target == "windows" {
        if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
            return Err(
                "the Windows build uses the MSVC tools (ADR 0753), whose linker reads the icon"
                    .into(),
            );
        }
        let resource = output.join("junto.res");
        std::fs::write(&resource, junto_icon::windows_resource()?)?;
        println!("cargo:rustc-link-arg-bin=junto={}", resource.display());
        // The picture of the notifications, which the program writes for the system (ADR 0742).
        std::fs::write(
            output.join("notification-icon.png"),
            junto_icon::png(junto_icon::NOTIFICATION)?,
        )?;
    }
    if target == "linux" {
        std::fs::write(
            output.join("window-icon.png"),
            junto_icon::png(junto_icon::WINDOW)?,
        )?;
    }
    tray_pictures(&target, &output)?;
    // The WebRTC library holds Objective-C categories that the linker drops on macOS unless
    // it is told to keep them. A binary without them stops at its start.
    if target == "macos" {
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
    if target == "linux" {
        println!("cargo:rustc-link-arg-bin=junto=-Wl,--disable-new-dtags,-rpath,$ORIGIN/lib");
    }
    Ok(())
}

/// The pictures of the tray icon (ADR 0759), one for each call state: on macOS a PNG picture
/// for the menu bar, elsewhere straight RGBA pixels in each size of `junto_icon::TRAY`. The
/// file `tray-pictures.rs` lists them as `(mark, side, bytes)`, with the mark `plain` outside
/// a call.
#[allow(clippy::disallowed_methods)]
fn tray_pictures(target: &str, output: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    let marks = std::iter::once(None).chain(junto_icon::Mark::ALL.map(Some));
    let mut list = String::from("&[\n");
    for mark in marks {
        let name = mark.map_or("plain", junto_icon::Mark::name);
        let pictures: Vec<(u32, Vec<u8>)> = if target == "macos" {
            let side = junto_icon::MENU_BAR;
            vec![(side, junto_icon::tray_png(mark, side)?)]
        } else {
            junto_icon::TRAY
                .iter()
                .map(|&side| junto_icon::tray(mark, side).map(|picture| (side, picture.rgba)))
                .collect::<Result<_, _>>()?
        };
        for (side, bytes) in pictures {
            let file = output.join(format!("tray-{name}-{side}"));
            std::fs::write(&file, bytes)?;
            list.push_str(&format!(
                "    ({name:?}, {side}, include_bytes!({:?}) as &[u8]),\n",
                file.display().to_string()
            ));
        }
    }
    list.push_str("]\n");
    std::fs::write(output.join("tray-pictures.rs"), list)?;
    Ok(())
}
