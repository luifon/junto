//! The generated avatar: a square of tiles in a pattern that is mirrored on both axes
//! (ADR 0703). The pattern and the colours follow from the identifier of what the avatar
//! stands for, so one account has the same avatar on every device and in every session.

use gpui::{
    App, Bounds, Corners, IntoElement, Pixels, RenderOnce, Styled, Window, canvas, fill, point, px,
    quad, size,
};

use crate::theme::{AvatarColors, theme, u};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MosaicKind {
    /// A person or a server: 5 by 5 tiles in three colours.
    Large,
    /// A workspace: 3 by 3 tiles in two colours.
    Small,
}

impl MosaicKind {
    /// The side of the quadrant that is mirrored.
    const fn quadrant(self) -> usize {
        match self {
            Self::Large => 3,
            Self::Small => 2,
        }
    }

    const fn colours(self) -> u64 {
        match self {
            Self::Large => 3,
            Self::Small => 2,
        }
    }
}

/// The pattern of one avatar.
#[derive(Debug, Clone, PartialEq)]
pub struct Pattern {
    /// The tiles per side.
    pub side: usize,
    /// The colour of each tile, by rows, as an index into the tile colours.
    pub tiles: Vec<u8>,
    /// The index of the colour set.
    pub colours: usize,
}

/// The pattern for an identifier. `sets` is the number of colour sets to choose from.
pub fn pattern(kind: MosaicKind, seed: u128, sets: usize) -> Pattern {
    let mut state = mix(seed);
    let colours = (next(&mut state) % sets.max(1) as u64) as usize;
    let quadrant = kind.quadrant();
    let mut corner: Vec<u8> = (0..quadrant * quadrant)
        .map(|_| (next(&mut state) % kind.colours()) as u8)
        .collect();
    // Every colour of the set is in the pattern: a pattern of one colour is a plain square,
    // and one of two colours of a set of three reads as another set. A missing colour takes
    // a tile of the colour that has the most tiles.
    for colour in 0..kind.colours() as u8 {
        if corner.contains(&colour) {
            continue;
        }
        let count = |wanted: u8| corner.iter().filter(|tile| **tile == wanted).count();
        let most = (0..kind.colours() as u8)
            .max_by_key(|candidate| count(*candidate))
            .unwrap_or(0);
        let places: Vec<usize> = (0..corner.len())
            .filter(|index| corner[*index] == most)
            .collect();
        let place = places[(next(&mut state) % places.len() as u64) as usize];
        corner[place] = colour;
    }
    let side = 2 * quadrant - 1;
    let fold = |index: usize| {
        if index < quadrant {
            index
        } else {
            side - 1 - index
        }
    };
    let tiles = (0..side * side)
        .map(|index| corner[fold(index / side) * quadrant + fold(index % side)])
        .collect();
    Pattern {
        side,
        tiles,
        colours,
    }
}

/// FNV-1a over the bytes of the identifier.
fn mix(seed: u128) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in seed.to_be_bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// The next value of a xorshift sequence.
fn next(state: &mut u64) -> u64 {
    if *state == 0 {
        *state = 0x9e37_79b9_7f4a_7c15;
    }
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state >> 11
}

/// A generated avatar at a size in design pixels.
#[derive(IntoElement)]
pub struct Mosaic {
    kind: MosaicKind,
    seed: u128,
    size: f32,
    radius: f32,
}

impl Mosaic {
    pub fn new(kind: MosaicKind, seed: u128, size: f32) -> Self {
        Self {
            kind,
            seed,
            size,
            radius: 3.0,
        }
    }

    pub fn radius(mut self, radius: f32) -> Self {
        self.radius = radius;
        self
    }
}

impl RenderOnce for Mosaic {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme(cx);
        let sets = match self.kind {
            MosaicKind::Large => theme.avatars,
            MosaicKind::Small => theme.workspace_avatars,
        };
        let pattern = pattern(self.kind, self.seed, sets.len());
        let colours = sets[pattern.colours];
        let radius = self.radius;
        canvas(
            |_, _, _| (),
            move |bounds, (), window, _| {
                let radius = px(radius) * (window.rem_size() / px(16.0));
                paint(&pattern, colours, bounds, radius, window);
            },
        )
        .size(u(self.size))
        .flex_none()
    }
}

