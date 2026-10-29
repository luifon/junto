use std::{cell::Cell, rc::Rc};

use gpui::{
    AnyElement, App, Bounds, Context, InteractiveElement, IntoElement, ParentElement, Pixels,
    Point, SharedString, StatefulInteractiveElement, Styled, Window, canvas, div,
};
use junto_client_core::{
    Owner,
    protocol::ids::{AccountId, ChannelId, MessageId},
};

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

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum AccountSource {
    Message(MessageId),
    Pending(u64),
    Search(crate::state::search::ResultTarget),
    Notice(uuid::Uuid),
    VoiceOccupant {
        channel_id: ChannelId,
        account_id: AccountId,
    },
}

#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(namespace = accounts, no_json, no_register)]
pub(crate) struct OpenAccountMenu {
    pub owner: Owner,
    pub people: Vec<AccountTarget>,
    pub source: Option<AccountSource>,
    pub position: Point<Pixels>,
}

impl OpenAccountMenu {
    pub(super) fn source_is_present(&self, view: &crate::state::view::ShellView) -> bool {
        use crate::state::{feed::FeedRow, view::PinsState};
        let Some(source) = self.source else {
            return true;
        };
        match source {
            AccountSource::Search(target) => {
                return view
                    .workspace()
                    .and_then(|workspace| workspace.search.as_ref())
                    .is_some_and(|panel| {
                        panel.server_id == self.owner.entry.server_id
                            && panel.results.iter().any(|row| row.target == target)
                    });
            }
            AccountSource::Notice(id) => {
                return view
                    .selected
                    .as_ref()
                    .is_some_and(|server| server.id == self.owner.entry.server_id)
                    && view
                        .lists()
                        .and_then(|lists| lists.notices.as_ref())
                        .is_some_and(|panel| panel.rows.iter().any(|row| row.id == id));
            }
            AccountSource::VoiceOccupant {
                channel_id,
                account_id,
            } => {
                return view
                    .selected
                    .as_ref()
                    .is_some_and(|server| server.id == self.owner.entry.server_id)
                    && view.workspace().is_some_and(|workspace| {
                        workspace
                            .sections
                            .iter()
                            .flat_map(|section| &section.rows)
                            .find(|channel| channel.id == channel_id)
                            .and_then(|channel| channel.voice.as_ref())
                            .is_some_and(|voice| {
                                voice
                                    .occupants
                                    .iter()
                                    .any(|person| person.account_id == account_id)
                            })
                    });
            }
            AccountSource::Message(_) | AccountSource::Pending(_) => {}
        }
        let held = |feed: Option<&crate::state::feed::Feed>| {
            feed.is_some_and(|feed| {
                feed.server_id == self.owner.entry.server_id
                    && feed.rows.iter().any(|row| match (source, row) {
                        (AccountSource::Message(id), FeedRow::Message(row)) => {
                            row.message.id == id && !row.message.deleted
                        }
                        (AccountSource::Pending(id), FeedRow::Waiting(row)) => row.pending.id == id,
                        _ => false,
                    })
            })
        };
        let workspace = view.workspace();
        let thread = workspace.and_then(|workspace| workspace.thread.as_ref());
        if held(view.lists().and_then(|lists| lists.feed.as_ref()))
            || held(thread.and_then(|thread| thread.feed.as_ref()))
        {
            return true;
        }
        let AccountSource::Message(id) = source else {
            return false;
        };
        if view
            .selected
            .as_ref()
            .is_none_or(|server| server.id != self.owner.entry.server_id)
        {
            return false;
        }
        thread.and_then(|thread| thread.root.as_ref()).is_some_and(|row| row.message.id == id && !row.message.deleted)
            || workspace.and_then(|workspace| workspace.pins.as_ref()).is_some_and(|pins| {
                matches!(&pins.state, PinsState::Loaded(rows) if rows.iter().any(|row| row.message.id == id && !row.message.deleted))
            })
    }
}

pub(super) fn account_target(
    id: SharedString,
    owner: Option<Owner>,
    target: Option<AccountTarget>,
    source: Option<AccountSource>,
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
                source,
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
        mut open: OpenAccountMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut accounts = std::collections::HashSet::new();
        open.people
            .retain(|person| accounts.insert(person.account_id));
        if !open.people.is_empty()
            && self
                .view
                .as_ref()
                .is_some_and(|view| open.source_is_present(view))
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
        matches!(&self.layer, Layer::AccountMenu(open) if open.owner == owner && open.people.contains(person)
            && self.view.as_ref().is_some_and(|view| open.source_is_present(view)))
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
