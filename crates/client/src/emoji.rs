//! Which emoji of the table the fonts of this system draw (ADR 0758).
//!
//! The window draws emoji with the emoji fonts of the system, and they can be older than
//! the table of the server, or lack a kind of sequence. The whole table is laid out once,
//! on the background executor after the first use, in the family of the text of messages.
//! Until the results arrive every emoji counts as drawn.

use std::borrow::Cow;
use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use gpui::{
    App, Font, FontId, Global, Pixels, SharedString, TextRun, TextSystem, WindowTextSystem, px,
};

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

/// A code point of private use, which no font of a system has.
const NO_FONT: &str = "\u{10fffd}";

/// A glyph whose advance is less than this part of the reference takes no place of its
/// own: a joiner, a variation selector, a tag or a combining keycap.
const LEAST_ADVANCE: f32 = 0.25;

/// Colour glyphs that take place on a line at least this many times as wide as the
/// reference stand side by side.
const MOST_WIDTH: f32 = 1.5;

/// The layout of signs at one size in one family, with a text system of its own, so that
/// nothing that it lays out stays in a cache of a window.
pub struct Measure {
    text: WindowTextSystem,
    font: Font,
    reference: Pixels,
    /// The font that draws a sign that no font has, where the system draws one: the font
    /// of last resort of CoreText.
    last_resort: Option<FontId>,
}

impl Measure {
    /// A measure with `text`, in `family`, the family of the text of messages.
    pub fn new(text: Arc<TextSystem>, family: impl Into<SharedString>) -> Self {
        let mut measure = Self {
            text: WindowTextSystem::new(text),
            font: gpui::font(family),
            reference: px(SIZE),
            last_resort: None,
        };
        let reference = measure.layout(REFERENCE);
        let glyphs: Vec<_> = reference.runs.iter().flat_map(|run| &run.glyphs).collect();
        if glyphs.len() == 1 && glyphs[0].id.0 != 0 && reference.width > px(0.0) {
            measure.reference = reference.width;
        }
        let nothing = measure.layout(NO_FONT);
        measure.last_resort = nothing
            .runs
            .iter()
            .find(|run| {
                run.glyphs
                    .iter()
                    .any(|glyph| glyph.id.0 != 0 && !glyph.is_emoji)
            })
            .map(|run| run.font_id);
        measure
    }

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

    /// How the fonts draw `sequence`.
    pub fn drawing(&self, sequence: &str) -> Drawing {
        let layout = self.layout(sequence);
        let glyphs: Vec<_> = layout
            .runs
            .iter()
            .flat_map(|run| run.glyphs.iter().map(move |glyph| (run.font_id, glyph)))
            .collect();
        let missing = glyphs.is_empty()
            || glyphs
                .iter()
                .any(|(font_id, glyph)| glyph.id.0 == 0 || Some(*font_id) == self.last_resort);
        // The advance of a glyph is the distance to the next glyph, or to the end of the
        // line for the last one.
        let ends = glyphs
            .iter()
            .skip(1)
            .map(|(_, glyph)| glyph.position.x)
            .chain(std::iter::once(layout.width));
        let placed: Vec<_> = glyphs
            .iter()
            .zip(ends)
            .filter(|((_, glyph), end)| *end - glyph.position.x > self.reference * LEAST_ADVANCE)
            .map(|((_, glyph), _)| glyph)
            .collect();
        // Several glyphs that take place fall apart when one of them is a sign without
        // colour, such as the two letters of a flag, or when they stand side by side. The
        // colour glyphs of one picture can overlap within its width, as the family
        // sequences of Segoe UI Emoji do.
        let apart = placed.len() > 1
            && (placed.iter().any(|glyph| !glyph.is_emoji)
                || layout.width >= self.reference * MOST_WIDTH);
        if missing || apart {
            Drawing::Broken
        } else if glyphs[0].1.is_emoji {
            Drawing::Colour
        } else {
            Drawing::Text
        }
    }

    /// The glyphs of `sequence` as text: for each, its number, whether it is a colour
    /// glyph, its font and its position; and the width of the line. For the log of a test.
    pub fn glyphs(&self, sequence: &str) -> String {
        let layout = self.layout(sequence);
        let mut out = String::new();
        for run in &layout.runs {
            for glyph in &run.glyphs {
                out.push_str(&format!(
                    "{}{} font {} at {:.1}; ",
                    glyph.id.0,
                    if glyph.is_emoji { " colour" } else { "" },
                    run.font_id.0,
                    f32::from(glyph.position.x)
                ));
            }
        }
        out.push_str(&format!(
            "width {:.1}, reference {:.1}, last resort {:?}",
            f32::from(layout.width),
            f32::from(self.reference),
            self.last_resort.map(|font| font.0)
        ));
        out
    }

    /// How the fonts draw each emoji of the table, with each skin tone.
    pub fn table(&self) -> HashMap<&'static str, Drawing> {
        table()
            .map(|emoji| (emoji.as_str(), self.drawing(emoji.as_str())))
            .collect()
    }
}

