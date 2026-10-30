use std::sync::Arc;

use gpui::{App, Context, Div, InteractiveElement, SharedString, Stateful, Styled, Window};
use junto_client_core::{
    Owner,
    model::{PendingMessage, PlaceId, SendState},
};

use super::{Layer, Shell};
use crate::{
    state::feed::{FeedRow, Names, can_send_again},
    text,
    ui::{RowAction, bind_lazy_action_menu},
};

#[derive(Clone, Copy, Debug)]
pub(super) struct PendingTarget {
    pub owner: Owner,
    pub place: PlaceId,
    pub id: u64,
    pub upload: Option<u64>,
}

#[derive(Clone, Copy)]
enum PendingAction {
    Copy,
    Retry,
    Discard,
}

impl Shell {
    fn pending_source(
        &self,
        target: PendingTarget,
        cx: &App,
    ) -> Option<(Arc<PendingMessage>, Arc<Names>)> {
        if !matches!(self.layer, Layer::None) {
            return None;
        }
        let state = self.state.read(cx);
        if state.server(target.owner.entry.server_id)?.owner() != Some(target.owner) {
            return None;
        }
        let view = state.view();
        if view.selected.as_ref()?.id != target.owner.entry.server_id {
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
            if feed.place != target.place {
                return None;
            }
            feed.rows.iter().find_map(|row| {
                let FeedRow::Waiting(row) = row else {
                    return None;
                };
                (row.pending.id == target.id
                    && target.upload.is_none_or(|upload| {
                        row.pending.files.iter().any(|file| file.upload == upload)
                    }))
                .then(|| (row.pending.clone(), feed.names.clone()))
            })
        })
    }

    fn pending_action(
        &self,
        target: PendingTarget,
        label: &'static str,
        action: PendingAction,
        cx: &mut Context<Self>,
    ) -> RowAction {
        RowAction::new(
            label,
            cx.listener(move |this, _, _, cx| {
                let Some((pending, names)) = this.pending_source(target, cx) else {
                    return;
                };
                match action {
                    PendingAction::Copy => {
                        let value = if let Some(upload) = target.upload {
                            let Some(file) =
                                pending.files.iter().find(|file| file.upload == upload)
                            else {
                                return;
                            };
                            file.name.clone()
                        } else {
                            crate::state::feed::copy_text(&pending.document, &names)
                        };
                        this.copy_text(value, cx);
                    }
                    PendingAction::Retry if can_send_again(pending.state) => {
                        this.state.update(cx, |state, _| {
                            state.send_again(target.owner.entry.server_id, target.id)
                        })
                    }
                    PendingAction::Discard if pending.state != SendState::Sending => {
                        this.state.update(cx, |state, cx| {
                            state.discard_message(target.owner.entry.server_id, target.id, cx)
                        })
                    }
                    _ => {}
                }
            }),
        )
    }

    pub(super) fn bind_pending_menu(
        &mut self,
        frame: Stateful<Div>,
        target: PendingTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let theme = crate::theme::theme(cx);
        let ring =
            crate::ui::focus_ring_inside(theme.colors.terracotta_deep, theme.metrics.focus_ring);
        bind_lazy_action_menu(
            frame.focus_visible(move |style| style.shadow(ring)),
            SharedString::from(format!("pending:{target:?}")),
            window,
            cx,
            |_, cx| {
                let Some((pending, _)) = self.pending_source(target, cx) else {
                    return Vec::new();
                };
                let mut actions = vec![self.pending_action(
                    target,
                    if target.upload.is_some() {
                        text::COPY_NAME
                    } else {
                        text::COPY_TEXT
                    },
                    PendingAction::Copy,
                    cx,
                )];
                if can_send_again(pending.state) {
                    actions.push(self.pending_action(
                        target,
                        text::SEND_AGAIN,
                        PendingAction::Retry,
                        cx,
                    ));
                }
                if pending.state != SendState::Sending {
                    actions.push(self.pending_action(
                        target,
                        text::DISCARD,
                        PendingAction::Discard,
                        cx,
                    ));
                }
                actions
            },
        )
    }
}
