//! The requests of the administration that change the structure of a server: workspaces,
//! categories, channels, the archive and the retention.

use gpui::Context;
use junto_client_core::api::{ApiError, ErrorCategory, ServerApi};
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

/// The new positions that put `moving` at the place `to` of a group, whose entries are
/// `order` in their shown order, with the fewest changed entries. Empty when nothing
/// moves. See [`placings`].
pub fn placed<Id: Copy + Ord>(order: &[(Id, i32)], moving: Id, to: usize) -> Vec<(Id, i32)> {
    placings(order, moving, to)
        .into_iter()
        .next()
        .unwrap_or_default()
}

/// The ways to put `moving` at the place `to` of a group, each as the new positions of the
/// entries that change, the way with the fewest changes first. `order` holds the entries in
/// their shown order, which is the order of their position and then of their identifier, as
/// the state and the server order them; it holds `moving` when `moving` moves inside its
/// group. Empty when nothing moves.
///
/// `moving` takes a position next to one of its new neighbours: the position of the entry
/// before it or one above, or the position of the entry after it or one below; with no
/// neighbour, position 0. An entry after it whose order would no longer follow moves up to
/// the lowest position at which it follows the entry before it: that entry's position when
/// its identifier comes after, else the one above; and so on until the order follows. An
/// entry before it moves down in the same way. A way that would pass the bounds of a 32-bit
/// position is not offered. The position that `moving` leaves in its own group is free, and
/// two entries can share a position, which their identifiers order. Of two ways with as
/// many changes, the one that gives `moving` a position of its own comes first, and then
/// the one that moves the entries after it.
pub fn placings<Id: Copy + Ord>(order: &[(Id, i32)], moving: Id, to: usize) -> Vec<Vec<(Id, i32)>> {
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
    let (before, after) = (&others[..at], &others[at..]);
    let key = |(id, position): (Id, i32)| (position, id);
    let mut starts: Vec<i32> = Vec::new();
    match (before.last(), after.first()) {
        (None, None) => starts.push(0),
        (previous, next) => {
            if let Some((_, position)) = previous {
                starts.extend(position.checked_add(1));
            }
            if let Some((_, position)) = next {
                starts.extend(position.checked_sub(1));
            }
            starts.extend(previous.map(|(_, position)| *position));
            starts.extend(next.map(|(_, position)| *position));
        }
    }
    starts.dedup();
    let mut ways: Vec<(Vec<(Id, i32)>, bool)> = Vec::new();
    for start in starts {
        let mut way = vec![(moving, start)];
        let mut fits = true;
        // The entries after the place move up while they would not follow.
        let mut last = (moving, start);
        for entry in after {
            if key(*entry) > key(last) {
                break;
            }
            // The lowest position at which the entry follows: the position of the entry
            // before it when its identifier comes after, else the one above.
            let lowest = if entry.0 > last.0 {
                Some(last.1)
            } else {
                last.1.checked_add(1)
            };
            match lowest {
                Some(position) => {
                    last = (entry.0, position);
                    way.push(last);
                }
                None => {
                    fits = false;
                    break;
                }
            }
        }
        // The entries before it move down while they would not come before.
        let mut first = (moving, start);
        for entry in before.iter().rev() {
            if !fits || key(*entry) < key(first) {
                break;
            }
            let highest = if entry.0 < first.0 {
                Some(first.1)
            } else {
                first.1.checked_sub(1)
            };
            match highest {
                Some(position) => {
                    first = (entry.0, position);
                    way.push(first);
                }
                None => {
                    fits = false;
                    break;
                }
            }
        }
        if !fits {
            continue;
        }
        let shared = others.iter().any(|(_, position)| *position == start);
        ways.push((way, shared));
    }
    ways.sort_by_key(|(way, shared)| (way.len(), *shared));
    let mut found: Vec<Vec<(Id, i32)>> = Vec::with_capacity(ways.len());
    for (way, _) in ways {
        if !found.contains(&way) {
            found.push(way);
        }
    }
    found
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
        let to = if up {
            index.checked_sub(1)
        } else {
            Some(index + 1)
        };
        let Some(to) = to else {
            return;
        };
        let changes = placed(&order, category_id, to);
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

#[cfg(test)]
mod tests {
    use super::{placed, placings};

    #[test]
    fn an_entry_moved_inside_its_group_changes_the_fewest_entries() {
        let order = [('a', 0), ('b', 4), ('c', 9), ('d', 12)];
        // Down: "a" after "c", where a position is free.
        assert_eq!(placed(&order, 'a', 2), [('a', 10)]);
        // Up: "d" to the first place, below the first one.
        assert_eq!(placed(&order, 'd', 0), [('d', -1)]);
        // A drop at its own place moves nothing.
        assert!(placings(&order, 'b', 1).is_empty());
        // A place after the end is the end.
        assert_eq!(placed(&order, 'c', 10), [('c', 13)]);
    }

    #[test]
    fn an_entry_shares_a_position_when_the_identifiers_keep_the_order() {
        // No position is free between "b" at 1 and "c" at 2. "x" after "b" at position 1
        // still comes after it, because the identifiers order equal positions.
        let order = [('a', 0), ('b', 1), ('c', 2)];
        assert_eq!(placed(&order, 'x', 2), [('x', 1)]);
        // 201 entries at position 0: one more after them takes position 1 alone, and the
        // first of them goes to the end alone.
        let order: Vec<(u32, i32)> = (0..=200).map(|id| (id, 0)).collect();
        assert_eq!(placed(&order, 1000, 201), [(1000, 1)]);
        assert_eq!(placed(&order, 0, 200), [(0, 1)]);
        // Equal positions in the group it goes to: only "x" changes.
        assert_eq!(placed(&[('a', 2), ('b', 2)], 'x', 2), [('x', 3)]);
    }

    #[test]
    fn a_moved_entry_keeps_a_shared_position_when_the_identifiers_allow_it() {
        // "a" and "b" share position 0, "c" is at 1. "z" between "a" and "b" takes 0; "b"
        // must follow "z" and moves to 1, where it comes before "c": "c" stays.
        let order = [('a', 0), ('b', 0), ('c', 1)];
        let ways = placings(&order, 'z', 1);
        assert!(ways.contains(&vec![('z', 0), ('b', 1)]), "{ways:?}");
        assert!(ways.iter().all(|way| way.len() >= 2), "{ways:?}");
        // "c" between "b" and "d", both at 6: at 6 nothing else changes. Above, "d" keeps
        // the position of "c"; below, "b" takes it.
        let ways = placings(&[('b', 6), ('d', 6)], 'c', 1);
        assert_eq!(ways[0], [('c', 6)]);
        assert!(ways.contains(&vec![('c', 7), ('d', 7)]), "{ways:?}");
        assert!(ways.contains(&vec![('c', 5), ('b', 5)]), "{ways:?}");
    }

    #[test]
    fn without_a_free_place_the_entries_move_to_the_next_free_position() {
        // "b" fits neither at the position of "c" before it nor at the one of "a" after it:
        // "a" moves up, or "c" moves down.
        let order = [('c', 1), ('a', 2)];
        assert_eq!(
            placings(&order, 'b', 1),
            [vec![('b', 2), ('a', 3)], vec![('b', 1), ('c', 0)]]
        );
        // A run of consecutive positions: the entries after the place move up to the next
        // free position, here 200 of them.
        let order: Vec<(u32, i32)> = (0..=400).map(|at| (2 * (1000 - at), at as i32)).collect();
        let ways = placings(&order, 1599, 201);
        assert_eq!(ways[0].len(), 201, "{:?}", &ways[0][..3]);
        assert!(ways.iter().all(|way| way.len() > 200));
    }

    #[test]
    fn a_move_at_the_bounds_of_a_position_changes_the_fewest_entries() {
        // After an entry at the highest position: the same position, or that entry one
        // below.
        assert_eq!(placed(&[('a', i32::MAX)], 'b', 1), [('b', i32::MAX)]);
        assert_eq!(
            placed(&[('c', i32::MAX)], 'b', 1),
            [('b', i32::MAX), ('c', i32::MAX - 1)]
        );
        // Before entries at the lowest positions: only the first one moves up.
        let order = [('a', i32::MIN), ('c', i32::MIN + 1), ('d', i32::MIN + 2)];
        assert_eq!(
            placed(&order, 'b', 0),
            [('b', i32::MIN), ('a', i32::MIN + 1)]
        );
        // A way that would pass the bound is not offered.
        let order = [('c', i32::MAX - 1), ('a', i32::MAX)];
        assert_eq!(
            placings(&order, 'b', 1),
            [vec![('b', i32::MAX - 1), ('c', i32::MAX - 2)]]
        );
    }

    #[test]
    fn an_entry_from_another_group_takes_a_free_position_or_moves_the_fewest_entries() {
        let order = [('a', 0), ('b', 4), ('c', 5)];
        // Between "a" and "b" a position is free.
        assert_eq!(placed(&order, 'x', 1), [('x', 1)]);
        // At the end, and in an empty group.
        assert_eq!(placed(&order, 'x', 3), [('x', 6)]);
        assert_eq!(placed::<char>(&[], 'x', 0), [('x', 0)]);
        // Before the first one, the position below it.
        assert_eq!(placed(&[('a', 3)], 'x', 0), [('x', 2)]);
        assert_eq!(placed(&order, 'x', 0), [('x', -1)]);
    }
}
