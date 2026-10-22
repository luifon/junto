//! Small marks that state a count or a state: badge, dot, chip, flag, the recording mark
//! and the status line of a server.

use gpui::{
    App, Hsla, IntoElement, ParentElement, RenderOnce, SharedString, Styled, Window, div,
    prelude::FluentBuilder,
};

use super::{StyleExt, icon};
use crate::assets::Icon;
use crate::theme::{theme, u};

/// A count on terracotta, such as the number of mentions of a channel.
#[derive(IntoElement)]
pub struct Badge {
    text: SharedString,
}

impl Badge {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self { text: text.into() }
    }
}

impl RenderOnce for Badge {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme(cx);
        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .h(u(18.0))
            .min_w(u(28.0))
            .px(u(5.0))
            .rounded(u(theme.metrics.radius))
            .bg(Hsla::from(theme.colors.terracotta))
            .typeface(theme.types.small_strong, &theme)
            .line_height(u(18.0))
            .ink(theme.colors.on_terracotta)
            .child(self.text)
    }
}

/// The sign of something new without a count.
pub fn unread_dot(cx: &App) -> impl IntoElement + use<> {
    div()
        .flex_none()
        .size(u(8.0))
        .rounded_full()
        .bg(Hsla::from(theme(cx).colors.terracotta))
}

/// A short fact in a frame, such as "Server found".
#[derive(IntoElement)]
pub struct Chip {
    text: SharedString,
    icon: Option<Icon>,
}

impl Chip {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            icon: None,
        }
    }

    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }
}

impl RenderOnce for Chip {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme(cx);
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(u(5.0))
            .h(u(20.0))
            .px(u(6.0))
            .rounded(u(theme.metrics.radius_small))
            .border_1()
            .border_color(Hsla::from(theme.colors.rule_soft))
            .bg(Hsla::from(theme.colors.plaster))
            .typeface(theme.types.small_strong, &theme)
            .ink(theme.colors.ink_2)
            .whitespace_nowrap()
            .when_some(self.icon, |chip, shown| {
                chip.child(icon(shown, 12.0, theme.colors.ink_2))
            })
            .child(self.text)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlagKind {
    /// A warning that passes: ochre.
    Warning,
    /// An error: the cinnabar wash.
    Error,
}

/// A state of the place that a person must notice.
#[derive(IntoElement)]
pub struct Flag {
    kind: FlagKind,
    text: SharedString,
    icon: Option<Icon>,
    shrinks: bool,
}

impl Flag {
    pub fn new(kind: FlagKind, text: impl Into<SharedString>) -> Self {
        Self {
            kind,
            text: text.into(),
            icon: None,
            shrinks: false,
        }
    }

    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// The flag gets narrower than its text where the place is too narrow, and its text
    /// ends with an ellipsis.
    pub fn shrinks(mut self) -> Self {
        self.shrinks = true;
        self
    }
}

impl RenderOnce for Flag {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme(cx);
        let colors = theme.colors;
        let flag = div()
            .flex()
            .map(|flag| {
                if self.shrinks {
                    flag.min_w_0().overflow_hidden()
                } else {
                    flag.flex_none()
                }
            })
            .items_center()
            .gap(u(6.0))
            .h(u(28.0))
            .px(u(9.0))
            .rounded(u(theme.metrics.radius))
            .typeface(theme.types.control_strong, &theme)
            .text_size(u(13.5))
            .whitespace_nowrap();
        let text = match self.kind {
            FlagKind::Warning => colors.ink,
            FlagKind::Error => colors.cinnabar_ink,
        };
        match self.kind {
            FlagKind::Warning => flag.bg(Hsla::from(colors.ochre)),
            FlagKind::Error => flag
                .bg(Hsla::from(colors.cinnabar_wash))
                .border_1()
                .border_color(Hsla::from(colors.cinnabar_wash_rule)),
        }
        .ink(text)
        .when_some(self.icon, |flag, shown| {
            flag.child(div().flex_none().child(icon(shown, 15.0, text)))
        })
        .map(|flag| {
            if self.shrinks {
                flag.child(div().min_w_0().truncate().child(self.text))
            } else {
                flag.child(self.text)
            }
        })
    }
}

/// The mark of a call that is recorded: a red dot and "REC".
#[derive(IntoElement)]
pub struct RecordingMark {
    text: SharedString,
}

impl RecordingMark {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self { text: text.into() }
    }
}

impl RenderOnce for RecordingMark {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme(cx);
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(u(4.0))
            .typeface(theme.types.mono_mark, &theme)
            .ink(theme.colors.cinnabar)
            .child(
                div()
                    .flex_none()
                    .size(u(8.0))
                    .rounded_full()
                    .bg(Hsla::from(theme.colors.cinnabar)),
            )
            .child(self.text)
    }
}

/// The line under the name of a server in the server book that states what the server is
/// to this device now.
#[derive(IntoElement)]
pub struct StatusLine {
    text: SharedString,
    icon: Option<Icon>,
}

impl StatusLine {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            icon: None,
        }
    }

    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }
}

impl RenderOnce for StatusLine {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme(cx);
        div()
            .flex()
            .items_center()
            .gap(u(5.0))
            .typeface(theme.types.small, &theme)
            .line_height(u(15.0))
            .font_weight(gpui::FontWeight::MEDIUM)
            .ink(theme.colors.bone_2)
            .whitespace_nowrap()
            .when_some(self.icon, |line, shown| {
                line.child(icon(shown, 12.0, theme.colors.bone_2))
            })
            .child(div().min_w_0().truncate().child(self.text))
    }
}
