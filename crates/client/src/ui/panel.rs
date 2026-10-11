//! The parts of a panel: its header, a section label, a notice, the box of a wait, and the
//! statement that fills the main area when there is nothing else to show.

use gpui::{
    AnyElement, App, Hsla, InteractiveElement, IntoElement, ParentElement, RenderOnce, Role,
    SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder,
};

use super::{StyleExt, icon};
use crate::assets::Icon;
use crate::theme::{theme, u};

/// The header of a side panel and of a dialog: an icon, a title, a short note and a slot
/// at the right for the close button.
#[derive(IntoElement)]
pub struct SideHeader {
    title: SharedString,
    icon: Option<Icon>,
    note: Option<SharedString>,
    trailing: Option<AnyElement>,
}

impl SideHeader {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            icon: None,
            note: None,
            trailing: None,
        }
    }

    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn note(mut self, note: impl Into<SharedString>) -> Self {
        self.note = Some(note.into());
        self
    }

    pub fn trailing(mut self, element: impl IntoElement) -> Self {
        self.trailing = Some(element.into_any_element());
        self
    }
}

impl RenderOnce for SideHeader {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme(cx);
        let colors = theme.colors;
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(u(8.0))
            .h(u(theme.metrics.side_header_height))
            .pl(u(14.0))
            .pr(u(10.0))
            .border_b_1()
            .border_color(Hsla::from(colors.rule))
            .when_some(self.icon, |header, shown| {
                header.child(icon(shown, 16.0, colors.terracotta_deep))
            })
            .child(
                div()
                    .id("panel-title")
                    .role(Role::Heading)
                    .aria_level(2)
                    .aria_label(self.title.clone())
                    .min_w_0()
                    .truncate()
                    .typeface(theme.types.panel_title, &theme)
                    .ink(colors.ink)
                    .child(self.title),
            )
            .when_some(self.note, |header, note| {
                header.child(
                    div()
                        .flex_shrink(1.0)
                        .min_w_0()
                        .truncate()
                        .typeface(theme.types.small, &theme)
                        .text_size(u(13.0))
                        .ink(colors.ink_2)
                        .child(note),
                )
            })
            .child(div().flex_1())
            .children(self.trailing)
    }
}

/// The label of a section of a panel, with a line and an optional count.
#[derive(IntoElement)]
pub struct SectionLabel {
    text: SharedString,
    count: Option<usize>,
}

impl SectionLabel {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            count: None,
        }
    }

    pub fn count(mut self, count: usize) -> Self {
        self.count = Some(count);
        self
    }
}

impl RenderOnce for SectionLabel {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme(cx);
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(u(7.0))
            .pt(u(12.0))
            .pb(u(5.0))
            .pl(u(14.0))
            .pr(u(12.0))
            .typeface(theme.types.label, &theme)
            .ink(theme.colors.bronze)
            .child(self.text)
            .child(
                div()
                    .flex_1()
                    .h(gpui::px(1.0))
                    .bg(Hsla::from(theme.colors.rule)),
            )
            .when_some(self.count, |label, count| {
                label.child(
                    div()
                        .typeface(theme.types.small_strong, &theme)
                        .ink(theme.colors.ink_2)
                        .child(count.to_string()),
                )
            })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NoticeKind {
    /// A fact that the person should read: the ochre edge.
    #[default]
    Information,
    /// A failure: the cinnabar wash.
    Error,
}

/// A statement in a frame, with an optional action at its end.
#[derive(IntoElement)]
pub struct Notice {
    kind: NoticeKind,
    text: SharedString,
    icon: Option<Icon>,
    action: Option<AnyElement>,
    selector: Option<&'static str>,
}

impl Notice {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self {
            kind: NoticeKind::Information,
            text: text.into(),
            icon: None,
            action: None,
            selector: None,
        }
    }

    pub fn error(text: impl Into<SharedString>) -> Self {
        Self {
            kind: NoticeKind::Error,
            text: text.into(),
            icon: Some(Icon::Warning),
            action: None,
            selector: None,
        }
    }

    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn action(mut self, element: impl IntoElement) -> Self {
        self.action = Some(element.into_any_element());
        self
    }

    pub fn selector(mut self, selector: &'static str) -> Self {
        self.selector = Some(selector);
        self
    }
}

