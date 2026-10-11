//! The button. One definition draws every kind of the design.

use gpui::{
    App, ClickEvent, ElementId, Hsla, InteractiveElement, IntoElement, ParentElement, Refineable,
    RenderOnce, Rgba, Role, SharedString, StatefulInteractiveElement, StyleRefinement, Styled,
    Window, div, prelude::FluentBuilder,
};

use super::{StyleExt, focus_ring, icon, ledge, tooltip};
use crate::assets::Icon;
use crate::theme::{Theme, theme, u};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ButtonKind {
    /// A raised button on stone.
    #[default]
    Default,
    /// The one main action of a place: terracotta.
    Primary,
    /// A small button on the server book.
    Rail,
    /// The terracotta button of the server book that adds a server.
    RailAccent,
    /// A square button of the profile strip.
    Square,
    /// An icon without a frame, such as the close button of a panel.
    Ghost,
}

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    kind: ButtonKind,
    label: Option<SharedString>,
    icon: Option<Icon>,
    disabled: bool,
    tooltip: Option<SharedString>,
    accessible_label: Option<SharedString>,
    selector: Option<&'static str>,
    style: StyleRefinement,
    on_click: Option<ClickHandler>,
}

impl Button {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            kind: ButtonKind::Default,
            label: None,
            icon: None,
            disabled: false,
            tooltip: None,
            accessible_label: None,
            selector: None,
            style: StyleRefinement::default(),
            on_click: None,
        }
    }

    pub fn kind(mut self, kind: ButtonKind) -> Self {
        self.kind = kind;
        self
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// A disabled button takes no click and no keyboard focus.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// The text that appears while the pointer rests on the button. A button without a
    /// label uses it as its accessible label too.
    pub fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.tooltip = Some(text.into());
        self
    }

    pub fn accessible_label(mut self, label: impl Into<SharedString>) -> Self {
        self.accessible_label = Some(label.into());
        self
    }

    /// The name under which a test finds the button.
    pub fn selector(mut self, selector: &'static str) -> Self {
        self.selector = Some(selector);
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl Styled for Button {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

/// The colours of one kind of button in one state.
struct Look {
    background: Rgba,
    border: Option<Rgba>,
    text: Rgba,
    icon: Rgba,
    ledge: Option<Rgba>,
    hover: Rgba,
    pressed: Rgba,
    ring: Rgba,
}

fn look(kind: ButtonKind, disabled: bool, theme: &Theme) -> Look {
    let c = &theme.colors;
    match (kind, disabled) {
        (ButtonKind::Default, false) => Look {
            background: c.travertine_hi,
            border: Some(c.rule),
            text: c.ink,
            icon: c.ink_2,
            ledge: Some(c.rule),
            hover: c.travertine,
            pressed: c.limestone_lo,
            ring: c.terracotta_deep,
        },
        (ButtonKind::Default | ButtonKind::Primary, true) => Look {
            background: c.plaster,
            border: Some(c.rule),
            text: c.ink_3,
            icon: c.ink_3,
            ledge: None,
            hover: c.plaster,
            pressed: c.plaster,
            ring: c.terracotta_deep,
        },
        (ButtonKind::Primary, false) => Look {
            background: c.terracotta,
            border: Some(c.terracotta_deep),
            text: c.on_terracotta,
            icon: c.on_terracotta,
            ledge: Some(c.terracotta_deep),
            hover: c.terracotta_deep,
            pressed: c.terracotta_deep,
            ring: c.ink,
        },
        (ButtonKind::Rail, _) => Look {
            background: c.basalt_2,
            border: Some(c.basalt_rule),
            text: c.bone_2,
            icon: c.bone_2,
            ledge: None,
            hover: c.basalt_3,
            pressed: c.basalt_rule,
            ring: c.ochre,
        },
        (ButtonKind::RailAccent, false) => Look {
            background: c.terracotta,
            border: None,
            text: c.on_terracotta,
            icon: c.on_terracotta,
            ledge: None,
            hover: c.terracotta_deep,
            pressed: c.terracotta_deep,
            ring: c.ochre,
        },
        (ButtonKind::RailAccent, true) => Look {
            background: c.basalt_3,
            border: None,
            text: c.bone_3,
            icon: c.bone_3,
            ledge: None,
            hover: c.basalt_3,
            pressed: c.basalt_3,
            ring: c.ochre,
        },
        (ButtonKind::Square, false) => Look {
            background: c.limestone,
            border: Some(c.rule),
            text: c.ink_2,
            icon: c.ink_2,
            ledge: Some(c.rule),
            hover: c.travertine,
            pressed: c.limestone_lo,
            ring: c.terracotta_deep,
        },
        (ButtonKind::Square, true) => Look {
            background: c.limestone_lo,
            border: Some(c.rule),
            text: c.ink_3,
            icon: c.ink_3,
            ledge: None,
            hover: c.limestone_lo,
            pressed: c.limestone_lo,
            ring: c.terracotta_deep,
        },
        (ButtonKind::Ghost, _) => Look {
            background: gpui::Rgba::default(),
            border: None,
            text: c.ink_2,
            icon: c.ink_2,
            ledge: None,
            hover: c.limestone,
            pressed: c.limestone_lo,
            ring: c.terracotta_deep,
        },
    }
}

impl RenderOnce for Button {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme(cx);
        let metrics = theme.metrics;
        let look = look(self.kind, self.disabled, &theme);
        let label = self
            .accessible_label
            .clone()
            .or_else(|| self.label.clone())
            .or_else(|| self.tooltip.clone());
        let icon_size = match self.kind {
            ButtonKind::Ghost | ButtonKind::RailAccent | ButtonKind::Square => 16.0,
            ButtonKind::Default | ButtonKind::Primary | ButtonKind::Rail => 14.0,
        };
        let type_style = match self.kind {
            ButtonKind::Primary => theme.types.control_strong,
            _ => theme.types.control,
        };

        let mut frame = div()
            .id(self.id)
            .role(Role::Button)
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .gap(u(6.0))
            .rounded(u(metrics.radius))
            .bg(Hsla::from(look.background))
            .typeface(type_style, &theme)
            .ink(look.text)
            .whitespace_nowrap();
        frame = match self.kind {
            ButtonKind::Default | ButtonKind::Primary => {
                frame.h(u(metrics.button_height)).px(u(9.0))
            }
            ButtonKind::Rail => frame.w(u(26.0)).h(u(24.0)),
            ButtonKind::RailAccent => frame.w(u(30.0)).h(u(30.0)),
            ButtonKind::Square => frame.w(u(30.0)).h(u(28.0)),
            ButtonKind::Ghost => frame.w(u(26.0)).h(u(26.0)),
        };
        if let Some(border) = look.border {
            frame = frame.border_1().border_color(Hsla::from(border));
        }
        if let Some(colour) = look.ledge {
            frame = frame.shadow(ledge(colour));
        }
        frame.style().refine(&self.style);

        let ring = focus_ring(look.ring, metrics.focus_ring);
        frame
            .when_some(label, |frame, label| frame.aria_label(label))
            .when_some(self.selector, |frame, selector| {
                frame.debug_selector(|| selector.to_owned())
            })
            .when_some(self.tooltip, |frame, text| frame.tooltip(tooltip(text)))
            .when(self.disabled, |frame| frame.cursor_not_allowed())
            .when(!self.disabled, |frame| {
                let (hover, pressed) = (look.hover, look.pressed);
                frame
                    .cursor_pointer()
                    .focusable()
                    .tab_stop(true)
                    .hover(move |style| style.bg(Hsla::from(hover)))
                    .active(move |style| style.bg(Hsla::from(pressed)))
                    .focus_visible(move |style| style.shadow(ring))
                    .when_some(self.on_click, |frame, handler| {
                        frame.on_click(move |event, window, cx| handler(event, window, cx))
                    })
            })
            .when_some(self.icon, |frame, shown| {
                frame.child(icon(shown, icon_size, look.icon))
            })
            .when_some(self.label, |frame, label| frame.child(label))
    }
}
