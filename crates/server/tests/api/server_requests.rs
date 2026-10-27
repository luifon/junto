use crate::support::{Member, Reply, TestServer};
use junto_protocol::error::ErrorCode;
use junto_protocol::events::{Cursor, Event, Level};
use junto_protocol::roles::{ServerRole, WorkspaceRole};
use junto_protocol::{ServerPermission as S, WorkspacePermission as W};
use reqwest::StatusCode;
use serde_json::{Value, json};

fn cursor(reply: &Reply) -> Option<Cursor> {
    serde_json::from_str(
        reply
            .header("junto-event-cursor")
            .expect("mutation receipt"),
    )
    .unwrap()
}

/// The cursor position after the newest change of the log (ADR 0010).
async fn last_position(server: &TestServer) -> u64 {
    let n: i64 = sqlx::query_scalar("select max(position) from events")
        .fetch_one(&server.db)
        .await
        .unwrap();
    junto_server::events::log::stream_position(n, junto_server::events::log::END_OF_CHANGE).unwrap()
}

fn change(position: u64) -> i64 {
    junto_server::events::log::change_of(position)
}

#[tokio::test]
async fn mutation_receipts_match_committed_events_and_noops() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let guest = server.join(&owner, "Guest").await;
    let workspace = server.default_workspace(&owner).await;
    let mut stream = server.subscribed(&owner, Level::Full).await;
    let route = format!("/api/v1/workspaces/{}/notification-default", workspace.id);
    server.gate().lose_next(1);
    let reply = server.put(&owner, &route, &json!({"level":"all"})).await;
    reply.ok::<Value>(StatusCode::OK);
    let receipt = cursor(&reply).unwrap();
    assert_eq!(receipt.position, last_position(&server).await);
    let traffic = server.traffic(&owner, &mut stream).await;
    assert!(
        traffic.iter().any(|(p, e)| *p <= receipt.position
            && matches!(e, Event::NotificationDefaultChanged { .. }))
    );
    let role: ServerRole = server
        .post(&owner, "/api/v1/server-roles", &json!({"name":"Role"}))
        .await
        .ok(StatusCode::CREATED);
    let assignment = format!(
        "/api/v1/accounts/{}/server-roles/{}",
        guest.account_id, role.id
    );
    let reply = server.put_empty(&owner, &assignment).await;
    reply.empty(StatusCode::NO_CONTENT);
    let receipt = cursor(&reply).unwrap();
    let traffic = server.traffic(&owner, &mut stream).await;
    assert_eq!(
        change(receipt.position),
        change(traffic.iter().map(|(position, _)| *position).max().unwrap())
    );
    assert!(traffic.iter().any(|(p,e)| *p <= receipt.position && matches!(e, Event::AccountServerRolesChanged{account_id,..} if *account_id == guest.account_id)));
    assert_eq!(cursor(&server.put_empty(&owner, &assignment).await), None);
    let denied = server.put_empty(&guest, &assignment).await;
    denied.error(ErrorCode::Forbidden);
    assert!(denied.header("junto-event-cursor").is_none());
    assert!(
        server
            .get(&owner, "/api/v1/account")
            .await
            .header("junto-event-cursor")
            .is_none()
    );
}

#[tokio::test]
async fn moderation_receipt_precedes_the_observed_removal() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let guest = server.join(&owner, "Guest").await;
    let workspace = server.default_workspace(&owner).await;
    let mut stream = server.subscribed(&owner, Level::Full).await;
    let reply = server
        .post(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}/removal",
                workspace.id, guest.account_id
            ),
            &json!({"reason":"A reason"}),
        )
        .await;
    reply.empty(StatusCode::NO_CONTENT);
    let receipt = cursor(&reply).unwrap();
    assert_eq!(receipt.position, last_position(&server).await);
    assert!(server.traffic(&owner, &mut stream).await.iter().any(|(p,e)| *p <= receipt.position && matches!(e, Event::WorkspaceMemberRemoved{account_id,..} if *account_id == guest.account_id)));
}

#[tokio::test]
async fn workspace_notice_survives_event_trim_and_is_private() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let guest = server.join(&owner, "Guest").await;
    let workspace = server.default_workspace(&owner).await;
    server
        .post(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}/removal",
                workspace.id, guest.account_id
            ),
            &json!({"recipient_message":"Only the subject"}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    sqlx::query("delete from events")
        .execute(&server.db)
        .await
        .unwrap();
    let snapshot: Value = server
        .get(&guest, "/api/v1/snapshot")
        .await
        .ok(StatusCode::OK);
    let notices = snapshot["moderation_notices"]
        .as_array()
        .expect("durable own notices");
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0]["message"], "Only the subject");
    assert_eq!(notices[0]["workspace_id"], json!(workspace.id));
    assert_eq!(notices[0]["name"], workspace.name);
    assert!(
        !notices[0]
            .to_string()
            .contains(&owner.account_id.to_string())
    );
    let other: Value = server
        .get(&owner, "/api/v1/snapshot")
        .await
        .ok(StatusCode::OK);
    assert_eq!(other["moderation_notices"], json!([]));
    assert_eq!(
        server
            .get(&owner, "/api/v1/account/moderation-notices")
            .await
            .ok::<Value>(StatusCode::OK),
        json!({"notices":[], "next_before":null})
    );
}

#[tokio::test]
async fn device_proof_reads_only_its_own_notice_after_removal_and_restart() {
    for removal in [false, true] {
        let server = TestServer::start().await;
        let owner = server.owner().await;
        let guest = server.join(&owner, "Guest").await;
        let route = format!(
            "/api/v1/accounts/{}/{}",
            guest.account_id,
            if removal { "removal" } else { "ban" }
        );
        let body = json!({"recipient_message":"Private reason"});
        let reply = if removal {
            server.post(&owner, &route, &body).await
        } else {
            server.put(&owner, &route, &body).await
        };
        reply.empty(StatusCode::NO_CONTENT);
        let notices = proven_notices(&server, &guest.key).await;
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0]["message"], "Private reason");
        assert_eq!(
            notices[0]["kind"],
            if removal { "server_removed" } else { "banned" }
        );
        assert!(
            !notices[0]
                .to_string()
                .contains(&owner.account_id.to_string())
        );
        server
            .get(&guest, "/api/v1/account")
            .await
            .error(ErrorCode::Unauthenticated);
        assert_eq!(
            server
                .get(&owner, "/api/v1/account/moderation-notices")
                .await
                .ok::<Value>(StatusCode::OK),
            json!({"notices":[], "next_before":null})
        );
        server.clock.advance(time::Duration::days(29));
        assert_eq!(proven_notices(&server, &guest.key).await, notices);
        assert!(
            proven_notices(&server, &crate::support::DeviceKey::generate())
                .await
                .is_empty()
        );
    }
}

