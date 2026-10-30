//! The dialog that starts a direct conversation (ADR 0307): a field for a part of a name,
//! and the accounts that the server lists for it, also those that are in no member list
//! that the person can open.

use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, Styled, Window, div,
};
use junto_client_core::protocol::ids::ServerId;

use super::{Layer, Shell};
use crate::assets::Icon;
use crate::text;
use crate::text::conversations as words;
use crate::theme::{theme, u};
use crate::ui::{Button, ChannelRow, Dialog, Field, Notice, ScrollArea, StyleExt, dialog_footer};

/// The height of the list of the accounts, in design pixels. It does not change with the
/// answers, so that the controls under it stay where they are (ADR 0730).
const LIST_HEIGHT: f32 = 300.0;

impl Shell {
    /// Opens the dialog for the selected server, with an empty field.
    pub(crate) fn open_new_direct(
        &mut self,
        server_id: ServerId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.groups.reset();
        self.contact_part
            .update(cx, |field, cx| field.set_value("", window, cx));
        self.state
            .update(cx, |state, cx| state.open_contacts(server_id, cx));
        self.open_layer(Layer::NewDirect, window, cx);
        self.contact_part
            .update(cx, |input, cx| input.focus(window, cx));
    }

    /// Puts text into the field of the dialog, as typing does.
    pub fn type_contact_part(&mut self, part: &str, window: &mut Window, cx: &mut Context<Self>) {
        let part = part.to_owned();
        self.contact_part
            .update(cx, |field, cx| field.set_value(part, window, cx));
        // A value that is set is not told as a change of the field.
        self.contact_part_changed(cx);
    }

    pub(crate) fn contact_part_changed(&mut self, cx: &mut Context<Self>) {
        let part = self.contact_part.read(cx).value().to_string();
        self.state
            .update(cx, |state, cx| state.type_contacts(&part, cx));
    }

