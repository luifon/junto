//! The places of channels that a person reads (ADR 0304). The position of a channel that a
//! person reads is its place among the channels of the same category, or of the root, that
//! the person sees in the lists, counted from 0, so that a gap does not tell how many
//! channels are hidden.

use std::collections::HashMap;

use super::{Access, ChannelRecord};
use crate::error::ApiResult;
use junto_authz::{ChannelScope, Request, WorkspacePermission, WorkspaceScope};
use junto_protocol::ids::{CategoryId, ChannelId, WorkspaceId};
use sqlx::PgConnection;

/// One channel of a workspace that is not deleted, in the stored order: its group, and
/// whether the person sees it in the lists, which is not the case for an archived channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placed {
    pub id: ChannelId,
    pub category_id: Option<CategoryId>,
    pub listed: bool,
}

impl Access {
    /// Whether the person sees the channel in the lists.
    pub fn listed(&self, record: &ChannelRecord, scope: &ChannelScope) -> bool {
        record.deleted_at.is_none()
            && record.archived_at.is_none()
            && self.allows(Request::Channel {
                scope,
                permission: WorkspacePermission::ViewChannel,
            })
    }

    /// Every channel of a workspace that is not deleted, in the stored order.
    pub async fn channel_order(
        &self,
        connection: &mut PgConnection,
        workspace_id: WorkspaceId,
        workspace: &WorkspaceScope,
    ) -> ApiResult<Vec<Placed>> {
        Ok(self
            .workspace_channels(connection, workspace_id, workspace)
            .await?
            .iter()
            .filter(|(record, _)| record.deleted_at.is_none())
            .map(|(record, scope)| Placed {
                id: record.channel.id,
                category_id: record.channel.category_id,
                listed: self.listed(record, scope),
            })
            .collect())
    }

    /// The place of one channel among the channels of its group that the person sees, or 0
    /// for a channel that the person does not see in the lists, such as an archived one.
    pub async fn channel_place(
        &self,
        connection: &mut PgConnection,
        workspace_id: WorkspaceId,
        channel_id: ChannelId,
    ) -> ApiResult<i32> {
        let workspace = self.workspace(connection, workspace_id).await?;
        let order = self
            .channel_order(connection, workspace_id, &workspace)
            .await?;
        Ok(channel_ranks(&order).get(&channel_id).copied().unwrap_or(0))
    }
}

/// The place of each channel that the person sees among the channels of its group that the
/// person sees.
pub fn channel_ranks(order: &[Placed]) -> HashMap<ChannelId, i32> {
    let mut next: HashMap<Option<CategoryId>, i32> = HashMap::new();
    order
        .iter()
        .filter(|placed| placed.listed)
        .map(|placed| {
            let rank = next.entry(placed.category_id).or_insert(0);
            let place = *rank;
            *rank += 1;
            (placed.id, place)
        })
        .collect()
}
