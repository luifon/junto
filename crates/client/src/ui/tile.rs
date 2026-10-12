//! The tile of one person in a call: the avatar on a dark ground, and a plate with the
//! name and the state of the microphone.

use gpui::{
    App, ElementId, Hsla, InteractiveElement, IntoElement, ParentElement, RenderOnce, Role,
    SharedString, StatefulInteractiveElement, Styled, StyledImage, Window, div,
    prelude::FluentBuilder,
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
/// The size of a tile of the strip under a share, as the design draws it.
pub const STRIP_TILE_SIZE: (f32, f32) = (148.0, 84.0);

#[derive(IntoElement)]
pub struct PersonTile {
    picture: Option<CameraPicture>,
    camera_view: Option<(junto_media::calls::Calls, junto_media::engine::ShareId)>,
    id: ElementId,
    name: SharedString,
    seed: u128,
    speaking: bool,
    marks: Vec<TileMark>,
    label: SharedString,
    selector: Option<SharedString>,
    in_strip: bool,
    focused: bool,
    sidebar: bool,
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
            picture: None,
            camera_view: None,
            id: id.into(),
            name: name.into(),
            seed,
            speaking: false,
            marks: Vec::new(),
            label: label.into(),
            selector: None,
            in_strip: false,
            focused: false,
            sidebar: false,
        }
    }

    pub fn camera_view(
        mut self,
        view: Option<(junto_media::calls::Calls, junto_media::engine::ShareId)>,
    ) -> Self {
        self.camera_view = view;
        self
    }

    pub fn sidebar(mut self, sidebar: bool) -> Self {
        self.sidebar = sidebar;
        self
    }

    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }
    pub fn picture(mut self, picture: Option<CameraPicture>) -> Self {
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
                    .child(Mosaic::new(MosaicKind::Large, self.seed, avatar)),
            )
            .when_some(self.picture, |tile, picture| tile.child(picture.element()))
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

#[derive(Clone)]
pub enum CameraPicture {
    Image(std::sync::Arc<gpui::RenderImage>),
    #[cfg(target_os = "macos")]
    Surface(core_video::pixel_buffer::CVPixelBuffer),
}
impl CameraPicture {
    pub(crate) fn image(&self) -> Option<&std::sync::Arc<gpui::RenderImage>> {
        match self {
            Self::Image(image) => Some(image),
            #[cfg(target_os = "macos")]
            Self::Surface(_) => None,
        }
    }
    fn element(self) -> gpui::AnyElement {
        match self {
            Self::Image(image) => gpui::img(image)
                .absolute()
                .size_full()
                .object_fit(gpui::ObjectFit::Contain)
                .into_any_element(),
            #[cfg(target_os = "macos")]
            Self::Surface(buffer) => gpui::surface(buffer)
                .absolute()
                .size_full()
                .object_fit(gpui::ObjectFit::Contain)
                .into_any_element(),
        }
    }
}