#[tokio::test]
async fn account_rows_state_the_actual_unban_rank_check() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let moderator = server.join(&owner, "Moderator").await;
    let guest = server.join(&owner, "Guest").await;
    let workspace = server.default_workspace(&owner).await;
    server
        .grant_server(&owner, &moderator, &[S::BanMembers])
        .await;
    server
        .grant_workspace(&owner, &guest, workspace.id, &[W::ManageRoles])
        .await;
    let route = format!("/api/v1/accounts/{}/ban", guest.account_id);
    server
        .put_empty(&owner, &route)
        .await
        .empty(StatusCode::NO_CONTENT);
    for (actor, allowed) in [(&moderator, false), (&owner, true)] {
        let rows: Vec<Value> = server
            .get(actor, "/api/v1/accounts")
            .await
            .ok(StatusCode::OK);
        let row = rows
            .iter()
            .find(|r| r["id"] == json!(guest.account_id))
            .unwrap();
        assert_eq!(row["may_unban"], allowed);
    }
    server
        .delete(&moderator, &route)
        .await
        .error(ErrorCode::Forbidden);
    server
        .delete(&owner, &route)
        .await
        .empty(StatusCode::NO_CONTENT);
}

async fn workspace_role(
    server: &TestServer,
    owner: &Member,
    workspace: junto_protocol::ids::WorkspaceId,
) -> WorkspaceRole {
    server
        .post(
            owner,
            &format!("/api/v1/workspaces/{workspace}/roles"),
            &json!({"name":"Lower"}),
        )
        .await
        .ok(StatusCode::CREATED)
}

#[tokio::test]
async fn role_capabilities_include_channel_grant_and_deny_rules() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let manager = server.join(&owner, "Manager").await;
    let plain = server.join(&owner, "Plain").await;
    let workspace = server.default_workspace(&owner).await;
    server
        .grant_workspace(&owner, &manager, workspace.id, &[W::ManageRoles])
        .await;
    let role = workspace_role(&server, &owner, workspace.id).await;
    let channel = server.channel(&owner, workspace.id, "Text").await;
    let override_route = format!("/api/v1/channels/{}/overrides/{}", channel.id, role.id);
    let list = format!("/api/v1/workspaces/{}/roles", workspace.id);
    for (allow, deny, assign, remove) in [
        (json!([]), json!([]), true, true),
        (json!(["mute_members"]), json!([]), false, true),
        (json!([]), json!(["mute_members"]), true, false),
    ] {
        server
            .put(&owner, &override_route, &json!({"allow":allow,"deny":deny}))
            .await
            .ok::<Value>(StatusCode::OK);
        let rows: Vec<Value> = server.get(&manager, &list).await.ok(StatusCode::OK);
        let row = rows.iter().find(|r| r["id"] == json!(role.id)).unwrap();
        assert_eq!(row["may_assign"], assign);
        assert_eq!(row["may_remove"], remove);
        assert_eq!(row["may_delete"], remove);
        let assignment = format!(
            "/api/v1/workspaces/{}/members/{}/roles/{}",
            workspace.id, plain.account_id, role.id
        );
        let assigned = server.put_empty(&manager, &assignment).await;
        if assign {
            assigned.empty(StatusCode::NO_CONTENT);
        } else {
            assigned.error(ErrorCode::ExceedsOwnPermissions);
        }
        let removed = server.delete(&manager, &assignment).await;
        if remove {
            removed.empty(StatusCode::NO_CONTENT);
        } else {
            removed.error(ErrorCode::ExceedsOwnPermissions);
            server
                .delete(&manager, &format!("{list}/{}", role.id))
                .await
                .error(ErrorCode::ExceedsOwnPermissions);
        }
        let rows: Vec<Value> = server.get(&plain, &list).await.ok(StatusCode::OK);
        assert!(rows.iter().all(|r| r["may_assign"] == false
            && r["may_remove"] == false
            && r["may_delete"] == false));
    }
}

#[tokio::test]
async fn concurrent_single_permission_changes_hold_at_both_role_levels() {
    for workspace_level in [false, true] {
        let server = TestServer::start().await;
        let owner = server.owner().await;
        let workspace = server.default_workspace(&owner).await;
        let (route, permissions) = if workspace_level {
            let role = workspace_role(&server, &owner, workspace.id).await;
            (
                format!("/api/v1/workspaces/{}/roles/{}", workspace.id, role.id),
                ["mute_members", "disconnect_members"],
            )
        } else {
            let role: ServerRole = server
                .post(&owner, "/api/v1/server-roles", &json!({"name":"Role"}))
                .await
                .ok(StatusCode::CREATED);
            (
                format!("/api/v1/server-roles/{}", role.id),
                ["ban_members", "create_invites"],
            )
        };
        let delta = format!("{route}/permissions");
        // First prove the endpoint exists before waiting on a commit gate.
        server
            .patch(
                &owner,
                &delta,
                &json!({"permission":permissions[0],"enabled":false}),
            )
            .await
            .ok::<Value>(StatusCode::OK);
        let one = json!({"permission":permissions[0],"enabled":true});
        let two = json!({"permission":permissions[1],"enabled":true});
        server.gate().arm(0);
        let first = server.patch(&owner, &delta, &one);
        tokio::pin!(first);
        crate::support::drive_until(&mut first, server.gate().arrived()).await;
        let second = server.patch(&owner, &delta, &two);
        tokio::pin!(second);
        crate::support::drive_until(&mut second, server.until_blocked()).await;
        server.gate().release();
        let (first, second) = tokio::join!(first, second);
        first.ok::<Value>(StatusCode::OK);
        let changed: Value = second.ok(StatusCode::OK);
        assert!(permissions.iter().all(|p| {
            changed["permissions"]
                .as_array()
                .unwrap()
                .contains(&json!(p))
        }));
    }
}

#[tokio::test]
async fn positions_shift_past_a_million_and_remain_movable() {
    for workspace_level in [false, true] {
        let server = TestServer::start().await;
        let owner = server.owner().await;
        let workspace = server.default_workspace(&owner).await;
        let list = if workspace_level {
            format!("/api/v1/workspaces/{}/roles", workspace.id)
        } else {
            "/api/v1/server-roles".into()
        };
        let role: Value = server
            .post(&owner, &list, &json!({"name":"High"}))
            .await
            .ok(StatusCode::CREATED);
        let route = format!("{list}/positions");
        server
            .patch(
                &owner,
                &route,
                &json!({"positions":[{"role_id":role["id"],"position":1000000}]}),
            )
            .await
            .empty(StatusCode::NO_CONTENT);
        server
            .post(&owner, &list, &json!({"name":"Low"}))
            .await
            .ok::<Value>(StatusCode::CREATED);
        let rows: Vec<Value> = server.get(&owner, &list).await.ok(StatusCode::OK);
        assert_eq!(
            rows.iter().find(|r| r["id"] == role["id"]).unwrap()["position"],
            1000001
        );
        server
            .patch(
                &owner,
                &route,
                &json!({"positions":[{"role_id":role["id"],"position":1000002}]}),
            )
            .await
            .empty(StatusCode::NO_CONTENT);
    }
}

