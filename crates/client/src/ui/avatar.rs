//! The avatar of a person (ADR 0783): the image that the account set, when the window
//! holds it, and the generated avatar otherwise. Both have the same size and the same
//! corners, so a row does not move when an image arrives.

use std::sync::Arc;

use gpui::{
    App, ElementId, InteractiveElement, IntoElement, ObjectFit, ParentElement, RenderImage,
    RenderOnce, Role, SharedString, StatefulInteractiveElement, Styled, StyledImage, Window, div,
    img,
};

use super::{Mosaic, MosaicKind};
use crate::theme::u;

#[derive(IntoElement)]
pub struct Avatar {
    seed: u128,
    size: f32,
    radius: f32,
    kind: MosaicKind,
    image: Option<Arc<RenderImage>>,
    described: Option<(ElementId, SharedString)>,
}

impl Avatar {
    /// The avatar for an identifier, at a size in design pixels.
    pub fn new(seed: u128, size: f32) -> Self {
        Self {
            seed,
            size,
            radius: 3.0,
            kind: MosaicKind::Large,
            image: None,
            described: None,
        }
    }

    pub fn radius(mut self, radius: f32) -> Self {
        self.radius = radius;
        self
    }

    /// The generated avatar has 3 by 3 tiles, as the row of a direct conversation draws
    /// it at 16 pixels.
    pub fn small_pattern(mut self) -> Self {
        self.kind = MosaicKind::Small;
        self
    }

    /// The image of the avatar of the account, when the window holds it.
    pub fn image(mut self, image: Option<Arc<RenderImage>>) -> Self {
        self.image = image;
        self
    }

    /// The readable alternative of the picture that the account set (ADR 0702): the
    /// avatar is an image with this label in the accessibility tree. A generated avatar
    /// of an account without a picture carries no information and gets none.
    pub fn described(mut self, id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        self.described = Some((id.into(), label.into()));
        self
    }
}

impl RenderOnce for Avatar {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let seed = self.seed;
        let picture = match self.image {
            // The image fills the square: its middle is shown, with its proportions. A test
            // finds the drawn image of an account by the seed of its avatar.
            Some(image) => img(image)
                .debug_selector(move || format!("avatar-image:{seed}"))
                .size(u(self.size))
                .flex_none()
                .rounded(u(self.radius))
                .object_fit(ObjectFit::Cover)
                .into_any_element(),
            None => Mosaic::new(self.kind, self.seed, self.size)
                .radius(self.radius)
                .into_any_element(),
        };
        match self.described {
            Some((id, label)) => div()
                .id(id)
                .role(Role::Image)
                .aria_label(label)
                .flex_none()
                .child(picture)
                .into_any_element(),
            None => picture,
        }
    }
}
