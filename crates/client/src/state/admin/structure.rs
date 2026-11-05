//! The requests of the administration that change the structure of a server: workspaces,
//! categories, channels, the archive and the retention.

use gpui::Context;
use junto_client_core::api::ErrorCategory;
use junto_client_core::protocol::call::UpdateCallSettings;
use junto_client_core::protocol::discovery::Retention;
use junto_client_core::protocol::ids::{
    CategoryId, ChannelId, ConversationId, ServerId, WorkspaceId,
};
use junto_client_core::protocol::secret::InviteCode;
use junto_client_core::protocol::structure::{
    ArchivePeriod, ChannelKind, ChannelMove, CreateCategory, CreateChannel, MoveChannels,
    UpdateCategory, UpdateChannel,
};

use super::{AskKind, Made};
use crate::state::AppState;
use crate::state::receipts::Awaited;
use crate::text;

impl AppState {
    pub fn create_workspace(&mut self, server_id: ServerId, name: String, cx: &mut Context<Self>) {
        self.admin_ask(
            server_id,
            AskKind::CreateWorkspace,
            |api| async move { api.create_workspace(name).await },
            |_, _, _| {},
            cx,
        );
    }

    pub fn rename_workspace(
        &mut self,
        server_id: ServerId,
        workspace_id: WorkspaceId,
        name: String,
        cx: &mut Context<Self>,
    ) {
        self.admin_ask(
            server_id,
            AskKind::RenameWorkspace(workspace_id),
            |api| async move { api.rename_workspace(workspace_id, name).await },
            |_, _, _| {},
            cx,
        );
    }

    /// Deletes a workspace with everything that it holds (ADR 0308).
    pub fn delete_workspace(
        &mut self,
        server_id: ServerId,
        workspace_id: WorkspaceId,
        cx: &mut Context<Self>,
    ) {
        self.admin_ask(
            server_id,
            AskKind::DeleteWorkspace(workspace_id),
            |api| async move { api.delete_workspace(workspace_id).await },
            |_, (), _| {},
            cx,
        );
    }

    /// Enters the workspaces of an invite with the account of this device (ADR 0115).
    pub fn redeem_invite(&mut self, server_id: ServerId, code: &str, cx: &mut Context<Self>) {
        let code = InviteCode::new(code.trim());
        self.admin_ask(
            server_id,
            AskKind::RedeemInvite,
            |api| async move { api.redeem_invite(code).await },
            |this, redemption, _| {
                this.admin.make(Made::Entered(redemption.workspace_ids));
            },
            cx,
        );
    }

    pub fn create_category(
        &mut self,
        server_id: ServerId,
        workspace_id: WorkspaceId,
        name: String,
        cx: &mut Context<Self>,
    ) {
        let category = CreateCategory {
            name,
            position: None,
        };
        self.admin_ask(
            server_id,
            AskKind::CreateCategory,
            |api| async move { api.create_category(workspace_id, &category).await },
            |_, _, _| {},
            cx,
        );
    }

    pub fn rename_category(
        &mut self,
        server_id: ServerId,
        workspace_id: WorkspaceId,
        category_id: CategoryId,
        name: String,
        cx: &mut Context<Self>,
    ) {
        let change = UpdateCategory {
            name: Some(name),
            position: None,
        };
        self.admin_ask(
            server_id,
            AskKind::UpdateCategory(category_id),
            |api| async move {
                api.update_category(workspace_id, category_id, &change)
                    .await
            },
            |_, _, _| {},
            cx,
        );
    }

    /// Moves a category one place up or down among the categories of its workspace.
    pub fn move_category(
        &mut self,
        server_id: ServerId,
        workspace_id: WorkspaceId,
        category_id: CategoryId,
        up: bool,
        cx: &mut Context<Self>,
    ) {
        let order: Vec<(CategoryId, i32)> = self
            .client
            .server(server_id)
            .and_then(|server| server.live().view().model)
            .and_then(|model| {
                model.workspace(workspace_id).map(|workspace| {
                    workspace
                        .categories
                        .iter()
                        .map(|held| (held.id, held.position))
                        .collect()
                })
            })
            .unwrap_or_default();
        let Some(index) = order.iter().position(|(id, _)| *id == category_id) else {
            return;
        };
        // The position of a request is the place among the categories that the person
        // sees, without the moved one (ADR 0304).
        let to = if up {
            index.checked_sub(1)
        } else {
            Some(index + 1).filter(|to| *to < order.len())
        };
        let Some(position) = to.and_then(|to| i32::try_from(to).ok()) else {
            return;
        };
        let change = UpdateCategory {
            name: None,
            position: Some(position),
        };
        self.admin_ask(
            server_id,
            AskKind::UpdateCategory(category_id),
            |api| async move {
                api.update_category(workspace_id, category_id, &change)
                    .await
                    .map(|_| ())
            },
            |_, (), _| {},
            cx,
        );
    }