#[tokio::test]
async fn message_and_thread_receipts_cover_the_last_event_of_the_transaction() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let guest = server.join(&owner, "Guest").await;
    let channel = crate::messages_support::channel(&server, &owner, "text").await;
    let mut stream = server.subscribed(&guest, Level::Full).await;
    let reply = server
        .post(
            &owner,
            &format!("/api/v1/channels/{}/messages", channel.id),
            &crate::messages_support::text("hello"),
        )
        .await;
    let message: junto_protocol::message::Message = reply.ok(StatusCode::CREATED);
    let receipt = cursor(&reply).unwrap();
    assert_eq!(receipt.position, last_position(&server).await);
    let events = server.traffic(&guest, &mut stream).await;
    assert!(events.iter().any(|(p, e)| *p <= receipt.position
        && matches!(e,Event::MessageChanged{message_id,..} if *message_id == message.id)));
    let reply = server
        .put(
            &guest,
            &format!("/api/v1/messages/{}/thread/follow", message.id),
            &json!({"following":true}),
        )
        .await;
    reply.ok::<Value>(StatusCode::OK);
    let receipt = cursor(&reply).unwrap();
    assert_eq!(receipt.position, last_position(&server).await);
    assert!(
        server
            .traffic(&guest, &mut stream)
            .await
            .iter()
            .any(|(p, e)| *p <= receipt.position && matches!(e, Event::ThreadFollowChanged { .. }))
    );
}

#[tokio::test]
async fn single_permission_changes_enforce_rank_ceiling_and_visibility() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let manager = server.join(&owner, "Manager").await;
    let plain = server.join(&owner, "Plain").await;
    let workspace = server.default_workspace(&owner).await;
    let high = server
        .grant_workspace(&owner, &manager, workspace.id, &[W::ManageRoles])
        .await;
    let role = workspace_role(&server, &owner, workspace.id).await;
    let route = format!(
        "/api/v1/workspaces/{}/roles/{}/permissions",
        workspace.id, role.id
    );
    let body = json!({"permission":"mute_members","enabled":true});
    server
        .patch(&manager, &route, &body)
        .await
        .error(ErrorCode::ExceedsOwnPermissions);
    server
        .patch(&plain, &route, &body)
        .await
        .error(ErrorCode::Forbidden);
    server
        .patch(
            &manager,
            &format!(
                "/api/v1/workspaces/{}/roles/{high}/permissions",
                workspace.id
            ),
            &json!({"permission":"manage_roles","enabled":false}),
        )
        .await
        .error(ErrorCode::Forbidden);
    server
        .patch(&owner, &route, &body)
        .await
        .ok::<Value>(StatusCode::OK);
    server
        .patch(
            &manager,
            &route,
            &json!({"permission":"mute_members","enabled":false}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    let hidden: junto_protocol::structure::Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({"name":"Hidden"}))
        .await
        .ok(StatusCode::CREATED);
    server
        .get(&plain, &format!("/api/v1/workspaces/{}/roles", hidden.id))
        .await
        .error(ErrorCode::NotFound);
    server
        .get(&plain, "/api/v1/accounts")
        .await
        .error(ErrorCode::Forbidden);
    server
        .get(&plain, "/api/v1/server-roles")
        .await
        .error(ErrorCode::Forbidden);
}

#[tokio::test]
async fn receipt_uses_commit_order_and_excludes_rolled_back_attempts() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let route = format!("/api/v1/workspaces/{}/notification-default", workspace.id);
    // Confirm the mechanism before holding a transaction.
    let reply = server
        .put(&owner, &route, &json!({"level":"mentions"}))
        .await;
    assert!(cursor(&reply).is_some());
    server.gate().arm(0);
    let body = json!({"level":"all"});
    let first = server.put(&owner, &route, &body);
    tokio::pin!(first);
    crate::support::drive_until(&mut first, server.gate().arrived()).await;
    let second = server
        .patch(
            &owner,
            &format!("/api/v1/account/devices/{}", owner.device_id),
            &json!({"name":"Other transaction"}),
        )
        .await;
    let earlier = cursor(&second).unwrap();
    server.gate().release();
    let later = cursor(&first.await).unwrap();
    assert_eq!(earlier.epoch, later.epoch);
    assert!(earlier.position < later.position);
    server.gate().lose_next(8);
    let failed = server
        .put(&owner, &route, &json!({"level":"mentions"}))
        .await;
    failed.error(ErrorCode::Busy);
    assert!(failed.header("junto-event-cursor").is_none());
    let current: Value = server.get(&owner, &route).await.ok(StatusCode::OK);
    assert_eq!(current["level"], "all");
}

#[tokio::test]
async fn tokens_never_offer_assignment_unban_or_private_notices() {
    use junto_protocol::access_token::TokenAction;
    use reqwest::Method;
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let guest = server.join(&owner, "Guest").await;
    let workspace = server.default_workspace(&owner).await;
    let token = server
        .access_token(
            &owner,
            &[
                TokenAction::ReadStructure,
                TokenAction::ManageStructure,
                TokenAction::ManageMembers,
            ],
        )
        .await;
    let role = workspace_role(&server, &owner, workspace.id).await;
    let sr: ServerRole = server
        .post(&owner, "/api/v1/server-roles", &json!({"name":"Role"}))
        .await
        .ok(StatusCode::CREATED);
    for (route, id) in [
        (
            format!("/api/v1/workspaces/{}/roles", workspace.id),
            json!(role.id),
        ),
        ("/api/v1/server-roles".into(), json!(sr.id)),
    ] {
        let rows: Vec<Value> = server
            .with_token(Method::GET, &route, &token, None)
            .await
            .ok(StatusCode::OK);
        let row = rows.iter().find(|r| r["id"] == id).unwrap();
        assert_eq!(row["may_assign"], false);
        assert_eq!(row["may_remove"], false);
        assert_eq!(row["may_delete"], true);
    }
    server
        .put_empty(
            &owner,
            &format!("/api/v1/accounts/{}/ban", guest.account_id),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let rows: Vec<Value> = server
        .with_token(Method::GET, "/api/v1/accounts", &token, None)
        .await
        .ok(StatusCode::OK);
    assert!(rows.iter().all(|r| r["may_unban"] == false));
    server
        .with_token(
            Method::GET,
            "/api/v1/account/moderation-notices",
            &token,
            None,
        )
        .await
        .error(ErrorCode::Forbidden);
    let read_only = server
        .access_token(&owner, &[TokenAction::ReadStructure])
        .await;
    let rows: Vec<Value> = server
        .with_token(Method::GET, "/api/v1/server-roles", &read_only, None)
        .await
        .ok(StatusCode::OK);
    assert!(rows.iter().all(|row| row["may_delete"] == false));
}

#[tokio::test]
async fn server_role_capabilities_follow_rank_and_current_permission() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let admin = server.administrator(&owner, "Administrator").await;
    let moderator = server.join(&owner, "Moderator").await;
    server
        .grant_server(&owner, &moderator, &[S::BanMembers])
        .await;
    let role: ServerRole = server
        .post(&owner, "/api/v1/server-roles", &json!({"name":"Lower"}))
        .await
        .ok(StatusCode::CREATED);
    for (actor, expected) in [(&owner, true), (&admin, true), (&moderator, false)] {
        let rows: Vec<Value> = server
            .get(actor, "/api/v1/server-roles")
            .await
            .ok(StatusCode::OK);
        let row = rows.iter().find(|r| r["id"] == json!(role.id)).unwrap();
        for field in ["may_assign", "may_remove", "may_delete"] {
            assert_eq!(row[field], expected);
        }
    }
}

