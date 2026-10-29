use crate::{text, theme::theme};
use gpui::{
    App, ElementId, InteractiveElement, IntoElement, ParentElement, RenderOnce, Role, SharedString,
    StatefulInteractiveElement, Styled, Window, div,
};
use junto_client_core::protocol::ids::{AccountId, ServerId};

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
        }
    }
}
impl RenderOnce for ProfileAccess {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme(cx);
        let focus = super::reveal::handle(&self.id, window, cx);
        let target = self.target;
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
                window.dispatch_action(Box::new(target), cx);
                cx.stop_propagation();
            })
            .on_key_down(move |event: &gpui::KeyDownEvent, window, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    window.dispatch_action(Box::new(target), cx);
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
        }
    }
}
impl RenderOnce for ProfileName {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
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
            .child(ProfileAccess::new(
                self.id,
                self.target,
                text::profile::open_for(&self.name),
                div()
                    .min_w_0()
                    .truncate()
                    .child(super::paint_text(self.name, None)),
            ))
    }
}