    /// Deletes a category. Its channels stay and have no category (ADR 0308).
    pub fn delete_category(
        &mut self,
        server_id: ServerId,
        workspace_id: WorkspaceId,
        category_id: CategoryId,
        cx: &mut Context<Self>,
    ) {
        self.admin_ask(
            server_id,
            AskKind::DeleteCategory(category_id),
            |api| async move { api.delete_category(workspace_id, category_id).await },
            |_, (), _| {},
            cx,
        );
    }

    pub fn create_channel(
        &mut self,
        server_id: ServerId,
        workspace_id: WorkspaceId,
        name: String,
        kind: ChannelKind,
        category_id: Option<CategoryId>,
        cx: &mut Context<Self>,
    ) {
        let channel = CreateChannel {
            topic: String::new(),
            name,
            kind,
            category_id,
            position: None,
        };
        self.admin_ask(
            server_id,
            AskKind::CreateChannel,
            |api| async move { api.create_channel(workspace_id, &channel).await },
            |_, _, _| {},
            cx,
        );
    }

    pub fn channel_topics_supported(&self, server_id: ServerId) -> bool {
        self.client
            .server(server_id)
            .is_some_and(|server| server.entry().capabilities.channel_topics)
    }

    /// Changes the name, the category or the setting "permanent messages" of a channel.
    pub fn update_channel(
        &mut self,
        server_id: ServerId,
        channel_id: ChannelId,
        change: UpdateChannel,
        cx: &mut Context<Self>,
    ) {
        if change.topic.is_some() && !self.channel_topics_supported(server_id) {
            let stamp = self.admin.begin(AskKind::UpdateChannel(channel_id));
            self.admin
                .settle(stamp, Some(text::TOPIC_UNSUPPORTED.to_owned()));
            self.changed(cx);
            return;
        }
        self.admin_ask(
            server_id,
            AskKind::UpdateChannel(channel_id),
            |api| async move { api.update_channel(channel_id, &change).await },
            |_, _, _| {},
            cx,
        );
    }

