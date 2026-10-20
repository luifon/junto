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
use crate::state::receipts::Awaited;

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
/// `order` in their shown order, with the fewest changed entries. Empty when nothing
/// moves. See [`placings`].
pub fn placed<Id: Copy + PartialEq>(
    order: &[(Id, i32)],
    moving: Id,
    held: i32,
    to: usize,
) -> Vec<(Id, i32)> {
    placings(order, moving, held, to)
        .into_iter()
        .next()
        .unwrap_or_default()
}

/// The ways to put `moving` at the place `to` of a group, whose entries are `order` in
/// their shown order, each as the new positions of the entries that change, the way with
/// the fewest changes first. `order` holds `moving` when it moves inside its group, and
/// `held` is its position before. Empty when nothing moves.
///
/// When the other entries have distinct positions, `moving` takes a free position between
/// its new neighbours, and no other entry changes. Without one, it takes the position after
/// the entry before it, and the entries after it move up by one until the first free
/// position; or it takes the position before the entry after it, and the entries before
/// it move down by one until the first free position. A position can be below 0, so the
/// first place is always free. The position that `moving` leaves in its own group is
/// free. An entry that comes from another group is
/// named also when its position stays, because its group changes. When two other entries
/// share a position, every entry of the group gets its place as its position.
pub fn placings<Id: Copy + PartialEq>(
    order: &[(Id, i32)],
    moving: Id,
    held: i32,
    to: usize,
) -> Vec<Vec<(Id, i32)>> {
    let from = order.iter().position(|(id, _)| *id == moving);
    let others: Vec<(Id, i32)> = order
        .iter()
        .copied()
        .filter(|(id, _)| *id != moving)
        .collect();
    let at = to.min(others.len());
    if from == Some(at) {
        return Vec::new();
    }
    if !others.windows(2).all(|pair| pair[0].1 < pair[1].1) {
        let mut next = others;
        next.insert(at, (moving, held));
        return vec![renumbered(&next, order)];
    }
    let before = at.checked_sub(1).map(|place| others[place].1);
    let after = others.get(at).map(|(_, position)| *position);
    let mut ways: Vec<Vec<(Id, i32)>> = Vec::new();
    let free = match (before, after) {
        (None, None) => Some(0),
        (Some(before), None) => before.checked_add(1),
        (None, Some(after)) => after.checked_sub(1),
        (Some(before), Some(after)) => before.checked_add(1).filter(|position| *position < after),
    };
    if let Some(position) = free {
        ways.push(vec![(moving, position)]);
    }
    // The entries after the place move up.
    if let Some(start) = before.map_or(Some(0), |before| before.checked_add(1)) {
        let mut way = vec![(moving, start)];
        let mut last = Some(start);
        for (id, position) in &others[at..] {
            match last {
                Some(below) if *position > below => break,
                Some(below) => {
                    last = below.checked_add(1);
                    if let Some(next) = last {
                        way.push((*id, next));
                    }
                }
                None => break,
            }
        }
        if last.is_some() {
            ways.push(way);
        }
    }
    // The entries before the place move down.
    if let Some(start) = after.and_then(|after| after.checked_sub(1)) {
        let mut way = vec![(moving, start)];
        let mut first = Some(start);
        for (id, position) in others[..at].iter().rev() {
            match first {
                Some(above) if *position < above => break,
                Some(above) => {
                    first = above.checked_sub(1);
                    if let Some(next) = first {
                        way.push((*id, next));
                    }
                }
                None => break,
            }
        }
        if first.is_some() {
            ways.push(way);
        }
    }
    ways.sort_by_key(Vec::len);
    ways.dedup();
    ways
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
        // The page shows the moves from now until the state holds them, or until the
        // server refuses them (ADR 0730).
        let number = self
            .admin
            .channel_moves
            .begin(server_id, workspace_id, moves.clone());
        let moves = MoveChannels { moves };
        let (told, outcome) = futures_channel::oneshot::channel();
        self.admin_ask(
            server_id,
            AskKind::MoveChannels(channel_id),
            |api| async move {
                let answer = api.move_channels(workspace_id, &moves).await;
                let _ = told.send(answer.as_ref().ok().copied());
                answer
            },
            |_, _, _| {},
            cx,
        );
        // The page tells the refusal of its newest request only. The moves of every
        // failed request wait no more, also of one that the page tells nothing about.
        cx.spawn(async move |this, cx| {
            let receipt = outcome.await.ok().flatten();
            let _ = this.update(cx, |this, cx| {
                match receipt {
                    Some(receipt) => {
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
                    None => this.admin.channel_moves.failed(number),
                }
                cx.notify();
            });
        })
        .detach();
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
    use super::{placed, placings};

    #[test]
    fn an_entry_moved_inside_its_group_changes_the_fewest_entries() {
        let order = [('a', 0), ('b', 4), ('c', 9), ('d', 12)];
        // Down: "a" after "c", where a position is free.
        assert_eq!(placed(&order, 'a', 0, 2), [('a', 10)]);
        // Up: "d" to the first place, below the first one.
        assert_eq!(placed(&order, 'd', 12, 0), [('d', -1)]);
        // A drop at its own place moves nothing.
        assert!(placed(&order, 'b', 4, 1).is_empty());
        assert!(placings(&order, 'b', 4, 1).is_empty());
        // A place after the end is the end.
        assert_eq!(placed(&order, 'c', 9, 10), [('c', 13)]);
        // Without a free position the entries after the place move up to the first free
        // one, or the entries before it move down to the position that the entry left.
        let order = [('a', 0), ('b', 1), ('c', 2), ('d', 3)];
        assert_eq!(
            placings(&order, 'a', 0, 2),
            [vec![('a', 3), ('d', 4)], vec![('a', 2), ('c', 1), ('b', 0)]]
        );
        assert_eq!(
            placings(&order, 'c', 2, 0),
            [vec![('c', -1)], vec![('c', 0), ('a', 1), ('b', 2)]]
        );
        // Equal positions: every entry gets its place.
        let order = [('a', 1), ('b', 1), ('c', 1)];
        assert_eq!(placed(&order, 'c', 1, 0), [('c', 0), ('b', 2)]);
    }

    #[test]
    fn a_move_at_the_end_of_a_long_group_changes_one_entry() {
        let order: Vec<(u32, i32)> = (0..=200).map(|at| (at, at as i32)).collect();
        assert_eq!(placed(&order, 0, 0, 200), [(0, 201)]);
        assert_eq!(placed(&order, 200, 200, 0), [(200, -1)]);
        // In the middle of a run without a free position, each way moves the entries to the
        // next free position: here 200 entries each.
        let order: Vec<(u32, i32)> = (0..=400).map(|at| (at, at as i32)).collect();
        let ways = placings(&order, 0, 0, 200);
        assert_eq!(ways.len(), 2);
        assert!(
            ways.iter().all(|way| way.len() == 201),
            "{:?}",
            ways[0].len()
        );
    }

    #[test]
    fn an_entry_from_another_group_takes_a_free_position_or_moves_the_fewest_entries() {
        let order = [('a', 0), ('b', 4), ('c', 5)];
        // Between "a" and "b" a position is free.
        assert_eq!(placed(&order, 'x', 7, 1), [('x', 1)]);
        // At the end, and in an empty group.
        assert_eq!(placed(&order, 'x', 7, 3), [('x', 6)]);
        assert_eq!(placed::<char>(&[], 'x', 7, 0), [('x', 0)]);
        // The entry is named although its position stays: its group changes.
        assert_eq!(placed(&order, 'x', 6, 3), [('x', 6)]);
        // Before the first one, the position below it.
        assert_eq!(placed(&[('a', 3)], 'x', 9, 0), [('x', 2)]);
        assert_eq!(placed(&[('a', 0)], 'x', 9, 0), [('x', -1)]);
        // No free position between "b" and "c": "c" moves up, or "b" moves down.
        assert_eq!(
            placings(&order, 'x', 7, 2),
            [vec![('x', 5), ('c', 6)], vec![('x', 4), ('b', 3)]]
        );
        // The first place.
        assert_eq!(placed(&order, 'x', 7, 0), [('x', -1)]);
        // Equal positions in the group it goes to.
        assert_eq!(
            placed(&[('a', 2), ('b', 2)], 'x', 0, 2),
            [('a', 0), ('b', 1), ('x', 2)]
        );
        // A position at the bound of the type is not passed.
        assert_eq!(
            placings(&[('a', i32::MAX)], 'x', 0, 1),
            Vec::<Vec<(char, i32)>>::new()
        );
    }
}
