//! The files that the application carries in its binary: the fonts of the design and its
//! icons. Nothing is read from the disk at run time, so a build runs from any directory.

use std::borrow::Cow;

use gpui::{App, AssetSource, SharedString};

/// The typefaces of the design, each under the SIL Open Font License 1.1. The licence
/// texts are beside the files.
const FONTS: &[&[u8]] = &[
    include_bytes!("../assets/fonts/AtkinsonHyperlegibleNext-Regular.ttf"),
    include_bytes!("../assets/fonts/AtkinsonHyperlegibleNext-Medium.ttf"),
    include_bytes!("../assets/fonts/AtkinsonHyperlegibleNext-SemiBold.ttf"),
    include_bytes!("../assets/fonts/AtkinsonHyperlegibleNext-Bold.ttf"),
    // The italic faces, for the italic text of a message (ADR 0740).
    include_bytes!("../assets/fonts/AtkinsonHyperlegibleNext-Italic.ttf"),
    include_bytes!("../assets/fonts/AtkinsonHyperlegibleNext-BoldItalic.ttf"),
    include_bytes!("../assets/fonts/AtkinsonHyperlegibleMono-Medium.ttf"),
    include_bytes!("../assets/fonts/AtkinsonHyperlegibleMono-SemiBold.ttf"),
    include_bytes!("../assets/fonts/AtkinsonHyperlegibleMono-Bold.ttf"),
    include_bytes!("../assets/fonts/Marcellus-Regular.ttf"),
];

macro_rules! icons {
    ($($variant:ident => $file:literal),+ $(,)?) => {
        /// An icon of the design: a drawing in one colour, which is the text colour of the
        /// element that shows it. The files under `icons/lucide/` are unchanged Lucide icons
        /// under the licence in that directory (ADR 0731, ADR 0801).
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Icon {
            $($variant),+
        }

        impl Icon {
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            /// The path under which the asset source holds the drawing.
            pub const fn path(self) -> &'static str {
                match self {
                    $(Self::$variant => concat!("icons/", $file, ".svg")),+
                }
            }

            const fn bytes(self) -> &'static [u8] {
                match self {
                    $(Self::$variant => {
                        include_bytes!(concat!("../assets/icons/", $file, ".svg"))
                    }),+
                }
            }
        }
    };
}