    /// Moves channels of a workspace as the page computed it from what it showed: one
    /// request that the server applies whole or not at all, and refuses when a channel is
    /// no longer where the page read it (ADR 0761). `channel_id` is the channel that the
    /// person moved.
    pub fn move_channels(
        &mut self,
        server_id: ServerId,
        workspace_id: WorkspaceId,
        channel_id: ChannelId,
        moves: Vec<ChannelMove>,
        cx: &mut Context<Self>,
    ) {
        if moves.is_empty() {
            return;
        }
        // The page shows the moves from now until the state holds them, or until the
        // server refuses them (ADR 0730).
        let number = self
            .admin
            .channel_moves
            .begin(server_id, workspace_id, channel_id, &moves);
        let moves = MoveChannels { moves };
        let what = AskKind::MoveChannels(channel_id);
        let (told, outcome) = futures_channel::oneshot::channel();
        self.admin_ask(
            server_id,
            what,
            |api| async move {
                let answer = api.move_channels(workspace_id, &moves).await;
                let _ = told.send(answer.as_ref().copied().map_err(|error| {
                    (
                        text::admin::refusal(what, error),
                        error.category() == Some(ErrorCategory::Conflict),
                    )
                }));
                answer
            },
            |_, _, _| {},
            cx,
        );
        // The page states the refusal of every refused move at its drawn place, also of
        // one that is not the newest request of the page (ADR 0730).
        cx.spawn(async move |this, cx| {
            let outcome = outcome.await;
            let _ = this.update(cx, |this, cx| {
                match outcome {
                    Ok(Ok(receipt)) => {
                        this.admin.channel_moves.answered(number, receipt);
                        let model = this
                            .client
                            .server(server_id)
                            .and_then(|server| server.live().view().model);
                        if let Some(model) = model {
                            this.refresh_for(
                                server_id,
                                &model,
                                Awaited::new(receipt),
                                move |this| this.admin.channel_moves.waits_for(number),
                                move |this| this.admin.channel_moves.refreshed(number),
                                cx,
                            );
                        }
                        this.settle_channel_moves();
                    }
                    Ok(Err((reason, conflict))) => {
                        this.admin.channel_moves.refused(number, reason);
                        // The page read an order that the server no longer holds: the state
                        // is read again now, and the page shows it when the person puts the
                        // refusal away.
                        if conflict && let Some(server) = this.client.server(server_id) {
                            let refreshing = server.live().refresh();
                            cx.spawn(async move |_, _| {
                                let _ = refreshing.await;
                            })
                            .detach();
                        }
                    }
                    // The request was dropped with its server: nothing is drawn of it.
                    Err(_) => this.admin.channel_moves.dropped(number),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The person acted on the layer: a refused move is no longer drawn.
    pub fn release_refused_moves(&mut self, cx: &mut Context<Self>) {
        self.admin.channel_moves.release();
        cx.notify();
    }

    /// The moves of channels that the state shows now wait no more.
    pub(super) fn settle_channel_moves(&mut self) {
        let snapshots = self.snapshots();
        self.admin.channel_moves.settle(|server_id, workspace_id| {
            snapshots
                .iter()
                .find(|server| server.id == server_id)
                .and_then(super::view::shown)
                .filter(|model| model.workspace(workspace_id).is_some())
        });
    }

    pub fn delete_channel(
        &mut self,
        server_id: ServerId,
        channel_id: ChannelId,
        cx: &mut Context<Self>,
    ) {
        self.admin_ask(
            server_id,
            AskKind::DeleteChannel(channel_id),
            |api| async move { api.delete_channel(channel_id).await },
            |_, (), _| {},
            cx,
        );
    }

    /// Archives a channel: it becomes read-only and leaves the channel list (ADR 0410).
    pub fn archive_channel(
        &mut self,
        server_id: ServerId,
        channel_id: ChannelId,
        cx: &mut Context<Self>,
    ) {
        self.admin_ask(
            server_id,
            AskKind::ArchiveChannel(channel_id),
            |api| async move { api.archive_channel(channel_id).await },
            |_, _, _| {},
            cx,
        );
    }

    pub fn restore_channel(
        &mut self,
        server_id: ServerId,
        channel_id: ChannelId,
        cx: &mut Context<Self>,
    ) {
        self.admin_ask(
            server_id,
            AskKind::RestoreChannel(channel_id),
            |api| async move { api.restore_channel(channel_id).await },
            |_, _, _| {},
            cx,
        );
    }

    /// Changes the user limit or the setting that hides who is inside a voice channel.
    pub fn set_call_settings(
        &mut self,
        server_id: ServerId,
        channel_id: ChannelId,
        change: UpdateCallSettings,
        cx: &mut Context<Self>,
    ) {
        self.admin_ask(
            server_id,
            AskKind::CallSettings(channel_id),
            |api| async move { api.update_call_settings(channel_id, &change).await },
            |_, _, _| {},
            cx,
        );
    }

    /// Sets the time after which the server deletes an archived channel of a workspace.
    /// Absent days: never.
    pub fn set_archive_period(
        &mut self,
        server_id: ServerId,
        workspace_id: WorkspaceId,
        days: Option<u32>,
        cx: &mut Context<Self>,
    ) {
        self.admin_ask(
            server_id,
            AskKind::ArchivePeriod,
            |api| async move {
                api.set_archive_period(workspace_id, ArchivePeriod { days })
                    .await
            },
            |_, _, _| {},
            cx,
        );
    }

    pub fn set_file_retention(
        &mut self,
        server_id: ServerId,
        workspace_id: WorkspaceId,
        days: Option<u32>,
        cx: &mut Context<Self>,
    ) {
        self.admin_ask(
            server_id,
            AskKind::FileRetention,
            |api| async move {
                api.set_file_retention(workspace_id, &Retention { days })
                    .await
            },
            |_, _, _| {},
            cx,
        );
    }

    pub fn set_recording_retention(
        &mut self,
        server_id: ServerId,
        workspace_id: WorkspaceId,
        days: Option<u32>,
        cx: &mut Context<Self>,
    ) {
        self.admin_ask(
            server_id,
            AskKind::RecordingRetention,
            |api| async move {
                api.set_recording_retention(workspace_id, &Retention { days })
                    .await
            },
            |_, _, _| {},
            cx,
        );
    }

    pub fn set_direct_retention(
        &mut self,
        server_id: ServerId,
        days: Option<u32>,
        cx: &mut Context<Self>,
    ) {
        self.admin_ask(
            server_id,
            AskKind::DirectRetention,
            |api| async move { api.set_direct_message_retention(&Retention { days }).await },
            |_, _, _| {},
            cx,
        );
    }

    /// Asks for an export of a channel (ADR 0411). Its state is on the page of the
    /// exports.
    pub fn export_channel(
        &mut self,
        server_id: ServerId,
        channel_id: ChannelId,
        cx: &mut Context<Self>,
    ) {
        self.admin_ask(
            server_id,
            AskKind::RequestExport,
            |api| async move { api.export_channel(channel_id).await },
            |this, export, _| this.admin.make(Made::Export(export.id)),
            cx,
        );
    }

    /// Asks for an export of a direct conversation of the person.
    pub fn export_conversation(
        &mut self,
        server_id: ServerId,
        conversation_id: ConversationId,
        cx: &mut Context<Self>,
    ) {
        self.admin_ask(
            server_id,
            AskKind::RequestExport,
            |api| async move { api.export_conversation(conversation_id).await },
            |this, export, _| this.admin.make(Made::Export(export.id)),
            cx,
        );
    }
}
