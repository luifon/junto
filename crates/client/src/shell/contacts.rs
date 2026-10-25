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

    pub(crate) fn render_new_direct(&mut self, cx: &mut Context<Self>) -> AnyElement {
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
        for row in view.rows.iter().flatten() {
            let account_id = row.account_id;
            // The element is named by the account, so that a press that starts on one
            // account and ends on another after an answer chooses nobody.
            let choice = ChannelRow::new(
                ("contact", account_id.as_uuid().as_u128() as u64),
                Icon::Person,
                row.name.clone(),
            )
            .avatar(account_id.as_uuid().as_u128())
            .picture(faces.image(account_id))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.state
                    .update(cx, |state, cx| state.choose_contact(account_id, cx));
                this.close_layer(window, cx);
            }));
            list = list.child(
                div()
                    .flex_none()
                    .debug_selector(move || format!("contact-{account_id}"))
                    .child(choice),
            );
        }
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
        Dialog::new(text::NEW_DIRECT_TITLE, self.layer_focus())
            .selector("new-direct-dialog")
            .on_close(Self::handler(cx, |this, window, cx| {
                this.close_layer(window, cx);
            }))
            .child(
                div()
                    .typeface(theme.types.notice, &theme)
                    .ink(theme.colors.ink_2)
                    .child(text::NEW_DIRECT_ABOUT),
            )
            .child(
                div().mt(u(10.0)).child(
                    Field::new(&self.contact_part, text::CONTACT_PART).selector("contact-part"),
                ),
            )
            .child(
                crate::ui::notice_slot("contacts-status-slot")
                    .mt(u(6.0))
                    .children(status),
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
            .child(dialog_footer([Button::new("cancel-new-direct")
                .label(text::CANCEL)
                .selector("cancel-new-direct")
                .on_click(cx.listener(|this, _, window, cx| this.close_layer(window, cx)))
                .into_any_element()]))
            .into_any_element()
    }
}
