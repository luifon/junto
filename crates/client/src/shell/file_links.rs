use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, Styled, Window, div,
    prelude::FluentBuilder,
};
use junto_client_core::model::{Named, PlaceId};

use super::{Layer, Shell};
use crate::state::{AppState, OpenFileLink};
use crate::{
    text,
    theme::{theme, u},
    ui::{Button, Dialog, RowAction, StyleExt},
};

impl OpenFileLink {
    pub(super) fn is_stale(&self, state: &AppState) -> bool {
        let Some(server) = state.server(self.owner.entry.server_id) else {
            return true;
        };
        if server.owner() != Some(self.owner) {
            return true;
        }
        let Some(model) = server.live().view().model else {
            return true;
        };
        if let Some(Named::Read(message)) = model.named(self.message.id)
            && (message.deleted
                || !message
                    .attachments
                    .iter()
                    .any(|file| file.id == self.attachment_id))
        {
            return true;
        }
        match PlaceId::of(&self.message.place) {
            PlaceId::Channel(id) => {
                model.channel(id).is_none() && model.admin.archived_channel(id).is_none()
            }
            PlaceId::Direct(id) => model.place(PlaceId::Direct(id)).is_none(),
            PlaceId::Thread { .. } => true,
        }
    }
}

impl Shell {
    pub(super) fn file_copy_actions(
        &self,
        server_id: junto_client_core::protocol::ids::ServerId,
        file: &junto_client_core::protocol::message::Attachment,
        cx: &mut Context<Self>,
    ) {
        let name = file.name.clone();
        let id = file.id;
        RowAction::new(
            text::COPY_FILE_NAME,
            cx.listener(move |this, _, _, cx| {
                this.copy_text(name.clone(), cx);
            }),
        )
        .register();
        RowAction::new(
            text::COPY_TARGET_LINK,
            cx.listener(move |this, _, _, cx| {
                if let Some(link) = this.state.read(cx).file_link(server_id, id) {
                    this.copy_text(link, cx);
                }
            }),
        )
        .register();
    }

    pub(super) fn render_linked_file(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Layer::FileLink(target) = &self.layer else {
            return div().into_any_element();
        };
        let Some(file) = target
            .message
            .attachments
            .iter()
            .find(|file| file.id == target.attachment_id)
            .cloned()
        else {
            return div().into_any_element();
        };
        let (server_id, message_id) = (target.owner.entry.server_id, target.message.id);
        let attachment_id = file.id;
        let preview = super::decode::Stated::of(&file).is_some();
        let picture = file.clone();
        let copied = file.name.clone();
        let theme = theme(cx);
        let (row, actions) = crate::ui::collect_actions(|| {
            RowAction::new(
                text::COPY_FILE_NAME,
                cx.listener(move |this, _, _, cx| this.copy_text(copied.clone(), cx)),
            )
            .register();
            div()
                .id("linked-file-row")
                .debug_selector(|| "linked-file-row".to_owned())
                .flex()
                .flex_col()
                .gap(u(10.0))
                .child(
                    div()
                        .min_w_0()
                        .debug_selector(move || format!("linked-file-name:{attachment_id}"))
                        .typeface(theme.types.control_strong, &theme)
                        .child(text::label(&file.name)),
                )
                .child(div().typeface(theme.types.small, &theme).child(format!(
                    "{} · {}",
                    text::file_size(file.bytes),
                    text::label(&file.media_type)
                )))
                .child(
                    div()
                        .flex()
                        .gap(u(8.0))
                        .child(self.download_control(server_id, &file, 0, "linked-", &theme, cx))
                        .child(
                            Button::new("file-copy-link")
                                .label(text::COPY_TARGET_LINK)
                                .selector("file-copy-link")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let link =
                                        this.state.read(cx).file_link(server_id, attachment_id);
                                    if let Some(link) = link {
                                        this.copy_text(link, cx);
                                    }
                                })),
                        )
                        .when(preview, |row| {
                            row.child(
                                Button::new("file-preview")
                                    .label(text::OPEN_PICTURE)
                                    .selector("file-preview")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.open_picture_viewer(
                                            server_id,
                                            message_id,
                                            vec![picture.clone()],
                                            0,
                                            window,
                                            cx,
                                        );
                                    })),
                            )
                        }),
                )
        });
        let row = crate::ui::bind_action_menu(
            row,
            gpui::SharedString::from(format!("linked-file-actions:{attachment_id}")),
            actions,
            window,
            cx,
        );
        let close = cx.listener(|this, _: &(), window, cx| this.close_layer(window, cx));
        Dialog::new(text::LINKED_FILE, self.layer_focus())
            .selector("linked-file-dialog")
            .on_close(move |window, cx| close(&(), window, cx))
            .child(row)
            .into_any_element()
    }
}
