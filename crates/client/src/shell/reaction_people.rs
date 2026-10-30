use gpui::{
    AnyElement, AnyView, App, AppContext as _, Context, Entity, Hsla, InteractiveElement,
    IntoElement, ParentElement, Render, Role, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Window, div, prelude::FluentBuilder,
};
use junto_client_core::model::PlaceId;
use junto_client_core::protocol::ids::{MessageId, ServerId};
use junto_client_core::protocol::message::ReactionPerson;

use super::{Layer, Shell};
use crate::{
    state::{AppState, reaction_people::Key},
    text,
    theme::{theme, u},
    ui::{Button, Dialog, Notice, ScrollArea, StyleExt, dialog_footer, float_shadow},
};

pub(crate) struct People {
    pub key: Key,
    emojis: Vec<String>,
    scroll: ScrollHandle,
}

pub(super) struct MenuParent {
    pub(super) people: Box<People>,
    focus: Option<gpui::WeakFocusHandle>,
}

struct NamesTooltip {
    state: Entity<AppState>,
    key: Key,
    caption: SharedString,
    _subscription: Subscription,
}

pub(super) fn tooltip(
    state: Entity<AppState>,
    key: Key,
    caption: SharedString,
) -> impl Fn(&mut Window, &mut App) -> AnyView {
    move |_, cx| {
        let state = state.clone();
        let key = key.clone();
        let caption = caption.clone();
        cx.new(|cx| {
            let subscription = cx.observe(&state, |_, _, cx| cx.notify());
            NamesTooltip {
                state,
                key,
                caption,
                _subscription: subscription,
            }
        })
        .into()
    }
}

fn person_name(person: &ReactionPerson) -> String {
    person
        .name
        .clone()
        .unwrap_or_else(|| text::DELETED_REACTOR.to_owned())
}

impl Render for NamesTooltip {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme(cx);
        let colors = theme.colors;
        let allowed = self.state.read(cx).reaction_people_allowed(&self.key);
        let read = self
            .state
            .read(cx)
            .reaction_people_read(&self.key)
            .map(|read| (read.page.clone(), read.failed));
        if allowed && read.is_none() {
            let key = self.key.clone();
            cx.defer_in(window, move |this, _, cx| {
                this.state
                    .update(cx, |state, cx| state.read_reaction_people(key, false, cx))
            });
        }
        let mut body = div()
            .id("reaction-names-tooltip")
            .role(Role::Tooltip)
            .flex()
            .flex_col()
            .gap(u(4.0))
            .mt(u(6.0))
            .max_w(u(340.0))
            .px(u(10.0))
            .pt(u(7.0))
            .pb(u(8.0))
            .bg(Hsla::from(colors.travertine_hi))
            .border_1()
            .border_color(Hsla::from(colors.rule))
            .rounded(u(theme.metrics.radius))
            .shadow(float_shadow(colors.shadow, 6.0, 16.0))
            .typeface(theme.types.small, &theme)
            .ink(colors.ink_2)
            .child(if allowed {
                self.caption.clone()
            } else {
                text::WHO_REACTED.into()
            });
        match read {
            _ if !allowed => body = body.child(text::REACTION_PEOPLE_UNAVAILABLE),
            Some((Some(page), _)) => {
                for person in page.people.iter().take(5) {
                    let id = person.id;
                    body = body.child(
                        div()
                            .debug_selector(move || format!("reaction-hover-person:{id}"))
                            .child(person_name(person)),
                    );
                }
                if page.people.len() > 5 || page.next_after.is_some() {
                    body = body.child(text::REACTION_PEOPLE_MORE);
                }
                if page.people.is_empty() {
                    body = body.child(text::REACTION_PEOPLE_EMPTY);
                }
            }
            Some((_, true)) => body = body.child(text::REACTION_PEOPLE_UNAVAILABLE),
            _ => body = body.child(text::REACTION_PEOPLE_LOADING),
        }
        body
    }
}

impl Shell {
    pub(super) fn retain_reaction_menu(&mut self, window: &Window, cx: &App) {
        if matches!(self.layer, Layer::ReactionPeople(_))
            && let Layer::ReactionPeople(people) = std::mem::replace(&mut self.layer, Layer::None)
        {
            self.reaction_menu = Some(Box::new(MenuParent {
                people,
                focus: window.focused(cx).map(|focus| focus.downgrade()),
            }));
        }
    }

    pub(super) fn close_reaction_menu(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !matches!(self.layer, Layer::AccountMenu(_)) {
            return false;
        }
        let Some(parent) = self.reaction_menu.take() else {
            return false;
        };
        if !self
            .state
            .read(cx)
            .reaction_people_allowed(&parent.people.key)
        {
            return false;
        }
        self.layer = Layer::ReactionPeople(parent.people);
        if let Some(focus) = parent.focus.and_then(|focus| focus.upgrade()) {
            focus.focus(window, cx);
        } else {
            self.layer_focus.focus(window, cx);
        }
        cx.notify();
        true
    }

