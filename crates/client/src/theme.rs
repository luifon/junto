//! The one theme of the first version: every colour, font, type size, radius and fixed
//! measure of the interface (ADR 0703, ADR 0731). A second theme is a second value of
//! [`Theme`]; no drawing code names a colour or a font by itself.
//!
//! A measure is given in the pixels of the design at 100 % and becomes rems through [`u`],
//! so it follows the interface scale. A border is one device-independent pixel at every
//! scale.

use std::rc::Rc;

use gpui::{App, FontWeight, Global, Hsla, Pixels, Rems, Rgba, SharedString, px, rems};

/// The size of one rem at an interface scale of 100 %, in pixels.
pub const REM_AT_100: f32 = 16.0;

/// The narrowest and the widest interface scale, in percent (ADR 0703).
pub const SCALE_MIN: u32 = junto_client_core::settings::SCALE_MIN_PERCENT;
pub const SCALE_MAX: u32 = junto_client_core::settings::SCALE_MAX_PERCENT;
/// The scales that the setting offers.
pub const SCALES: [u32; 5] = [90, 100, 110, 125, 150];
pub const SCALE_DEFAULT: u32 = 100;

/// A measure of the design, in its pixels at 100 %, as a length that follows the scale.
pub fn u(design_px: f32) -> Rems {
    rems(design_px / REM_AT_100)
}

/// The rem size of a window at an interface scale.
pub fn rem_size(scale_percent: u32) -> Pixels {
    px(REM_AT_100 * clamp_scale(scale_percent) as f32 / 100.0)
}

pub fn clamp_scale(scale_percent: u32) -> u32 {
    scale_percent.clamp(SCALE_MIN, SCALE_MAX)
}

const fn hex(value: u32) -> Rgba {
    Rgba {
        r: ((value >> 16) & 0xff) as f32 / 255.0,
        g: ((value >> 8) & 0xff) as f32 / 255.0,
        b: (value & 0xff) as f32 / 255.0,
        a: 1.0,
    }
}

const fn hex_alpha(value: u32, alpha: f32) -> Rgba {
    let mut colour = hex(value);
    colour.a = alpha;
    colour
}

/// The colours of the design, by the names of its palette.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Colors {
    // Stone: the light surfaces.
    /// The main surface.
    pub travertine: Rgba,
    /// Raised parts: inputs, buttons, chips.
    pub travertine_hi: Rgba,
    /// Side panels.
    pub plaster: Rgba,
    /// The channel column.
    pub limestone: Rgba,
    /// A selected row and the profile strip.
    pub limestone_lo: Rgba,
    /// A row under the pointer in the channel column.
    pub limestone_hover: Rgba,
    pub rule: Rgba,
    pub rule_soft: Rgba,
    // Ink: text on stone.
    pub ink: Rgba,
    pub ink_2: Rgba,
    pub ink_3: Rgba,
    // Basalt: the dark server book.
    pub basalt: Rgba,
    pub basalt_2: Rgba,
    pub basalt_3: Rgba,
    pub basalt_rule: Rgba,
    pub basalt_deep: Rgba,
    /// The edge of the server book and of an avatar on it.
    pub basalt_edge: Rgba,
    /// The border of a floating layer on basalt.
    pub basalt_float_rule: Rgba,
    // Bone: text on basalt.
    pub bone: Rgba,
    pub bone_2: Rgba,
    pub bone_3: Rgba,
    /// The text of a selected row on basalt.
    pub bone_hi: Rgba,
    /// A label on basalt.
    pub bone_label: Rgba,
    // Accents.
    pub terracotta: Rgba,
    pub terracotta_deep: Rgba,
    pub terracotta_wash: Rgba,
    pub terracotta_wash_rule: Rgba,
    /// Text and icons on terracotta.
    pub on_terracotta: Rgba,
    pub cypress: Rgba,
    pub cypress_wash: Rgba,
    pub cypress_wash_rule: Rgba,
    pub olive: Rgba,
    pub olive_bright: Rgba,
    pub bronze: Rgba,
    pub ochre: Rgba,
    /// Red: recording, leaving a call and errors only.
    pub cinnabar: Rgba,
    pub cinnabar_wash: Rgba,
    pub cinnabar_wash_rule: Rgba,
    /// Text on the cinnabar wash.
    pub cinnabar_ink: Rgba,
    /// An unlit segment of a level meter.
    pub meter_off: Rgba,
    /// A switch that is off.
    pub toggle_off: Rgba,
    /// The ground of the stage of a call.
    pub stage: Rgba,
    /// The plate with the name on the tile of a person, and the edge of the avatar there.
    pub plate: Rgba,
    pub tile_edge: Rgba,
    /// A closed microphone on the plate of a tile.
    pub plate_warning: Rgba,
    /// The buttons of the strip of the person's call, on cypress.
    pub call_button: Rgba,
    pub call_button_rule: Rgba,
    pub call_text: Rgba,
    pub call_text_hi: Rgba,
    pub call_on: Rgba,
    pub call_on_text: Rgba,
    pub call_disabled_rule: Rgba,
    pub call_disabled_text: Rgba,
    /// The edge of the button that leaves a call, and its text.
    pub cinnabar_deep: Rgba,
    pub on_cinnabar: Rgba,
    /// The flat cover behind a dialog and behind an overlay panel.
    pub scrim: Rgba,
    pub scrim_soft: Rgba,
    /// The shadow of a floating layer on stone and on basalt.
    pub shadow: Rgba,
    pub shadow_deep: Rgba,
}

