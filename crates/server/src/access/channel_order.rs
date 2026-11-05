//! The positions of channels that a person reads (ADR 0304, `access::order`): the stored
//! positions of a group for a person who sees every channel of it, and otherwise the place
//! among the channels of the same category, or of the root, that the person sees in the
//! lists, counted from 0, so that a gap does not tell how many channels are hidden.

use std::collections::HashMap;

use super::order::{Held, read_positions};
use super::{Access, ChannelRecord};
use crate::error::ApiResult;
use junto_authz::{ChannelScope, Request, WorkspacePermission, WorkspaceScope};
use junto_protocol::ids::{CategoryId, ChannelId, WorkspaceId};
use sqlx::PgConnection;

/// One channel of a workspace that is not deleted, in the stored order: its group, its
/// stored position, whether the person sees it in the lists, which is not the case for an
/// archived channel, and whether the person may see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placed {
    pub id: ChannelId,
    pub category_id: Option<CategoryId>,
    pub position: i32,
    pub listed: bool,
    pub viewable: bool,
}

impl Placed {
    pub fn held(&self) -> Held<ChannelId> {
        Held {
            id: self.id,
            position: self.position,
            listed: self.listed,
            viewable: self.viewable,
        }
    }
}

impl Access {
    /// Whether the person sees the channel in the lists.
    pub fn listed(&self, record: &ChannelRecord, scope: &ChannelScope) -> bool {
        record.deleted_at.is_none() && record.archived_at.is_none() && self.viewable(scope)
    }

    fn viewable(&self, scope: &ChannelScope) -> bool {
        self.allows(Request::Channel {
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
                position: record.channel.position,
                listed: self.listed(record, scope),
                viewable: self.viewable(scope),
            })
            .collect())
    }

    /// The position that the person reads of one channel, or 0 for a channel that the
    /// person does not see in the lists, such as an archived one.
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

/// The channels of one group, in the stored order.
pub fn group_of(order: &[Placed], category_id: Option<CategoryId>) -> Vec<Held<ChannelId>> {
    order
        .iter()
        .filter(|placed| placed.category_id == category_id)
        .map(Placed::held)
        .collect()
}

/// The position that the person reads of each channel that the person sees.
pub fn channel_ranks(order: &[Placed]) -> HashMap<ChannelId, i32> {
    let mut groups: Vec<Option<CategoryId>> =
        order.iter().map(|placed| placed.category_id).collect();
    groups.sort();
    groups.dedup();
    groups
        .into_iter()
        .flat_map(|group| read_positions(&group_of(order, group)))
        .collect()
}
