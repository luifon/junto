use crate::error::ApiResult;
use junto_protocol::ids::ChannelId;
use sqlx::PgConnection;

pub(crate) async fn detach_channels(
    connection: &mut PgConnection,
    channels: &[ChannelId],
) -> ApiResult<()> {
    sqlx::query("select id from channels where id=any($1) order by id for update")
        .bind(channels)
        .fetch_all(&mut *connection)
        .await?;
    for statement in [
        "delete from channel_overrides o using channels c where c.id=o.channel_id and c.id=any($1) and c.permissions_synced and c.category_id is not null",
        "delete from channel_person_overrides o using channels c where c.id=o.channel_id and c.id=any($1) and c.permissions_synced and c.category_id is not null",
    ] {
        sqlx::query(statement)
            .bind(channels)
            .execute(&mut *connection)
            .await?;
    }
    sqlx::query(
        "insert into channel_overrides(channel_id,role_id,workspace_id,allow,deny)
        select c.id,o.role_id,c.workspace_id,o.allow,o.deny from channels c
        join category_overrides o on o.category_id=c.category_id
        where c.id=any($1) and c.permissions_synced",
    )
    .bind(channels)
    .execute(&mut *connection)
    .await?;
    sqlx::query(
        "insert into channel_person_overrides(channel_id,account_id,workspace_id,allow,deny)
        select c.id,o.account_id,c.workspace_id,o.allow,o.deny from channels c
        join category_person_overrides o on o.category_id=c.category_id
        where c.id=any($1) and c.permissions_synced",
    )
    .bind(channels)
    .execute(&mut *connection)
    .await?;
    sqlx::query(
        "update channels set permissions_synced=false where id=any($1) and permissions_synced",
    )
    .bind(channels)
    .execute(connection)
    .await?;
    Ok(())
}
