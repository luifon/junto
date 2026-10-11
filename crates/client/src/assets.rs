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
        /// An icon of the design: a drawing of 16 by 16 units with one colour, which is the
        /// text colour of the element that shows it.
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
    Camera => "cam",
    Chat => "thread",
    Check => "check",
    ChevronDown => "chev",
    ChevronRight => "chevr",
    Clip => "clip",
    Clock => "clock",
    Code => "code",
    CodeBlock => "cblock",
    Collapse => "collapse",
    Detach => "detach",
    Download => "down",
    Edit => "edit",
    Expand => "expand",
    EyeOff => "eyeoff",
    Gear => "gear",
    Hand => "hand",
    Hash => "hash",
    Invite => "invite",
    Key => "key",
    Leave => "leave",
    Link => "link",
    List => "list",
    Lock => "lock",
    Microphone => "mic",
    MicrophoneHeld => "michold",
    MicrophoneOff => "micoff",
    Minus => "minus",
    Open => "open",
    People => "people",
    Phones => "phones",
    PhonesOff => "phonesoff",
    Picture => "img",
    Plus => "plus",
    Quote => "quote",
    Record => "recd",
    Reply => "reply",
    Retry => "retry",
    Screen => "screen",
    Search => "search",
    Send => "send",
    Smile => "smile",
    Speaker => "spk",
    Stop => "stop",
    Trash => "trash",
    Unlock => "unlock",
    Warning => "warn",
    X => "x",
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

/// Gives the text system the typefaces of the design.
pub fn load_fonts(cx: &App) -> gpui::Result<()> {
    cx.text_system()
        .add_fonts(FONTS.iter().map(|font| Cow::Borrowed(*font)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_is_a_drawing_with_one_colour_that_the_source_finds() {
        for icon in Icon::ALL {
            let bytes = Assets.load(icon.path()).unwrap().unwrap();
            let text = std::str::from_utf8(&bytes).unwrap();
            assert!(text.starts_with("<svg "), "{icon:?}");
            assert!(text.contains("viewBox=\"0 0 16 16\""), "{icon:?}");
        }
        assert!(Assets.load("icons/none.svg").unwrap().is_none());
        assert_eq!(Assets.list("icons/").unwrap().len(), Icon::ALL.len());
    }
}