/// One text style of the design.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Type {
    /// In design pixels.
    pub size: f32,
    pub line_height: f32,
    pub weight: FontWeight,
    pub family: Family,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// All text and controls.
    Sans,
    /// Titles of 18 pixels and more.
    Display,
    /// Addresses, codes and timers.
    Mono,
}

const fn text(size: f32, line_height: f32, weight: FontWeight, family: Family) -> Type {
    Type {
        size,
        line_height,
        weight,
        family,
    }
}

/// The text styles of the design. No interface text is smaller than 12 pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Types {
    /// Running text.
    pub body: Type,
    /// Text of a notice and of a wait box.
    pub notice: Type,
    /// The name in a channel row.
    pub row: Type,
    pub row_selected: Type,
    /// A person in a voice channel, a workspace row, a button, a tab.
    pub control: Type,
    pub control_strong: Type,
    /// The name of a person or a server next to an avatar.
    pub name: Type,
    /// Help text, a status line, a secondary line.
    pub small: Type,
    pub small_strong: Type,
    /// A section label: bold, sentence case.
    pub label: Type,
    /// The label above a workspace name.
    pub overline: Type,
    /// A heading inside a card.
    pub heading: Type,
    pub heading_large: Type,
    /// The heading of a state that fills the main area.
    pub state_title: Type,
    /// The workspace name and the channel title.
    pub title: Type,
    /// The title of a panel and of a dialog.
    pub panel_title: Type,
    /// A server address.
    pub mono_small: Type,
    /// The address field of the server book.
    pub mono_field: Type,
    /// A code field.
    pub mono_code: Type,
    /// The recording mark.
    pub mono_mark: Type,
    /// A count or a short state at the end of a row.
    pub tally: Type,
}

/// The fixed measures of the layout, in design pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    pub rail_width: f32,
    pub rail_collapsed_width: f32,
    pub channel_column_width: f32,
    pub side_panel_width: f32,
    pub call_panel_width: f32,
    pub header_height: f32,
    pub side_header_height: f32,
    pub profile_strip_height: f32,
    /// The height of the area of the window controls at the top of the server book.
    pub window_controls_height: f32,
    pub workspace_row_height: f32,
    pub channel_row_height: f32,
    pub occupant_row_height: f32,
    pub button_height: f32,
    pub dialog_width: f32,
    pub menu_width: f32,
    pub fly_width: f32,
    /// Below this width of the window, in design pixels, the server book collapses and the
    /// side panel lies over the content (ADR 0731).
    pub narrow_below: f32,
    pub radius_small: f32,
    pub radius: f32,
    pub radius_menu: f32,
    pub radius_dialog: f32,
    /// The width of the mark of a keyboard focus.
    pub focus_ring: f32,
}

/// The colours of one generated avatar: three tile colours and the colour between the
/// tiles.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AvatarColors {
    pub tiles: [Rgba; 3],
    pub grout: Rgba,
}

const fn avatar(tiles: [u32; 3], grout: u32) -> AvatarColors {
    AvatarColors {
        tiles: [hex(tiles[0]), hex(tiles[1]), hex(tiles[2])],
        grout: hex(grout),
    }
}

/// The colour sets of the generated avatars of people and servers.
const AVATARS: &[AvatarColors] = &[
    avatar([0xEBD9BC, 0xB4532F, 0x2F4B3C], 0xCDB893),
    avatar([0x2F4B3C, 0xDCCAA6, 0xC79A45], 0x213528),
    avatar([0xC79A45, 0x2A221B, 0xF3E8D2], 0xA57D33),
    avatar([0x747A3F, 0xEBD9BC, 0x8E3D20], 0x5A5F30),
    avatar([0x3E6373, 0xE8DCC3, 0xC79A45], 0x2E4C59),
    avatar([0xB4532F, 0xF3E8D2, 0x2A221B], 0x8E3D20),
    avatar([0x6E3044, 0xE3C9A8, 0xC79A45], 0x522232),
    avatar([0xE6D8BD, 0xB4532F, 0x2A221B], 0xBFAE8C),
    avatar([0x3E6373, 0xE6D8BD, 0x1D3340], 0x2E4C59),
];

