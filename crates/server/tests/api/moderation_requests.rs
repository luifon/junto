use crate::calls_support::Scene;
use crate::support::TestServer;
use junto_protocol::audit::{AuditEvent, AuditPage};
use junto_protocol::error::ErrorCode;
use junto_protocol::structure::Workspace;
use junto_protocol::{ServerPermission as S, WorkspacePermission as W};
use reqwest::StatusCode;
use serde_json::{Value, json};

#[tokio::test]
async fn reasons_are_bounded_and_audited() {
    for removal in [false, true] {
        let server = TestServer::start().await;
        let owner = server.owner().await;
        let guest = server.join(&owner, "Guest").await;
        let route = format!(
            "/api/v1/accounts/{}/{}",
            guest.account_id,
            if removal { "removal" } else { "ban" }
        );
        for reason in ["x".repeat(1001), "line\nbreak".into()] {
            let body = json!({"reason":reason});
            let reply = if removal {
                server.post(&owner, &route, &body).await
            } else {
                server.put(&owner, &route, &body).await
            };
            reply.error(ErrorCode::InvalidRequest);
        }
        for body in [
            json!({"reason":"ok","unknown":true}),
            json!([]),
            json!("reason"),
            json!(42),
        ] {
            let reply = if removal {
                server.post(&owner, &route, &body).await
            } else {
                server.put(&owner, &route, &body).await
            };
            reply.error(ErrorCode::InvalidRequest);
        }
        let reason = "é".repeat(1000);
        let body = json!({"reason":format!("  {reason}  ")});
        let reply = if removal {
            server.post(&owner, &route, &body).await
        } else {
            server.put(&owner, &route, &body).await
        };
        reply.empty(StatusCode::NO_CONTENT);
        let log: AuditPage = server
            .get(&owner, "/api/v1/audit-log")
            .await
            .ok(StatusCode::OK);
        match &log.entries[0].event {
            AuditEvent::AccountRemoved { reason: actual, .. }
            | AuditEvent::AccountBanned { reason: actual, .. } => {
                assert_eq!(actual.as_deref(), Some(reason.as_str()))
            }
            event => panic!("unexpected {event:?}"),
        }
        server
            .get(&guest, "/api/v1/audit-log")
            .await
            .error(ErrorCode::Unauthenticated);
        let events: Vec<Value> = sqlx::query_scalar("select payload from events")
            .fetch_all(&server.db)
            .await
            .unwrap();
        assert!(events.iter().all(|e| !e.to_string().contains(&reason)));
    }
}

#[tokio::test]
async fn workspace_removal_is_ranked_and_scoped() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let high = server.join(&owner, "High").await;
    let low = server.join(&owner, "Low").await;
    let guest = server.join(&owner, "Guest").await;
    let workspace = server.default_workspace(&owner).await;
    let other: Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({"name":"Other"}))
        .await
        .ok(StatusCode::CREATED);
    server
        .put_empty(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}",
                other.id, guest.account_id
            ),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .grant_server(&owner, &high, &[S::Administrator])
        .await;
    server.grant_server(&owner, &low, &[S::Administrator]).await;
    server
        .grant_workspace(&owner, &guest, workspace.id, &[W::ManageRoles])
        .await;
    let route = |account| {
        format!(
            "/api/v1/workspaces/{}/members/{account}/removal",
            workspace.id
        )
    };
    server
        .post(&low, &route(high.account_id), &Value::Null)
        .await
        .error(ErrorCode::Forbidden);
    server
        .post(&high, &route(low.account_id), &Value::Null)
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .post(&owner, &route(guest.account_id), &Value::Null)
        .await
        .empty(StatusCode::NO_CONTENT);
    let memberships: Vec<uuid::Uuid> =
        sqlx::query_scalar("select workspace_id from workspace_members where account_id=$1")
            .bind(guest.account_id)
            .fetch_all(&server.db)
            .await
            .unwrap();
    assert_eq!(memberships, [other.id.as_uuid()]);
    let count: i64 = sqlx::query_scalar(
        "select count(*) from member_roles where workspace_id=$1 and account_id=$2",
    )
    .bind(workspace.id)
    .bind(guest.account_id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(count, 0);
    server
        .get(&guest, "/api/v1/account")
        .await
        .ok::<Value>(StatusCode::OK);
    server
        .get(
            &guest,
            &format!("/api/v1/workspaces/{}/members", workspace.id),
        )
        .await
        .error(ErrorCode::NotFound);
    server
        .post(&owner, &route(guest.account_id), &Value::Null)
        .await
        .error(ErrorCode::NotFound);
    let audit: AuditPage = server
        .get(
            &owner,
            &format!("/api/v1/workspaces/{}/audit-log", workspace.id),
        )
        .await
        .ok(StatusCode::OK);
    assert!(audit.entries.iter().any(|e| matches!(e.event,AuditEvent::WorkspaceMemberRemoved{account_id,..} if account_id==guest.account_id)));
}

