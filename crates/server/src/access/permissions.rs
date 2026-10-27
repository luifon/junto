use super::{Access, stored_override};
use crate::{
    db,
    error::{ApiError, ApiResult},
};
use junto_authz::{Override, ServerOverride};
use junto_protocol::ids::{AccountId, ChannelId, WorkspaceId};
use sqlx::PgConnection;
use std::collections::HashMap;

pub(crate) fn server_rule(allow: i64, deny: i64) -> ApiResult<ServerOverride> {
    ServerOverride::new(
        db::server_permissions(allow)?,
        db::server_permissions(deny)?,
    )
    .map_err(|error| ApiError::internal(&error))
}

pub(super) async fn server_person(
    connection: &mut PgConnection,
    account: AccountId,
) -> ApiResult<ServerOverride> {
    let row: Option<(i64, i64)> =
        sqlx::query_as("select allow,deny from server_person_overrides where account_id=$1")
            .bind(account)
            .fetch_optional(connection)
            .await?;
    row.map(|(allow, deny)| server_rule(allow, deny))
        .transpose()
        .map(Option::unwrap_or_default)
}

impl Access {
    pub(super) async fn workspace_person(
        &self,
        connection: &mut PgConnection,
        workspace: WorkspaceId,
    ) -> ApiResult<Override> {
        let row: Option<(i64, i64)> = sqlx::query_as(
            "select allow,deny from workspace_person_overrides where workspace_id=$1 and account_id=$2",
        ).bind(workspace).bind(self.account_id).fetch_optional(connection).await?;
        row.map(|(allow, deny)| stored_override(allow, deny))
            .transpose()
            .map(Option::unwrap_or_default)
    }

    pub(super) async fn channel_people(
        &self,
        connection: &mut PgConnection,
        channels: &[ChannelId],
    ) -> ApiResult<HashMap<ChannelId, Override>> {
        let rows: Vec<(ChannelId,i64,i64)> = sqlx::query_as(
            "select channel_id,allow,deny from effective_channel_person_overrides where channel_id=any($1) and account_id=$2",
        ).bind(channels).bind(self.account_id).fetch_all(connection).await?;
        rows.into_iter()
            .map(|(id, allow, deny)| Ok((id, stored_override(allow, deny)?)))
            .collect()
    }
}