/// Every emoji of the table with each of its skin tones.
pub fn table() -> impl Iterator<Item = &'static emojis::Emoji> {
    emojis::iter().flat_map(|emoji| {
        std::iter::once(emoji).chain(emoji.skin_tones().into_iter().flatten().skip(1))
    })
}

/// The results of the process: absent until the measurement has ended.
#[derive(Default)]
struct Results {
    started: bool,
    table: Option<Arc<HashMap<&'static str, Drawing>>>,
}

impl Global for Results {}

/// The measurements of the table as the window reads them. Before they arrive every emoji
/// counts as drawn.
#[derive(Clone, Default)]
pub struct Drawings(Option<Arc<HashMap<&'static str, Drawing>>>);

impl Drawings {
    /// The measurements, after the measurement of the table is started on first use.
    pub fn of(cx: &mut App) -> Self {
        if !cx
            .try_global::<Results>()
            .is_some_and(|results| results.started)
        {
            start(cx);
        }
        Self::current(cx)
    }

    /// The measurements that have arrived, without starting one.
    pub fn current(cx: &App) -> Self {
        Self(
            cx.try_global::<Results>()
                .and_then(|results| results.table.clone()),
        )
    }

    /// Measurements that are given, for a test.
    pub fn given(table: HashMap<&'static str, Drawing>) -> Self {
        Self(Some(Arc::new(table)))
    }

    /// How the fonts draw `emoji`; an emoji not measured yet counts as drawn.
    pub fn drawing(&self, emoji: &emojis::Emoji) -> Drawing {
        self.0
            .as_ref()
            .and_then(|table| table.get(emoji.as_str()).copied())
            .unwrap_or(Drawing::Colour)
    }

    /// `text` as the window shows it: an emoji of the table that the fonts do not draw is
    /// replaced by a stand-in (ADR 0758).
    pub fn shown<'a>(&self, text: &'a str) -> Shown<'a> {
        let mut shown = Shown {
            text: Cow::Borrowed(text),
            replaced: Vec::new(),
        };
        if self.0.is_none() {
            return shown;
        }
        let mut out = String::new();
        let mut copied = 0;
        for (start, grapheme) in
            unicode_segmentation::UnicodeSegmentation::grapheme_indices(text, true)
        {
            if grapheme.is_ascii() {
                continue;
            }
            // A sign such as © without the selector of the emoji form is text; a sequence
            // without some of its selectors is the emoji of the table.
            let Some(emoji) = emojis::get(grapheme)
                .filter(|emoji| emoji.as_str() == grapheme || grapheme.chars().count() > 1)
            else {
                continue;
            };
            if self.drawing(emoji) != Drawing::Broken {
                continue;
            }
            out.push_str(&text[copied..start]);
            let at = out.len();
            out.push_str(&stand_in(emoji));
            shown.replaced.push(Replaced {
                from: start..start + grapheme.len(),
                to: at..out.len(),
            });
            copied = start + grapheme.len();
        }
        if !shown.replaced.is_empty() {
            out.push_str(&text[copied..]);
            shown.text = Cow::Owned(out);
        }
        shown
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Replaced {
    from: Range<usize>,
    to: Range<usize>,
}

/// A text as the window shows it, with the places of its stand-ins.
#[derive(Debug, Clone)]
pub struct Shown<'a> {
    pub text: Cow<'a, str>,
    replaced: Vec<Replaced>,
}

impl Shown<'_> {
    /// The ranges of the stand-ins in the shown text.
    pub fn stand_ins(&self) -> impl Iterator<Item = Range<usize>> + '_ {
        self.replaced.iter().map(|replaced| replaced.to.clone())
    }

    /// A range of the original text as a range of the shown text. A range that starts or
    /// ends in a replaced emoji takes in its whole stand-in.
    pub fn range(&self, range: Range<usize>) -> Range<usize> {
        self.offset(range.start, false)..self.offset(range.end, true)
    }

    fn offset(&self, offset: usize, end: bool) -> usize {
        let mut shift = 0isize;
        for replaced in &self.replaced {
            if offset <= replaced.from.start {
                break;
            }
            if offset < replaced.from.end {
                return if end {
                    replaced.to.end
                } else {
                    replaced.to.start
                };
            }
            shift = replaced.to.end as isize - replaced.from.end as isize;
        }
        offset.saturating_add_signed(shift)
    }

    pub fn into_owned(self) -> String {
        self.text.into_owned()
    }
}

/// The text that stands for an emoji that the fonts do not draw: its short code, or its
/// name, between colons.
pub fn stand_in(emoji: &emojis::Emoji) -> String {
    match emoji.shortcode() {
        Some(code) => format!(":{code}:"),
        None => format!(":{}:", emoji.name().replace(": ", ", ")),
    }
}

/// Measures the table on the calling thread and keeps the results, for a tool that draws
/// screens once.
pub fn measure_now(cx: &mut App) {
    let text = cx.text_system().clone();
    let family = crate::theme::theme(cx).fonts.sans.clone();
    let table = Measure::new(text, family).table();
    cx.set_global(Results {
        started: true,
        table: Some(Arc::new(table)),
    });
}