icons! {
    Account => "lucide/circle-user",
    AppWindow => "lucide/app-window",
    Archive => "lucide/archive",
    ArrowLeftRight => "lucide/arrow-right-left",
    Ball => "lucide/volleyball",
    Bell => "lucide/bell",
    BellOff => "lucide/bell-off",
    Book => "lucide/book-a",
    Bulb => "lucide/lightbulb",
    Camera => "lucide/video",
    CameraOff => "lucide/video-off",
    Car => "lucide/car",
    Chat => "lucide/message-square-text",
    Check => "lucide/check",
    CheckDouble => "lucide/check-check",
    ChevronDown => "lucide/chevron-down",
    ChevronLeft => "lucide/chevron-left",
    ChevronRight => "lucide/chevron-right",
    Clip => "lucide/paperclip",
    Clock => "lucide/clock",
    Code => "lucide/code",
    CodeBlock => "lucide/square-code",
    Collapse => "lucide/panel-left-close",
    Cup => "lucide/coffee",
    Detach => "lucide/picture-in-picture-2",
    Devices => "lucide/monitor-smartphone",
    Document => "lucide/file-text",
    Download => "lucide/download",
    Edit => "lucide/pencil",
    EditOff => "lucide/pencil-off",
    Expand => "lucide/panel-left-open",
    EyeOff => "lucide/eye-off",
    Flag => "lucide/flag",
    FolderOpen => "lucide/folder-open",
    Fork => "lucide/git-fork",
    FullScreen => "lucide/maximize",
    Gear => "lucide/settings",
    Grip => "lucide/grip-vertical",
    Hand => "lucide/hand",
    Hash => "lucide/hash",
    Heart => "lucide/heart",
    History => "lucide/rotate-ccw-clock",
    Invite => "lucide/user-plus",
    Key => "lucide/key-round",
    Layers => "lucide/layers",
    LayoutGrid => "lucide/layout-grid",
    Leaf => "lucide/leaf",
    Leave => "lucide/log-out",
    Link => "lucide/link",
    List => "lucide/list",
    Lock => "lucide/lock",
    Microphone => "lucide/mic",
    MicrophoneHeld => "mic-held",
    MicrophoneOff => "lucide/mic-off",
    Minus => "lucide/minus",
    Open => "lucide/external-link",
    People => "lucide/users",
    Person => "lucide/user",
    Phone => "lucide/phone",
    PhoneIncoming => "lucide/phone-incoming",
    PhoneOff => "lucide/phone-off",
    PhoneOutgoing => "lucide/phone-outgoing",
    Phones => "lucide/headphones",
    PhonesOff => "lucide/headphone-off",
    Picture => "lucide/image",
    Pin => "lucide/pin",
    Play => "lucide/play",
    Plug => "lucide/plug",
    Plus => "lucide/plus",
    Quote => "lucide/text-quote",
    Record => "lucide/circle-dot",
    Reply => "lucide/reply",
    Retry => "lucide/rotate-cw",
    Screen => "lucide/screen-share",
    ScreenOff => "lucide/screen-share-off",
    Scroll => "lucide/scroll-text",
    Search => "lucide/search",
    Send => "lucide/send-horizontal",
    Shield => "lucide/shield",
    ShieldCheck => "lucide/shield-check",
    Shrink => "lucide/minimize-2",
    Smile => "lucide/face-slightly-smiling",
    SmilePlus => "lucide/face-slightly-smiling-plus",
    Speaker => "lucide/volume-2",
    Stop => "lucide/circle-stop",
    Ticket => "lucide/ticket",
    Trash => "lucide/trash",
    Unlock => "lucide/lock-open",
    UserCheck => "lucide/user-check",
    Warning => "lucide/triangle-alert",
    WindowClose => "winclose",
    WindowMaximize => "winmax",
    WindowMinimize => "winmin",
    WindowRestore => "winrestore",
    X => "lucide/x",
}

/// The asset source of the application.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        Ok(Icon::ALL
            .iter()
            .find(|icon| icon.path() == path)
            .map(|icon| Cow::Borrowed(icon.bytes())))
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        Ok(Icon::ALL
            .iter()
            .map(|icon| icon.path())
            .filter(|held| held.starts_with(path))
            .map(SharedString::from)
            .collect())
    }
}

/// The typefaces of the design.
pub fn fonts() -> Vec<Cow<'static, [u8]>> {
    FONTS.iter().map(|font| Cow::Borrowed(*font)).collect()
}

