//! The own menu of the user bar (ADR 0401): the four status choices and the status text of
//! the account on the shown server, and the dialog that sets the status text.

use super::{Layer, Shell};
use crate::state::presence::StatusTextEnd;
use crate::text;
use crate::theme::{theme, u};
use crate::ui::{
    Button, ButtonKind, Dialog, Field, Menu, MenuItem, Notice, PresenceMark, Segmented, StyleExt,
    menu_label, menu_separator,
};
use gpui::{
    AnyElement, Context, Hsla, InteractiveElement, IntoElement, ParentElement, SharedString,
    Styled, Window, div, prelude::FluentBuilder,
};
use junto_client_core::{
    Owner,
    activity::ActivityState,
    protocol::presence::{PresenceChoice, STATUS_TEXT_MAX_CHARS, StatusText},
};

impl Shell {
    pub(crate) fn render_presence(&mut self, owner: Owner, cx: &mut Context<Self>) -> AnyElement {
        let theme = theme(cx);
        let server = owner.entry.server_id;
        let Some(view) = self.state.read(cx).presence_server(server).cloned() else {
            return div().into_any_element();
        };
        let held = view.changing || view.choice.is_none() || !view.connected;
        let mut menu = Menu::new("own-menu", text::presence::OWN_MENU, self.layer_focus())
            .selector("own-menu")
            .on_close(Self::handler(cx, |this, window, cx| {
                this.close_layer(window, cx)
            }));
        for (index, choice) in PresenceChoice::ALL.into_iter().enumerate() {
            let shown = choice.shown(true, false, true);
            let mut item =
                MenuItem::new(("presence-choice", index), text::presence::choice(choice))
                    .swatch(Hsla::from(PresenceMark::color(shown, &theme)))
                    .selector(format!("presence-choice:{index}"))
                    .disabled(held)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.state.update(cx, |state, cx| {
                            state.set_presence_choice(server, choice, cx)
                        });
                        this.close_layer(window, cx);
                    }));
            if view.choice == Some(choice) {
                item = item.end_icon(crate::assets::Icon::Check);
            }
            menu = menu.child(item);
        }
        let mut notes: Vec<(&'static str, SharedString)> = Vec::new();
        if self.state.read(cx).presence_activity() == Some(ActivityState::Unavailable) {
            notes.push((
                "presence-activity-unavailable",
                text::presence::ACTIVITY_UNAVAILABLE.into(),
            ));
        }
        if !view.connected {
            notes.push(("presence-connecting", text::presence::CONNECTING.into()));
        } else if view.changing {
            notes.push(("presence-saving", text::presence::SAVING.into()));
        } else if view.choice.is_none() && view.reading {
            notes.push(("presence-reading", text::presence::CHECKING.into()));
        }
        if let Some(reason) = &view.failed {
            let note = format!("{} {reason}", text::presence::NOT_CHANGED);
            notes.push(("presence-write-failed", note.into()));
        }
        if let Some(reason) = &view.read_failed {
            let note = format!("{} {reason}", text::presence::NOT_READ);
            notes.push(("presence-read-failed", note.into()));
        }
        if let Some(reason) = &view.text_failed {
            let note = format!("{} {reason}", text::presence::TEXT_NOT_CHANGED);
            notes.push(("status-text-failed", note.into()));
        }
        for (selector, note) in notes {
            menu = menu.child(
                menu_label(note, false, cx)
                    .max_w(u(260.0))
                    .debug_selector(move || selector.to_owned()),
            );
        }
        if view.failed.is_some() {
            menu = menu.child(
                MenuItem::new("presence-write-retry", text::TRY_AGAIN)
                    .selector("presence-write-retry")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state
                            .update(cx, |state, cx| state.retry_presence_choice(server, cx));
                    })),
            );
        } else if view.read_failed.is_some() {
            menu = menu.child(
                MenuItem::new("presence-read-retry", text::TRY_AGAIN)
                    .selector("presence-read-retry")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state
                            .update(cx, |state, cx| state.retry_presence(server, cx));
                    })),
            );
        }
        if view.takes_text {
            menu = menu.child(menu_separator(cx));
            let own = view.own_text().cloned();
            if let Some(own) = &own {
                let words = self.state.read(cx).status_text_words(own);
                menu = menu
                    .child(
                        menu_label(words, false, cx)
                            .max_w(u(260.0))
                            .truncate()
                            .debug_selector(|| "own-status-text".to_owned()),
                    )
                    .child(
                        MenuItem::new("status-text-clear", text::presence::CLEAR_STATUS_TEXT)
                            .selector("status-text-clear")
                            .disabled(view.text_changing)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.state.update(cx, |state, cx| {
                                    state.set_status_text(server, None, cx)
                                });
                                this.close_layer(window, cx);
                            })),
                    );
            }
            menu = menu.child(
                MenuItem::new(
                    "status-text-open",
                    if own.is_some() {
                        text::presence::CHANGE_STATUS_TEXT
                    } else {
                        text::presence::SET_STATUS_TEXT
                    },
                )
                .selector("status-text-open")
                .disabled(view.text_changing)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_status_text(owner, own.clone(), window, cx);
                })),
            );
        }
        div()
            .absolute()
            .left(u(self.rail_width + 8.0))
            .bottom(u(theme.metrics.profile_strip_height + 4.0))
            .child(menu)
            .into_any_element()
    }

    /// Opens the own menu of the shown server, as a press on the own avatar does.
    pub fn open_own_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let owner = self.state.read(cx).view().selected.and_then(|server| {
            self.state
                .read(cx)
                .presence_server(server.id)
                .map(crate::state::presence::ServerPresence::owner)
        });
        if let Some(owner) = owner {
            self.open_layer(Layer::Presence(owner), window, cx);
        }
    }

    /// Opens the dialog of the own status text of the shown server.
    pub fn open_status_text_dialog(
        &mut self,
        end: StatusTextEnd,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let owner = self.state.read(cx).view().selected.and_then(|server| {
            self.state
                .read(cx)
                .presence_server(server.id)
                .map(crate::state::presence::ServerPresence::owner)
        });
        if let Some(owner) = owner {
            let own = self
                .state
                .read(cx)
                .status_text(owner.entry.server_id, owner.account_id)
                .cloned();
            self.open_status_text(owner, own, window, cx);
            self.status_text_end = end;
        }
    }

    /// Opens the dialog of the own status text with the text that the server holds.
    pub(crate) fn open_status_text(
        &mut self,
        owner: Owner,
        own: Option<StatusText>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (value, until) = own.map(|own| (own.text, own.until)).unwrap_or_default();
        self.status_text_field
            .update(cx, |field, cx| field.set_value(value, window, cx));
        self.status_text_refusal = None;
        // A saved end stays when the person changes only the text: it is shown as a chosen
        // date and time.
        let (end, date, clock) = match until {
            Some(until) => {
                let (date, clock) = self.state.read(cx).status_text_end_fields(until);
                self.status_text_kept = Some((until, date.clone(), clock.clone()));
                (StatusTextEnd::Custom, date, clock)
            }
            None => {
                self.status_text_kept = None;
                (StatusTextEnd::Never, String::new(), String::new())
            }
        };
        self.status_text_end = end;
        for (field, value) in [
            (self.status_text_date.clone(), date),
            (self.status_text_time.clone(), clock),
        ] {
            field.update(cx, |field, cx| field.set_value(value, window, cx));
        }
        self.layer = Layer::StatusText(owner);
        self.layer_focus().clone().focus(window, cx);
        self.focus_field(&self.status_text_field.clone(), window, cx);
        cx.notify();
    }

    /// Why the typed status text was not sent.
    pub fn status_text_refusal(&self) -> Option<&'static str> {
        self.status_text_refusal
    }

    /// Puts text into the field of the status text, as typing does.
    pub fn type_status_text(&mut self, typed: &str, window: &mut Window, cx: &mut Context<Self>) {
        let typed = typed.to_owned();
        self.status_text_field
            .update(cx, |field, cx| field.set_value(typed, window, cx));
        self.status_text_refusal = None;
        cx.notify();
    }

    /// Puts a date and a time into the fields of a chosen end, as typing does.
    pub fn type_status_text_end(
        &mut self,
        date: &str,
        clock: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for (field, typed) in [
            (self.status_text_date.clone(), date.to_owned()),
            (self.status_text_time.clone(), clock.to_owned()),
        ] {
            field.update(cx, |field, cx| field.set_value(typed, window, cx));
        }
        self.status_text_refusal = None;
        cx.notify();
    }

    /// Sends the typed status text with its end, and closes the dialog.
    pub(crate) fn save_status_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Layer::StatusText(owner) = self.layer else {
            return;
        };
        let typed = self.status_text_field.read(cx).value().trim().to_owned();
        if typed.is_empty() {
            self.status_text_refusal = Some(text::presence::STATUS_TEXT_EMPTY);
            cx.notify();
            return;
        }
        if typed.chars().count() > STATUS_TEXT_MAX_CHARS || typed.chars().any(char::is_control) {
            self.status_text_refusal = Some(text::presence::STATUS_TEXT_TOO_LONG);
            cx.notify();
            return;
        }
        let until = if self.status_text_end == StatusTextEnd::Custom {
            let date = self.status_text_date.read(cx).value().to_string();
            let clock = self.status_text_time.read(cx).value().to_string();
            let kept = self
                .status_text_kept
                .as_ref()
                .filter(|(_, kept_date, kept_clock)| *kept_date == date && *kept_clock == clock)
                .map(|(until, ..)| *until);
            match kept.map_or_else(
                || self.state.read(cx).status_text_custom_until(&date, &clock),
                Ok,
            ) {
                Ok(until) => Some(until),
                Err(reason) => {
                    self.status_text_refusal = Some(reason);
                    cx.notify();
                    return;
                }
            }
        } else {
            self.state.read(cx).status_text_until(self.status_text_end)
        };
        let status = StatusText { text: typed, until };
        let server = owner.entry.server_id;
        self.state.update(cx, |state, cx| {
            state.set_status_text(server, Some(status), cx)
        });
        self.close_layer(window, cx);
    }

    pub(crate) fn render_status_text(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = theme(cx);
        // The ends in two rows, so that every choice keeps its whole word: the near ones,
        // and the later ones with the choice of a date and a time.
        let row = |id: &'static str, ends: &'static [StatusTextEnd], cx: &mut Context<Self>| {
            Segmented::new(
                id,
                text::presence::CLEAR_AFTER,
                ends.iter()
                    .map(|end| SharedString::from(text::presence::end_name(*end))),
            )
            .selected(ends.iter().position(|end| *end == self.status_text_end))
            .selector(id)
            .on_select({
                let select = cx.listener(move |this, index: &usize, _, cx| {
                    if let Some(end) = ends.get(*index).copied() {
                        this.status_text_end = end;
                        cx.notify();
                    }
                });
                move |index, window, cx| select(&index, window, cx)
            })
        };
        let near = row("status-text-end", &StatusTextEnd::NEAR, cx);
        let later = row("status-text-end-later", &StatusTextEnd::LATER, cx);
        let label = |label: &'static str| {
            div()
                .mb(u(4.0))
                .typeface(theme.types.label, &theme)
                .ink(theme.colors.bronze)
                .child(label)
        };
        let custom = (self.status_text_end == StatusTextEnd::Custom).then(|| {
            div()
                .flex()
                .gap(u(8.0))
                .mt(u(8.0))
                .child(
                    div()
                        .w(u(140.0))
                        .child(label(text::presence::END_DATE))
                        .child(
                            Field::new(&self.status_text_date, text::presence::END_DATE)
                                .selector("status-text-date"),
                        ),
                )
                .child(
                    div()
                        .w(u(110.0))
                        .child(label(text::presence::END_TIME))
                        .child(
                            Field::new(&self.status_text_time, text::presence::END_TIME)
                                .selector("status-text-time"),
                        ),
                )
        });
        Dialog::new(text::presence::STATUS_TEXT, self.layer_focus())
            .selector("status-text-dialog")
            .on_close(Self::handler(cx, |this, window, cx| {
                this.close_layer(window, cx)
            }))
            .child(
                Field::new(&self.status_text_field, text::presence::STATUS_TEXT)
                    .selector("status-text-field"),
            )
            .child(
                div()
                    .mt(u(5.0))
                    .typeface(theme.types.small, &theme)
                    .ink(theme.colors.ink_2)
                    .child(text::presence::STATUS_TEXT_HELP),
            )
            .child(div().mt(u(12.0)).child(label(text::presence::CLEAR_AFTER)))
            .child(div().flex().child(near))
            .child(div().flex().mt(u(6.0)).child(later))
            .child(div().children(custom))
            .child(
                crate::ui::notice_slot("status-text-refusal-slot").when_some(
                    self.status_text_refusal,
                    |slot, reason| {
                        slot.child(
                            div()
                                .mt(u(10.0))
                                .child(Notice::error(reason).selector("status-text-refusal")),
                        )
                    },
                ),
            )
            .child(crate::ui::dialog_footer([
                Button::new("status-text-cancel")
                    .label(text::presence::CANCEL)
                    .selector("status-text-cancel")
                    .h(u(30.0))
                    .px(u(12.0))
                    .on_click(cx.listener(|this, _, window, cx| this.close_layer(window, cx)))
                    .into_any_element(),
                Button::new("status-text-save")
                    .kind(ButtonKind::Primary)
                    .label(text::presence::SAVE)
                    .selector("status-text-save")
                    .h(u(30.0))
                    .px(u(12.0))
                    .on_click(cx.listener(|this, _, window, cx| this.save_status_text(window, cx)))
                    .into_any_element(),
            ]))
            .into_any_element()
    }
}