/// Measures the table on the background executor once, keeps the results and draws the
/// windows again.
fn start(cx: &mut App) {
    cx.set_global(Results {
        started: true,
        table: None,
    });
    let text = cx.text_system().clone();
    let family = crate::theme::theme(cx).fonts.sans.clone();
    let measured = cx
        .background_executor()
        .spawn(async move { Measure::new(text, family).table() });
    cx.spawn(async move |cx| {
        let table = Arc::new(measured.await);
        cx.update(|cx| {
            cx.set_global(Results {
                started: true,
                table: Some(table),
            });
            cx.refresh_windows();
        });
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn given(pairs: &[(&'static str, Drawing)]) -> Drawings {
        Drawings::given(pairs.iter().copied().collect())
    }

    #[test]
    fn an_emoji_that_is_not_drawn_is_shown_by_its_short_code_or_its_name() {
        let drawings = given(&[
            ("\u{1f600}", Drawing::Colour),
            ("\u{1f1e7}\u{1f1f7}", Drawing::Broken),
            ("\u{1f3c1}", Drawing::Text),
            ("\u{1f44d}\u{1f3fd}", Drawing::Broken),
            ("\u{1f3f3}\u{fe0f}\u{200d}\u{1f308}", Drawing::Broken),
        ]);
        let shown = drawings.shown("Hi \u{1f600} from \u{1f1e7}\u{1f1f7} at \u{1f3c1}.");
        assert_eq!(shown.text, "Hi \u{1f600} from :brazil: at \u{1f3c1}.");
        let stand_ins: Vec<&str> = shown.stand_ins().map(|range| &shown.text[range]).collect();
        assert_eq!(stand_ins, [":brazil:"]);
        // A skin tone has no short code: its name, without a colon inside.
        assert_eq!(
            drawings.shown("\u{1f44d}\u{1f3fd}").text,
            ":thumbs up, medium skin tone:"
        );
        // A sequence without its selector is the emoji of the table.
        assert_eq!(
            drawings.shown("\u{1f3f3}\u{200d}\u{1f308}").text,
            ":rainbow_flag:"
        );
        assert!(matches!(
            drawings
                .shown("No emoji, and \u{a9} in its text form.")
                .text,
            Cow::Borrowed(_)
        ));
        // Before the measurement has ended nothing is replaced.
        assert!(matches!(
            Drawings::default().shown("\u{1f1e7}\u{1f1f7}").text,
            Cow::Borrowed(_)
        ));
    }

    #[test]
    fn a_range_of_the_text_follows_the_stand_ins() {
        let drawings = given(&[("\u{1f1e7}\u{1f1f7}", Drawing::Broken)]);
        let text = "a \u{1f1e7}\u{1f1f7} b \u{1f1e7}\u{1f1f7} c";
        let shown = drawings.shown(text);
        assert_eq!(shown.text, "a :brazil: b :brazil: c");
        let b = text.find('b').unwrap();
        let c = text.find('c').unwrap();
        assert_eq!(&shown.text[shown.range(b..b + 1)], "b");
        assert_eq!(&shown.text[shown.range(c..c + 1)], "c");
        assert_eq!(&shown.text[shown.range(0..b + 1)], "a :brazil: b");
        // A range inside an emoji takes in its stand-in.
        assert_eq!(&shown.text[shown.range(3..4)], ":brazil:");
    }

    #[test]
    fn two_letters_are_broken_however_narrow_they_are() {
        let platform: Arc<dyn gpui::PlatformTextSystem> = Arc::new(gpui::NoopTextSystem);
        let measure = Measure::new(Arc::new(TextSystem::new(platform)), "Sans");
        // The test text system draws each character of the astral planes as one picture
        // two wide, and every other character as one sign one wide: two letters are as
        // wide as one picture, as a flag that falls apart into two letters can be.
        assert_eq!(measure.drawing("\u{1f600}"), Drawing::Colour);
        assert_eq!(measure.drawing("\u{1f1e7}\u{1f1f7}"), Drawing::Broken);
        assert_eq!(measure.drawing("a"), Drawing::Text);
        assert_eq!(measure.drawing("BR"), Drawing::Broken);
    }

    #[gpui::test]
    fn the_table_is_measured_off_the_window_thread_and_then_the_windows_draw_again(
        cx: &mut gpui::TestAppContext,
    ) {
        let brazil = emojis::get("\u{1f1e7}\u{1f1f7}").unwrap();
        cx.update(|cx| crate::theme::install(crate::theme::Theme::stone(), cx));
        // The first use starts the measurement and does not wait for it.
        let first = cx.update(|cx| Drawings::of(cx).drawing(brazil));
        assert_eq!(first, Drawing::Colour);
        cx.run_until_parked();
        let measured = cx.update(|cx| Drawings::current(cx).drawing(brazil));
        assert_eq!(measured, Drawing::Broken);
    }
}
