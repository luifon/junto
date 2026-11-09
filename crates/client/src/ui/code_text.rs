//! The text of a code block with the colour of each part. The runs of colour are made from
//! the text style of the block and given to the text as they are, so that the colours that
//! a test reads are the colours that the text is drawn with (ADR 0902).

use std::ops::Range;

use gpui::{Hsla, IntoElement, SharedString, StyledText, TextRun};

/// Text whose parts in `colours` are drawn in their colour, and the rest in the colour of
/// the text style.
pub(crate) fn code_text(text: SharedString, colours: Vec<(Range<usize>, Hsla)>) -> CodeText {
    CodeText {
        text,
        colours,
        styled: None,
        #[cfg(feature = "test-support")]
        drawn: Vec::new(),
    }
}

pub(crate) struct CodeText {
    text: SharedString,
    colours: Vec<(Range<usize>, Hsla)>,
    styled: Option<StyledText>,
    #[cfg(feature = "test-support")]
    drawn: Vec<(Range<usize>, Hsla)>,
}

/// The runs of the text: the parts of `colours`, in order and within the text, take their
/// colour; the parts between them take the colour of `base`.
fn runs(text: &str, colours: &[(Range<usize>, Hsla)], base: &TextRun) -> Vec<TextRun> {
    let mut runs = Vec::new();
    let mut at = 0;
    let mut push = |len: usize, color: Hsla| {
        if len > 0 {
            runs.push(TextRun {
                len,
                color,
                ..base.clone()
            });
        }
    };
    for (range, color) in colours {
        if range.start < at || range.end > text.len() || range.start > range.end {
            continue;
        }
        push(range.start - at, base.color);
        push(range.end - range.start, *color);
        at = range.end;
    }
    push(text.len() - at, base.color);
    runs
}

impl IntoElement for CodeText {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl gpui::Element for CodeText {
    type RequestLayoutState = <StyledText as gpui::Element>::RequestLayoutState;
    type PrepaintState = <StyledText as gpui::Element>::PrepaintState;

    fn id(&self) -> Option<gpui::ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        id: Option<&gpui::GlobalElementId>,
        inspector: Option<&gpui::InspectorElementId>,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> (gpui::LayoutId, Self::RequestLayoutState) {
        let base = window.text_style().to_run(self.text.len());
        let runs = runs(&self.text, &self.colours, &base);
        #[cfg(feature = "test-support")]
        {
            let mut at = 0;
            self.drawn = runs
                .iter()
                .map(|run| {
                    let part = (at..at + run.len, run.color);
                    at += run.len;
                    part
                })
                .collect();
        }
        self.styled
            .insert(StyledText::new(self.text.clone()).with_runs(runs))
            .request_layout(id, inspector, window, cx)
    }
    fn prepaint(
        &mut self,
        id: Option<&gpui::GlobalElementId>,
        inspector: Option<&gpui::InspectorElementId>,
        bounds: gpui::Bounds<gpui::Pixels>,
        layout: &mut Self::RequestLayoutState,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> Self::PrepaintState {
        match &mut self.styled {
            Some(styled) => styled.prepaint(id, inspector, bounds, layout, window, cx),
            None => Default::default(),
        }
    }
    fn paint(
        &mut self,
        id: Option<&gpui::GlobalElementId>,
        inspector: Option<&gpui::InspectorElementId>,
        bounds: gpui::Bounds<gpui::Pixels>,
        layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) {
        if let Some(styled) = &mut self.styled {
            styled.paint(id, inspector, bounds, layout, prepaint, window, cx);
        }
        #[cfg(feature = "test-support")]
        if window.content_mask().bounds.intersects(&bounds) {
            PAINTED.with_borrow_mut(|painted| {
                if let Some(painted) = painted {
                    painted.push(PaintedCode {
                        text: self.text.clone(),
                        colours: self.drawn.clone(),
                    });
                }
            });
        }
    }
}

/// A code text that was painted, with the colour of each of its parts.
#[cfg(feature = "test-support")]
#[derive(Debug, Clone)]
pub struct PaintedCode {
    pub text: SharedString,
    pub colours: Vec<(Range<usize>, Hsla)>,
}

#[cfg(feature = "test-support")]
thread_local! {
    static PAINTED: std::cell::RefCell<Option<Vec<PaintedCode>>> = const { std::cell::RefCell::new(None) };
}

/// Records the code texts that are painted until it is dropped.
#[cfg(feature = "test-support")]
#[must_use]
pub struct CodePaintRecord(());

#[cfg(feature = "test-support")]
impl Drop for CodePaintRecord {
    fn drop(&mut self) {
        PAINTED.set(None);
    }
}

#[cfg(feature = "test-support")]
pub fn record_painted_code() -> CodePaintRecord {
    PAINTED.set(Some(Vec::new()));
    CodePaintRecord(())
}

#[cfg(feature = "test-support")]
pub fn take_painted_code() -> Vec<PaintedCode> {
    PAINTED.with_borrow_mut(|painted| painted.as_mut().map(std::mem::take).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_parts_of_colour_and_the_parts_between_them_cover_the_text() {
        let base = TextRun {
            len: 11,
            font: gpui::font("Mono"),
            color: gpui::black(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let red = gpui::red();
        let made = runs("fn main() {", &[(0..2, red), (3..7, red)], &base);
        let parts: Vec<(usize, Hsla)> = made.iter().map(|run| (run.len, run.color)).collect();
        assert_eq!(
            parts,
            [(2, red), (1, gpui::black()), (4, red), (4, gpui::black())]
        );
    }
}
