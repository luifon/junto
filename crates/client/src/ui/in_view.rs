//! A column of lines of known heights of which only the lines in view are built (ADR 0735).
//! The column takes the full height of all its lines in the layout, so a scroll area that
//! holds it keeps its length and its offset. When the column is prepainted it reads the
//! part of it that the areas around it show (the content mask of gpui, which a scroll area
//! sets to its bounds), builds the lines in that part, lays each out at its height, and
//! places it at its place in the column, in the same frame, as the uniform list of gpui
//! does for a list that scrolls by itself. A column can lie anywhere inside a scroll area,
//! with other content above and below it.

use std::ops::Range;

use gpui::{
    AnyElement, App, AvailableSpace, Bounds, Element, ElementId, GlobalElementId,
    InspectorElementId, IntoElement, LayoutId, Pixels, Style, Window, point, px, relative, size,
};

use crate::theme::{REM_AT_100, u};

/// Builds the lines of a range, one element for each index, in order.
pub type BuildLines = Box<dyn FnOnce(Range<usize>, &mut Window, &mut App) -> Vec<AnyElement>>;

/// A column of lines of `heights`, in pixels of the design, `gap` apart, whose lines in view
/// `build` makes. The column has the full width of its parent.
pub fn in_view(
    id: impl Into<ElementId>,
    heights: Vec<f32>,
    gap: f32,
    build: impl FnOnce(Range<usize>, &mut Window, &mut App) -> Vec<AnyElement> + 'static,
) -> InView {
    InView {
        id: id.into(),
        heights,
        gap,
        build: Some(Box::new(build)),
    }
}

pub struct InView {
    id: ElementId,
    heights: Vec<f32>,
    gap: f32,
    build: Option<BuildLines>,
}

impl IntoElement for InView {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for InView {
    type RequestLayoutState = ();
    type PrepaintState = Vec<AnyElement>;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.0).into();
        style.size.height = u(length(&self.heights, self.gap)).into();
        style.flex_shrink = 0.0;
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Vec<AnyElement> {
        let Some(build) = self.build.take() else {
            return Vec::new();
        };
        let scale = f32::from(window.rem_size()) / REM_AT_100;
        let view = window.content_mask().bounds;
        let from = f32::from(view.top() - bounds.top()) / scale;
        let to = f32::from(view.bottom() - bounds.top()) / scale;
        let range = meeting(&self.heights, self.gap, from, to);
        let mut lines = build(range.clone(), window, cx);
        let mut top = top_of(&self.heights, self.gap, range.start);
        for (line, index) in lines.iter_mut().zip(range) {
            let height = self.heights[index];
            line.layout_as_root(
                size(
                    AvailableSpace::Definite(bounds.size.width),
                    AvailableSpace::Definite(px(height * scale)),
                ),
                window,
                cx,
            );
            line.prepaint_at(bounds.origin + point(px(0.0), px(top * scale)), window, cx);
            top += height + self.gap;
        }
        lines
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        lines: &mut Vec<AnyElement>,
        window: &mut Window,
        cx: &mut App,
    ) {
        for line in lines {
            line.paint(window, cx);
        }
    }
}

/// The height of a column of lines of `heights`, `gap` apart.
fn length(heights: &[f32], gap: f32) -> f32 {
    heights.iter().sum::<f32>() + gap * heights.len().saturating_sub(1) as f32
}

/// The top of the line `index` of a column of lines of `heights`, `gap` apart.
fn top_of(heights: &[f32], gap: f32, index: usize) -> f32 {
    heights[..index.min(heights.len())]
        .iter()
        .map(|height| height + gap)
        .sum()
}

/// The lines of a column of lines of `heights`, `gap` apart, that meet the part from `from`
/// to `to` of the column. Empty when that part is empty or outside the column.
fn meeting(heights: &[f32], gap: f32, from: f32, to: f32) -> Range<usize> {
    if to <= from {
        return 0..0;
    }
    let mut top = 0.0;
    let mut first = None;
    let mut last = heights.len();
    for (index, height) in heights.iter().enumerate() {
        let bottom = top + height;
        if top >= to {
            last = index;
            break;
        }
        if first.is_none() && bottom > from {
            first = Some(index);
        }
        top = bottom + gap;
    }
    match first {
        Some(first) => first..last,
        None => 0..0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lines_that_meet_a_part_of_the_column_are_those_drawn_in_it() {
        for (heights, gap) in [(vec![84.0; 100], 8.0), (vec![28.0; 601], 0.0)] {
            let full = length(&heights, gap);
            for (from, to) in [
                (0.0, 600.0),
                (2_000.0, 2_600.0),
                (2_004.0, 2_005.0),
                (9_000.0, 9_600.0),
                (-500.0, 20_000.0),
                (300.0, 300.0),
                (full + 10.0, full + 600.0),
            ] {
                let range = meeting(&heights, gap, from, to);
                let lies_in = |index: usize| {
                    let top = top_of(&heights, gap, index);
                    from < to && top < to && top + heights[index] > from
                };
                for index in 0..heights.len() {
                    assert_eq!(
                        range.contains(&index),
                        lies_in(index),
                        "{from}..{to}: line {index}, range {range:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_column_of_hundreds_of_rows_builds_the_rows_of_its_view() {
        let heights = vec![28.0; 601];
        assert_eq!(meeting(&heights, 0.0, 0.0, 280.0), 0..10);
        assert_eq!(meeting(&heights, 0.0, 2_800.0, 3_080.0), 100..110);
        assert_eq!(meeting(&heights, 0.0, 16_800.0, 20_000.0), 600..601);
        assert_eq!(meeting(&heights, 0.0, -100.0, 10.0), 0..1);
        assert_eq!(length(&heights, 0.0), 28.0 * 601.0);
        assert_eq!(top_of(&[84.0; 4], 8.0, 2), 184.0);
    }
}
