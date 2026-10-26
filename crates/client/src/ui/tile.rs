//! The tile of one person in a call: the avatar on a dark ground, and a plate with the
//! name and the state of the microphone.

use gpui::{
    App, ElementId, Hsla, InteractiveElement, IntoElement, ParentElement, RenderOnce, Role,
    SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder,
};

use super::{Avatar, DrawnPicture, Meter, StyleExt, icon};
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
/// The size of a tile of the strip under a share, as the design draws it.
pub const STRIP_TILE_SIZE: (f32, f32) = (148.0, 84.0);
/// The space between two tiles of the stage.
pub const TILE_GAP: f32 = 8.0;

/// The columns of a grid of `count` tiles and the size of each tile: the largest 16:9 size
/// with which every tile fits in `stage`, with `TILE_GAP` between the tiles, and at least the
/// size of a tile of the strip. Below that size the tiles keep it and the stage scrolls. A
/// wide stage has no more rows than columns and a tall stage no more columns than rows, so
/// two people are side by side in a wide stage.
pub fn grid(count: usize, stage: (f32, f32)) -> (usize, (f32, f32)) {
    let (width, height) = stage;
    let count = count.max(1);
    let wide = width >= height;
    let fitted = (1..=count)
        .filter(|columns| {
            let rows = count.div_ceil(*columns);
            if wide {
                rows <= *columns
            } else {
                *columns <= rows
            }
        })
        .map(|columns| {
            let rows = count.div_ceil(columns);
            let across = (width - TILE_GAP * (columns - 1) as f32) / columns as f32;
            let down = (height - TILE_GAP * (rows - 1) as f32) / rows as f32 * 16.0 / 9.0;
            (columns, across.min(down))
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap_or((1, 0.0));
    if fitted.1 < STRIP_TILE_SIZE.0 {
        let columns = ((width + TILE_GAP) / (STRIP_TILE_SIZE.0 + TILE_GAP)).floor() as usize;
        return (columns.clamp(1, count), STRIP_TILE_SIZE);
    }
    let tile = fitted.1.floor();
    (fitted.0, (tile, (tile * 9.0 / 16.0).floor()))
}

#[derive(IntoElement)]
pub struct PersonTile {
    camera_picture: Option<DrawnPicture>,
    camera_view: Option<(junto_media::calls::Calls, junto_media::engine::Person)>,
    id: ElementId,
    name: SharedString,
    seed: u128,
    picture: Option<std::sync::Arc<gpui::RenderImage>>,
    speaking: bool,
    marks: Vec<TileMark>,
    label: SharedString,
    selector: Option<SharedString>,
    in_strip: bool,
    focused: bool,
    sidebar: bool,
    size: Option<(f32, f32)>,
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
            camera_picture: None,
            camera_view: None,
            id: id.into(),
            name: name.into(),
            seed,
            picture: None,
            speaking: false,
            marks: Vec::new(),
            label: label.into(),
            selector: None,
            in_strip: false,
            focused: false,
            sidebar: false,
            size: None,
        }
    }

    pub fn camera_view(
        mut self,
        view: Option<(junto_media::calls::Calls, junto_media::engine::Person)>,
    ) -> Self {
        self.camera_view = view;
        self
    }

    pub fn sidebar(mut self, sidebar: bool) -> Self {
        self.sidebar = sidebar;
        self
    }

    /// The size of the tile in a grid that fills the stage (`grid`).
    pub fn size(mut self, size: (f32, f32)) -> Self {
        self.size = Some(size);
        self
    }

    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// The picture of the camera of the person. It covers the avatar.
    pub fn camera_picture(mut self, picture: Option<DrawnPicture>) -> Self {
        self.camera_picture = picture;
        self
    }

    /// The image of the avatar of the person, when the window holds it.
    pub fn picture(mut self, picture: Option<std::sync::Arc<gpui::RenderImage>>) -> Self {
        self.picture = picture;
        self
    }

    /// The tile of the strip under a share: the size of the design.
    pub fn in_strip(mut self) -> Self {
        self.in_strip = true;
        self
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
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme(cx);
        let colors = theme.colors;
        let (size, avatar) = if self.sidebar {
            ((96.0, 60.0), 28.0)
        } else if let Some(size) = self.size {
            (size, (size.1 * 0.4).clamp(34.0, 128.0))
        } else if self.focused {
            ((480.0, 270.0), 51.0)
        } else if self.in_strip {
            (STRIP_TILE_SIZE, 34.0)
        } else {
            (TILE_SIZE, 51.0)
        };
        if let Some((calls, id)) = self.camera_view {
            let scale = f32::from(window.rem_size()) / 16.0 * window.scale_factor();
            calls.set_camera_view(
                id,
                (
                    (size.0 * scale).ceil() as u32,
                    (size.1 * scale).ceil() as u32,
                ),
            );
        }
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
            .w(u(size.0))
            .h(u(size.1))
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
                    .child(Avatar::new(self.seed, avatar).image(self.picture)),
            )
            .when_some(self.camera_picture, |tile, picture| {
                tile.child(div().absolute().size_full().child(picture.element()))
            })
            .child(
                div()
                    .absolute()
                    .left(u(4.0))
                    .bottom(u(4.0))
                    .max_w(u(size.0 - 10.0))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn fits(count: usize, stage: (f32, f32)) {
        let (columns, (width, height)) = grid(count, stage);
        let rows = count.div_ceil(columns);
        assert!(columns as f32 * width + TILE_GAP * (columns - 1) as f32 <= stage.0);
        assert!(rows as f32 * height + TILE_GAP * (rows - 1) as f32 <= stage.1);
        assert!((width / height - 16.0 / 9.0).abs() < 0.02);
    }

    #[test]
    fn the_tiles_fill_the_stage_with_the_largest_size_that_fits() {
        for stage in [(1176.0, 746.0), (656.0, 380.0)] {
            for count in [1, 2, 4, 9] {
                fits(count, stage);
            }
        }
        // One person: the width of the stage.
        assert_eq!(grid(1, (1176.0, 746.0)), (1, (1176.0, 661.0)));
        // Two people side by side, each about half of the width.
        assert_eq!(grid(2, (1176.0, 746.0)), (2, (584.0, 328.0)));
        // Four people in two rows of two.
        assert_eq!(grid(4, (1176.0, 746.0)), (2, (584.0, 328.0)));
        // Nine people in three rows of three.
        assert_eq!(grid(9, (1176.0, 746.0)), (3, (386.0, 217.0)));
        // A small stage: one person limited by the height, two side by side.
        assert_eq!(grid(1, (656.0, 380.0)), (1, (656.0, 369.0)));
        assert_eq!(grid(2, (656.0, 380.0)), (2, (324.0, 182.0)));
        assert_eq!(grid(4, (656.0, 380.0)), (2, (324.0, 182.0)));
        assert_eq!(grid(9, (656.0, 380.0)), (3, (213.0, 119.0)));
        // A tall stage: two people one above the other.
        assert_eq!(grid(2, (400.0, 700.0)), (1, (400.0, 225.0)));
    }

    #[test]
    fn many_tiles_keep_the_size_of_the_strip_and_the_stage_scrolls() {
        assert_eq!(grid(40, (656.0, 380.0)), (4, STRIP_TILE_SIZE));
        assert_eq!(grid(3, (100.0, 380.0)), (1, STRIP_TILE_SIZE));
    }
}
