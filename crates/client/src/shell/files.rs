use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, Role, SharedString,
    StatefulInteractiveElement, Styled, Window, div,
};
use junto_client_core::model::PlaceId;
use junto_client_core::protocol::ids::ServerId;

use super::Shell;
use crate::state::search::{ResultRow, ResultTarget};
use crate::text;
use crate::theme::{Theme, u};
use crate::ui::{Button, ButtonKind, RowAction, StyleExt};

impl Shell {
    pub(super) fn show_place_files(
        &mut self,
        server: ServerId,
        place: PlaceId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.members_open = false;
        self.type_search("", window, cx);
        self.state
            .update(cx, |state, cx| state.show_place_files(server, place, cx));
        self.search_scroll.set_offset(gpui::Point::default());
        cx.notify();
    }

    pub(super) fn listed_files(
        &mut self,
        server: ServerId,
        result: &ResultRow,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let ResultTarget::Message(message) = result.target else {
            return Vec::new();
        };
        let owner = self
            .state
            .read(cx)
            .server(server)
            .and_then(|server| server.owner());
        result
            .attachments
            .iter()
            .enumerate()
            .map(|(index, file)| {
                let id = file.id;
                let (row, actions) = crate::ui::collect_actions(|| {
                    self.file_copy_actions(server, file, cx);
                    RowAction::new(
                        text::GO_TO_MESSAGE,
                        cx.listener(move |this, _, _, cx| {
                            this.go_to_result(server, ResultTarget::Message(message), cx);
                        }),
                    )
                    .register();
                    div()
                        .flex()
                        .flex_col()
                        .flex_none()
                        .px(u(14.0))
                        .py(u(6.0))
                        .gap(u(4.0))
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .typeface(theme.types.control_strong, theme)
                                .child(text::label(&file.name)),
                        )
                        .child(
                            div()
                                .typeface(theme.types.small, theme)
                                .child(text::file_size(file.bytes)),
                        )
                        .child(
                            div()
                                .flex()
                                .gap(u(6.0))
                                .child(
                                    div()
                                        .debug_selector(move || format!("listed-open:{id}"))
                                        .child(
                                            Button::new(("open-listed-file", index))
                                                .kind(ButtonKind::Ghost)
                                                .label("Open file")
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    let address = this
                                                        .state
                                                        .read(cx)
                                                        .server(server)
                                                        .map(|server| server.entry().address);
                                                    if let Some(address) = address {
                                                        this.state.update(cx, |state, cx| {
                                                            state.open_file_link(&address, id, cx);
                                                        });
                                                    }
                                                })),
                                        ),
                                )
                                .child(
                                    self.download_control(
                                        server, file, index, "listed-", theme, cx,
                                    ),
                                ),
                        )
                });
                let key: SharedString = format!("listed-file:{owner:?}:{message}:{id}").into();
                let row = div()
                    .id(key.clone())
                    .focusable()
                    .tab_stop(true)
                    .role(Role::ListItem)
                    .debug_selector(move || format!("listed-file:{id}"))
                    .child(row);
                crate::ui::bind_action_menu(row, key, actions, window, cx).into_any_element()
            })
            .collect()
    }
}
