use crate::support::TestServer;
use junto_authz::{Request, ServerPermission as S, WorkspacePermission as W};
use junto_protocol::ids::ChannelId;
use junto_protocol::structure::{Category, Channel};
use junto_server::access::Access;
use reqwest::StatusCode;
use serde_json::json;

#[tokio::test]
async fn server_person_rules_are_loaded_with_role_grants() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let member = server.join(&owner, "Reader").await;
    server
        .grant_server(&owner, &member, &[S::CreateInvites])
        .await;
    sqlx::query("insert into server_person_overrides(account_id,allow,deny) values($1,$2,$3)")
        .bind(member.account_id)
        .bind(1_i64 << S::BanMembers as u8)
        .bind(1_i64 << S::CreateInvites as u8)
        .execute(&server.db)
        .await
        .unwrap();
    let mut connection = server.db.acquire().await.unwrap();
    let access = Access::of_account(&mut connection, member.account_id)
        .await
        .unwrap()
        .unwrap();
    assert!(access.allows(Request::Server(S::BanMembers)));
    assert!(!access.allows(Request::Server(S::CreateInvites)));
}

#[tokio::test]
async fn workspace_person_rules_are_loaded_after_roles() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let member = server.join(&owner, "Reader").await;
    let workspace = server.default_workspace(&owner).await;
    sqlx::query("insert into workspace_person_overrides(workspace_id,account_id,allow,deny) values($1,$2,$3,$4)")
        .bind(workspace.id).bind(member.account_id).bind(1_i64 << W::ManageChannel as u8).bind(1_i64 << W::SendMessages as u8)
        .execute(&server.db).await.unwrap();
    let mut connection = server.db.acquire().await.unwrap();
    let access = Access::of_account(&mut connection, member.account_id)
        .await
        .unwrap()
        .unwrap();
    let scope = access
        .workspace(&mut connection, workspace.id)
        .await
        .unwrap();
    assert!(access.allows(Request::Workspace {
        scope: &scope,
        permission: W::ManageChannel
    }));
    assert!(!access.allows(Request::Workspace {
        scope: &scope,
        permission: W::SendMessages
    }));
}

#[tokio::test]
async fn inherited_person_and_role_rules_agree_for_single_and_batch_channel_loads() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let member = server.join(&owner, "Reader").await;
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
    sqlx::query("insert into category_overrides(category_id,role_id,workspace_id,allow,deny) select $1,id,workspace_id,0,$3 from workspace_roles where workspace_id=$2 and is_everyone")
        .bind(category.id).bind(workspace.id).bind(1_i64 << W::ViewChannel as u8).execute(&server.db).await.unwrap();
    for person_allow in [false, true] {
        if person_allow {
            sqlx::query("insert into category_person_overrides(category_id,account_id,workspace_id,allow,deny) values($1,$2,$3,$4,$5)")
                .bind(category.id).bind(member.account_id).bind(workspace.id).bind(1_i64 << W::ViewChannel as u8).bind(1_i64 << W::SendMessages as u8).execute(&server.db).await.unwrap();
        }
        let mut connection = server.db.acquire().await.unwrap();
        let access = Access::of_account(&mut connection, member.account_id)
            .await
            .unwrap()
            .unwrap();
        let workspace_scope = access
            .workspace(&mut connection, workspace.id)
            .await
            .unwrap();
        let places = junto_server::access::visibility::places(&mut connection, &access)
            .await
            .unwrap();
        assert_eq!(places.channels.contains(&channel.id), person_allow);
        let single = access.channel(&mut connection, channel.id).await.unwrap();
        let named = access
            .named_channels(
                &mut connection,
                workspace.id,
                &workspace_scope,
                &[channel.id],
            )
            .await
            .unwrap();
        let all = access
            .workspace_channels(&mut connection, workspace.id, &workspace_scope)
            .await
            .unwrap();
        for (_, scope) in std::iter::once(&single).chain(named.iter()).chain(
            all.iter()
                .filter(|(record, _)| record.channel.id == channel.id),
        ) {
            assert_eq!(
                access.allows(Request::Channel {
                    scope,
                    permission: W::ViewChannel
                }),
                person_allow
            );
            assert!(!access.allows(Request::Channel {
                scope,
                permission: W::SendMessages
            }));
        }
    }
    sqlx::query("update channels set permissions_synced=false where id=$1")
        .bind(channel.id)
        .execute(&server.db)
        .await
        .unwrap();
    sqlx::query("insert into channel_person_overrides(channel_id,account_id,workspace_id,allow,deny) values($1,$2,$3,0,$4)")
        .bind(channel.id).bind(member.account_id).bind(workspace.id).bind(1_i64 << W::ViewChannel as u8).execute(&server.db).await.unwrap();
    let list: junto_protocol::structure::ChannelList = server
        .get(
            &member,
            &format!("/api/v1/workspaces/{}/channels", workspace.id),
        )
        .await
        .ok(StatusCode::OK);
    let ids: Vec<ChannelId> = list.channels.iter().map(|channel| channel.id).collect();
    assert!(!ids.contains(&channel.id));
}

#[tokio::test]
async fn batch_visibility_and_contact_standing_apply_person_rules() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let member = server.join(&owner, "Reader").await;
    let workspace = server.default_workspace(&owner).await;
    sqlx::query("insert into workspace_person_overrides(workspace_id,account_id,allow,deny) values($1,$2,0,1)")
        .bind(workspace.id).bind(member.account_id).execute(&server.db).await.unwrap();
    let mut connection = server.db.acquire().await.unwrap();
    let access = Access::of_account(&mut connection, member.account_id)
        .await
        .unwrap()
        .unwrap();
    let places = junto_server::access::visibility::places(&mut connection, &access)
        .await
        .unwrap();
    assert!(places.channels.is_empty());
    sqlx::query("delete from workspace_members where account_id=$1")
        .bind(member.account_id)
        .execute(&mut *connection)
        .await
        .unwrap();
    sqlx::query("insert into server_person_overrides(account_id,allow,deny) values($1,1,0)")
        .bind(member.account_id)
        .execute(&mut *connection)
        .await
        .unwrap();
    let places = junto_server::access::visibility::places(&mut connection, &access)
        .await
        .unwrap();
    assert!(places.workspaces.contains(&workspace.id));
    let owner_access = Access::of_account(&mut connection, owner.account_id)
        .await
        .unwrap()
        .unwrap();
    let contacts =
        junto_server::store::contacts(&mut connection, &owner_access, &[member.account_id])
            .await
            .unwrap();
    assert_eq!(
        contacts.get(&member.account_id),
        Some(&junto_authz::Contact::Administrator)
    );
}
