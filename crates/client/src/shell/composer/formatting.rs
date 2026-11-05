//! The formatting of the text of the composer while a person writes it (ADR 0740).
//!
//! The marks stay in the text. The content of each formatted part has the style that the
//! message will have, and the marks of a part are dimmed while the selection does not
//! touch the part. The parts come from the same parse as the message ([`compose::formats`]).

use std::collections::BTreeSet;
use std::ops::Range;

use gpui::{FontStyle, FontWeight, HighlightStyle, StrikethroughStyle, UnderlineStyle, px};
use gpui_base::input::{Rope, TextDecoration};

use crate::compose::{self, Format, Formatted, Mention};
use crate::theme::Colors;

/// The formatted parts of the text of one composer, and the decorations that it shows.
#[derive(Default)]
pub(crate) struct Formatting {
    /// The text and the mentions that the parts were read from.
    read: Option<(Rope, Vec<Mention>)>,
    parts: Vec<Formatted>,
    /// The selection and the colours of the shown decorations.
    shown: Option<(Range<usize>, Colors)>,
}

impl Formatting {
    /// The decorations for the text, its mentions and its selection, when they differ from
    /// the shown decorations. The parts are read again only after a change of the text or
    /// of its mentions.
    pub(crate) fn update(
        &mut self,
        text: &Rope,
        mentions: Vec<Mention>,
        selection: Range<usize>,
        colors: Colors,
    ) -> Option<Vec<TextDecoration>> {
        let same_text = self
            .read
            .as_ref()
            .is_some_and(|(read, read_mentions)| read == text && *read_mentions == mentions);
        if !same_text {
            self.parts = compose::formats(&text.to_string(), &mentions);
            self.read = Some((text.clone(), mentions));
            self.shown = None;
        }
        let shown = (selection, colors);
        if self.shown.as_ref() == Some(&shown) {
            return None;
        }
        let decorations = decorations(&self.parts, shown.0.clone(), &colors);
        self.shown = Some(shown);
        Some(decorations)
    }
}

/// The selection touches a part when it starts or ends inside it or at one of its ends, or
/// holds it.
fn touches(part: &Range<usize>, selection: &Range<usize>) -> bool {
    selection.start <= part.end && selection.end >= part.start
}

/// The style of the content of a part, as the message shows it (`shell::feed`).
fn content_style(format: Format, colors: &Colors) -> HighlightStyle {
    let line = || Some(px(1.0));
    match format {
        Format::Bold | Format::Heading => HighlightStyle {
            font_weight: Some(FontWeight::BOLD),
            ..HighlightStyle::default()
        },
        Format::Italic => HighlightStyle {
            font_style: Some(FontStyle::Italic),
            ..HighlightStyle::default()
        },
        Format::Strikethrough => HighlightStyle {
            strikethrough: line().map(|thickness| StrikethroughStyle {
                thickness,
                color: None,
            }),
            ..HighlightStyle::default()
        },
        Format::Code => HighlightStyle {
            color: Some(colors.terracotta_deep.into()),
            background_color: Some(colors.code_wash.into()),
            font_weight: Some(FontWeight::MEDIUM),
            ..HighlightStyle::default()
        },
        Format::CodeBlock => HighlightStyle {
            color: Some(colors.terracotta_deep.into()),
            background_color: Some(colors.code_wash.into()),
            ..HighlightStyle::default()
        },
        Format::Link => HighlightStyle {
            color: Some(colors.cypress_ink.into()),
            underline: line().map(|thickness| UnderlineStyle {
                thickness,
                color: None,
                wavy: false,
            }),
            ..HighlightStyle::default()
        },
        Format::Spoiler => HighlightStyle {
            background_color: Some(colors.rule_soft.into()),
            ..HighlightStyle::default()
        },
        Format::Quote | Format::Small => HighlightStyle {
            color: Some(colors.ink_2.into()),
            ..HighlightStyle::default()
        },
        Format::Item => HighlightStyle::default(),
    }
}

/// The decorations of the text: the content of each part in its format, then the marks of
/// each part that the selection does not touch, dimmed. A later style sets a property over
/// an earlier one: the content of an inner part over that of an outer part, and a mark
/// over a content. The decorations do not overlap, because the text area combines
/// overlapping decorations in no fixed order.
pub(crate) fn decorations(
    parts: &[Formatted],
    selection: Range<usize>,
    colors: &Colors,
) -> Vec<TextDecoration> {
    let mut layers: Vec<(Range<usize>, HighlightStyle)> = parts
        .iter()
        .map(|part| (part.content.clone(), content_style(part.format, colors)))
        .collect();
    let dimmed = HighlightStyle {
        color: Some(colors.ink_3.into()),
        ..HighlightStyle::default()
    };
    for part in parts {
        if !touches(&part.whole, &selection) {
            layers.push((part.whole.start..part.content.start, dimmed));
            layers.push((part.content.end..part.whole.end, dimmed));
        }
    }
    flattened(&layers)
}

