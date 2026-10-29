use std::{cell::Cell, rc::Rc};

use gpui::{
    AnyElement, App, Bounds, Context, InteractiveElement, IntoElement, ParentElement, Pixels,
    Point, SharedString, StatefulInteractiveElement, Styled, Window, canvas, div,
};
use junto_client_core::{Owner, protocol::ids::AccountId};

use super::{Layer, Shell};
use crate::{
    text,
    ui::{ContextMenuExt, Menu, MenuItem, menu_label},
};

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AccountTarget {
    pub account_id: AccountId,
    pub name: String,
    pub departed: bool,
}

#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(namespace = accounts, no_json, no_register)]
pub(crate) struct OpenAccountMenu {
    pub owner: Owner,
    pub people: Vec<AccountTarget>,
    pub position: Point<Pixels>,
}

pub(super) fn account_target(
    id: SharedString,
    owner: Option<Owner>,
    target: Option<AccountTarget>,
    child: impl IntoElement,
    cx: &App,
) -> AnyElement {
    let (Some(owner), Some(target)) = (owner, target) else {
        return child.into_any_element();
    };
    let bounds = Rc::new(Cell::new(Bounds::<Pixels>::default()));
    let placed = bounds.clone();
    let label = target.name.clone();
    let open = Rc::new(move |window: &mut Window, cx: &mut App| {
        let position = if window.last_input_was_keyboard() {
            let bounds = bounds.get();
            gpui::point(bounds.left(), bounds.bottom())
        } else {
            window.mouse_position()
        };
        window.dispatch_action(
            Box::new(OpenAccountMenu {
                owner,
                people: vec![target.clone()],
                position,
            }),
            cx,
        );
    });
    let by_key = open.clone();
    let theme = crate::theme::theme(cx);
    let ring = crate::ui::focus_ring_inside(theme.colors.terracotta_deep, theme.metrics.focus_ring);
    div()
        .id(id)
        .relative()
        .min_w_0()
        .role(gpui::Role::Group)
        .aria_label(label)
        .focusable()
        .tab_stop(true)
        .focus_visible(move |style| style.shadow(ring))
        .on_context_menu(move |window, cx| open(window, cx))
        .on_key_down(move |event, window, cx| {
            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                cx.stop_propagation();
                by_key(window, cx);
            }
        })
        .child(child)
        .child(
            canvas(move |value, _, _| placed.set(value), |_, (), _, _| {})
                .absolute()
                .inset_0(),
        )
        .into_any_element()
}

impl Shell {
    pub(super) fn open_account_menu(
        &mut self,
        open: OpenAccountMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !open.people.is_empty()
            && self
                .state
                .read(cx)
                .server(open.owner.entry.server_id)
                .is_some_and(|server| server.owner() == Some(open.owner))
        {
            self.open_layer(Layer::AccountMenu(Box::new(open)), window, cx);
        }
    }

    fn account_menu_current(&self, owner: Owner, person: &AccountTarget, cx: &App) -> bool {
        matches!(&self.layer, Layer::AccountMenu(open) if open.owner == owner && open.people.contains(person))
            && self
                .state
                .read(cx)
                .server(owner.entry.server_id)
                .is_some_and(|server| server.owner() == Some(owner))
    }

    pub(super) fn render_account_menu(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Layer::AccountMenu(open) = &self.layer else {
            return div().into_any_element();
        };
        let open = open.as_ref().clone();
        let mut items = Vec::new();
        for (index, person) in open.people.iter().enumerate() {
            let account = person.account_id;
            let owner = open.owner;
            items.push(menu_label(text::label(&person.name), false, cx).into_any_element());
            for (id, label, value) in [
                ("account-copy-name", text::COPY_NAME, person.name.clone()),
                (
                    "account-copy-id",
                    text::COPY_ACCOUNT_ID,
                    account.to_string(),
                ),
            ] {
                let person = person.clone();
                let selector = format!("{id}:{account}");
                items.push(
                    div()
                        .debug_selector(move || selector)
                        .child(MenuItem::new((id, index), label).on_click(cx.listener(
                            move |this, _, window, cx| {
                                if this.account_menu_current(owner, &person, cx) {
                                    this.close_layer(window, cx);
                                    this.copy_text(value.clone(), cx);
                                }
                            },
                        )))
                        .into_any_element(),
                );
            }
            if account != owner.account_id && !person.departed {
                let person = person.clone();
                items.push(
                    div()
                        .debug_selector(move || format!("account-message:{account}"))
                        .child(
                            MenuItem::new(
                                ("account-message", index),
                                text::message_person(&person.name),
                            )
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    if this.account_menu_current(owner, &person, cx) {
                                        this.close_layer(window, cx);
                                        this.state.update(cx, |state, cx| {
                                            state.open_conversation(
                                                owner.entry.server_id,
                                                account,
                                                cx,
                                            )
                                        });
                                    }
                                },
                            )),
                        )
                        .into_any_element(),
                );
            }
        }
        let close = Self::handler(cx, |this, window, cx| this.close_layer(window, cx));
        Menu::new("account-menu", text::ACCOUNT_ACTIONS, self.layer_focus())
            .selector("account-menu")
            .on_close(move |window, cx| close(window, cx))
            .children(items)
            .popup_at(open.position)
    }
}