/// Gives the text system the typefaces of the design.
pub fn load_fonts(cx: &App) -> gpui::Result<()> {
    cx.text_system().add_fonts(fonts())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    /// The window buttons of Windows and Linux keep the thin line of the buttons of the
    /// system (ADR 0731). Every other drawing has the form of a Lucide icon.
    const WINDOW_BUTTONS: [Icon; 4] = [
        Icon::WindowMinimize,
        Icon::WindowMaximize,
        Icon::WindowRestore,
        Icon::WindowClose,
    ];

    const LUCIDE_ROOT: [(&str, &str); 9] = [
        ("xmlns", "http://www.w3.org/2000/svg"),
        ("width", "24"),
        ("height", "24"),
        ("viewBox", "0 0 24 24"),
        ("fill", "none"),
        ("stroke", "currentColor"),
        ("stroke-width", "2"),
        ("stroke-linecap", "round"),
        ("stroke-linejoin", "round"),
    ];

    const SHAPE_ATTRIBUTES: [&str; 17] = [
        "d", "x", "y", "width", "height", "rx", "ry", "cx", "cy", "r", "x1", "y1", "x2", "y2",
        "points", "fill", "stroke",
    ];

    /// Each attribute `name="value"` of a piece of SVG text, in order.
    fn attributes(text: &str) -> Vec<(&str, &str)> {
        let mut found = Vec::new();
        let mut rest = text;
        while let Some(equals) = rest.find("=\"") {
            let name = rest[..equals]
                .rsplit(|c: char| c.is_whitespace() || c == '<')
                .next()
                .unwrap_or_default();
            let after = &rest[equals + 2..];
            let Some(close) = after.find('"') else {
                break;
            };
            found.push((name, &after[..close]));
            rest = &after[close + 1..];
        }
        found
    }

    fn icons_directory() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/icons")
    }

    #[test]
    fn every_icon_is_a_drawing_with_one_colour_that_the_source_finds() {
        for icon in Icon::ALL {
            let bytes = Assets.load(icon.path()).unwrap().unwrap();
            let text = std::str::from_utf8(&bytes).unwrap();
            assert!(text.starts_with("<svg"), "{icon:?}");
            let end = text.find('>').unwrap();
            let (root, body) = (&text[..end], &text[end + 1..]);
            assert!(body.trim_end().ends_with("</svg>"), "{icon:?}");
            if WINDOW_BUTTONS.contains(icon) {
                assert!(root.contains("viewBox=\"0 0 16 16\""), "{icon:?}");
                for (name, value) in attributes(text) {
                    if name == "fill" || name == "stroke" {
                        assert!(value == "none" || value == "#000", "{icon:?}: {name}");
                    }
                }
            } else {
                assert_eq!(attributes(root), LUCIDE_ROOT, "{icon:?}");
                for (name, value) in attributes(body) {
                    assert!(SHAPE_ATTRIBUTES.contains(&name), "{icon:?}: {name}");
                    if name == "fill" || name == "stroke" {
                        assert!(value == "none" || value == "currentColor", "{icon:?}");
                    }
                }
            }
        }
        assert!(Assets.load("icons/none.svg").unwrap().is_none());
        assert_eq!(Assets.list("icons/").unwrap().len(), Icon::ALL.len());
    }

    #[test]
    fn every_file_of_the_icons_is_the_drawing_of_one_icon() {
        let mut files = Vec::new();
        for (directory, prefix) in [
            (icons_directory(), "icons/"),
            (icons_directory().join("lucide"), "icons/lucide/"),
        ] {
            for entry in std::fs::read_dir(&directory).unwrap() {
                let entry = entry.unwrap();
                let name = entry.file_name().into_string().unwrap();
                if name.starts_with('.') {
                    continue;
                }
                if name.ends_with(".svg") {
                    files.push(format!("{prefix}{name}"));
                } else {
                    assert!(
                        matches!(
                            (prefix, name.as_str()),
                            ("icons/", "lucide") | ("icons/lucide/", "LICENSE")
                        ),
                        "{prefix}{name}"
                    );
                }
            }
        }
        files.sort();
        let mut paths: Vec<String> = Icon::ALL
            .iter()
            .map(|icon| icon.path().to_owned())
            .collect();
        paths.sort();
        paths.dedup();
        assert_eq!(paths.len(), Icon::ALL.len(), "two icons share a file");
        assert_eq!(files, paths);
    }

    #[test]
    fn the_licence_of_lucide_is_beside_its_icons() {
        let licence = std::fs::read_to_string(icons_directory().join("lucide/LICENSE")).unwrap();
        assert!(licence.starts_with("ISC License"));
        assert!(
            licence.contains("Permission to use, copy, modify, and/or distribute this software")
        );
        assert!(licence.contains("are derived from the Feather project"));
        assert!(licence.contains("The MIT License (MIT)"));
        assert!(licence.contains("Permission is hereby granted, free of charge"));
    }
}
