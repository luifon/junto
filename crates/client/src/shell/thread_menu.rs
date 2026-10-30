use gpui::{
    AnyElement, Context, IntoElement, ParentElement, Styled, Window, div, prelude::FluentBuilder,
};
use junto_client_core::{
    Owner,
    protocol::ids::{ChannelId, MessageId, WorkspaceId},
};

use super::{Layer, Shell};
use crate::{
    state::{
        AppState,
        view::{ShellView, ThreadEntry},
    },
    text,
    theme::u,
    ui::{Menu, MenuItem, Notice, menu_label},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ThreadSource {
    Followed,
    Preview(junto_client_core::model::PlaceId),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ThreadMenuOpen {
    pub owner: Owner,
    pub workspace_id: WorkspaceId,
    pub channel_id: ChannelId,
    pub root_id: MessageId,
    waiting: bool,
    source: ThreadSource,
    position: Option<gpui::Point<gpui::Pixels>>,
}

impl ThreadMenuOpen {
    pub(super) fn same_target(&self, other: &Self) -> bool {
        (self.owner, self.workspace_id, self.channel_id, self.root_id)
            == (
                other.owner,
                other.workspace_id,
                other.channel_id,
                other.root_id,
            )
    }

    pub(super) fn followed(&self) -> bool {
        self.source == ThreadSource::Followed
    }

    pub(super) fn preview(
        owner: Owner,
        workspace_id: WorkspaceId,
        channel_id: ChannelId,
        root_id: MessageId,
        place: junto_client_core::model::PlaceId,
        position: gpui::Point<gpui::Pixels>,
    ) -> Self {
        Self {
            owner,
            workspace_id,
            channel_id,
            root_id,
            waiting: false,
            source: ThreadSource::Preview(place),
            position: Some(position),
        }
    }

    pub(super) fn preview_of(
        &self,
        owner: Owner,
        place: junto_client_core::model::PlaceId,
        root: MessageId,
    ) -> bool {
        self.owner == owner && self.root_id == root && self.source == ThreadSource::Preview(place)
    }

    pub(super) fn preview_entry(&self, view: &ShellView) -> Option<ThreadEntry> {
        let ThreadSource::Preview(place) = self.source else {
            return None;
        };
        if view.selected.as_ref()?.id != self.owner.entry.server_id
            || view.workspace()?.id != self.workspace_id
        {
            return None;
        }
        let lists = view.lists()?;
        [
            lists.feed.as_ref(),
            lists
                .workspace
                .as_ref()
                .and_then(|workspace| workspace.thread.as_ref()?.feed.as_ref()),
        ]
        .into_iter()
        .flatten()
        .find_map(|feed| {
            if feed.place != place {
                return None;
            }
            feed.rows.iter().find_map(|row| {
                let crate::state::feed::FeedRow::Message(row) = row else {
                    return None;
                };
                if row.message.id != self.root_id || row.message.deleted {
                    return None;
                }
                let mark = row.thread.as_ref()?;
                Some(ThreadEntry {
                    root_id: self.root_id,
                    title: row
                        .message
                        .document
                        .as_ref()
                        .map(|document| crate::state::feed::plain_text(document, &feed.names, 200)),
                    unread: mark.unread,
                    mentions: mark.mentions,
                    open: mark.open,
                    available: row.can_thread && row.thread_refused.is_none(),
                })
            })
        })
    }

    fn entry<'a>(&self, view: &'a ShellView) -> Option<&'a ThreadEntry> {
        if view.selected.as_ref()?.id != self.owner.entry.server_id {
            return None;
        }
        view.workspace()
            .filter(|workspace| workspace.id == self.workspace_id)?
            .sections
            .iter()
            .flat_map(|section| &section.rows)
            .find(|channel| channel.id == self.channel_id)?
            .threads
            .iter()
            .find(|thread| thread.root_id == self.root_id)
    }

    fn source_gone(&self, view: &ShellView, state: &AppState) -> bool {
        state
            .server(self.owner.entry.server_id)
            .and_then(|server| server.owner())
            != Some(self.owner)
            || if self.followed() {
                self.entry(view).is_none()
            } else {
                self.preview_entry(view).is_none()
            }
    }

    pub(super) fn is_stale(&self, view: &ShellView, state: &AppState) -> bool {
        self.source_gone(view, state)
            || self.waiting
                && state
                    .thread_menu_control(self.owner, self.root_id)
                    .is_some_and(|control| !control.busy && control.refused.is_none())
    }
}

impl Shell {
    pub(super) fn open_thread_menu(
        &mut self,
        owner: Owner,
        workspace_id: WorkspaceId,
        channel_id: ChannelId,
        root_id: MessageId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let open = ThreadMenuOpen {
            owner,
            workspace_id,
            channel_id,
            root_id,
            waiting: false,
            source: ThreadSource::Followed,
            position: None,
        };
        self.open_thread_menu_target(open, window, cx);
    }