    pub(super) fn render_reaction_menu_parent(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let mut parent = self.reaction_menu.take()?;
        let menu = std::mem::replace(&mut self.layer, Layer::ReactionPeople(parent.people));
        let shown = self.render_reaction_people(window, cx);
        if let Layer::ReactionPeople(people) = std::mem::replace(&mut self.layer, menu) {
            parent.people = people;
            self.reaction_menu = Some(parent);
        }
        Some(shown)
    }

    #[cfg(feature = "test-support")]
    pub fn show_reaction_people_fixture(
        &mut self,
        target: (ServerId, PlaceId, MessageId),
        emoji: String,
        people: Vec<ReactionPerson>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let account_id = people.first().map(|person| person.id).unwrap_or_else(|| {
            junto_client_core::protocol::ids::AccountId::from_uuid(uuid::Uuid::nil())
        });
        let owner = junto_client_core::Owner {
            entry: junto_client_core::EntryKey {
                server_id: target.0,
                generation: 0,
            },
            account_id,
        };
        let key = Key {
            owner,
            place: target.1,
            message: target.2,
            emoji: emoji.clone(),
            after: None,
            before: None,
        };
        self.state.update(cx, |state, cx| {
            state.show_reaction_people_fixture(
                key.clone(),
                junto_client_core::protocol::message::ReactionPeople {
                    people,
                    next_after: None,
                    previous_before: None,
                },
                cx,
            )
        });
        self.open_layer(
            Layer::ReactionPeople(Box::new(People {
                key,
                emojis: vec![emoji],
                scroll: ScrollHandle::new(),
            })),
            window,
            cx,
        );
    }

    pub(super) fn reactor_key(
        &self,
        server: ServerId,
        place: PlaceId,
        message: MessageId,
        emoji: String,
        cx: &App,
    ) -> Option<Key> {
        Some(Key {
            owner: self.state.read(cx).owner(server)?,
            place,
            message,
            emoji,
            after: None,
            before: None,
        })
    }

    pub(super) fn open_reaction_people(
        &mut self,
        key: Key,
        emojis: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.state.read(cx).reaction_people_allowed(&key) {
            return;
        }
        self.state.update(cx, |state, cx| {
            state.read_reaction_people(key.clone(), false, cx)
        });
        self.open_layer(
            Layer::ReactionPeople(Box::new(People {
                key,
                emojis,
                scroll: ScrollHandle::new(),
            })),
            window,
            cx,
        );
    }

