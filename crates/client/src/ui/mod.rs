//! The components that the screens are made of (ADR 0731). Each has one definition here,
//! takes its colours, type and measures from the theme, and carries its own keyboard
//! behaviour: a control is a tab stop, shows a mark while it holds the keyboard focus, and
//! is activated with Enter or Space. A control has a role and a label for the accessibility
//! tree (ADR 0702).

mod button;
mod controls;
mod dialog;
mod field;
mod marks;
mod menu;
mod mosaic;
mod panel;
mod pattern;
mod rows;
mod scroll;
mod segmented;
mod tile;
mod tooltip;

use std::sync::Arc;

use gpui::{App, BoxShadow, FocusHandle, FontFeatures, Hsla, Rgba, Styled, Svg, Window, px, svg};

pub use button::{Button, ButtonKind};
pub use controls::{SelectBox, Stepper, Tab, Toggle, VolumeSlider};
pub use dialog::{Dialog, dialog_footer};
pub use field::{Field, FieldEvent, FieldKind};
pub use marks::{Badge, Chip, Flag, FlagKind, RecordingMark, StatusLine, unread_dot};
pub use menu::{Menu, MenuItem, menu_label, menu_separator};
pub use mosaic::{Mosaic, MosaicKind};
pub use panel::{Notice, NoticeKind, SectionLabel, SideHeader, StatePanel, WaitBox};
pub use pattern::{Band, BandKind, Meter, selection_mark};
pub use rows::{
    CategoryHeader, ChannelRow, ChannelState, OccupantMark, OccupantRow, UnreadSign, WorkspaceRow,
};
pub use scroll::ScrollArea;
pub use segmented::Segmented;
pub use tile::{PersonTile, TileMark};
pub use tooltip::tooltip;

use crate::assets::Icon;
use crate::theme::{Theme, Type, u};

/// Text styles and the marks of a keyboard focus, for every element.
pub trait StyleExt: Styled + Sized {
    /// Sets the family, the size, the line height and the weight of a text style.
    fn typeface(self, style: Type, theme: &Theme) -> Self {
        self.font_family(theme.family(style.family))
            .text_size(u(style.size))
            .line_height(u(style.line_height))
            .font_weight(style.weight)
    }

    fn ink(self, colour: Rgba) -> Self {
        self.text_color(Hsla::from(colour))
    }
}

impl<T: Styled> StyleExt for T {}

/// Digits of one width, so that a number that changes does not move the text after it.
pub fn tabular_numerals() -> FontFeatures {
    FontFeatures(Arc::new(vec![("tnum".into(), 1)]))
}

/// An icon at a size in design pixels, in one colour. An icon does not take the text colour
/// of its place.
pub fn icon(icon: Icon, size: f32, colour: Rgba) -> Svg {
    svg()
        .path(icon.path())
        .size(u(size))
        .flex_none()
        .text_color(Hsla::from(colour))
}

/// The mark of a keyboard focus around a control.
pub(crate) fn focus_ring(colour: Rgba, width: f32) -> Vec<BoxShadow> {
    vec![BoxShadow::new(px(0.0), px(0.0), colour.into()).spread_radius(px(width))]
}

/// The mark of a keyboard focus inside the edge of a row.
pub(crate) fn focus_ring_inside(colour: Rgba, width: f32) -> Vec<BoxShadow> {
    vec![
        BoxShadow::new(px(0.0), px(0.0), colour.into())
            .spread_radius(px(width))
            .inset(),
    ]
}

/// Moves the keyboard focus to the next or the previous control inside a layer that holds
/// the focus. At the last control the focus returns to the first one.
pub(crate) fn move_focus_inside(
    layer: &FocusHandle,
    forward: bool,
    window: &mut Window,
    cx: &mut App,
) {
    // The tab order of the window continues outside the layer: the focus is moved until it
    // is inside again. The bound ends the search in a layer without a control.
    for _ in 0..200 {
        if forward {
            window.focus_next(cx);
        } else {
            window.focus_prev(cx);
        }
        if layer.contains_focused(window, cx) && !layer.is_focused(window) {
            return;
        }
    }
    layer.focus(window, cx);
}

/// The line under a raised control.
pub(crate) fn ledge(colour: Rgba) -> Vec<BoxShadow> {
    vec![BoxShadow::new(px(0.0), px(1.0), colour.into())]
}

/// The shadow of a layer that floats above the window content.
pub(crate) fn float_shadow(colour: Rgba, offset: f32, blur: f32) -> Vec<BoxShadow> {
    vec![BoxShadow::new(px(0.0), px(offset), colour.into()).blur_radius(px(blur))]
}
