//! The small repeated shapes of the design: the tessera band, the mark of a selected row
//! and the segments of a level meter.

use gpui::{
    App, Bounds, Hsla, IntoElement, ParentElement, Pixels, RenderOnce, Rgba, Styled, Window,
    canvas, div, fill, point, px, size,
};

use crate::theme::{theme, u};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BandKind {
    /// Terracotta, ochre and cypress, under the header of the channel column.
    Tessera,
    /// One dim colour, between two servers of the server book.
    Rail,
    /// Terracotta and ochre, at the top of the open tab of a panel.
    Tab,
}

/// A line of tiles, 4 design pixels high, that fills the width of its place.
#[derive(IntoElement)]
pub struct Band {
    kind: BandKind,
}

impl Band {
    pub const fn new(kind: BandKind) -> Self {
        Self { kind }
    }
}

/// The width of a tile and of the gap after it, in design pixels.
const TILE: f32 = 4.0;
const GAP: f32 = 2.0;

impl RenderOnce for Band {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = theme(cx).colors;
        let tiles: Vec<Rgba> = match self.kind {
            BandKind::Tessera => vec![colors.terracotta, colors.ochre, colors.cypress],
            BandKind::Rail => vec![colors.basalt_rule],
            BandKind::Tab => vec![colors.terracotta, colors.ochre],
        };
        canvas(
            |_, _, _| (),
            move |bounds, (), window, _| {
                let scale = window.rem_size() / px(16.0);
                let (tile, step) = (px(TILE) * scale, px(TILE + GAP) * scale);
                let mut x = bounds.origin.x;
                let mut index = 0;
                while x < bounds.right() {
                    let width = tile.min(bounds.right() - x);
                    window.paint_quad(fill(
                        Bounds {
                            origin: point(x, bounds.origin.y),
                            size: size(width, bounds.size.height),
                        },
                        Hsla::from(tiles[index % tiles.len()]),
                    ));
                    x += step;
                    index += 1;
                }
            },
        )
        .h(u(TILE))
        .w_full()
        .flex_none()
    }
}

/// The mark at the left edge of a selected row: three tiles, 4 by 16 design pixels. The
/// row is positioned relative.
pub fn selection_mark(cx: &App) -> impl IntoElement + use<> {
    let colors = theme(cx).colors;
    let tiles = [colors.terracotta, colors.ochre, colors.terracotta];
    div()
        .absolute()
        .left_0()
        .top_0()
        .bottom_0()
        .w(u(4.0))
        .flex()
        .items_center()
        .child(
            canvas(
                |_, _, _| (),
                move |bounds: Bounds<Pixels>, (), window, _| {
                    let scale = window.rem_size() / px(16.0);
                    let (tile, step) = (px(TILE) * scale, px(TILE + GAP) * scale);
                    for (index, colour) in tiles.into_iter().enumerate() {
                        window.paint_quad(fill(
                            Bounds {
                                origin: point(
                                    bounds.origin.x,
                                    bounds.origin.y + step * index as f32,
                                ),
                                size: size(bounds.size.width, tile),
                            },
                            Hsla::from(colour),
                        ));
                    }
                },
            )
            .w(u(4.0))
            .h(u(16.0)),
        )
}

/// The form of a level meter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MeterKind {
    /// Five segments on stone, beside a person.
    Person,
    /// Five small segments on the dark plate of a tile.
    Plate,
    /// Twenty wide segments: the input level of the sound settings.
    Wide,
}

/// A level meter.
#[derive(IntoElement)]
pub struct Meter {
    lit: usize,
    kind: MeterKind,
}

impl Meter {
    pub const SEGMENTS: usize = 5;
    pub const WIDE_SEGMENTS: usize = 20;

    /// A meter with `lit` of its five segments lit.
    pub fn new(lit: usize) -> Self {
        Self {
            lit: lit.min(Self::SEGMENTS),
            kind: MeterKind::Person,
        }
    }

    /// The meter on the plate of a tile.
    pub fn plate(lit: usize) -> Self {
        Self {
            lit: lit.min(Self::SEGMENTS),
            kind: MeterKind::Plate,
        }
    }

    /// The input meter with `lit` of its twenty segments lit.
    pub fn wide(lit: usize) -> Self {
        Self {
            lit: lit.min(Self::WIDE_SEGMENTS),
            kind: MeterKind::Wide,
        }
    }
}

impl RenderOnce for Meter {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = theme(cx).colors;
        let (segments, width, height, on, off) = match self.kind {
            MeterKind::Person => (Self::SEGMENTS, 4.0, 11.0, colors.olive, colors.meter_off),
            MeterKind::Plate => (
                Self::SEGMENTS,
                3.0,
                9.0,
                colors.olive_bright,
                colors.basalt_rule,
            ),
            MeterKind::Wide => (
                Self::WIDE_SEGMENTS,
                9.0,
                8.0,
                colors.olive,
                colors.meter_off,
            ),
        };
        div()
            .flex()
            .flex_none()
            .gap(u(2.0))
            .children((0..segments).map(|segment| {
                let colour = if segment < self.lit { on } else { off };
                div()
                    .w(u(width))
                    .h(u(height))
                    .rounded(u(1.0))
                    .bg(Hsla::from(colour))
            }))
    }
}
