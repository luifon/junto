use crate::error::ApiResult;
use junto_protocol::{
    appearance::{AccountAppearance, RoleColor},
    ids::AccountId,
    roles::{UpdateServerRole, UpdateWorkspaceRole},
};
use sqlx::PgConnection;

#[derive(Clone, Copy, Default)]
pub(crate) struct Patch {
    pub color: Option<Option<RoleColor>>,
    pub hoist: Option<bool>,
}
impl From<&UpdateServerRole> for Patch {
    fn from(role: &UpdateServerRole) -> Self {
        Self {
            color: role.color,
            hoist: role.hoist,
        }
    }
}
impl From<&UpdateWorkspaceRole> for Patch {
    fn from(role: &UpdateWorkspaceRole) -> Self {
        Self {
            color: role.color,
            hoist: role.hoist,
        }
    }
}
pub(crate) fn color(value: Option<i32>) -> Option<RoleColor> {
    value.map(|value| {
        let bytes = value.to_be_bytes();
        RoleColor::from_rgb(bytes[1], bytes[2], bytes[3])
    })
}
pub(crate) fn stored(value: Option<RoleColor>) -> Option<i32> {
    value.map(|value| value.rgb() as i32)
}
pub(crate) async fn of(
    connection: &mut PgConnection,
    account: AccountId,
) -> ApiResult<AccountAppearance> {
    let value: Option<sqlx::types::Json<AccountAppearance>> =
        sqlx::query_scalar("select account_role_appearance($1)")
            .bind(account)
            .fetch_one(connection)
            .await?;
    Ok(value.map(|value| value.0).unwrap_or_default())
}
