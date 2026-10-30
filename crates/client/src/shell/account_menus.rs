use std::{cell::Cell, rc::Rc};

use gpui::{
    AnyElement, App, Bounds, Context, InteractiveElement, IntoElement, ParentElement, Pixels,
    Point, SharedString, StatefulInteractiveElement, Styled, Window, canvas, div,
    prelude::FluentBuilder,
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
pub(crate) enum CallSurface {
    Tile,
    Share {
        id: junto_media::engine::ShareId,
        watched: bool,
    },
    Member {
        inside: bool,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum AccountSource {
    CallOccupant {
        seat: Box<junto_media::calls::Seat>,
        account_id: AccountId,
        surface: CallSurface,
    },
    ThreadAuthor {
        root: MessageId,
        account: AccountId,
    },
    ReplyAuthor {
        parent: Box<AccountSource>,
        quoted: MessageId,
        account: AccountId,
    },
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
    pub(super) fn source_is_present(
        &self,
        view: &crate::state::view::ShellView,
        state: &crate::state::AppState,
    ) -> bool {
        use crate::state::{feed::FeedRow, view::PinsState};
        let Some(source) = &self.source else {
            return true;
        };
        match source {
            AccountSource::CallOccupant {
                seat,
                account_id,
                surface,
            } => {
                return self.people.len() == 1
                    && self.people[0].account_id == *account_id
                    && seat.target.server_id == self.owner.entry.server_id
                    && state.calls().view().call.is_some_and(|call| {
                        call.phase == junto_media::calls::Phase::Connected
                            && call.seat() == *seat.as_ref()
                    })
                    && view
                        .lists()
                        .and_then(|lists| lists.call.as_ref())
                        .is_some_and(|panel| match surface {
                            CallSurface::Share { id, watched } => {
                                panel.stage.as_ref().is_some_and(|stage| {
                                    stage.shares.iter().any(|share| {
                                        share.id == *id && share.account_id == *account_id
                                    }) && (!watched
                                        || stage.watched().is_some_and(|share| share.id == *id))
                                })
                            }
                            CallSurface::Member { inside: false } => {
                                panel.channel().is_some_and(|channel| {
                                    channel
                                        .outside
                                        .iter()
                                        .any(|person| person.account_id == *account_id)
                                })
                            }
                            _ => panel
                                .people
                                .iter()
                                .any(|person| person.account_id == *account_id),
                        });
            }
            AccountSource::Search(target) => {
                return view
                    .workspace()
                    .and_then(|workspace| workspace.search.as_ref())
                    .or_else(|| view.lists().and_then(|lists| lists.search.as_ref()))
                    .is_some_and(|panel| {
                        panel.server_id == self.owner.entry.server_id
                            && panel.results.iter().any(|row| row.target == *target)
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
                        .is_some_and(|panel| panel.rows.iter().any(|row| row.id == *id));
            }
            AccountSource::ThreadAuthor { root, account } => {
                return view.selected.as_ref().is_some_and(|server| server.id == self.owner.entry.server_id)
                    && view.lists().and_then(|lists| lists.feed.as_ref()).is_some_and(|feed| {
                        feed.rows.iter().any(|row| matches!(row, FeedRow::Message(row)
                            if row.message.id == *root && !row.message.deleted && row.thread.as_ref().is_some_and(|mark| {
                                mark.authors.contains(account) || (mark.authors.is_empty() && mark.last.as_ref().is_some_and(|last| last.author_id == Some(*account)))
                            })))
                    });
            }
            AccountSource::ReplyAuthor {
                parent,
                quoted,
                account,
            } => {
                if view
                    .selected
                    .as_ref()
                    .is_none_or(|server| server.id != self.owner.entry.server_id)
                {
                    return false;
                }
                let quote = |quote: &crate::state::feed::Quote| {
                    quote.message_id == *quoted && quote.author_id == Some(*account)
                };
                let message = |row: &crate::state::feed::MessageRow| {
                    matches!(&**parent, AccountSource::Message(id) if row.message.id == *id)
                        && !row.message.deleted
                        && row.reply.as_ref().is_some_and(quote)
                };
                let held = |feed: &crate::state::feed::Feed| {
                    feed.server_id == self.owner.entry.server_id && feed.rows.iter().any(|row| match row {
                    FeedRow::Message(row) => message(row),
                    FeedRow::Waiting(row) => matches!(&**parent, AccountSource::Pending(id) if row.pending.id == *id) && row.reply.as_ref().is_some_and(quote),
                    _ => false,
                })
                };
                let workspace = view.workspace();
                let thread = workspace.and_then(|workspace| workspace.thread.as_ref());
                return view.lists().and_then(|lists| lists.feed.as_ref()).is_some_and(held)
                    || thread.and_then(|thread| thread.feed.as_ref()).is_some_and(held)
                    || thread.and_then(|thread| thread.root.as_ref()).is_some_and(message)
                    || workspace.and_then(|workspace| workspace.pins.as_ref()).is_some_and(|pins| matches!(&pins.state, PinsState::Loaded(rows) if rows.iter().any(message)));
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
                            .find(|channel| channel.id == *channel_id)
                            .and_then(|channel| channel.voice.as_ref())
                            .is_some_and(|voice| {
                                voice
                                    .occupants
                                    .iter()
                                    .any(|person| person.account_id == *account_id)
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
                            row.message.id == *id && !row.message.deleted
                        }
                        (AccountSource::Pending(id), FeedRow::Waiting(row)) => {
                            row.pending.id == *id
                        }
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
        thread.and_then(|thread| thread.root.as_ref()).is_some_and(|row| row.message.id == *id && !row.message.deleted)
            || workspace.and_then(|workspace| workspace.pins.as_ref()).is_some_and(|pins| {
                matches!(&pins.state, PinsState::Loaded(rows) if rows.iter().any(|row| row.message.id == *id && !row.message.deleted))
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
    let label = target.name.clone();
    bind_account_frame(
        div()
            .id(id)
            .relative()
            .min_w_0()
            .role(gpui::Role::Group)
            .aria_label(label)
            .child(child),
        OpenAccountMenu {
            owner,
            people: vec![target],
            source,
            position: Point::default(),
        },
        true,
        cx,
    )
    .into_any_element()
}

fn bind_account_frame(
    frame: gpui::Stateful<gpui::Div>,
    context: OpenAccountMenu,
    activate: bool,
    cx: &App,
) -> gpui::Stateful<gpui::Div> {
    let bounds = Rc::new(Cell::new(Bounds::<Pixels>::default()));
    let placed = bounds.clone();
    let open = Rc::new(move |window: &mut Window, cx: &mut App| {
        let position = if window.last_input_was_keyboard() {
            let bounds = bounds.get();
            gpui::point(bounds.left(), bounds.bottom())
        } else {
            window.mouse_position()
        };
        let mut open = context.clone();
        open.position = position;
        window.dispatch_action(Box::new(open), cx);
    });
    let by_key = open.clone();
    let theme = crate::theme::theme(cx);
    let ring = crate::ui::focus_ring_inside(theme.colors.terracotta_deep, theme.metrics.focus_ring);
    frame
        .focusable()
        .tab_stop(true)
        .focus_visible(move |style| style.shadow(ring))
        .on_context_menu(move |window, cx| open(window, cx))
        .when(activate, |frame| {
            frame.on_key_down(move |event, window, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    cx.stop_propagation();
                    by_key(window, cx);
                }
            })
        })
        .child(
            canvas(move |value, _, _| placed.set(value), |_, (), _, _| {})
                .absolute()
                .inset_0(),
        )
}

impl Shell {
    pub(super) fn call_menu_trigger(
        &self,
        person: &crate::state::view::Occupant,
        cx: &App,
    ) -> Option<impl Fn(Point<Pixels>, &mut Window, &mut App) + 'static> {
        let open = self.call_account_menu(
            person.account_id,
            super::channels::occupant_name(person).to_string(),
            CallSurface::Tile,
            cx,
        )?;
        Some(move |position, window: &mut Window, cx: &mut App| {
            let mut open = open.clone();
            open.position = position;
            window.dispatch_action(Box::new(open), cx);
        })
    }

    fn call_account_menu(
        &self,
        account_id: AccountId,
        name: String,
        surface: CallSurface,
        cx: &App,
    ) -> Option<OpenAccountMenu> {
        let state = self.state.read(cx);
        let call = state.shown_calls().call?;
        let owner = state.server(call.target.server_id)?.owner()?;
        Some(OpenAccountMenu {
            owner,
            people: vec![AccountTarget {
                account_id,
                name,
                departed: false,
            }],
            source: Some(AccountSource::CallOccupant {
                seat: Box::new(call.seat()),
                account_id,
                surface,
            }),
            position: Point::default(),
        })
    }

    pub(super) fn call_share_frame(
        &self,
        frame: gpui::Stateful<gpui::Div>,
        share: &crate::state::view::ShareCard,
        watched: bool,
        cx: &App,
    ) -> gpui::Stateful<gpui::Div> {
        let Some(open) = self.call_account_menu(
            share.account_id,
            share
                .name
                .clone()
                .unwrap_or_else(|| text::UNKNOWN_PERSON.to_owned()),
            CallSurface::Share {
                id: share.id,
                watched,
            },
            cx,
        ) else {
            return frame;
        };
        let theme = crate::theme::theme(cx);
        let ring = crate::ui::focus_ring(theme.colors.ochre, theme.metrics.focus_ring);
        bind_account_frame(frame, open, watched, cx).focus_visible(move |style| style.shadow(ring))
    }

    pub(super) fn call_member_target(
        &self,
        account: AccountId,
        name: String,
        inside: bool,
        child: impl IntoElement,
        cx: &App,
    ) -> AnyElement {
        let Some(open) = self.call_account_menu(account, name, CallSurface::Member { inside }, cx)
        else {
            return child.into_any_element();
        };
        account_target(
            format!("call-member-target:{account}").into(),
            Some(open.owner),
            open.people.into_iter().next(),
            open.source,
            child,
            cx,
        )
    }

    pub(super) fn account_source_visible(
        &self,
        open: &OpenAccountMenu,
        view: &crate::state::view::ShellView,
        cx: &App,
    ) -> bool {
        let Some(AccountSource::CallOccupant {
            account_id,
            surface,
            ..
        }) = &open.source
        else {
            return true;
        };
        let Some(lists) = view.lists() else {
            return false;
        };
        let Some(panel) = &lists.call else {
            return false;
        };
        match surface {
            CallSurface::Share { .. } => self.detached || !self.state.read(cx).is_call_detached(),
            CallSurface::Tile => {
                (self.detached || !self.state.read(cx).is_call_detached())
                    && (!panel.hide_without_video
                        || self
                            .people_with_video(&panel.people, cx)
                            .contains(account_id))
            }
            CallSurface::Member { .. } => {
                self.call_tab == super::call::CallTab::Members
                    && lists.notices.is_none()
                    && self.call_panel_shown(self.call_panel_overlay, cx)
            }
        }
    }

    pub(super) fn drop_hidden_account_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let hidden = match &self.layer {
            Layer::AccountMenu(open) | Layer::CallMoveMenu(open) => self
                .view
                .as_ref()
                .is_some_and(|view| !self.account_source_visible(open, view, cx)),
            _ => false,
        };
        if hidden {
            self.close_layer(window, cx);
        }
    }

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
            && self.view.as_ref().is_some_and(|view| {
                open.source_is_present(view, self.state.read(cx))
                    && self.account_source_visible(&open, view, cx)
            })
            && self
                .state
                .read(cx)
                .server(open.owner.entry.server_id)
                .is_some_and(|server| server.owner() == Some(open.owner))
        {
            self.open_layer(Layer::AccountMenu(Box::new(open.clone())), window, cx);
            self.start_call_menu_volume(&open, cx);
        }
    }

    pub(super) fn account_menu_current(
        &self,
        owner: Owner,
        person: &AccountTarget,
        cx: &App,
    ) -> bool {
        matches!(&self.layer, Layer::AccountMenu(open) if open.owner == owner && open.people.contains(person)
            && self.view.as_ref().is_some_and(|view| open.source_is_present(view, self.state.read(cx)) && self.account_source_visible(open, view, cx)))
            && self
                .state
                .read(cx)
                .server(owner.entry.server_id)
                .is_some_and(|server| server.owner() == Some(owner))
    }

    pub(super) fn render_account_menu(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Layer::AccountMenu(open) = &self.layer else {
            return div().into_any_element();
        };
        let open = open.as_ref().clone();
        let mut items: Vec<_> = self
            .call_volume_row(&open, window, cx)
            .into_iter()
            .collect();
        items.extend(self.share_call_controls(&open, cx));
        items.extend(self.own_call_controls(&open, cx));
        items.extend(self.participant_controls(&open, cx));
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
            .popup_at_stable(open.position, self.account_menu_placement.clone())
    }
}