async fn proven_notices(server: &TestServer, key: &crate::support::DeviceKey) -> Vec<Value> {
    let challenge: junto_protocol::auth::Challenge = server
        .open(
            "/api/v1/auth/challenges",
            &json!({"public_key":key.public(),"operation":"moderation_notices"}),
        )
        .await
        .ok(StatusCode::CREATED);
    assert_eq!(challenge.account_id, None);
    let proof = junto_protocol::auth::Proof {
        challenge_id: challenge.id,
        origin: crate::support::origin(),
        signature: key.sign(&challenge),
    };
    let body = json!({"proof":proof});
    let page: Value = server
        .open("/api/v1/auth/moderation-notices", &body)
        .await
        .ok(StatusCode::OK);
    server
        .open("/api/v1/auth/moderation-notices", &body)
        .await
        .error(ErrorCode::ChallengeInvalid);
    assert!(page["next_before"].is_null());
    page["notices"].as_array().unwrap().clone()
}

#[tokio::test]
async fn private_notices_and_their_historical_keys_expire_together() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let guest = server.join(&owner, "Guest").await;
    server
        .post(
            &owner,
            &format!("/api/v1/accounts/{}/removal", guest.account_id),
            &json!({"reason":"Temporary private notice"}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(proven_notices(&server, &guest.key).await.len(), 1);
    server.clock.advance(time::Duration::days(30));
    assert!(proven_notices(&server, &guest.key).await.is_empty());
    junto_server::maintenance::run_once(&server.state)
        .await
        .unwrap();
    for table in ["moderation_notices", "moderation_notice_devices"] {
        let count: i64 =
            sqlx::query_scalar(sqlx::AssertSqlSafe(format!("select count(*) from {table}")))
                .fetch_one(&server.db)
                .await
                .unwrap();
        assert_eq!(count, 0, "{table}");
    }
}

#[tokio::test]
async fn revoking_a_device_ends_its_private_notice_access() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let guest = server.join(&owner, "Guest").await;
    let second = crate::devices::link_second_device(&server, &guest).await;
    let workspace = server.default_workspace(&owner).await;
    server
        .post(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}/removal",
                workspace.id, guest.account_id
            ),
            &json!({"reason":"Private history"}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(proven_notices(&server, &guest.key).await.len(), 1);
    server
        .delete(
            &second,
            &format!("/api/v1/account/devices/{}", guest.device_id),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    assert!(proven_notices(&server, &guest.key).await.is_empty());
    assert_eq!(proven_notices(&server, &second.key).await.len(), 1);
    let readers: i64 =
        sqlx::query_scalar("select count(*) from moderation_notice_devices where public_key=$1")
            .bind(guest.key.public().as_bytes().as_slice())
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(readers, 0);
    server
        .post(
            &owner,
            &format!("/api/v1/accounts/{}/removal", guest.account_id),
            &json!({}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    assert!(proven_notices(&server, &guest.key).await.is_empty());
    assert_eq!(proven_notices(&server, &second.key).await.len(), 2);
}

#[tokio::test]
async fn a_request_without_a_session_and_a_token_get_no_event_cursor() {
    use junto_protocol::{access_token::TokenAction, auth::ChallengeOperation};
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let key = crate::support::DeviceKey::generate();
    let proof = server.proof(&key, ChallengeOperation::RequestToJoin).await;
    let reply = server
        .open(
            "/api/v1/join-requests",
            &json!({"proof":proof,"name":"Applicant"}),
        )
        .await;
    reply.ok::<Value>(StatusCode::CREATED);
    assert!(reply.header("junto-event-cursor").is_none());
    let role: ServerRole = server
        .post(&owner, "/api/v1/server-roles", &json!({"name":"Role"}))
        .await
        .ok(StatusCode::CREATED);
    let token = server
        .access_token(&owner, &[TokenAction::ManageStructure])
        .await;
    let route = format!("/api/v1/server-roles/{}/permissions", role.id);
    let body = json!({"permission":"create_invites","enabled":true});
    let reply = server
        .with_token(reqwest::Method::PATCH, &route, &token, Some(&body))
        .await;
    reply.ok::<Value>(StatusCode::OK);
    assert!(reply.header("junto-event-cursor").is_none());
    assert!(
        server
            .patch(&owner, &route, &body)
            .await
            .header("junto-event-cursor")
            .is_some()
    );
}

#[tokio::test]
async fn audit_reason_is_private_and_recipient_message_is_separate() {
    for kind in ["workspace", "removal", "ban"] {
        let server = TestServer::start().await;
        let owner = server.owner().await;
        let guest = server.join(&owner, "Guest").await;
        let workspace = server.default_workspace(&owner).await;
        let route = if kind == "workspace" {
            format!(
                "/api/v1/workspaces/{}/members/{}/removal",
                workspace.id, guest.account_id
            )
        } else {
            format!("/api/v1/accounts/{}/{kind}", guest.account_id)
        };
        let body = json!({"reason":"Confidential moderator text","recipient_message":"Message for the person"});
        let reply = if kind == "ban" {
            server.put(&owner, &route, &body).await
        } else {
            server.post(&owner, &route, &body).await
        };
        reply.empty(StatusCode::NO_CONTENT);
        let notices = proven_notices(&server, &guest.key).await;
        assert_eq!(notices[0]["message"], "Message for the person");
        assert!(
            !serde_json::to_string(&notices)
                .unwrap()
                .contains("Confidential")
        );
        assert!(proven_notices(&server, &owner.key).await.is_empty());
        let audit: Vec<Value> = sqlx::query_scalar("select event from audit_entries")
            .fetch_all(&server.db)
            .await
            .unwrap();
        assert!(
            audit
                .iter()
                .any(|a| a["reason"] == "Confidential moderator text")
        );
    }
}

#[tokio::test]
async fn an_audit_reason_alone_is_never_a_notice_reason() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let guest = server.join(&owner, "Guest").await;
    server
        .post(
            &owner,
            &format!("/api/v1/accounts/{}/removal", guest.account_id),
            &json!({"reason":"Private audit only"}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    assert!(proven_notices(&server, &guest.key).await[0]["message"].is_null());
}

#[tokio::test]
async fn a_key_registered_again_reads_no_notice_of_the_removed_account() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let guest = server.join(&owner, "Guest").await;
    server
        .post(
            &owner,
            &format!("/api/v1/accounts/{}/removal", guest.account_id),
            &json!({}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(proven_notices(&server, &guest.key).await.len(), 1);
    let invite = server.invite(&owner, json!({"workspace_ids":[]})).await;
    server
        .join_reply(&guest.key, invite.code.expose(), "Returned")
        .await
        .ok::<Value>(StatusCode::CREATED);
    assert!(proven_notices(&server, &guest.key).await.is_empty());
    let readers: i64 =
        sqlx::query_scalar("select count(*) from moderation_notice_devices where public_key=$1")
            .bind(guest.key.public().as_bytes().as_slice())
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(readers, 0);
}

#[tokio::test]
async fn an_unchanged_permission_writes_nothing() {
    for workspace_level in [false, true] {
        let server = TestServer::start().await;
        let owner = server.owner().await;
        let workspace = server.default_workspace(&owner).await;
        let (route, permission) = if workspace_level {
            let role = workspace_role(&server, &owner, workspace.id).await;
            (
                format!(
                    "/api/v1/workspaces/{}/roles/{}/permissions",
                    workspace.id, role.id
                ),
                "mute_members",
            )
        } else {
            let role: ServerRole = server
                .post(&owner, "/api/v1/server-roles", &json!({"name":"Role"}))
                .await
                .ok(StatusCode::CREATED);
            (
                format!("/api/v1/server-roles/{}/permissions", role.id),
                "create_invites",
            )
        };
        let body = json!({"permission":permission,"enabled":true});
        server
            .patch(&owner, &route, &body)
            .await
            .ok::<Value>(StatusCode::OK);
        let before = last_position(&server).await;
        let audits: i64 = sqlx::query_scalar("select count(*) from audit_entries")
            .fetch_one(&server.db)
            .await
            .unwrap();
        let reply = server.patch(&owner, &route, &body).await;
        reply.ok::<Value>(StatusCode::OK);
        assert_eq!(cursor(&reply), None);
        assert_eq!(last_position(&server).await, before);
        let after: i64 = sqlx::query_scalar("select count(*) from audit_entries")
            .fetch_one(&server.db)
            .await
            .unwrap();
        assert_eq!(after, audits);
    }
}

#[tokio::test]
async fn role_and_account_lists_have_a_statement_bound() {
    let server = TestServer::start_measured_with(|c| {
        c.limits.stream.poll_interval = time::Duration::hours(1)
    })
    .await;
    let owner = server.owner().await;
    let admin = server.administrator(&owner, "Administrator").await;
    let workspace = server.default_workspace(&owner).await;
    for count in [1, 249] {
        sqlx::query("insert into workspace_roles(id,workspace_id,name,permissions,is_everyone,created_at,position) select gen_random_uuid(),$1,'Role',0,false,now(),(select max(position) from workspace_roles where workspace_id=$1)+n from generate_series(1,$2) n").bind(workspace.id).bind(count).execute(&mut server.test_data().await).await.unwrap();
        sqlx::query("insert into server_roles(id,name,permissions,created_at,position) select gen_random_uuid(),'Role',0,now(),(select coalesce(max(position),0) from server_roles)+n from generate_series(1,$1) n").bind(count).execute(&mut server.test_data().await).await.unwrap();
        for n in 0..if count == 1 { 1 } else { 9 } {
            let guest = server.join(&owner, &format!("Banned {count} {n}")).await;
            server
                .put_empty(
                    &owner,
                    &format!("/api/v1/accounts/{}/ban", guest.account_id),
                )
                .await
                .empty(StatusCode::NO_CONTENT);
        }
        let mut observer = server.db.acquire().await.unwrap();
        let role: i64 =
            sqlx::query_scalar("select oid::bigint from pg_roles where rolname=current_user")
                .fetch_one(&mut *observer)
                .await
                .unwrap();
        for route in [
            format!("/api/v1/workspaces/{}/roles", workspace.id),
            "/api/v1/server-roles".into(),
            "/api/v1/accounts".into(),
        ] {
            let before = crate::messages_queries::statements(&mut observer, role).await;
            server
                .get(&admin, &route)
                .await
                .ok::<Vec<Value>>(StatusCode::OK);
            let counts = crate::messages_queries::since(
                crate::messages_queries::statements(&mut observer, role).await,
                before,
            );
            let statements: i64 = counts
                .iter()
                .filter(|(sql, _)| !sql.contains("pg_stat_statements"))
                .map(|(_, calls)| calls)
                .sum();
            assert!(
                statements <= 30,
                "{route}: {statements} statements for {count} new roles: {counts:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_event_commit_does_not_read_the_epoch_table() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let mut lock = server.db.begin().await.unwrap();
    sqlx::query("lock table event_stream in access exclusive mode")
        .execute(&mut *lock)
        .await
        .unwrap();
    let mut writer = server.db.begin().await.unwrap();
    junto_server::events::log::tell_account(
        &mut writer,
        server.clock.now(),
        owner.account_id,
        &Event::PermissionsChanged { workspace_id: None },
    )
    .await
    .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), writer.commit())
        .await
        .expect("event assignment must not read the locked epoch table")
        .unwrap();
    lock.rollback().await.unwrap();
}

#[tokio::test]
async fn a_proof_of_another_operation_origin_or_age_reads_no_notice() {
    use junto_protocol::auth::ChallengeOperation as Op;
    let server = TestServer::start().await;
    let owner = server.owner().await;
    for op in [Op::JoinRequestStatus, Op::Login] {
        let proof = server.proof(&owner.key, op).await;
        server
            .open("/api/v1/auth/moderation-notices", &json!({"proof":proof}))
            .await
            .error(ErrorCode::ChallengeInvalid);
    }
    let mut proof = server.proof(&owner.key, Op::ModerationNotices).await;
    proof.origin = "https://elsewhere.example".parse().unwrap();
    server
        .open("/api/v1/auth/moderation-notices", &json!({"proof":proof}))
        .await
        .error(ErrorCode::OriginRejected);
    let proof = server.proof(&owner.key, Op::ModerationNotices).await;
    server.clock.advance(time::Duration::hours(1));
    server
        .open("/api/v1/auth/moderation-notices", &json!({"proof":proof}))
        .await
        .error(ErrorCode::ChallengeExpired);
}

#[tokio::test]
async fn a_device_removed_before_the_notice_is_no_reader() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let guest = server.join(&owner, "Guest").await;
    let second = crate::devices::link_second_device(&server, &guest).await;
    server
        .delete(
            &second,
            &format!("/api/v1/account/devices/{}", guest.device_id),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let workspace = server.default_workspace(&owner).await;
    server
        .post(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}/removal",
                workspace.id, guest.account_id
            ),
            &json!({}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    assert!(proven_notices(&server, &guest.key).await.is_empty());
    assert_eq!(proven_notices(&server, &second.key).await.len(), 1);
}

#[tokio::test]
async fn notices_are_isolated_and_expire_in_sessions_and_snapshots() {
    let server = TestServer::start().await;
    let mut owner = server.owner().await;
    let mut guests = [
        server.join(&owner, "First").await,
        server.join(&owner, "Second").await,
    ];
    let workspace = server.default_workspace(&owner).await;
    for (n, guest) in guests.iter().enumerate() {
        server
            .post(
                &owner,
                &format!(
                    "/api/v1/workspaces/{}/members/{}/removal",
                    workspace.id, guest.account_id
                ),
                &json!({"recipient_message":format!("Message {n}")}),
            )
            .await
            .empty(StatusCode::NO_CONTENT);
        assert_eq!(
            proven_notices(&server, &guest.key).await[0]["message"],
            format!("Message {n}")
        );
    }
    for days in [0, 30] {
        server.clock.advance(time::Duration::days(days));
        server.renew(&mut owner).await;
        for (n, guest) in guests.iter_mut().enumerate() {
            server.renew(guest).await;
            let page: Value = server
                .get(guest, "/api/v1/account/moderation-notices")
                .await
                .ok(StatusCode::OK);
            let snapshot: Value = server
                .get(guest, "/api/v1/snapshot")
                .await
                .ok(StatusCode::OK);
            assert_eq!(page["notices"], snapshot["moderation_notices"]);
            if days == 0 {
                assert_eq!(page["notices"].as_array().unwrap().len(), 1);
                assert_eq!(page["notices"][0]["message"], format!("Message {n}"));
            } else {
                assert_eq!(page["notices"], json!([]));
            }
        }
    }
}

#[tokio::test]
async fn a_role_of_another_workspace_is_not_found_and_server_rank_is_enforced() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let admin = server.administrator(&owner, "Admin").await;
    let workspace = server.default_workspace(&owner).await;
    let other: Value = server
        .post(&owner, "/api/v1/workspaces", &json!({"name":"Other"}))
        .await
        .ok(StatusCode::CREATED);
    let role = workspace_role(&server, &owner, workspace.id).await;
    server
        .patch(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/roles/{}/permissions",
                other["id"].as_str().unwrap(),
                role.id
            ),
            &json!({"permission":"mute_members","enabled":true}),
        )
        .await
        .error(ErrorCode::NotFound);
    let roles: Vec<Value> = server
        .get(
            &owner,
            &format!("/api/v1/workspaces/{}/roles", workspace.id),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(
        roles.iter().find(|r| r["id"] == json!(role.id)).unwrap()["permissions"],
        json!([])
    );
    let roles: Vec<Value> = server
        .get(&admin, "/api/v1/server-roles")
        .await
        .ok(StatusCode::OK);
    let high = roles
        .iter()
        .find(|r| {
            r["permissions"]
                .as_array()
                .unwrap()
                .contains(&json!("administrator"))
        })
        .unwrap();
    server
        .patch(
            &admin,
            &format!(
                "/api/v1/server-roles/{}/permissions",
                high["id"].as_str().unwrap()
            ),
            &json!({"permission":"administrator","enabled":false}),
        )
        .await
        .error(ErrorCode::Forbidden);
}

#[tokio::test]
async fn role_flags_are_private_and_permission_changes_publish_and_audit() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let guest = server.join(&owner, "Guest").await;
    let workspace = server.default_workspace(&owner).await;
    for workspace_level in [false, true] {
        let (list, permission, kind) = if workspace_level {
            (
                format!("/api/v1/workspaces/{}/roles", workspace.id),
                "mute_members",
                "workspace_role_updated",
            )
        } else {
            (
                "/api/v1/server-roles".into(),
                "create_invites",
                "server_role_updated",
            )
        };
        let role: Value = server
            .post(&owner, &list, &json!({"name":"Role"}))
            .await
            .ok(StatusCode::CREATED);
        let id = role["id"].as_str().unwrap();
        let assignment = if workspace_level {
            format!(
                "/api/v1/workspaces/{}/members/{}/roles/{id}",
                workspace.id, guest.account_id
            )
        } else {
            format!("/api/v1/accounts/{}/server-roles/{id}", guest.account_id)
        };
        server
            .put_empty(&owner, &assignment)
            .await
            .empty(StatusCode::NO_CONTENT);
        let before = last_position(&server).await;
        server
            .patch(
                &owner,
                &format!("{list}/{id}/permissions"),
                &json!({"permission":permission,"enabled":true}),
            )
            .await
            .ok::<Value>(StatusCode::OK);
        let events: Vec<Value> =
            sqlx::query_scalar("select payload from events where position>$1 order by position")
                .bind(change(before))
                .fetch_all(&server.db)
                .await
                .unwrap();
        assert!(events.iter().any(|e| e["type"] == kind));
        assert!(events.iter().any(|e| e["type"] == "permissions_changed"));
        let audits: i64 = sqlx::query_scalar(
            "select count(*) from audit_entries where action=$1 and event->>'role_id'=$2",
        )
        .bind(kind)
        .bind(id)
        .fetch_one(&server.db)
        .await
        .unwrap();
        assert_eq!(audits, 1);
        assert!(
            !serde_json::to_string(&events)
                .unwrap()
                .contains("may_assign")
        );
        let deleted = server.delete(&owner, &assignment).await;
        deleted.empty(StatusCode::NO_CONTENT);
        assert!(cursor(&deleted).is_some());
    }
    let roles: Vec<Value> = server
        .get(
            &owner,
            &format!("/api/v1/workspaces/{}/roles", workspace.id),
        )
        .await
        .ok(StatusCode::OK);
    let everyone = roles.iter().find(|r| r["is_everyone"] == true).unwrap();
    for field in ["may_assign", "may_remove", "may_delete"] {
        assert_eq!(everyone[field], false);
    }
    for path in ["/api/v1/snapshot", "/api/v1/structure"] {
        let value: Value = server.get(&owner, path).await.ok(StatusCode::OK);
        for flag in ["may_assign", "may_remove", "may_delete", "may_unban"] {
            assert!(!value.to_string().contains(flag));
        }
    }
}

#[tokio::test]
async fn a_get_receipt_survives_a_deferred_commit_failure_and_a_later_eventless_commit() {
    use axum::{Json, extract::State};
    use junto_server::{error::ApiResult, extract::Bearer, state::AppState, transact};
    async fn handler(State(app): State<AppState>, bearer: Bearer) -> ApiResult<Json<Value>> {
        let mut attempts = 0;
        transact!(app, |connection| {
            let access = junto_server::access::Access::open(connection, &bearer, app.now()).await?;
            attempts += 1;
            junto_server::events::log::tell_account(
                connection,
                app.now(),
                access.account_id,
                &Event::PermissionsChanged { workspace_id: None },
            )
            .await?;
            if attempts == 1 {
                sqlx::query("insert into receipt_commit_unique values(1),(1)")
                    .execute(&mut *connection)
                    .await?;
            }
            Ok(())
        })?;
        transact!(app, |connection| {
            sqlx::query("select 1").execute(&mut *connection).await?;
            Ok(())
        })?;
        Ok(Json(json!({"attempts":attempts})))
    }
    let server = TestServer::start().await;
    let owner = server.owner().await;
    sqlx::query(
        "create table receipt_commit_unique (value integer unique deferrable initially deferred)",
    )
    .execute(&server.db)
    .await
    .unwrap();
    let before = last_position(&server).await;
    let router = axum::Router::new()
        .route("/probe", axum::routing::get(handler))
        .layer(axum::middleware::from_fn_with_state(
            server.state.clone(),
            junto_server::receipts::middleware,
        ))
        .with_state(server.state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let serving = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let reply = reqwest::Client::new()
        .get(format!("http://{addr}/probe"))
        .bearer_auth(&owner.token)
        .send()
        .await
        .unwrap();
    assert_eq!(reply.status(), StatusCode::OK);
    let receipt: Cursor =
        serde_json::from_str(reply.headers()["junto-event-cursor"].to_str().unwrap()).unwrap();
    assert_eq!(reply.json::<Value>().await.unwrap()["attempts"], 2);
    assert_eq!(receipt.position, last_position(&server).await);
    assert_eq!(
        change(receipt.position),
        change(before) + 2,
        "the failed commit assigned a position, which was rolled back"
    );
    serving.abort();
}

#[tokio::test]
async fn audit_only_text_stays_private_and_recipient_message_length_is_bounded() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let guest = server.join(&owner, "Guest").await;
    let route = format!("/api/v1/accounts/{}/ban", guest.account_id);
    server
        .put(
            &owner,
            &route,
            &json!({"recipient_message":"x".repeat(1001)}),
        )
        .await
        .error(ErrorCode::InvalidRequest);
    server
        .put(
            &owner,
            &route,
            &json!({"recipient_message":"x".repeat(1000)}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(
        proven_notices(&server, &guest.key).await[0]["message"]
            .as_str()
            .unwrap()
            .len(),
        1000
    );
    server
        .delete(&owner, &route)
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .put(
            &owner,
            &route,
            &json!({"reason":"Private audit under the original request contract"}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    assert!(proven_notices(&server, &guest.key).await[0]["message"].is_null());
}

#[tokio::test]
async fn a_workspace_reorder_to_the_highest_position_does_not_stop_creation() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let manager = server.join(&owner, "Manager").await;
    let workspace = server.default_workspace(&owner).await;
    server
        .grant_workspace(&owner, &manager, workspace.id, &[W::ManageRoles])
        .await;
    let role = workspace_role(&server, &owner, workspace.id).await;
    let list = format!("/api/v1/workspaces/{}/roles", workspace.id);
    server
        .patch(
            &owner,
            &format!("{list}/positions"),
            &json!({"positions":[{"role_id":role.id,"position":i32::MAX}]}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .post(
            &manager,
            &list,
            &json!({"name":"Created despite the higher role"}),
        )
        .await
        .ok::<Value>(StatusCode::CREATED);
    let rows: Vec<Value> = server.get(&owner, &list).await.ok(StatusCode::OK);
    assert_eq!(
        rows.iter().find(|r| r["id"] == json!(role.id)).unwrap()["position"],
        3
    );
}

async fn receipt_of_moderation(operation: &str) {
    let mut relative = Vec::new();
    for rich in [false, true] {
        let scene = crate::calls_support::Scene::start().await;
        let (server, owner, subject) = (&scene.server, &scene.owner, &scene.ann);
        let moderator = server.join(owner, "Moderator").await;
        server
            .grant_server(owner, &moderator, &[S::BanMembers])
            .await;
        server
            .post(
                owner,
                &format!(
                    "/api/v1/workspaces/{}/members/{}/removal",
                    scene.workspace_id, moderator.account_id
                ),
                &json!({}),
            )
            .await
            .empty(StatusCode::NO_CONTENT);
        if rich {
            server.enter_direct(subject, &scene.ben).await;
            for n in 0..2 {
                let workspace: Value = server
                    .post(
                        owner,
                        "/api/v1/workspaces",
                        &json!({"name":format!("Private {n}")}),
                    )
                    .await
                    .ok(StatusCode::CREATED);
                server
                    .put_empty(
                        owner,
                        &format!(
                            "/api/v1/workspaces/{}/members/{}",
                            workspace["id"].as_str().unwrap(),
                            subject.account_id
                        ),
                    )
                    .await
                    .empty(StatusCode::NO_CONTENT);
            }
        }
        let ban = format!("/api/v1/accounts/{}/ban", subject.account_id);
        if operation == "unban" {
            server
                .put_empty(owner, &ban)
                .await
                .empty(StatusCode::NO_CONTENT);
        }
        let mut stream = server.subscribed(&moderator, Level::Full).await;
        let before = last_position(server).await;
        let reply = match operation {
            "ban" => server.put_empty(&moderator, &ban).await,
            "unban" => server.delete(&moderator, &ban).await,
            _ => {
                server
                    .post(
                        &moderator,
                        &format!("/api/v1/accounts/{}/removal", subject.account_id),
                        &json!({}),
                    )
                    .await
            }
        };
        reply.empty(StatusCode::NO_CONTENT);
        let traffic = server.traffic(&moderator, &mut stream).await;
        assert_eq!(traffic.len(), 1, "moderator's own traffic: {traffic:?}");
        assert!(matches!(traffic[0].1, Event::AccountStatusChanged { .. }));
        let receipt = cursor(&reply).unwrap();
        assert_eq!(
            change(receipt.position),
            change(traffic[0].0),
            "{operation}, private state={rich}"
        );
        relative.push(change(receipt.position) - change(before));
    }
    assert_eq!(
        relative,
        [1, 1],
        "private state must not change the receipt's relative position"
    );
}

#[tokio::test]
async fn a_ban_receipt_names_only_the_callers_last_event() {
    receipt_of_moderation("ban").await;
}
#[tokio::test]
async fn an_unban_receipt_names_only_the_callers_last_event() {
    receipt_of_moderation("unban").await;
}
#[tokio::test]
async fn a_removal_receipt_names_only_the_callers_last_event() {
    receipt_of_moderation("removal").await;
}

#[tokio::test]
async fn the_message_to_the_person_is_in_the_audit_entry() {
    for kind in ["workspace", "ban", "removal"] {
        let server = TestServer::start().await;
        let owner = server.owner().await;
        let guest = server.join(&owner, "Guest").await;
        let workspace = server.default_workspace(&owner).await;
        let route = if kind == "workspace" {
            format!(
                "/api/v1/workspaces/{}/members/{}/removal",
                workspace.id, guest.account_id
            )
        } else {
            format!("/api/v1/accounts/{}/{kind}", guest.account_id)
        };
        let body = json!({"reason":"Private audit","recipient_message":"Text sent to the person"});
        let reply = if kind == "ban" {
            server.put(&owner, &route, &body).await
        } else {
            server.post(&owner, &route, &body).await
        };
        reply.empty(StatusCode::NO_CONTENT);
        let audit_route = if kind == "workspace" {
            format!("/api/v1/workspaces/{}/audit-log", workspace.id)
        } else {
            "/api/v1/audit-log".into()
        };
        let log: Value = server.get(&owner, &audit_route).await.ok(StatusCode::OK);
        assert_eq!(log["entries"][0]["event"]["reason"], "Private audit");
        assert_eq!(
            log["entries"][0]["event"]["recipient_message"],
            "Text sent to the person"
        );
        let denied = server.get(&guest, &audit_route).await;
        assert!(!denied.status.is_success());
    }
}

#[tokio::test]
async fn notice_storage_and_wire_name_the_message() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let guest = server.join(&owner, "Guest").await;
    server
        .put(
            &owner,
            &format!("/api/v1/accounts/{}/ban", guest.account_id),
            &json!({"reason":"Audit only","recipient_message":"For the person"}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let columns: Vec<String> = sqlx::query_scalar("select column_name::text from information_schema.columns where table_name='moderation_notices'").fetch_all(&server.db).await.unwrap();
    assert!(columns.iter().any(|c| c == "message"));
    assert!(
        !columns
            .iter()
            .any(|c| c == "reason" || c == "recipient_message")
    );
    let notices = proven_notices(&server, &guest.key).await;
    assert_eq!(notices[0]["message"], "For the person");
    assert!(notices[0].get("reason").is_none());
}

#[tokio::test]
async fn a_role_reader_without_management_reads_no_channel_checks() {
    let server = TestServer::start_measured_with(|c| {
        c.limits.stream.poll_interval = time::Duration::hours(1)
    })
    .await;
    let owner = server.owner().await;
    let guest = server.join(&owner, "Guest").await;
    let workspace = server.default_workspace(&owner).await;
    let mut observer = server.db.acquire().await.unwrap();
    let role: i64 =
        sqlx::query_scalar("select oid::bigint from pg_roles where rolname=current_user")
            .fetch_one(&mut *observer)
            .await
            .unwrap();
    let before = crate::messages_queries::statements(&mut observer, role).await;
    let roles: Vec<Value> = server
        .get(
            &guest,
            &format!("/api/v1/workspaces/{}/roles", workspace.id),
        )
        .await
        .ok(StatusCode::OK);
    assert!(
        roles.iter().all(|r| r["may_assign"] == false
            && r["may_remove"] == false
            && r["may_delete"] == false)
    );
    let queries = crate::messages_queries::since(
        crate::messages_queries::statements(&mut observer, role).await,
        before,
    );
    assert!(
        !queries
            .keys()
            .any(|q| q.contains("from channels") || q.contains("from channel_overrides")),
        "{queries:?}"
    );
}

#[tokio::test]
async fn no_receivable_event_is_null_and_skips_unneeded_epoch_reads() {
    use axum::{
        Json,
        extract::{Path, State},
    };
    use junto_server::{error::ApiResult, extract::Bearer, state::AppState, transact};
    async fn handler(
        State(app): State<AppState>,
        bearer: Bearer,
        Path((mode, target)): Path<(u8, junto_protocol::ids::AccountId)>,
    ) -> ApiResult<Json<Value>> {
        transact!(app, |connection| {
            if mode != 0 {
                junto_server::access::Access::open(connection, &bearer, app.now()).await?;
            }
            if mode != 1 {
                junto_server::events::log::tell_account(
                    connection,
                    app.now(),
                    target,
                    &Event::PermissionsChanged { workspace_id: None },
                )
                .await?;
            }
            Ok(())
        })?;
        Ok(Json(json!({})))
    }
    let server = TestServer::start_measured_with(|c| {
        c.limits.stream.poll_interval = time::Duration::hours(1)
    })
    .await;
    let owner = server.owner().await;
    let guest = server.join(&owner, "Guest").await;
    let router = axum::Router::new()
        .route("/probe/{mode}/{target}", axum::routing::post(handler))
        .layer(axum::middleware::from_fn_with_state(
            server.state.clone(),
            junto_server::receipts::middleware,
        ))
        .with_state(server.state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let serving = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let mut observer = server.db.acquire().await.unwrap();
    let role: i64 =
        sqlx::query_scalar("select oid::bigint from pg_roles where rolname=current_user")
            .fetch_one(&mut *observer)
            .await
            .unwrap();
    for mode in [0, 1, 2] {
        let before = crate::messages_queries::statements(&mut observer, role).await;
        let reply = reqwest::Client::new()
            .post(format!("http://{addr}/probe/{mode}/{}", guest.account_id))
            .bearer_auth(&owner.token)
            .send()
            .await
            .unwrap();
        assert_eq!(reply.status(), StatusCode::OK);
        if mode == 0 {
            assert!(reply.headers().get("junto-event-cursor").is_none());
        } else {
            assert_eq!(reply.headers()["junto-event-cursor"], "null");
        }
        if mode != 2 {
            let queries = crate::messages_queries::since(
                crate::messages_queries::statements(&mut observer, role).await,
                before,
            );
            assert!(
                !queries.keys().any(|q| q.contains("from event_stream")),
                "{queries:?}"
            );
        }
    }
    serving.abort();
}

#[tokio::test]
async fn a_failed_receipt_read_does_not_refuse_a_committed_change() {
    let server = TestServer::start_with(|config| {
        config.limits.database.lock = time::Duration::seconds(1);
        config.limits.stream.poll_interval = time::Duration::hours(1);
    })
    .await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let before = last_position(&server).await;
    let mut lock = server.db.begin().await.unwrap();
    sqlx::query("lock table notification_receivers in access exclusive mode")
        .execute(&mut *lock)
        .await
        .unwrap();
    let reply = server
        .put(
            &owner,
            &format!("/api/v1/workspaces/{}/notification-default", workspace.id),
            &json!({"level":"all"}),
        )
        .await;
    lock.rollback().await.unwrap();
    reply.ok::<Value>(StatusCode::OK);
    assert!(reply.header("junto-event-cursor").is_none());
    let level: String = sqlx::query_scalar(
        "select level from workspace_notification_defaults where workspace_id=$1",
    )
    .bind(workspace.id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(level, "all");
    let count: i64 = sqlx::query_scalar("select count(*) from events where position>$1")
        .bind(change(before))
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(count, 1, "the committed change must not be executed again");
}