    pub(super) fn render_reaction_people(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Layer::ReactionPeople(dialog) = &self.layer else {
            return div().into_any_element();
        };
        let key = dialog.key.clone();
        let emojis = self
            .state
            .read(cx)
            .reaction_emojis(&key)
            .unwrap_or_else(|| dialog.emojis.clone());
        let scroll = dialog.scroll.clone();
        let faces = self.faces(key.server_id()).for_place(key.place);
        let allowed = self.state.read(cx).reaction_people_allowed(&key);
        let read = self
            .state
            .read(cx)
            .reaction_people_read(&key)
            .map(|read| (read.page.clone(), read.failed));
        if allowed && read.is_none() {
            let key = key.clone();
            cx.defer_in(window, move |this, _, cx| {
                this.state
                    .update(cx, |state, cx| state.read_reaction_people(key, false, cx))
            });
        }
        let theme = theme(cx);
        let failed = !allowed || read.as_ref().is_some_and(|(_, failed)| *failed);
        let wanted = read.as_ref().and_then(|(page, _)| page.as_ref()).map_or(
            theme.types.body.line_height + 20.0,
            |page| {
                if page.people.is_empty() {
                    return theme.types.body.line_height + 20.0;
                }
                page.people
                    .iter()
                    .map(|person| {
                        theme.types.body.line_height
                            + 14.0
                            + if person.departed {
                                theme.types.small.line_height + 2.0
                            } else {
                                0.0
                            }
                    })
                    .sum::<f32>()
            },
        );
        let height = (window.viewport_size().height - u(240.0).to_pixels(window.rem_size()))
            .clamp(
                u(60.0).to_pixels(window.rem_size()),
                u(300.0).to_pixels(window.rem_size()),
            )
            .min(u(wanted).to_pixels(window.rem_size()));
        let mut choices = div().flex().flex_wrap().gap(u(5.0));
        for (index, emoji) in emojis.into_iter().filter(|_| allowed).enumerate() {
            let selected = emoji == key.emoji;
            let mut target = key.clone();
            target.emoji = emoji.clone();
            target.after = None;
            target.before = None;
            let selector = format!("reaction-emoji:{emoji}");
            choices = choices.child(
                div().debug_selector(move || selector.clone()).child(
                    Button::new(("reaction-emoji", index))
                        .label(crate::emoji::Drawings::of(cx).shown(&emoji).into_owned())
                        .selected(selected)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.select_reaction_page(target.clone(), cx)
                        })),
                ),
            );
        }
        let mut list = ScrollArea::new("reaction-people-list", &scroll, text::WHO_REACTED)
            .selector("reaction-people-list");
        let mut previous = None;
        let mut next = None;
        match read {
            _ if !allowed => {}
            Some((Some(page), _)) => {
                previous = page.previous_before;
                next = page.next_after;
                let empty = page.people.is_empty();
                for person in page.people.into_iter().take(50) {
                    let id = person.id;
                    let name = person_name(&person);
                    let name = if person.name.is_some() && !person.departed {
                        let state = self.state.clone();
                        let source = key.clone();
                        let own = cx.entity_id();
                        let target = crate::ui::OpenProfile {
                            server: key.server_id(),
                            account: id,
                        };
                        let presence = faces.presence(id);
                        div()
                            .flex()
                            .items_center()
                            .gap(u(5.0))
                            .child(
                                div()
                                    .debug_selector(move || {
                                        format!("reaction-presence:{id}:{presence:?}")
                                    })
                                    .child(crate::ui::PresenceMark::new(presence, 10.0)),
                            )
                            .child(
                                div()
                                    .debug_selector(move || format!("reaction-profile:{id}"))
                                    .child(
                                        crate::ui::ProfileName::new(
                                            ("reactor", id.as_uuid().as_u128() as u64),
                                            name,
                                            target,
                                        )
                                        .role_color(faces.color(id))
                                        .on_context_menu(super::account_menus::profile_menu(
                                            key.owner,
                                            id,
                                            person_name(&person),
                                            super::account_menus::AccountSource::Reactor {
                                                window: own,
                                                key: key.clone(),
                                                account: id,
                                            },
                                        ))
                                        .on_open(
                                            move |window, cx| {
                                                if state
                                                    .read(cx)
                                                    .reaction_profile_allowed(own, &source, id)
                                                {
                                                    window.dispatch_action(Box::new(target), cx);
                                                }
                                            },
                                        ),
                                    ),
                            )
                            .into_any_element()
                    } else {
                        div().min_w_0().truncate().child(name).into_any_element()
                    };
                    list = list.child(
                        div()
                            .debug_selector(move || format!("reaction-person:{id}"))
                            .typeface(theme.types.body, &theme)
                            .flex()
                            .flex_col()
                            .py(u(7.0))
                            .px(u(10.0))
                            .gap(u(2.0))
                            .child(div().min_w_0().truncate().ink(theme.colors.ink).child(name))
                            .when(person.departed, |row| {
                                row.child(
                                    div()
                                        .typeface(theme.types.small, &theme)
                                        .ink(theme.colors.ink_3)
                                        .child(text::UNAVAILABLE_REACTOR),
                                )
                            }),
                    );
                }
                if empty {
                    list = list.child(div().p(u(10.0)).child(text::REACTION_PEOPLE_EMPTY));
                }
            }
            Some((_, true)) => {}
            _ => list = list.child(div().p(u(10.0)).child(text::REACTION_PEOPLE_LOADING)),
        }
        let mut previous_key = key.clone();
        previous_key.after = None;
        previous_key.before = previous;
        let mut next_key = key.clone();
        next_key.before = None;
        next_key.after = next;
        let mut buttons = vec![
            Button::new("reaction-previous")
                .label(text::REACTION_PREVIOUS)
                .selector("reaction-previous")
                .disabled(previous.is_none() || failed)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.select_reaction_page(previous_key.clone(), cx)
                }))
                .into_any_element(),
            Button::new("reaction-next")
                .label(text::REACTION_NEXT)
                .selector("reaction-next")
                .disabled(next.is_none() || failed)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.select_reaction_page(next_key.clone(), cx)
                }))
                .into_any_element(),
        ];
        if failed && allowed {
            buttons.push(
                Button::new("reaction-retry")
                    .label(text::TRY_AGAIN)
                    .selector("reaction-retry")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state.update(cx, |state, cx| {
                            state.read_reaction_people(key.clone(), true, cx)
                        })
                    }))
                    .into_any_element(),
            );
        }
        Dialog::new(text::WHO_REACTED, self.layer_focus())
            .selector("reaction-people-dialog")
            .on_close(Self::handler(cx, |this, window, cx| {
                this.close_layer(window, cx)
            }))
            .child(choices)
            .when(!failed, |dialog| {
                dialog.child(div().h(height).flex().flex_col().child(list))
            })
            .when(failed, |dialog| {
                dialog.child(
                    Notice::error(text::REACTION_PEOPLE_UNAVAILABLE)
                        .selector("reaction-people-error"),
                )
            })
            .child(dialog_footer(buttons))
            .into_any_element()
    }

    fn select_reaction_page(&mut self, key: Key, cx: &mut Context<Self>) {
        let own = cx.entity_id();
        self.state.update(cx, |state, cx| {
            state.read_reaction_people(key.clone(), false, cx);
            state.watch_reaction_people(own, Some(key.clone()), cx);
        });
        if let Layer::ReactionPeople(dialog) = &mut self.layer {
            dialog.key = key;
            dialog.scroll = ScrollHandle::new();
        }
        cx.notify();
    }
}
