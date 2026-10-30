use crate::{text, theme::theme};
use gpui::prelude::FluentBuilder;
use gpui::{
    App, ElementId, InteractiveElement, IntoElement, ParentElement, RenderOnce, Role, SharedString,
    StatefulInteractiveElement, Styled, Window, div,
};
use junto_client_core::protocol::ids::{AccountId, ServerId};
use std::rc::Rc;

type OpenHandler = Rc<dyn Fn(&mut Window, &mut App)>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, gpui::Action)]
#[action(namespace = junto, no_json)]
pub struct OpenProfile {
    pub server: ServerId,
    pub account: AccountId,
}

#[derive(IntoElement)]
pub struct ProfileAccess {
    id: ElementId,
    target: OpenProfile,
    label: SharedString,
    child: gpui::AnyElement,
    on_open: Option<OpenHandler>,
}
impl ProfileAccess {
    pub fn new(
        id: impl Into<ElementId>,
        target: OpenProfile,
        label: impl Into<SharedString>,
        child: impl IntoElement,
    ) -> Self {
        let id = id.into();
        let id = SharedString::from(format!(
            "profile-{}-{}-{id:?}",
            target.server, target.account
        ))
        .into();
        Self {
            id,
            target,
            label: label.into(),
            child: child.into_any_element(),
            on_open: None,
        }
    }
}
impl RenderOnce for ProfileAccess {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme(cx);
        let focus = super::reveal::handle(&self.id, window, cx);
        let target = self.target;
        let open = self.on_open.unwrap_or_else(|| {
            Rc::new(move |window, cx| window.dispatch_action(Box::new(target), cx))
        });
        let click = open.clone();
        let ring = super::focus_ring_inside(theme.colors.terracotta_deep, theme.metrics.focus_ring);
        div()
            .id(self.id)
            .key_context("ProfileAccount")
            .role(Role::Button)
            .aria_label(self.label)
            .track_focus(&focus)
            .flex()
            .flex_none()
            .min_w_0()
            .max_w_full()
            .cursor_pointer()
            .children(super::reveal::report(&focus, window))
            .focus_visible(move |style| style.shadow(ring))
            .on_click(move |_, window, cx| {
                click(window, cx);
                cx.stop_propagation();
            })
            .on_key_down(move |event: &gpui::KeyDownEvent, window, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    open(window, cx);
                    cx.stop_propagation();
                }
            })
            .child(self.child)
    }
}

#[derive(IntoElement)]
pub struct ProfileName {
    id: ElementId,
    name: SharedString,
    target: OpenProfile,
    color: Option<gpui::Rgba>,
    role_color: Option<junto_client_core::protocol::appearance::RoleColor>,
    on_open: Option<OpenHandler>,
}
impl ProfileName {
    pub fn new(
        id: impl Into<ElementId>,
        name: impl Into<SharedString>,
        target: OpenProfile,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            target,
            color: None,
            role_color: None,
            on_open: None,
        }
    }
    pub fn role_color(
        mut self,
        color: Option<junto_client_core::protocol::appearance::RoleColor>,
    ) -> Self {
        self.role_color = color;
        self
    }
    pub fn color(mut self, color: Option<gpui::Rgba>) -> Self {
        self.color = color;
        self
    }
    pub fn on_open(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_open = Some(Rc::new(handler));
        self
    }
}
impl RenderOnce for ProfileName {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let color = self.color.or_else(|| {
            self.role_color
                .map(|color| crate::theme::role_name_ink(color, &theme(cx)))
        });
        let mut access = ProfileAccess::new(
            self.id,
            self.target,
            text::profile::open_for(&self.name),
            div()
                .min_w_0()
                .truncate()
                .when_some(color, |name, color| name.text_color(color))
                .child(super::paint_text(self.name, None)),
        );
        access.on_open = self.on_open;
        div()
            .debug_selector(move || {
                format!(
                    "profile-name:{}:{}",
                    self.target.server, self.target.account
                )
            })
            .flex()
            .min_w_0()
            .max_w_full()
            .child(access)
    }
}

pub(crate) fn role_name(
    name: impl Into<SharedString>,
    color: Option<junto_client_core::protocol::appearance::RoleColor>,
    theme: &crate::theme::Theme,
) -> gpui::Div {
    div()
        .min_w_0()
        .truncate()
        .when_some(color, |name, color| {
            name.text_color(crate::theme::role_name_ink(color, theme))
        })
        .child(super::paint_text(name.into(), None))
}
