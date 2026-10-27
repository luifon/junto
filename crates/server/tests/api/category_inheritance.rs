use crate::support::{Member, TestServer};
use junto_protocol::WorkspacePermission as W;
use junto_protocol::ids::WorkspaceRoleId;
use junto_protocol::structure::{Category, Channel, ChannelOverride};
use reqwest::StatusCode;
use serde_json::{Value, json};

async fn fixture() -> (TestServer, Member, Category, Channel, WorkspaceRoleId) {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let workspace = server.default_workspace(&owner).await;
    let category: Category = server
        .post(
            &owner,
            &format!("/api/v1/workspaces/{}/categories", workspace.id),
            &json!({"name":"Plans"}),
        )
        .await
        .ok(StatusCode::CREATED);
    let channel: Channel = server
        .post(
            &owner,
            &format!("/api/v1/workspaces/{}/channels", workspace.id),
            &json!({"name":"Notes","kind":"text","category_id":category.id}),
        )
        .await
        .ok(StatusCode::CREATED);
    let role = server.everyone_role(&owner, workspace.id).await;
    sqlx::query("insert into category_overrides(category_id,role_id,workspace_id,allow,deny) values($1,$2,$3,0,$4)")
        .bind(category.id).bind(role.id).bind(workspace.id).bind(1_i64 << W::SendMessages as u8).execute(&server.db).await.unwrap();
    sqlx::query("insert into category_person_overrides(category_id,account_id,workspace_id,allow,deny) values($1,$2,$3,$4,$5)")
        .bind(category.id).bind(reader.account_id).bind(workspace.id).bind(1_i64 << W::SendMessages as u8).bind(1_i64 << W::AttachFiles as u8).execute(&server.db).await.unwrap();
    (server, owner, category, channel, role.id)
}

#[tokio::test]
async fn a_synced_channel_lists_its_inherited_rules() {
    let (server, owner, _, channel, role) = fixture().await;
    let rules: Vec<ChannelOverride> = server
        .get(
            &owner,
            &format!("/api/v1/channels/{}/overrides", channel.id),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].role_id, role);
    assert_eq!(rules[0].deny, [W::SendMessages]);
}

#[tokio::test]
async fn an_identical_inherited_rule_keeps_the_channel_synced() {
    let (server, owner, _, channel, role) = fixture().await;
    server
        .put(
            &owner,
            &format!("/api/v1/channels/{}/overrides/{role}", channel.id),
            &json!({"allow":[],"deny":["send_messages"]}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    let rows: i64 =
        sqlx::query_scalar("select count(*) from channel_overrides where channel_id=$1")
            .bind(channel.id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(rows, 0);
    let synced: bool = sqlx::query_scalar("select permissions_synced from channels where id=$1")
        .bind(channel.id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert!(synced);
}

#[tokio::test]
async fn changing_one_inherited_rule_copies_every_role_and_person_rule() {
    let (server, owner, category, channel, role) = fixture().await;
    let changed: ChannelOverride = server
        .patch(
            &owner,
            &format!("/api/v1/channels/{}/overrides/{role}", channel.id),
            &json!({"permission":"add_reactions","state":"deny"}),
        )
        .await
        .ok(StatusCode::OK);
    assert!(changed.deny.contains(&W::SendMessages));
    assert!(changed.deny.contains(&W::AddReactions));
    let synced: bool = sqlx::query_scalar("select permissions_synced from channels where id=$1")
        .bind(channel.id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert!(!synced);
    let people: i64 =
        sqlx::query_scalar("select count(*) from channel_person_overrides where channel_id=$1")
            .bind(channel.id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(people, 1);
    sqlx::query("delete from category_overrides where category_id=$1")
        .bind(category.id)
        .execute(&server.db)
        .await
        .unwrap();
    let deny: i64 = sqlx::query_scalar(
        "select deny from effective_channel_overrides where channel_id=$1 and role_id=$2",
    )
    .bind(channel.id)
    .bind(role)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(
        deny,
        (1_i64 << W::SendMessages as u8) | (1_i64 << W::AddReactions as u8)
    );
}

#[tokio::test]
async fn deleting_a_category_keeps_its_child_permissions() {
    let (server, owner, category, channel, role) = fixture().await;
    server
        .delete(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/categories/{}",
                category.workspace_id, category.id
            ),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let deny: Option<i64> = sqlx::query_scalar(
        "select deny from effective_channel_overrides where channel_id=$1 and role_id=$2",
    )
    .bind(channel.id)
    .bind(role)
    .fetch_optional(&server.db)
    .await
    .unwrap();
    assert_eq!(deny, Some(1_i64 << W::SendMessages as u8));
    let people: i64 = sqlx::query_scalar(
        "select count(*) from effective_channel_person_overrides where channel_id=$1",
    )
    .bind(channel.id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(people, 1);
}

#[tokio::test]
async fn a_fork_preserves_inherited_person_rules_when_the_category_changes() {
    let (server, owner, category, channel, _) = fixture().await;
    let reader = server.join(&owner, "Another reader").await;
    sqlx::query("insert into category_person_overrides(category_id,account_id,workspace_id,allow,deny) values($1,$2,$3,0,1)")
        .bind(category.id).bind(reader.account_id).bind(category.workspace_id).execute(&server.db).await.unwrap();
    let root = crate::messages_support::send(&server, &owner, &channel, "Root").await;
    let copied: Channel = server
        .post(
            &owner,
            &format!("/api/v1/messages/{}/fork", root.id),
            &json!({"name":"Copy","kind":"text"}),
        )
        .await
        .ok(StatusCode::CREATED);
    sqlx::query("delete from category_person_overrides where category_id=$1")
        .bind(category.id)
        .execute(&server.db)
        .await
        .unwrap();
    server
        .get(&reader, &format!("/api/v1/channels/{}/messages", copied.id))
        .await
        .error(junto_protocol::error::ErrorCode::NotFound);
}
