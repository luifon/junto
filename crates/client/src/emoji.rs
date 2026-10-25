//! Which emoji of the table the fonts of this system draw (ADR 0757).
//!
//! The window draws emoji with the emoji fonts of the system, and they can be older than
//! the table of the server, or lack a kind of sequence. Each emoji is laid out once in the
//! family of the text of messages, and the result is kept for the life of the process.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{App, Font, Global, Pixels, SharedString, TextRun, TextSystem, WindowTextSystem, px};

/// How the fonts of the system draw one emoji.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drawing {
    /// One picture of a colour emoji font.
    Colour,
    /// A sign of a font without colour.
    Text,
    /// A sign that no font has, or a sequence that falls apart into several pictures.
    Broken,
}

/// The size at which an emoji is laid out for the measurement.
const SIZE: f32 = 20.0;

/// An emoji of the first version of the standard, which every emoji font draws.
const REFERENCE: &str = "\u{1f600}";

/// A sequence of several glyphs that is at least this many times as wide as the reference
/// has fallen apart.
const MOST_WIDTH: f32 = 1.5;

struct Measure {
    text: WindowTextSystem,
    font: Font,
    /// The width of the reference, or the size of the type when no font draws it.
    reference: Option<Pixels>,
    held: HashMap<&'static str, Drawing>,
}

impl Measure {
    fn layout(&self, sequence: &str) -> Arc<gpui::LineLayout> {
        let run = TextRun {
            len: sequence.len(),
            font: self.font.clone(),
            color: gpui::black(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        self.text.layout_line(sequence, px(SIZE), &[run], None)
    }

    fn reference(&mut self) -> Pixels {
        if let Some(width) = self.reference {
            return width;
        }
        let layout = self.layout(REFERENCE);
        let glyphs: Vec<_> = layout.runs.iter().flat_map(|run| &run.glyphs).collect();
        let width = if glyphs.len() == 1 && glyphs[0].id.0 != 0 && layout.width > px(0.0) {
            layout.width
        } else {
            px(SIZE)
        };
        self.reference = Some(width);
        width
    }

    fn drawing(&mut self, sequence: &'static str) -> Drawing {
        if let Some(drawing) = self.held.get(sequence) {
            return *drawing;
        }
        let reference = self.reference();
        let layout = self.layout(sequence);
        let glyphs: Vec<_> = layout.runs.iter().flat_map(|run| &run.glyphs).collect();
        let missing = glyphs.is_empty() || glyphs.iter().any(|glyph| glyph.id.0 == 0);
        let apart = glyphs.len() > 1 && layout.width >= reference * MOST_WIDTH;
        let drawing = if missing || apart {
            Drawing::Broken
        } else if glyphs[0].is_emoji {
            Drawing::Colour
        } else if cfg!(target_os = "macos") {
            // CoreText draws a sign that no font has with its font of last resort, which
            // has a glyph for it. Every emoji of the table that the system draws comes
            // from its colour font.
            Drawing::Broken
        } else {
            Drawing::Text
        };
        self.held.insert(sequence, drawing);
        drawing
    }
}

/// The measurements of the process. A clone shares them.
#[derive(Clone)]
pub struct Drawings(Rc<RefCell<Measure>>);

impl Global for Drawings {}

impl Drawings {
    /// Measurements with `text`, in the family of the text of messages.
    pub fn new(text: Arc<TextSystem>, family: impl Into<SharedString>) -> Self {
        Self(Rc::new(RefCell::new(Measure {
            text: WindowTextSystem::new(text),
            font: gpui::font(family),
            reference: None,
            held: HashMap::new(),
        })))
    }

    /// The measurements of the application, made on first use.
    pub fn of(cx: &mut App) -> Self {
        if let Some(drawings) = cx.try_global::<Self>() {
            return drawings.clone();
        }
        let family = crate::theme::theme(cx).fonts.sans.clone();
        let drawings = Self::new(cx.text_system().clone(), family);
        cx.set_global(drawings.clone());
        drawings
    }

    pub fn drawing(&self, emoji: &'static emojis::Emoji) -> Drawing {
        self.0.borrow_mut().drawing(emoji.as_str())
    }

    /// `text` as the window shows it: an emoji of the table that the fonts do not draw is
    /// replaced by its short code between colons, or by its name.
    pub fn shown<'a>(&self, text: &'a str) -> Cow<'a, str> {
        let mut shown = String::new();
        let mut copied = 0;
        for (start, grapheme) in
            unicode_segmentation::UnicodeSegmentation::grapheme_indices(text, true)
        {
            if grapheme.is_ascii() {
                continue;
            }
            // Only the full form of an emoji is one: a sign such as © without the
            // selector of the emoji form is text.
            let Some(emoji) = emojis::get(grapheme).filter(|emoji| emoji.as_str() == grapheme)
            else {
                continue;
            };
            if self.drawing(emoji) != Drawing::Broken {
                continue;
            }
            shown.push_str(&text[copied..start]);
            shown.push_str(&stand_in(emoji));
            copied = start + grapheme.len();
        }
        if copied == 0 {
            return Cow::Borrowed(text);
        }
        shown.push_str(&text[copied..]);
        Cow::Owned(shown)
    }
}