fn paint(
    pattern: &Pattern,
    colours: AvatarColors,
    bounds: Bounds<Pixels>,
    radius: Pixels,
    window: &mut Window,
) {
    window.paint_quad(quad(
        bounds,
        Corners::all(radius),
        gpui::Hsla::from(colours.grout),
        px(0.0),
        gpui::transparent_black(),
        gpui::BorderStyle::default(),
    ));
    let side = pattern.side;
    let unit = bounds.size.width / side as f32;
    // The line between two tiles is a tenth of a tile, as in the design, and at least one
    // pixel of the display, so that a small avatar keeps its lines. Every edge lies on a
    // pixel of the display, so the lines have one width.
    let device = window.scale_factor();
    let snap = |length: Pixels| px((f32::from(length) * device).round() / device);
    let line = snap(unit * 0.1).max(px(1.0 / device));
    let edge = |index: usize| snap(unit * index as f32);
    let last = side - 1;
    for row in 0..side {
        for column in 0..side {
            let (left, top) = (edge(column) + line, edge(row) + line);
            let right = if column == last {
                bounds.size.width - line
            } else {
                edge(column + 1)
            };
            let bottom = if row == last {
                bounds.size.height - line
            } else {
                edge(row + 1)
            };
            let tile = Bounds {
                origin: point(bounds.origin.x + left, bounds.origin.y + top),
                size: size(right - left, bottom - top),
            };
            let colour = colours.tiles[pattern.tiles[row * side + column] as usize];
            let outer = (radius - line).max(px(0.0));
            let corner = |at_row: usize, at_column: usize| {
                if row == at_row && column == at_column {
                    outer
                } else {
                    px(0.0)
                }
            };
            let corners = Corners {
                top_left: corner(0, 0),
                top_right: corner(0, last),
                bottom_right: corner(last, last),
                bottom_left: corner(last, 0),
            };
            if corners == Corners::all(px(0.0)) {
                window.paint_quad(fill(tile, gpui::Hsla::from(colour)));
            } else {
                window.paint_quad(quad(
                    tile,
                    corners,
                    gpui::Hsla::from(colour),
                    px(0.0),
                    gpui::transparent_black(),
                    gpui::BorderStyle::default(),
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pattern_is_mirrored_on_both_axes_and_is_the_same_for_the_same_identifier() {
        for seed in [
            0u128,
            1,
            0x0198_f1c2_7d3a_7b1e_9c55_0a1b_2c3d_4e5f,
            u128::MAX,
        ] {
            for kind in [MosaicKind::Large, MosaicKind::Small] {
                let made = pattern(kind, seed, 9);
                assert_eq!(made, pattern(kind, seed, 9));
                let side = made.side;
                assert_eq!(side, 2 * kind.quadrant() - 1);
                assert_eq!(made.tiles.len(), side * side);
                assert!(made.colours < 9);
                for row in 0..side {
                    for column in 0..side {
                        let tile = made.tiles[row * side + column];
                        assert_eq!(tile, made.tiles[row * side + (side - 1 - column)]);
                        assert_eq!(tile, made.tiles[(side - 1 - row) * side + column]);
                        assert!(u64::from(tile) < kind.colours());
                    }
                }
                for colour in 0..kind.colours() as u8 {
                    assert!(made.tiles.contains(&colour), "{made:?}");
                }
            }
        }
    }

    #[test]
    fn identifiers_spread_over_the_colour_sets_and_patterns() {
        let mut sets = std::collections::BTreeSet::new();
        let mut patterns = std::collections::BTreeSet::new();
        for seed in 0..200u128 {
            let made = pattern(MosaicKind::Large, seed << 64 | (seed * 7919), 9);
            sets.insert(made.colours);
            patterns.insert(made.tiles);
        }
        assert_eq!(sets.len(), 9);
        assert!(patterns.len() > 100, "{}", patterns.len());
    }
}