/// The layers as ranges that do not overlap, each with the styles of the layers over it
/// applied in their order. Adjacent ranges of the same style are joined.
fn flattened(layers: &[(Range<usize>, HighlightStyle)]) -> Vec<TextDecoration> {
    let mut ends: Vec<(usize, bool, usize)> = Vec::with_capacity(layers.len() * 2);
    for (index, (range, _)) in layers.iter().enumerate() {
        if !range.is_empty() {
            ends.push((range.start, true, index));
            ends.push((range.end, false, index));
        }
    }
    ends.sort_unstable_by_key(|(at, _, _)| *at);
    let mut active = BTreeSet::new();
    let mut made: Vec<TextDecoration> = Vec::new();
    let mut from = 0;
    for (at, starts, index) in ends {
        if at > from && !active.is_empty() {
            let style = active
                .iter()
                .fold(HighlightStyle::default(), |style, index: &usize| {
                    style.highlight(layers[*index].1)
                });
            match made.last_mut() {
                Some(last) if last.range.end == from && last.style == style => last.range.end = at,
                _ => made.push(TextDecoration::new(from..at, style)),
            }
        }
        from = at;
        if starts {
            active.insert(index);
        } else {
            active.remove(&index);
        }
    }
    made
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(format: Format, whole: Range<usize>, content: Range<usize>) -> Formatted {
        Formatted {
            format,
            whole,
            content,
        }
    }

    /// The marks of a part are dimmed until the selection touches the part; the content
    /// keeps its format; the decorations do not overlap and a mark inside an outer part
    /// takes the dimmed colour over the colour of the outer part.
    #[test]
    fn the_marks_are_dimmed_until_the_caret_is_in_their_part() {
        let colors = crate::theme::Theme::stone().colors;
        // "> a **b** c": a quote with a bold part.
        let parts = [
            part(Format::Quote, 0..11, 2..11),
            part(Format::Bold, 4..9, 6..7),
        ];
        let dimmed = Some(colors.ink_3.into());
        let quote = Some(colors.ink_2.into());
        let away = decorations(&parts, 0..0, &colors);
        // The caret at the start of the text touches the quote, not the bold part.
        let styles: Vec<_> = away
            .iter()
            .map(|decoration| {
                (
                    decoration.range.clone(),
                    decoration.style.color,
                    decoration.style.font_weight,
                )
            })
            .collect();
        assert_eq!(
            styles,
            vec![
                (2..4, quote, None),
                (4..6, dimmed, None),
                (6..7, quote, Some(FontWeight::BOLD)),
                (7..9, dimmed, None),
                (9..11, quote, None),
            ]
        );
        let inside = decorations(&parts, 7..7, &colors);
        assert!(
            inside
                .iter()
                .all(|decoration| decoration.style.color != dimmed),
            "{inside:?}"
        );
        // A caret after the text touches no part: the mark of the quote is dimmed too.
        let far = decorations(&parts, 20..20, &colors);
        assert_eq!(far.first().map(|first| first.range.clone()), Some(0..2));
        assert!(
            far.windows(2)
                .all(|pair| pair[0].range.end <= pair[1].range.start)
        );
    }

    /// The shown decorations change only when the text, its mentions, the selection or the
    /// colours change.
    #[test]
    fn the_decorations_are_made_again_only_after_a_change() {
        let colors = crate::theme::Theme::stone().colors;
        let mut formatting = Formatting::default();
        let text = Rope::from("**a** b");
        let first = formatting.update(&text, Vec::new(), 7..7, colors);
        assert!(first.is_some_and(|decorations| !decorations.is_empty()));
        assert_eq!(formatting.update(&text, Vec::new(), 7..7, colors), None);
        assert!(formatting.update(&text, Vec::new(), 1..1, colors).is_some());
        let longer = Rope::from("**a** b *c*");
        assert!(
            formatting
                .update(&longer, Vec::new(), 1..1, colors)
                .is_some()
        );
        assert_eq!(formatting.parts.len(), 2);
        let dark = crate::theme::Theme::broadcast().colors;
        assert!(formatting.update(&longer, Vec::new(), 1..1, dark).is_some());
    }
}