/// The colour sets of the generated avatars of workspaces. They use the first two tile
/// colours.
const WORKSPACE_AVATARS: &[AvatarColors] = &[
    avatar([0xE6D8BD, 0xB4532F, 0xB4532F], 0x8E3D20),
    avatar([0xC79A45, 0x2A221B, 0x2A221B], 0x8C6B3A),
    avatar([0x747A3F, 0xE6D8BD, 0xE6D8BD], 0x5A5F30),
    avatar([0x3E6373, 0xE6D8BD, 0xE6D8BD], 0x2E4C59),
];

#[derive(Debug, Clone, PartialEq)]
pub struct Fonts {
    pub sans: SharedString,
    pub display: SharedString,
    pub mono: SharedString,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub colors: Colors,
    pub types: Types,
    pub metrics: Metrics,
    pub fonts: Fonts,
    pub avatars: &'static [AvatarColors],
    pub workspace_avatars: &'static [AvatarColors],
}

impl Theme {
    /// The theme of design 0.1: light stone surfaces, a dark server book, and terracotta,
    /// green and ochre accents.
    pub fn stone() -> Self {
        use Family::{Display, Mono, Sans};
        const REGULAR: FontWeight = FontWeight::NORMAL;
        const MEDIUM: FontWeight = FontWeight::MEDIUM;
        const SEMIBOLD: FontWeight = FontWeight::SEMIBOLD;
        const BOLD: FontWeight = FontWeight::BOLD;
        Self {
            colors: Colors {
                travertine: hex(0xF2EADA),
                travertine_hi: hex(0xFAF5EA),
                plaster: hex(0xEBE1CD),
                limestone: hex(0xE3D7C1),
                limestone_lo: hex(0xD6C8AD),
                limestone_hover: hex(0xDACDB4),
                rule: hex(0xC9B999),
                rule_soft: hex(0xDCCFB7),
                ink: hex(0x2A221B),
                ink_2: hex(0x51453A),
                ink_3: hex(0x695B4B),
                basalt: hex(0x24201C),
                basalt_2: hex(0x2E2823),
                basalt_3: hex(0x3A322B),
                basalt_rule: hex(0x4A4037),
                basalt_deep: hex(0x191613),
                basalt_edge: hex(0x110F0D),
                basalt_float_rule: hex(0x5B4F43),
                bone: hex(0xEDE3D0),
                bone_2: hex(0xC2B49D),
                bone_3: hex(0xA39581),
                bone_hi: hex(0xF4EBDA),
                bone_label: hex(0xD2B783),
                terracotta: hex(0xB4532F),
                terracotta_deep: hex(0x8E3D20),
                terracotta_wash: hex(0xEBCDB7),
                terracotta_wash_rule: hex(0xC98A6B),
                on_terracotta: hex(0xFFF4E4),
                cypress: hex(0x2F4B3C),
                cypress_wash: hex(0xD2DAC2),
                cypress_wash_rule: hex(0xB5C2A2),
                olive: hex(0x5E6A2C),
                olive_bright: hex(0xA9BC62),
                bronze: hex(0x6B4F24),
                ochre: hex(0xC79A45),
                cinnabar: hex(0xC5311F),
                cinnabar_wash: hex(0xF0CFC2),
                cinnabar_wash_rule: hex(0xD9A898),
                cinnabar_ink: hex(0x7A2416),
                meter_off: hex(0xC2B291),
                toggle_off: hex(0xA99A80),
                stage: hex(0x1C1815),
                plate: hex(0x161310),
                tile_edge: hex(0x14110F),
                plate_warning: hex(0xEE9A76),
                call_button: hex(0x274034),
                call_button_rule: hex(0x547562),
                call_text: hex(0xE8EEDC),
                call_text_hi: hex(0xF4F6EA),
                call_on: hex(0xE3EAD3),
                call_on_text: hex(0x1F3529),
                call_disabled_rule: hex(0x3C5646),
                call_disabled_text: hex(0x9DB3A2),
                cinnabar_deep: hex(0x8E2114),
                on_cinnabar: hex(0xFFF1E6),
                scrim: hex_alpha(0x191613, 0.5),
                scrim_soft: hex_alpha(0x191613, 0.4),
                shadow: hex_alpha(0x2A221B, 0.3),
                shadow_deep: hex_alpha(0x000000, 0.45),
            },
            types: Types {
                body: text(14.5, 21.0, REGULAR, Sans),
                notice: text(14.0, 20.0, REGULAR, Sans),
                row: text(14.5, 19.0, REGULAR, Sans),
                row_selected: text(14.5, 19.0, SEMIBOLD, Sans),
                control: text(14.0, 19.0, MEDIUM, Sans),
                control_strong: text(14.0, 19.0, BOLD, Sans),
                name: text(14.5, 17.0, BOLD, Sans),
                small: text(12.5, 17.0, REGULAR, Sans),
                small_strong: text(12.5, 17.0, BOLD, Sans),
                label: text(12.5, 16.0, BOLD, Sans),
                overline: text(12.0, 14.0, SEMIBOLD, Sans),
                heading: text(14.5, 20.0, BOLD, Sans),
                heading_large: text(17.0, 22.0, BOLD, Sans),
                state_title: text(22.0, 28.0, BOLD, Sans),
                title: text(20.0, 23.0, REGULAR, Display),
                panel_title: text(18.0, 22.0, REGULAR, Display),
                mono_small: text(12.0, 15.0, MEDIUM, Mono),
                mono_field: text(12.5, 17.0, MEDIUM, Mono),
                mono_code: text(14.0, 20.0, SEMIBOLD, Mono),
                mono_mark: text(12.0, 16.0, BOLD, Mono),
                tally: text(12.5, 17.0, MEDIUM, Sans),
            },
            metrics: Metrics {
                rail_width: 200.0,
                rail_collapsed_width: 64.0,
                channel_column_width: 240.0,
                side_panel_width: 336.0,
                call_panel_width: 328.0,
                header_height: 48.0,
                side_header_height: 44.0,
                profile_strip_height: 52.0,
                window_controls_height: 38.0,
                workspace_row_height: 28.0,
                channel_row_height: 28.0,
                occupant_row_height: 26.0,
                button_height: 28.0,
                dialog_width: 500.0,
                menu_width: 292.0,
                fly_width: 236.0,
                narrow_below: 1200.0,
                radius_small: 2.0,
                radius: 3.0,
                radius_menu: 4.0,
                radius_dialog: 5.0,
                focus_ring: 2.0,
            },
            fonts: Fonts {
                sans: "Atkinson Hyperlegible Next".into(),
                display: "Marcellus".into(),
                mono: "Atkinson Hyperlegible Mono".into(),
            },
            avatars: AVATARS,
            workspace_avatars: WORKSPACE_AVATARS,
        }
    }