/// The text that stands for an emoji that the fonts do not draw.
pub fn stand_in(emoji: &emojis::Emoji) -> String {
    match emoji.shortcode() {
        Some(code) => format!(":{code}:"),
        None => format!(":{}:", emoji.name()),
    }
}

/// Measurements with the text system of the platform and the fonts of the design. The
/// text system of macOS is made on the main thread only.
#[cfg(all(test, not(target_os = "macos")))]
pub(crate) fn of_the_platform() -> Drawings {
    let platform = gpui_platform::current_platform(true).text_system();
    let text = Arc::new(TextSystem::new(platform));
    let _ = text.add_fonts(crate::assets::fonts());
    Drawings::new(text, crate::theme::Theme::stone().fonts.sans)
}

/// Measurements with the text system of the test platform of gpui, which has no fonts.
#[cfg(test)]
pub(crate) fn without_fonts() -> Drawings {
    let platform: Arc<dyn gpui::PlatformTextSystem> = Arc::new(gpui::NoopTextSystem);
    Drawings::new(Arc::new(TextSystem::new(platform)), "Sans")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn a_sign_without_a_glyph_and_a_sequence_that_falls_apart_are_broken() {
        let drawings = of_the_platform();
        let grinning = emojis::get("\u{1f600}").unwrap();
        assert_ne!(drawings.drawing(grinning), Drawing::Broken);
        let mut measure = drawings.0.borrow_mut();
        // A code point of private use has no glyph in any font of a system.
        assert_eq!(measure.drawing("\u{10fffd}"), Drawing::Broken);
        // Two emoji joined where no font has a picture of the pair.
        assert_eq!(
            measure.drawing("\u{1f600}\u{200d}\u{1f600}"),
            Drawing::Broken
        );
        assert_eq!(measure.drawing("a"), Drawing::Text);
    }

    #[test]
    fn an_emoji_that_is_not_drawn_is_shown_by_its_short_code() {
        let drawings = without_fonts();
        let grinning = emojis::get("\u{1f600}").unwrap();
        let brazil = emojis::get("\u{1f1e7}\u{1f1f7}").unwrap();
        let flags = emojis::get("\u{1f3c1}").unwrap();
        {
            let mut measure = drawings.0.borrow_mut();
            measure.held.insert(grinning.as_str(), Drawing::Colour);
            measure.held.insert(brazil.as_str(), Drawing::Broken);
            measure.held.insert(flags.as_str(), Drawing::Text);
        }
        assert_eq!(
            drawings.shown("Hi \u{1f600} from \u{1f1e7}\u{1f1f7} at \u{1f3c1}."),
            "Hi \u{1f600} from :brazil: at \u{1f3c1}."
        );
        assert!(matches!(
            drawings.shown("No emoji, and \u{a9} in its text form."),
            Cow::Borrowed(_)
        ));
    }
}