impl RenderOnce for Notice {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme(cx);
        let colors = theme.colors;
        let (background, border, edge, text, mark) = match self.kind {
            NoticeKind::Information => (
                colors.plaster,
                colors.rule_soft,
                colors.ochre,
                colors.ink,
                colors.bronze,
            ),
            NoticeKind::Error => (
                colors.cinnabar_wash,
                colors.cinnabar_wash_rule,
                colors.cinnabar,
                colors.cinnabar_ink,
                colors.cinnabar_ink,
            ),
        };
        div()
            .id(self.selector.unwrap_or("notice"))
            .role(match self.kind {
                NoticeKind::Information => Role::Note,
                NoticeKind::Error => Role::Alert,
            })
            .aria_label(self.text.clone())
            .when_some(self.selector, |notice, selector| {
                notice.debug_selector(|| selector.to_owned())
            })
            .flex()
            .flex_none()
            .items_start()
            .gap(u(8.0))
            .py(u(8.0))
            .pr(u(10.0))
            .pl(u(11.0))
            .rounded(u(theme.metrics.radius))
            .border_1()
            .border_l_0()
            .border_color(Hsla::from(border))
            .bg(Hsla::from(background))
            .typeface(theme.types.notice, &theme)
            .ink(text)
            // The left edge has its own colour and takes the place of the border.
            .relative()
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top(gpui::px(-1.0))
                    .bottom(gpui::px(-1.0))
                    .w(u(3.0))
                    .rounded_l(u(theme.metrics.radius))
                    .bg(Hsla::from(edge)),
            )
            .when_some(self.icon, |notice, shown| {
                notice.child(div().mt(u(2.0)).child(icon(shown, 15.0, mark)))
            })
            .child(div().flex_1().min_w_0().child(self.text))
            .children(self.action)
    }
}

/// The box of something that waits for another person.
#[derive(IntoElement)]
pub struct WaitBox {
    title: SharedString,
    text: SharedString,
    detail: Option<SharedString>,
}

impl WaitBox {
    pub fn new(title: impl Into<SharedString>, text: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            text: text.into(),
            detail: None,
        }
    }

    /// A line in fixed-width type under the text, such as a code.
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

impl RenderOnce for WaitBox {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme(cx);
        let colors = theme.colors;
        div()
            .flex()
            .flex_none()
            .gap(u(10.0))
            .pt(u(10.0))
            .px(u(12.0))
            .pb(u(12.0))
            .rounded(u(theme.metrics.radius))
            .border_1()
            .border_color(Hsla::from(colors.rule))
            .bg(Hsla::from(colors.travertine_hi))
            .child(
                div()
                    .mt(u(2.0))
                    .child(icon(Icon::Clock, 18.0, colors.bronze)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .typeface(theme.types.heading, &theme)
                            .text_size(u(15.0))
                            .line_height(u(21.0))
                            .ink(colors.ink)
                            .child(self.title),
                    )
                    .child(
                        div()
                            .mt(u(3.0))
                            .typeface(theme.types.notice, &theme)
                            .ink(colors.ink_2)
                            .child(self.text),
                    )
                    .when_some(self.detail, |column, detail| {
                        column.child(
                            div()
                                .mt(u(6.0))
                                .typeface(theme.types.mono_code, &theme)
                                .ink(colors.ink)
                                .child(detail),
                        )
                    }),
            )
    }
}

/// The statement that fills the main area when a server or a channel has no content to
/// show: a heading with a mark, a sentence, and the actions that the state accepts.
#[derive(IntoElement)]
pub struct StatePanel {
    title: SharedString,
    text: SharedString,
    icon: Option<Icon>,
    error: Option<SharedString>,
    actions: Vec<AnyElement>,
    selector: Option<&'static str>,
}

impl StatePanel {
    pub fn new(title: impl Into<SharedString>, text: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            text: text.into(),
            icon: None,
            error: None,
            actions: Vec::new(),
            selector: None,
        }
    }

    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Why the last action of the state failed.
    pub fn error(mut self, error: Option<SharedString>) -> Self {
        self.error = error;
        self
    }

    pub fn action(mut self, element: impl IntoElement) -> Self {
        self.actions.push(element.into_any_element());
        self
    }

    pub fn selector(mut self, selector: &'static str) -> Self {
        self.selector = Some(selector);
        self
    }
}

impl RenderOnce for StatePanel {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme(cx);
        let colors = theme.colors;
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .px(u(24.0))
            .pb(u(48.0))
            .when_some(self.selector, |panel, selector| {
                panel.debug_selector(|| selector.to_owned())
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .max_w(u(520.0))
                    .child(
                        div()
                            .id("state-title")
                            .role(Role::Heading)
                            .aria_level(2)
                            .aria_label(self.title.clone())
                            .flex()
                            .items_center()
                            .gap(u(10.0))
                            .typeface(theme.types.state_title, &theme)
                            .ink(colors.ink)
                            .text_center()
                            .when_some(self.icon, |title, shown| {
                                title.child(icon(shown, 22.0, colors.bronze))
                            })
                            .child(self.title),
                    )
                    .child(
                        div()
                            .mt(u(8.0))
                            .typeface(theme.types.notice, &theme)
                            .ink(colors.ink_2)
                            .text_center()
                            .child(self.text),
                    )
                    .when_some(self.error, |column, error| {
                        column.child(div().mt(u(12.0)).child(Notice::error(error)))
                    })
                    .when(!self.actions.is_empty(), |column| {
                        column.child(
                            div()
                                .mt(u(14.0))
                                .flex()
                                .flex_wrap()
                                .justify_center()
                                .gap(u(8.0))
                                .children(self.actions),
                        )
                    }),
            )
    }
}
