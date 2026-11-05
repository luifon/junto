use gpui::{AnyElement, App, Context, Hsla, InteractiveElement, IntoElement, Styled};
use junto_client_core::Owner;
use junto_client_core::arrangement::channels::{Change, Layout, Location};
use junto_client_core::protocol::ids::{ChannelId, WorkspaceId};

use super::Shell;
use crate::state::view::ChannelSection;
use crate::text;
use crate::theme::theme;
use crate::ui::{CategoryHeader, MenuItem, RowAction, UnreadSign};

#[derive(Debug, Clone, Copy)]
pub(super) struct ChannelDrag {
    pub owner: Owner,
    pub workspace: WorkspaceId,
    pub channel: ChannelId,
}

pub(super) fn collapsed_unread(section: &ChannelSection) -> Option<UnreadSign> {
    let (messages, mentions, unread) = section
        .rows
        .iter()
        .filter(|row| !row.selected && !row.unread.muted && !row.mute.is_on())
        .fold((0u64, 0u64, false), |(messages, mentions, unread), row| {
            row.threads.iter().fold(
                (
                    messages.saturating_add(row.unread.messages),
                    mentions.saturating_add(row.unread.mentions),
                    unread || row.unread.any(),
                ),
                |(messages, mentions, unread), thread| {
                    (
                        messages.saturating_add(thread.unread),
                        mentions.saturating_add(thread.mentions),
                        unread || thread.unread > 0 || thread.mentions > 0,
                    )
                },
            )
        });
    unread.then(|| UnreadSign {
        mentions: (mentions > 0).then(|| text::mention_badge(mentions).into()),
        label: text::unread_tooltip(messages, mentions).into(),
    })
}

impl Shell {
    pub(super) fn personal_source_current(
        &self,
        owner: Owner,
        workspace: WorkspaceId,
        cx: &App,
    ) -> bool {
        let state = self.state.read(cx);
        let view = state.view();
        state
            .server(owner.entry.server_id)
            .is_some_and(|server| server.owner() == Some(owner))
            && view
                .selected
                .as_ref()
                .is_some_and(|server| server.id == owner.entry.server_id)
            && view.workspace().is_some_and(|shown| shown.id == workspace)
    }

    fn personal_layout(&self, owner: Owner, workspace: WorkspaceId, cx: &App) -> Layout {
        self.state
            .read(cx)
            .arrangement()
            .channels(owner.entry.server_id, owner.account_id, workspace)
            .cloned()
            .unwrap_or_default()
    }

    fn personal_channel_current(&self, source: ChannelDrag, cx: &App) -> bool {
        self.personal_source_current(source.owner, source.workspace, cx)
            && self
                .state
                .read(cx)
                .view()
                .workspace()
                .is_some_and(|workspace| {
                    workspace.sections.iter().any(|section| {
                        section.rows.iter().any(|row| {
                            row.id == source.channel && (!section.collapsed || row.selected)
                        })
                    })
                })
    }

    fn change_personal(
        &mut self,
        owner: Owner,
        workspace: WorkspaceId,
        change: Change,
        cx: &mut Context<Self>,
    ) {
        if self.personal_source_current(owner, workspace, cx) {
            self.state.update(cx, |state, cx| {
                state.arrange_channels(owner, workspace, change, cx)
            });
        }
    }

    pub(super) fn personal_channel_items(
        &mut self,
        source: ChannelDrag,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let layout = self.personal_layout(source.owner, source.workspace, cx);
        let location = layout.location(source.channel);
        let favourite = location == Location::Favourites;
        let to = if favourite {
            Location::Default
        } else {
            Location::Favourites
        };
        let mut items = vec![
            MenuItem::new(
                "channel-favourite",
                if favourite {
                    text::REMOVE_FAVOURITE
                } else {
                    text::ADD_FAVOURITE
                },
            )
            .selector("channel-favourite")
            .on_click(cx.listener(move |this, _, window, cx| {
                if !this.personal_channel_current(source, cx) {
                    return;
                }
                this.close_layer(window, cx);
                this.change_personal(
                    source.owner,
                    source.workspace,
                    Change::Place {
                        channel: source.channel,
                        to,
                        before: None,
                    },
                    cx,
                );
            }))
            .into_any_element(),
        ];
        if let Some(channels) = layout.channels(location) {
            let position = channels
                .iter()
                .position(|channel| *channel == source.channel)
                .unwrap_or(0);
            for (up, label, selector) in [
                (true, text::MOVE_CHANNEL_UP, "channel-personal-up"),
                (false, text::MOVE_CHANNEL_DOWN, "channel-personal-down"),
            ] {
                items.push(
                    MenuItem::new(selector, label)
                        .selector(selector)
                        .disabled(if up {
                            position == 0
                        } else {
                            position + 1 >= channels.len()
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if !this.personal_channel_current(source, cx) {
                                return;
                            }
                            this.close_layer(window, cx);
                            this.change_personal(
                                source.owner,
                                source.workspace,
                                Change::MoveChannel {
                                    channel: source.channel,
                                    up,
                                },
                                cx,
                            );
                        }))
                        .into_any_element(),
                );
            }
        }
        items
    }

    pub(super) fn personal_header(
        &mut self,
        header: CategoryHeader,
        owner: Owner,
        workspace: WorkspaceId,
        section: &ChannelSection,
        cx: &mut Context<Self>,
    ) -> CategoryHeader {
        if section.personal != Some(Location::Favourites) {
            return header;
        }
        let collapsed = section.collapsed;
        header
            .toggle(
                collapsed,
                RowAction::new(
                    if collapsed {
                        text::EXPAND_SECTION
                    } else {
                        text::COLLAPSE_SECTION
                    },
                    cx.listener(move |this, _, _, cx| {
                        this.change_personal(
                            owner,
                            workspace,
                            Change::Collapse {
                                collapsed: !collapsed,
                            },
                            cx,
                        )
                    }),
                ),
            )
            .unread(
                "personal-favourites-unread",
                collapsed.then(|| collapsed_unread(section)).flatten(),
            )
    }

    pub(super) fn personal_drop(
        &mut self,
        frame: gpui::Stateful<gpui::Div>,
        owner: Owner,
        workspace: WorkspaceId,
        location: Location,
        before: Option<ChannelId>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let state = self.state.clone();
        let colour = theme(cx).colors.terracotta_wash;
        frame
            .can_drop(move |value, _, cx| {
                value.downcast_ref::<ChannelDrag>().is_some_and(|drag| {
                    drag.owner == owner
                        && drag.workspace == workspace
                        && state
                            .read(cx)
                            .server(owner.entry.server_id)
                            .is_some_and(|server| server.owner() == Some(owner))
                })
            })
            .drag_over::<ChannelDrag>(move |style, drag, _, _| {
                if drag.owner == owner && drag.workspace == workspace {
                    style.bg(Hsla::from(colour))
                } else {
                    style
                }
            })
            .on_drop(cx.listener(move |this, drag: &ChannelDrag, _, cx| {
                if drag.owner == owner
                    && drag.workspace == workspace
                    && this.personal_channel_current(*drag, cx)
                {
                    this.change_personal(
                        owner,
                        workspace,
                        Change::Place {
                            channel: drag.channel,
                            to: location,
                            before,
                        },
                        cx,
                    );
                }
            }))
    }
}