    pub(crate) fn render_new_direct(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let Some(view) = self.state.read(cx).contacts_view() else {
            return div().into_any_element();
        };
        let faces = self.shown_faces();
        let status: Option<AnyElement> = match (&view.failed, &view.rows, view.reading) {
            (Some(reason), _, _) => Some(
                Notice::error(format!("{} {reason}", text::CONTACTS_NOT_READ))
                    .selector("contacts-failed")
                    .action(
                        Button::new("contacts-retry")
                            .label(text::TRY_AGAIN)
                            .selector("contacts-retry")
                            .h(u(28.0))
                            .px(u(10.0))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.update(cx, |state, cx| state.retry_contacts(cx));
                            })),
                    )
                    .into_any_element(),
            ),
            (None, _, true) => Some(
                div()
                    .typeface(theme.types.small, &theme)
                    .ink(theme.colors.ink_2)
                    .debug_selector(|| "contacts-reading".to_owned())
                    .child(text::account::READING)
                    .into_any_element(),
            ),
            (None, Some(rows), false) if rows.is_empty() => Some(
                div()
                    .typeface(theme.types.small, &theme)
                    .ink(theme.colors.ink_2)
                    .debug_selector(|| "contacts-none".to_owned())
                    .child(text::NO_CONTACTS)
                    .into_any_element(),
            ),
            _ => None,
        };
        let mut list = ScrollArea::new("contacts-list", &self.contacts_scroll, text::CONTACTS)
            .selector("contacts-list");
        if self.groups.mode.is_none() {
            let hidden = self.state.read(cx).hidden_conversations(view.server_id);
            let owner = self
                .state
                .read(cx)
                .server(view.server_id)
                .and_then(|server| server.owner());
            if !hidden.is_empty() {
                list = list.child(div().flex_none().child(words::HIDDEN));
                for row in hidden {
                    let id = row.id;
                    let server_id = view.server_id;
                    list = list.child(
                        div()
                            .flex_none()
                            .debug_selector(move || format!("hidden-direct:{id}"))
                            .child(
                                ChannelRow::new(
                                    ("hidden", id.as_uuid().as_u128() as u64),
                                    Icon::Chat,
                                    row.name.unwrap_or_else(|| text::UNKNOWN_PERSON.to_owned()),
                                )
                                .on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        if owner.is_none()
                                            || this
                                                .state
                                                .read(cx)
                                                .server(server_id)
                                                .and_then(|server| server.owner())
                                                != owner
                                            || !matches!(this.layer, Layer::NewDirect)
                                            || this.groups.mode.is_some()
                                        {
                                            return;
                                        }
                                        this.close_layer(window, cx);
                                        this.state.update(cx, |state, cx| {
                                            state.select_conversation(server_id, id, cx)
                                        });
                                    },
                                )),
                            ),
                    );
                }
            }
        }
        for row in view.rows.iter().flatten() {
            let account_id = row.account_id;
            if !self.group_candidate(account_id, cx) {
                continue;
            }
            // The element is named by the account, so that a press that starts on one
            // account and ends on another after an answer chooses nobody.
            let group_mode = self.groups.mode.is_some();
            let selected = self.groups.selected.contains_key(&account_id);
            let name = row.name.clone();
            let choice = ChannelRow::new(
                ("contact", account_id.as_uuid().as_u128() as u64),
                Icon::Person,
                row.name.clone(),
            )
            .selected(selected)
            .avatar(account_id.as_uuid().as_u128())
            .presence(faces.presence(account_id))
            .name_color(faces.color(account_id))
            .profile(crate::ui::OpenProfile {
                server: view.server_id,
                account: account_id,
            })
            .picture(faces.image(account_id))
            .on_click(cx.listener(move |this, _, window, cx| {
                if group_mode {
                    this.toggle_group_person(account_id, &name, cx);
                } else {
                    this.state
                        .update(cx, |state, cx| state.choose_contact(account_id, cx));
                    this.close_layer(window, cx);
                }
            }));
            let identity = div()
                .id(("contact-identity", account_id.as_uuid().as_u128() as u64))
                .flex_none()
                .debug_selector(move || format!("contact-{account_id}"))
                .child(choice);
            let identity = if let Some(owner) = self
                .state
                .read(cx)
                .server(view.server_id)
                .and_then(|server| server.owner())
            {
                let open = self.identity_context(
                    owner,
                    super::account_menus::AccountTarget {
                        account_id,
                        name: row.name.clone(),
                        departed: false,
                    },
                    super::account_menus::AccountSource::Contacts,
                );
                self.bind_identity_context(identity, "contacts", open, window, cx)
            } else {
                identity
            };
            list = list.child(identity);
        }
        // The state of the read follows the rows, so an answer moves no row (ADR 0730).
        let list = list.children(status.map(|status| div().flex_none().pt(u(6.0)).child(status)));
        // "Show more" stands under the list, at a place of its own in every state: the rows
        // of its answer join the list after the rows before, and nothing moves under the
        // pointer (ADR 0730).
        let more = div().flex_none().mt(u(6.0)).child(
            Button::new("contacts-more")
                .label(text::MORE_CONTACTS)
                .selector("contacts-more")
                .disabled(!view.more || view.reading)
                .h(u(28.0))
                .px(u(10.0))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.state.update(cx, |state, cx| state.more_contacts(cx));
                })),
        );
        let mut footer = vec![
            Button::new("cancel-new-direct")
                .label(text::CANCEL)
                .selector("cancel-new-direct")
                .on_click(cx.listener(|this, _, window, cx| this.close_layer(window, cx)))
                .into_any_element(),
        ];
        if self.groups.mode.is_some() {
            footer.push(
                Button::new("group-save")
                    .label(
                        if matches!(self.groups.mode, Some(super::groups::Mode::Add(..))) {
                            words::ADD_SELECTED
                        } else {
                            words::CREATE
                        },
                    )
                    .selector("group-save")
                    .disabled(
                        self.groups.busy
                            || self.groups.selected.len()
                                < if matches!(
                                    self.groups.mode,
                                    Some(super::groups::Mode::Create(_))
                                ) {
                                    2
                                } else {
                                    1
                                },
                    )
                    .on_click(cx.listener(|this, _, window, cx| this.save_group(false, window, cx)))
                    .into_any_element(),
            );
        } else {
            footer.push(
                Button::new("new-group")
                    .label(words::NEW)
                    .selector("new-group")
                    .on_click(cx.listener(|this, _, window, cx| this.open_group_create(window, cx)))
                    .into_any_element(),
            );
            footer.push(
                Button::new("manage-group-memberships")
                    .label(words::MANAGE)
                    .selector("manage-group-memberships")
                    .on_click(
                        cx.listener(|this, _, window, cx| this.open_group_memberships(window, cx)),
                    )
                    .into_any_element(),
            );
        }
        let mut dialog = Dialog::new(
            if self.groups.mode.is_some() {
                words::TITLE
            } else {
                text::NEW_DIRECT_TITLE
            },
            self.layer_focus(),
        )
        .selector("new-direct-dialog")
        .on_close(Self::handler(cx, |this, window, cx| {
            this.close_layer(window, cx);
        }))
        .child(
            div()
                .typeface(theme.types.notice, &theme)
                .ink(theme.colors.ink_2)
                .child(if self.groups.mode.is_some() {
                    words::PEOPLE_HELP
                } else {
                    text::NEW_DIRECT_ABOUT
                }),
        )
        .children(
            matches!(self.groups.mode, Some(super::groups::Mode::Create(_)))
                .then(|| Field::new(&self.groups.name, words::NAME).selector("group-name")),
        )
        .child(
            div()
                .mt(u(10.0))
                .child(Field::new(&self.contact_part, text::CONTACT_PART).selector("contact-part")),
        )
        .child(
            div()
                .mt(u(6.0))
                .h(u(LIST_HEIGHT))
                .flex()
                .flex_col()
                .child(list),
        )
        .child(more)
        .child(dialog_footer(footer));
        if self.groups.mode.is_some() {
            dialog = dialog.child(
                div().debug_selector(|| "group-selected".to_owned()).child(
                    self.groups
                        .selected
                        .values()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", "),
                ),
            );
        }
        if let Some(reason) = &self.groups.failed {
            dialog = dialog.child(Notice::error(reason.clone()));
        }
        dialog.into_any_element()
    }
}
