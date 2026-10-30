use gpui::{
    AnyElement, App, Context, Div, InteractiveElement, IntoElement, ParentElement, SharedString,
    Stateful, StatefulInteractiveElement, Styled, Window, div,
};
use junto_client_core::Owner;

use super::{
    Layer, Shell,
    account_menus::{AccountSource, AccountTarget, account_target},
};
use crate::{
    text,
    ui::{RowAction, bind_lazy_action_menu},
};

impl Shell {
    pub(super) fn pending_author_identity(
        &self,
        (owner, pending): (Option<Owner>, u64),
        name: &str,
        part: &'static str,
        child: impl IntoElement,
        cx: &App,
    ) -> AnyElement {
        let selector = format!("pending-author-{part}:{pending}");
        account_target(
            selector.clone().into(),
            owner,
            owner.map(|owner| AccountTarget {
                account_id: owner.account_id,
                name: name.to_owned(),
                departed: false,
            }),
            Some(AccountSource::Pending(pending)),
            div().debug_selector(move || selector).child(child),
            cx,
        )
    }

    fn preset_profile_current(&self, name: &str, cx: &App) -> bool {
        if !matches!(self.layer, Layer::None) || self.detached || name.is_empty() {
            return false;
        }
        let view = self.state.read(cx).view();
        view.person.account_id.is_none() && view.person.name == name
    }

    pub(super) fn bind_preset_profile_context(
        &mut self,
        frame: Stateful<Div>,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        if name.is_empty() {
            return frame;
        }
        let theme = crate::theme::theme(cx);
        let ring =
            crate::ui::focus_ring_inside(theme.colors.terracotta_deep, theme.metrics.focus_ring);
        bind_lazy_action_menu(
            frame
                .focusable()
                .tab_stop(true)
                .focus_visible(move |style| style.shadow(ring)),
            SharedString::from(format!("profile-preset-context:{name}")),
            window,
            cx,
            |_, cx| {
                if !self.preset_profile_current(&name, cx) {
                    return Vec::new();
                }
                let name = name.clone();
                vec![RowAction::new(
                    text::COPY_NAME,
                    cx.listener(move |this, _, _, cx| {
                        if this.preset_profile_current(&name, cx) {
                            this.copy_text(name.clone(), cx);
                        }
                    }),
                )]
            },
        )
    }
}
