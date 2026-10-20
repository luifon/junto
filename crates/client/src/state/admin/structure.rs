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
    ArchivePeriod, ChannelKind, ChannelMove, CreateCategory, CreateChannel, MoveChannels,
    UpdateCategory, UpdateChannel,
};

use super::{AskKind, Made};
use crate::state::AppState;

/// Every entry of `order` with its place as its position, without the entries that hold
/// it already.
fn renumbered<Id: Copy + PartialEq>(order: &[(Id, i32)], held: &[(Id, i32)]) -> Vec<(Id, i32)> {
    order
        .iter()
        .enumerate()
        .filter_map(|(place, (id, _))| {
            let position = i32::try_from(place).unwrap_or(i32::MAX);
            let before = held.iter().find(|(of, _)| of == id).map(|(_, at)| *at);
            (before != Some(position)).then_some((*id, position))
        })
        .collect()
}

/// The new positions that put `moving` at the place `to` of a group, whose entries are
/// `order` in their shown order. `order` holds `moving` when it moves inside its group,
/// and `held` is its position before. The entries from `to` on move one place down.
///
/// Inside a group with distinct positions, the entries between the two places take the
/// positions that they held among themselves in the new order, so no other entry changes.
/// An entry that comes from another group takes a free position between its new
/// neighbours, and is named also when its position stays, because its group changes.
/// Without such a position, or when two entries share one, every entry of the group gets
/// its place as its position. Empty when nothing moves.
pub fn placed<Id: Copy + PartialEq>(
    order: &[(Id, i32)],
    moving: Id,
    held: i32,
    to: usize,
) -> Vec<(Id, i32)> {
    let from = order.iter().position(|(id, _)| *id == moving);
    let mut next: Vec<(Id, i32)> = order
        .iter()
        .copied()
        .filter(|(id, _)| *id != moving)
        .collect();
    let at = to.min(next.len());
    next.insert(at, (moving, held));
    let distinct = |entries: &[(Id, i32)]| entries.windows(2).all(|pair| pair[0].1 < pair[1].1);
    match from {
        Some(from) => {
            if from == at {
                return Vec::new();
            }
            if !distinct(order) {
                return renumbered(&next, order);
            }
            let (first, last) = (from.min(at), from.max(at));
            let positions: Vec<i32> = order[first..=last].iter().map(|(_, at)| *at).collect();
            next[first..=last]
                .iter()
                .zip(positions)
                .filter(|((id, _), position)| {
                    order.iter().any(|(of, at)| of == id && at != position)
                })
                .map(|((id, _), position)| (*id, position))
                .collect()
        }
        None => {
            let others: Vec<(Id, i32)> = next
                .iter()
                .copied()
                .filter(|(id, _)| *id != moving)
                .collect();
            let before = at
                .checked_sub(1)
                .and_then(|place| next.get(place))
                .map(|(_, at)| *at);
            let after = next.get(at + 1).map(|(_, at)| *at);
            let free = match (before, after) {
                (None, None) => Some(0),
                (Some(before), None) => before.checked_add(1),
                (None, Some(after)) => after.checked_sub(1).filter(|position| *position >= 0),
                (Some(before), Some(after)) => {
                    before.checked_add(1).filter(|position| *position < after)
                }
            };
            match free {
                Some(position) if distinct(&others) => vec![(moving, position)],
                _ => {
                    let mut changes = renumbered(&next, &others);
                    // The entry that changes its group is always named.
                    if !changes.iter().any(|(id, _)| *id == moving) {
                        let position = i32::try_from(at).unwrap_or(i32::MAX);
                        changes.push((moving, position));
                    }
                    changes
                }
            }
        }
    }
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
        let to = if up {
            index.checked_sub(1)
        } else {
            Some(index + 1)
        };
        let Some(to) = to else {
            return;
        };
        let changes = placed(&order, category_id, order[index].1, to);
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
        let moves = MoveChannels { moves };
        self.admin_ask(
            server_id,
            AskKind::MoveChannels(channel_id),
            |api| async move { api.move_channels(workspace_id, &moves).await },
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
    use super::placed;

    #[test]
    fn an_entry_moved_inside_its_group_takes_the_positions_of_the_entries_that_it_passes() {
        let order = [('a', 0), ('b', 4), ('c', 9), ('d', 12)];
        // Down: "a" to the place of "c", which moves up with "b".
        assert_eq!(placed(&order, 'a', 0, 2), [('b', 0), ('c', 4), ('a', 9)]);
        // Up: "d" to the first place.
        assert_eq!(
            placed(&order, 'd', 12, 0),
            [('d', 0), ('a', 4), ('b', 9), ('c', 12)]
        );
        // A drop at its own place moves nothing.
        assert!(placed(&order, 'b', 4, 1).is_empty());
        // A place after the end is the end.
        assert_eq!(placed(&order, 'c', 9, 10), [('d', 9), ('c', 12)]);
        // Equal positions: every entry gets its place.
        let order = [('a', 1), ('b', 1), ('c', 1)];
        assert_eq!(placed(&order, 'c', 1, 0), [('c', 0), ('b', 2)]);
    }

    #[test]
    fn an_entry_from_another_group_takes_a_free_position_or_the_group_is_numbered_again() {
        let order = [('a', 0), ('b', 4), ('c', 5)];
        // Between "a" and "b" a position is free.
        assert_eq!(placed(&order, 'x', 7, 1), [('x', 1)]);
        // At the end, and in an empty group.
        assert_eq!(placed(&order, 'x', 7, 3), [('x', 6)]);
        assert_eq!(placed::<char>(&[], 'x', 7, 0), [('x', 0)]);
        // The entry is named although its position stays: its group changes.
        assert_eq!(placed(&order, 'x', 6, 3), [('x', 6)]);
        // Before the first one, a position below it that is not negative.
        assert_eq!(placed(&[('a', 3)], 'x', 9, 0), [('x', 2)]);
        // No free position between "b" and "c", and none before position 0: the group is
        // numbered again.
        assert_eq!(placed(&order, 'x', 7, 2), [('b', 1), ('x', 2), ('c', 3)]);
        assert_eq!(
            placed(&order, 'x', 7, 0),
            [('x', 0), ('a', 1), ('b', 2), ('c', 3)]
        );
        // Equal positions in the group it goes to.
        assert_eq!(
            placed(&[('a', 2), ('b', 2)], 'x', 0, 2),
            [('a', 0), ('b', 1), ('x', 2)]
        );
    }
}