#[tokio::test]
async fn workspace_removal_refuses_unauthorized_targets() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let plain = server.join(&owner, "Plain").await;
    let admin = server.join(&owner, "Administrator").await;
    let equal = server.join(&owner, "Equal").await;
    let workspace = server.default_workspace(&owner).await;
    let role = server
        .grant_server(&owner, &admin, &[S::Administrator])
        .await;
    server
        .put_empty(
            &owner,
            &format!("/api/v1/accounts/{}/server-roles/{role}", equal.account_id),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let route = |account| {
        format!(
            "/api/v1/workspaces/{}/members/{account}/removal",
            workspace.id
        )
    };
    server
        .post(&plain, &route(admin.account_id), &Value::Null)
        .await
        .error(ErrorCode::Forbidden);
    server.grant_server(&owner, &plain, &[S::BanMembers]).await;
    server
        .grant_workspace(&owner, &plain, workspace.id, &[W::ManageRoles])
        .await;
    let guest = server.join(&owner, "Target").await;
    server
        .post(&plain, &route(guest.account_id), &Value::Null)
        .await
        .error(ErrorCode::Forbidden);
    server
        .post(&admin, &route(equal.account_id), &Value::Null)
        .await
        .error(ErrorCode::Forbidden);
    server
        .post(&admin, &route(admin.account_id), &Value::Null)
        .await
        .error(ErrorCode::OwnAccountTarget);
    server
        .post(&admin, &route(owner.account_id), &Value::Null)
        .await
        .error(ErrorCode::OwnerProtected);
    server
        .post(&owner, &route(owner.account_id), &Value::Null)
        .await
        .error(ErrorCode::OwnerProtected);
}

