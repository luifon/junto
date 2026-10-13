//! The avatar of a person (ADR 0783): the image that the account set, when the window
//! holds it, and the generated avatar otherwise. Both have the same size and the same
//! corners, so a row does not move when an image arrives.

use std::sync::Arc;

use gpui::{
    App, IntoElement, ObjectFit, RenderImage, RenderOnce, Styled, StyledImage, Window, img,
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
}

impl RenderOnce for Avatar {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        match self.image {
            // The image fills the square: its middle is shown, with its proportions.
            Some(image) => img(image)
                .size(u(self.size))
                .flex_none()
                .rounded(u(self.radius))
                .object_fit(ObjectFit::Cover)
                .into_any_element(),
            None => Mosaic::new(self.kind, self.seed, self.size)
                .radius(self.radius)
                .into_any_element(),
        }
    }
}