    pub(super) fn open_thread_menu_target(
        &mut self,
        open: ThreadMenuOpen,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(&self.layer, Layer::ThreadMenu(before) if before.same_target(&open) && before.source == open.source)
        {
            self.layer_focus.focus(window, cx);
            return;
        }
        let current = self.state.read(cx).view();
        if open.source_gone(&current, self.state.read(cx)) {
            return;
        }
        if self.state.update(cx, |state, cx| {
            state.begin_thread_menu(open.owner, open.root_id, cx)
        }) {
            self.thread_menu_placement = Default::default();
            self.open_layer(Layer::ThreadMenu(open), window, cx);
        }
    }

    fn thread_menu_current(&self, open: ThreadMenuOpen, cx: &gpui::App) -> bool {
        matches!(&self.layer, Layer::ThreadMenu(current) if current.same_target(&open) && current.source == open.source)
            && !open.source_gone(&self.state.read(cx).view(), self.state.read(cx))
    }

    pub(super) fn render_thread_menu(
        &mut self,
        open: ThreadMenuOpen,
        entry: &ThreadEntry,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = self.state.read(cx);
        let control = state.thread_menu_control(open.owner, open.root_id);
        let failed = state.thread_menu_read_failed(open.owner, open.root_id);
        let busy = control.as_ref().is_none_or(|control| control.busy);
        let following = control.as_ref().is_some_and(|control| control.following);
        let refused = control.as_ref().and_then(|control| control.refused.clone());
        let title = entry
            .title
            .clone()
            .unwrap_or_else(|| text::THREAD.to_owned());
        let unread = entry.unread > 0 || entry.mentions > 0;
        let available = entry.available;
        let close = Self::handler(cx, |this, window, cx| this.close_layer(window, cx));
        let menu = Menu::new("thread-menu", text::THREAD, self.layer_focus())
            .selector("thread-menu")
            .on_close(move |window, cx| close(window, cx))
            .child(menu_label(title, false, cx))
            .child(
                MenuItem::new("thread-open", text::OPEN_THREAD)
                    .selector("thread-open")
                    .disabled(!available)
                    .when(!available && open.followed(), |item| {
                        item.detail(text::THREAD_IN_CALL)
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if !this.thread_menu_current(open, cx) {
                            return;
                        }
                        this.close_layer(window, cx);
                        this.state.update(cx, |state, cx| {
                            state.select_channel(open.workspace_id, open.channel_id, cx)
                        });
                        this.open_thread(
                            open.owner.entry.server_id,
                            open.channel_id,
                            open.root_id,
                            window,
                            cx,
                        );
                    })),
            )
            .child(
                MenuItem::new("thread-read", text::MARK_AS_READ)
                    .selector("thread-read")
                    .disabled(busy || !unread)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.thread_menu_current(open, cx)
                            && this.state.update(cx, |state, cx| {
                                state.mark_thread_menu_read(open.owner, open.root_id, cx)
                            })
                        {
                            if let Layer::ThreadMenu(open) = &mut this.layer {
                                open.waiting = true;
                            }
                            cx.notify();
                        }
                    })),
            )
            .child(
                MenuItem::new("thread-follow", text::followed::action(following))
                    .selector("thread-follow")
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.thread_menu_current(open, cx)
                            && this.state.update(cx, |state, cx| {
                                state.set_thread_menu_follow(
                                    open.owner,
                                    open.root_id,
                                    !following,
                                    cx,
                                )
                            })
                        {
                            if let Layer::ThreadMenu(open) = &mut this.layer {
                                open.waiting = true;
                            }
                            cx.notify();
                        }
                    })),
            )
            .child(
                MenuItem::new("thread-copy-link", text::COPY_TARGET_LINK)
                    .selector("thread-copy-link")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if !this.thread_menu_current(open, cx) {
                            return;
                        }
                        let link = this
                            .state
                            .read(cx)
                            .thread_link(open.owner.entry.server_id, open.root_id);
                        this.close_layer(window, cx);
                        if let Some(link) = link {
                            this.copy_text(link, cx);
                        }
                    })),
            )
            .when(failed, |menu| {
                menu.child(Notice::error(text::THREAD_FAILED).selector("thread-menu-failed"))
                    .child(
                        MenuItem::new("thread-menu-retry", text::TRY_AGAIN)
                            .selector("thread-menu-retry")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if this.thread_menu_current(open, cx) {
                                    this.state.update(cx, |state, cx| {
                                        state.retry_thread_menu(open.owner, open.root_id, cx)
                                    });
                                }
                            })),
                    )
            })
            .when_some(refused, |menu, reason| {
                menu.child(Notice::error(reason).selector("thread-menu-refused"))
            });
        let popup = match open.position {
            Some(position) => menu.popup_at_stable(position, self.thread_menu_placement.clone()),
            None => menu.popup_stable(gpui::Point::default(), self.thread_menu_placement.clone()),
        };
        div()
            .absolute()
            .left(u(28.0))
            .top(u(4.0))
            .child(popup)
            .into_any_element()
    }
}
