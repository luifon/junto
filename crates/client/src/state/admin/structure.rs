//! The requests of the administration that change the structure of a server: workspaces,
//! categories, channels, the archive and the retention.

use gpui::Context;
use junto_client_core::api::{ApiError, ServerApi};
use junto_client_core::protocol::call::UpdateCallSettings;
use junto_client_core::protocol::discovery::Retention;
use junto_client_core::protocol::ids::{
    CategoryId, ChannelId, ConversationId, ServerId, WorkspaceId,
};
use junto_client_core::protocol::secret::InviteCode;
use junto_client_core::protocol::structure::{
    ArchivePeriod, ChannelKind, CreateCategory, CreateChannel, UpdateCategory, UpdateChannel,
};

use super::{AskKind, Made};
use crate::state::AppState;

/// The new positions that move the entry at `index` one place up or down among `order`,
/// which holds each entry with its position in the shown order. Two neighbours with
/// different positions exchange them. When two entries share a position, the order of the
/// server follows their identifiers, so every entry gets its place in the new order as
/// its position. Entries that keep their position are not named.
pub fn moved<Id: Copy + PartialEq>(order: &[(Id, i32)], index: usize, up: bool) -> Vec<(Id, i32)> {
    let other = if up {
        index.checked_sub(1)
    } else {
        index.checked_add(1).filter(|other| *other < order.len())
    };
    let (Some(other), Some(entry)) = (other, order.get(index)) else {
        return Vec::new();
    };
    let neighbour = order[other];
    let distinct = order
        .iter()
        .enumerate()
        .all(|(at, (_, position))| order[..at].iter().all(|(_, before)| before != position));
    if distinct {
        return vec![(entry.0, neighbour.1), (neighbour.0, entry.1)];
    }
    let mut next: Vec<Id> = order.iter().map(|(id, _)| *id).collect();
    next.swap(index, other);
    next.into_iter()
        .enumerate()
        .filter_map(|(place, id)| {
            let position = i32::try_from(place).unwrap_or(i32::MAX);
            let held = order.iter().find(|(of, _)| *of == id)?.1;
            (held != position).then_some((id, position))
        })
        .collect()
}

async fn move_channels(api: ServerApi, changes: Vec<(ChannelId, i32)>) -> Result<(), ApiError> {
    for (channel_id, position) in changes {
        let change = UpdateChannel {
            position: Some(position),
            ..UpdateChannel::default()
        };
        api.update_channel(channel_id, &change).await?;
    }
    Ok(())
}

async fn move_categories(
    api: ServerApi,
    workspace_id: WorkspaceId,
    changes: Vec<(CategoryId, i32)>,
) -> Result<(), ApiError> {
    for (category_id, position) in changes {
        let change = UpdateCategory {
            name: None,
            position: Some(position),
        };
        api.update_category(workspace_id, category_id, &change)
            .await?;
    }
    Ok(())
}

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
                this.admin.made = Some(Made::Entered(redemption.workspace_ids));
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
        let changes = moved(&order, index, up);
        if changes.is_empty() {
            return;
        }
        self.admin_ask(
            server_id,
            AskKind::UpdateCategory(category_id),
            |api| move_categories(api, workspace_id, changes),
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

    /// Changes the name, the category or the setting "permanent messages" of a channel.
    pub fn update_channel(
        &mut self,
        server_id: ServerId,
        channel_id: ChannelId,
        change: UpdateChannel,
        cx: &mut Context<Self>,
    ) {
        self.admin_ask(
            server_id,
            AskKind::UpdateChannel(channel_id),
            |api| async move { api.update_channel(channel_id, &change).await },
            |_, _, _| {},
            cx,
        );
    }

    /// Moves a channel one place up or down among the channels of its group: the
    /// channels of its category, or the channels without a category.
    pub fn move_channel(
        &mut self,
        server_id: ServerId,
        workspace_id: WorkspaceId,
        channel_id: ChannelId,
        up: bool,
        cx: &mut Context<Self>,
    ) {
        let order: Vec<(ChannelId, i32)> = self
            .client
            .server(server_id)
            .and_then(|server| server.live().view().model)
            .and_then(|model| {
                let workspace = model.workspace(workspace_id)?;
                let group = |category_id: Option<CategoryId>| {
                    category_id.filter(|id| workspace.categories.iter().any(|held| held.id == *id))
                };
                let of = group(workspace.channel(channel_id)?.channel.category_id);
                Some(
                    workspace
                        .channels
                        .iter()
                        .filter(|held| group(held.channel.category_id) == of)
                        .map(|held| (held.channel.id, held.channel.position))
                        .collect(),
                )
            })
            .unwrap_or_default();
        let Some(index) = order.iter().position(|(id, _)| *id == channel_id) else {
            return;
        };
        let changes = moved(&order, index, up);
        if changes.is_empty() {
            return;
        }
        self.admin_ask(
            server_id,
            AskKind::UpdateChannel(channel_id),
            |api| move_channels(api, changes),
            |_, (), _| {},
            cx,
        );
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
            |this, export, _| this.admin.made = Some(Made::Export(export.id)),
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
            |this, export, _| this.admin.made = Some(Made::Export(export.id)),
            cx,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::moved;

    #[test]
    fn two_neighbours_with_different_positions_exchange_them() {
        let order = [('a', 0), ('b', 4), ('c', 9)];
        assert_eq!(moved(&order, 1, true), [('b', 0), ('a', 4)]);
        assert_eq!(moved(&order, 1, false), [('b', 9), ('c', 4)]);
    }

    #[test]
    fn the_first_entry_does_not_move_up_and_the_last_not_down() {
        let order = [('a', 0), ('b', 1)];
        assert!(moved(&order, 0, true).is_empty());
        assert!(moved(&order, 1, false).is_empty());
        assert!(moved(&order, 5, true).is_empty());
        assert!(moved::<char>(&[], 0, false).is_empty());
    }

    #[test]
    fn entries_that_share_a_position_get_their_places_as_positions() {
        // The server orders equal positions by identifier, so an exchange of two equal
        // positions would change nothing.
        let order = [('a', 3), ('b', 3), ('c', 3)];
        assert_eq!(moved(&order, 2, true), [('a', 0), ('c', 1), ('b', 2)]);
        // An entry that has its place as position already is not sent.
        let order = [('a', 0), ('b', 1), ('c', 1)];
        assert_eq!(moved(&order, 2, true), [('b', 2)]);
    }
}