#[tokio::test]
async fn workspace_removal_ends_the_call_and_contact() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let seat = server.enter_call(ann, scene.voice.id).await;
    server
        .post(
            ann,
            "/api/v1/conversations",
            &json!({"account_id":ben.account_id}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    let cursor = server.snapshot(ann).await.cursor;
    server
        .post(
            owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}/removal",
                scene.workspace_id, ann.account_id
            ),
            &json!({"reason":"Workspace boundary"}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let audit: AuditPage = server
        .get(
            owner,
            &format!("/api/v1/workspaces/{}/audit-log", scene.workspace_id),
        )
        .await
        .ok(StatusCode::OK);
    assert!(audit.entries.iter().any(|entry| matches!(
        &entry.event,
        AuditEvent::WorkspaceMemberRemoved { account_id, reason }
            if *account_id == ann.account_id && reason.as_deref() == Some("Workspace boundary")
    )));
    let stored: Vec<String> =
        sqlx::query_scalar("select payload::text || audience::text from events")
            .fetch_all(&server.db)
            .await
            .unwrap();
    assert!(
        stored
            .iter()
            .all(|entry| !entry.contains("Workspace boundary"))
    );
    assert!(server.own_call(ann).await.call.is_none());
    server.check().await;
    assert!(
        server
            .fake()
            .removed()
            .iter()
            .any(|(_, identity)| identity == &seat.identity)
    );
    let received = crate::discovery_support::events(
        server,
        ann,
        cursor,
        junto_protocol::events::Level::Background,
    )
    .await;
    assert!(received.iter().any(|e| matches!(
        e,
        junto_protocol::events::Event::WorkspaceMemberRemoved { .. }
    )));
    let contacts: Vec<Value> = server
        .get(ann, "/api/v1/conversations")
        .await
        .ok(StatusCode::OK);
    assert!(contacts.is_empty());
}

#[tokio::test]
async fn a_token_cannot_remove_a_workspace_membership() {
    use junto_protocol::access_token::TokenAction;
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let admin = server.administrator(&owner, "Administrator").await;
    let guest = server.join(&owner, "Guest").await;
    let workspace = server.default_workspace(&owner).await;
    let route = format!(
        "/api/v1/workspaces/{}/members/{}/removal",
        workspace.id, guest.account_id
    );
    let token = server.access_token(&admin, TokenAction::ALL).await;
    let role = server
        .grant_workspace(&owner, &guest, workspace.id, &[])
        .await;
    server
        .with_token(
            reqwest::Method::PUT,
            &format!(
                "/api/v1/workspaces/{}/members/{}",
                workspace.id, guest.account_id
            ),
            &token,
            Some(&json!({"role_ids":[]})),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let kept: bool = sqlx::query_scalar(
        "select exists(select 1 from member_roles where account_id=$1 and role_id=$2)",
    )
    .bind(guest.account_id)
    .bind(role)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert!(kept);

    server
        .with_token(reqwest::Method::POST, &route, &token, None)
        .await
        .error(ErrorCode::Forbidden);
    server
        .post(&admin, &route, &Value::Null)
        .await
        .empty(StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn workspace_removal_stops_an_open_stream_and_content_reads() {
    use crate::discovery_support::{search, send};
    use junto_protocol::events::{Event, Level};
    let files = crate::messages_support::Files::open().await;
    let (server, owner) = (&files.server, &files.owner);
    let target = server.join(owner, "Target").await;
    let outsider = server.join(owner, "Outsider").await;
    let workspace = files.channel.workspace_id;
    let remove = |account| format!("/api/v1/workspaces/{workspace}/members/{account}/removal");
    server
        .post(owner, &remove(outsider.account_id), &Value::Null)
        .await
        .empty(StatusCode::NO_CONTENT);
    let upload = files.upload(b"kept content").await;
    let attached = files.attach(&[upload.id]).await;
    let message = send(server, &target, &files.channel, "searchable quartz").await;
    assert!(!search(server, &target, "quartz").await.results.is_empty());
    let download = format!("/api/v1/attachments/{}/file", attached.attachments[0].id);
    assert_eq!(server.get(&target, &download).await.status, StatusCode::OK);
    let mut target_stream = server.stream(&target).await;
    target_stream.subscribe(Level::Full, None).await;
    let mut outsider_stream = server.stream(&outsider).await;
    outsider_stream.subscribe(Level::Full, None).await;
    server
        .post(owner, &remove(target.account_id), &Value::Null)
        .await
        .empty(StatusCode::NO_CONTENT);
    let category = server
        .category(owner, workspace, "hidden afterwards", 0)
        .await;
    let target_events = server.traffic(&target, &mut target_stream).await;
    assert!(target_events.iter().any(|(_,event)| matches!(event,Event::WorkspaceMemberRemoved{account_id,..} if *account_id==target.account_id)));
    assert!(
        !target_events.iter().any(
            |(_, event)| matches!(event,Event::CategoryCreated{category:c} if c.id==category.id)
        )
    );
    let outsider_events = server.traffic(&outsider, &mut outsider_stream).await;
    assert!(
        !outsider_events
            .iter()
            .any(|(_, event)| matches!(event, Event::WorkspaceMemberRemoved { .. }))
    );
    server
        .get(
            &target,
            &format!("/api/v1/channels/{}/messages", files.channel.id),
        )
        .await
        .error(ErrorCode::NotFound);
    server
        .get(&target, &download)
        .await
        .error(ErrorCode::NotFound);
    assert!(search(server, &target, "quartz").await.results.is_empty());
    let kept: bool = sqlx::query_scalar("select not deleted from messages where id=$1")
        .bind(message.id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert!(kept);
}

#[tokio::test]
async fn workspace_removal_ends_pending_call_changes() {
    use crate::calls_review2::{invite, move_to};
    for movement in [false, true] {
        let scene = Scene::start().await;
        let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
        if movement {
            server.enter_call(ann, scene.voice.id).await;
            let destination = server.voice(owner, scene.workspace_id, "Destination").await;
            move_to(
                server,
                owner,
                scene.voice.id,
                ann.account_id,
                destination.id,
            )
            .await;
            assert_eq!(server.count("call_moves").await, 1);
        } else {
            server.enter_call(ben, scene.voice.id).await;
            server
                .put_empty(
                    ben,
                    &format!("/api/v1/channels/{}/call/lock", scene.voice.id),
                )
                .await
                .empty(StatusCode::NO_CONTENT);
            server
                .post(
                    ann,
                    &format!("/api/v1/channels/{}/call/entry-requests", scene.voice.id),
                    &Value::Null,
                )
                .await
                .ok::<Value>(StatusCode::CREATED);
            invite(server, ben, scene.voice.id, ann).await;
            assert_eq!(server.count("call_entry_requests").await, 1);
            assert_eq!(server.count("call_invitations").await, 1);
        }
        server
            .post(
                owner,
                &format!(
                    "/api/v1/workspaces/{}/members/{}/removal",
                    scene.workspace_id, ann.account_id
                ),
                &Value::Null,
            )
            .await
            .empty(StatusCode::NO_CONTENT);
        server.check().await;
        assert_eq!(server.count("call_moves").await, 0);
        assert_eq!(server.count("call_entry_requests").await, 0);
        assert_eq!(server.count("call_invitations").await, 0);
        assert!(server.snapshot(ann).await.own_call.is_none());
    }
}

#[tokio::test]
async fn workspace_removal_keeps_private_preferences_for_readmission() {
    use crate::discovery_support::{notices, reply, send, settings};
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let target = server.join(&owner, "Target").await;
    let workspace = server.default_workspace(&owner).await;
    let channel = server.channel(&owner, workspace.id, "text").await;
    settings(
        &server,
        &target,
        channel.id.as_uuid(),
        json!({"level":"all"}),
    )
    .await;
    let root = send(&server, &target, &channel, "kept authored root").await;
    server
        .put(
            &target,
            &format!("/api/v1/messages/{}/thread/follow", root.id),
            &json!({"following":true}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    reply(&server, &owner, root.id, "notice").await;
    let state = || async {
        sqlx::query_scalar::<_,Value>("select jsonb_build_object('settings',(select jsonb_agg(s order by scope_id) from notification_settings s where account_id=$1),'threads',(select jsonb_agg(t order by root_id) from thread_reads t where account_id=$1),'reads',(select jsonb_agg(r order by place_id) from message_reads r where account_id=$1),'notices',(select jsonb_agg(n order by id) from notifications n where account_id=$1))")
            .bind(target.account_id).fetch_one(&server.db).await.unwrap()
    };
    let before = state().await;
    assert!(!notices(&server, &target).await.is_empty());
    server
        .post(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}/removal",
                workspace.id, target.account_id
            ),
            &Value::Null,
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(state().await, before);
    assert!(notices(&server, &target).await.is_empty());
    server
        .get(
            &target,
            &format!("/api/v1/messages/{}/thread/read", root.id),
        )
        .await
        .error(ErrorCode::NotFound);
    server
        .get(
            &target,
            &format!("/api/v1/notification-settings/{}", channel.id),
        )
        .await
        .error(ErrorCode::NotFound);
    server
        .put_empty(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}",
                workspace.id, target.account_id
            ),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(state().await, before);
    assert!(!notices(&server, &target).await.is_empty());
    server
        .get(
            &target,
            &format!("/api/v1/messages/{}/thread/read", root.id),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    server
        .put_empty(
            &owner,
            &format!("/api/v1/accounts/{}/ban", target.account_id),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .post(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}/removal",
                workspace.id, target.account_id
            ),
            &Value::Null,
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let status: String = sqlx::query_scalar("select status from accounts where id=$1")
        .bind(target.account_id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(status, "banned");
}

#[tokio::test]
async fn reasons_have_the_audit_readers_and_cannot_be_changed() {
    use junto_protocol::access_token::TokenAction;
    let scene = Scene::start().await;
    let (server, owner, ann) = (&scene.server, &scene.owner, &scene.ann);
    let reason = "membership decision";
    server
        .post(
            owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}/removal",
                scene.workspace_id, ann.account_id
            ),
            &json!({"reason":reason}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let route = format!("/api/v1/workspaces/{}/audit-log", scene.workspace_id);
    server.get(ann, &route).await.error(ErrorCode::NotFound);
    server
        .put_empty(
            owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}",
                scene.workspace_id, ann.account_id
            ),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .grant_workspace(owner, ann, scene.workspace_id, &[W::ViewAuditEntries])
        .await;
    let returned: AuditPage = server.get(ann, &route).await.ok(StatusCode::OK);
    assert!(returned.entries.iter().any(|entry|matches!(&entry.event,AuditEvent::WorkspaceMemberRemoved{reason:Some(text),..} if text==reason)));
    let token = server
        .access_token(owner, &[TokenAction::ReadAuditLog])
        .await;
    let token_read: AuditPage = server
        .with_token(reqwest::Method::GET, &route, &token, None)
        .await
        .ok(StatusCode::OK);
    assert!(token_read.entries.iter().any(|entry|matches!(&entry.event,AuditEvent::WorkspaceMemberRemoved{reason:Some(text),..} if text==reason)));
    assert!(sqlx::query("update audit_entries set event=jsonb_set(event,'{reason}','\"changed\"'::jsonb) where action='workspace_member_removed'").execute(&server.db).await.is_err());
}