    pub fn family(&self, family: Family) -> SharedString {
        match family {
            Family::Sans => self.fonts.sans.clone(),
            Family::Display => self.fonts.display.clone(),
            Family::Mono => self.fonts.mono.clone(),
        }
    }
}

struct ActiveTheme(Rc<Theme>);

impl Global for ActiveTheme {}

/// Makes `theme` the theme of the application.
pub fn install(theme: Theme, cx: &mut App) {
    cx.set_global(ActiveTheme(Rc::new(theme)));
}

/// The theme of the application.
pub fn theme(cx: &App) -> Rc<Theme> {
    cx.global::<ActiveTheme>().0.clone()
}

/// A colour of the theme as the type that the drawing functions take.
pub fn paint(colour: Rgba) -> Hsla {
    colour.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_measure_follows_the_scale_and_the_scale_stays_in_its_range() {
        assert_eq!(u(16.0), rems(1.0));
        assert_eq!(u(8.0), rems(0.5));
        assert_eq!(rem_size(100), px(16.0));
        assert_eq!(rem_size(150), px(24.0));
        assert_eq!(rem_size(90), px(14.4));
        assert_eq!(rem_size(40), rem_size(SCALE_MIN));
        assert_eq!(rem_size(400), rem_size(SCALE_MAX));
        assert!(SCALES.contains(&SCALE_DEFAULT));
        assert!(
            SCALES
                .iter()
                .all(|scale| (SCALE_MIN..=SCALE_MAX).contains(scale))
        );
    }

    #[test]
    fn no_text_style_is_smaller_than_12_pixels_and_body_text_has_at_least_14() {
        let types = Theme::stone().types;
        let all = [
            types.body,
            types.notice,
            types.row,
            types.row_selected,
            types.control,
            types.control_strong,
            types.name,
            types.small,
            types.small_strong,
            types.label,
            types.overline,
            types.heading,
            types.heading_large,
            types.state_title,
            types.title,
            types.panel_title,
            types.mono_small,
            types.mono_field,
            types.mono_code,
            types.mono_mark,
            types.tally,
        ];
        for style in all {
            assert!(style.size >= 12.0, "{style:?}");
            assert!(style.line_height >= style.size, "{style:?}");
        }
        assert!(types.body.size >= 14.0 && types.notice.size >= 14.0);
        assert!(types.title.size >= 18.0 && types.panel_title.size >= 18.0);
    }

    #[test]
    fn a_colour_is_read_from_its_hexadecimal_value() {
        let colors = Theme::stone().colors;
        assert_eq!(colors.travertine, gpui::rgb(0xF2EADA));
        assert_eq!(colors.cinnabar, gpui::rgb(0xC5311F));
        assert_eq!(colors.scrim.a, 0.5);
    }
}
