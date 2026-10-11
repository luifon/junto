//! The tile of one person in a call: the avatar on a dark ground, and a plate with the
//! name and the state of the microphone.

use gpui::{
    App, ElementId, Hsla, InteractiveElement, IntoElement, ParentElement, RenderOnce, Role,
    SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder,
};

use super::{Meter, Mosaic, MosaicKind, StyleExt, icon};
use crate::assets::Icon;
use crate::theme::{theme, u};

/// What the plate of a tile shows before the name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TileMark {
    MicrophoneOff,
    MicrophoneHeld,
    SoundOff,
    Sharing,
    HandRaised,
    /// The level of an open microphone, as lit segments of five.
    Level(usize),
}

/// The size of a tile in design pixels: one and a half times the tile of the call strip of
/// the design, which keeps its proportions.
pub const TILE_SIZE: (f32, f32) = (222.0, 126.0);

#[derive(IntoElement)]
pub struct PersonTile {
    id: ElementId,
    name: SharedString,
    seed: u128,
    speaking: bool,
    marks: Vec<TileMark>,
    label: SharedString,
    selector: Option<SharedString>,
}

impl PersonTile {
    /// `label` is the name with the state of the person, for the accessibility tree.
    pub fn new(
        id: impl Into<ElementId>,
        name: impl Into<SharedString>,
        seed: u128,
        label: impl Into<SharedString>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            seed,
            speaking: false,
            marks: Vec::new(),
            label: label.into(),
            selector: None,
        }
    }

    /// The person speaks: the tile has the bright edge.
    pub fn speaking(mut self, speaking: bool) -> Self {
        self.speaking = speaking;
        self
    }

    pub fn marks(mut self, marks: Vec<TileMark>) -> Self {
        self.marks = marks;
        self
    }

    pub fn selector(mut self, selector: impl Into<SharedString>) -> Self {
        self.selector = Some(selector.into());
        self
    }
}

impl RenderOnce for PersonTile {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme(cx);
        let colors = theme.colors;
        let marks = self.marks.into_iter().map(|mark| match mark {
            TileMark::MicrophoneOff => {
                icon(Icon::MicrophoneOff, 13.0, colors.plate_warning).into_any_element()
            }
            TileMark::MicrophoneHeld => {
                icon(Icon::MicrophoneHeld, 13.0, colors.plate_warning).into_any_element()
            }
            TileMark::SoundOff => {
                icon(Icon::PhonesOff, 13.0, colors.plate_warning).into_any_element()
            }
            TileMark::Sharing => icon(Icon::Screen, 13.0, colors.ochre).into_any_element(),
            TileMark::HandRaised => icon(Icon::Hand, 13.0, colors.ochre).into_any_element(),
            TileMark::Level(lit) => Meter::plate(lit).into_any_element(),
        });
        div()
            .id(self.id)
            .role(Role::ListItem)
            .aria_label(self.label)
            .when_some(self.selector, |tile, selector| {
                tile.debug_selector(move || selector.to_string())
            })
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .w(u(TILE_SIZE.0))
            .h(u(TILE_SIZE.1))
            .overflow_hidden()
            .rounded(u(theme.metrics.radius))
            .bg(Hsla::from(colors.basalt_2))
            .map(|tile| {
                if self.speaking {
                    tile.border_2()
                        .border_color(Hsla::from(colors.olive_bright))
                } else {
                    tile.border_1().border_color(Hsla::from(colors.basalt_rule))
                }
            })
            .child(
                div()
                    .mb(u(16.0))
                    .child(Mosaic::new(MosaicKind::Large, self.seed, 51.0)),
            )
            .child(
                div()
                    .absolute()
                    .left(u(4.0))
                    .bottom(u(4.0))
                    .max_w(u(TILE_SIZE.0 - 10.0))
                    .flex()
                    .items_center()
                    .gap(u(6.0))
                    .h(u(21.0))
                    .px(u(6.0))
                    .rounded(u(theme.metrics.radius_small))
                    .bg(Hsla::from(colors.plate))
                    .typeface(theme.types.small, &theme)
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .ink(colors.bone)
                    .whitespace_nowrap()
                    .children(marks)
                    .child(div().min_w_0().truncate().child(self.name)),
            )
    }
}
