use junto_protocol::auth::ChallengeOperation;
use junto_protocol::error::ErrorCode;
use junto_protocol::invite::JoinRequest;
use junto_protocol::roles::WorkspaceRole;
use junto_protocol::structure::{Channel, ChannelList, ChannelOverride};
use junto_protocol::{ServerPermission, WorkspacePermission};
use junto_server::error::ApiResult;
use junto_server::state::AppState;
use junto_server::{read, transact};
use reqwest::StatusCode;
use serde_json::json;

use crate::structure::set_override;
use crate::support::{DeviceKey, TestServer, drive_until, join_all};

use WorkspacePermission as W;

const ISOLATION: &str = "select current_setting('transaction_isolation'),
                                  current_setting('transaction_read_only')";

async fn isolation_of_a_read(state: &AppState) -> ApiResult<(String, String)> {
    read!(state, |connection| {
        Ok(sqlx::query_as(ISOLATION).fetch_one(connection).await?)
    })
}

async fn isolation_of_a_write(state: &AppState) -> ApiResult<(String, String)> {
    transact!(state, |connection| {
        Ok(sqlx::query_as(ISOLATION).fetch_one(connection).await?)
    })
}

#[tokio::test]
async fn every_transaction_of_the_server_is_serializable() {
    let server = TestServer::start().await;
    let default: String = sqlx::query_scalar("select current_setting('transaction_isolation')")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(default, "serializable");
    assert_eq!(
        isolation_of_a_read(&server.state).await.unwrap(),
        ("serializable".to_owned(), "on".to_owned())
    );
    assert_eq!(
        isolation_of_a_write(&server.state).await.unwrap(),
        ("serializable".to_owned(), "off".to_owned())
    );
}

/// A transaction that ends with `sqlstate` on each of its first `losses` attempts.
async fn losing(
    state: &AppState,
    sqlstate: &'static str,
    losses: u32,
    attempts: &mut u32,
) -> ApiResult<i32> {
    transact!(state, |connection| {
        *attempts += 1;
        if *attempts <= losses {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "do $$ begin raise exception using errcode = '{sqlstate}'; end $$"
            )))
            .execute(&mut *connection)
            .await?;
        }
        Ok(sqlx::query_scalar("select 7").fetch_one(connection).await?)
    })
}

#[tokio::test]
async fn a_transaction_that_loses_runs_again_a_bounded_number_of_times() {
    let server = TestServer::start().await;

    for sqlstate in ["40001", "40P01"] {
        let mut attempts = 0;
        let value = losing(&server.state, sqlstate, 2, &mut attempts).await;
        assert_eq!(value.unwrap(), 7);
        assert_eq!(attempts, 3);
    }

    let mut attempts = 0;
    let error = losing(&server.state, "40001", u32::MAX, &mut attempts)
        .await
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::Busy);
    assert_eq!(attempts, 8);

    let mut attempts = 0;
    let error = losing(&server.state, "23505", u32::MAX, &mut attempts)
        .await
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::Internal);
    assert_eq!(attempts, 1);
}

#[tokio::test]
async fn a_signed_request_is_sent_again_after_busy_and_is_used_once() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let invite = server.invite(&owner, json!({})).await;
    let newcomer = DeviceKey::generate();

    let login = json!({ "proof": server.proof(&owner.key, ChallengeOperation::Login).await });
    let join = json!({
        "proof": server.proof(&newcomer, ChallengeOperation::Join).await,
        "invite_code": invite.code,
        "name": "Newcomer",
    });
    for (path, body) in [("/api/v1/auth/sessions", &login), ("/api/v1/join", &join)] {
        server.gate().lose_next(8);
        let busy = server.open(path, body).await;
        busy.error(ErrorCode::Busy);
        assert_eq!(busy.retry_after.as_deref(), Some("1"));
        assert!(busy.text().contains("same request"), "{}", busy.text());

        let accepted = server.open(path, body).await;
        assert_eq!(
            accepted.status,
            StatusCode::CREATED,
            "{path}: {}",
            accepted.text()
        );
        server
            .open(path, body)
            .await
            .error(ErrorCode::ChallengeInvalid);
    }
    let accounts: i64 = sqlx::query_scalar("select count(*) from accounts")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(accounts, 2);
}

#[tokio::test]
async fn a_ban_cancels_an_invite_whose_creation_overlaps_it() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let guest = server.join(&owner, "Guest").await;
    server
        .grant_server(&owner, &guest, &[ServerPermission::CreateInvites])
        .await;
    let ban_path = format!("/api/v1/accounts/{}/ban", guest.account_id);

    server.gate().arm(0);
    let ban = server.put_empty(&owner, &ban_path);
    tokio::pin!(ban);
    drive_until(&mut ban, server.gate().arrived()).await;

    let empty = json!({});
    let invite = server.post(&guest, "/api/v1/invites", &empty);
    tokio::pin!(invite);
    drive_until(&mut invite, server.until_blocked()).await;

    server.gate().release();
    ban.await.empty(StatusCode::NO_CONTENT);
    invite.await.error(ErrorCode::Unauthenticated);

    let usable: i64 = sqlx::query_scalar(
        "select count(*) from invites where created_by = $1 and revoked_at is null",
    )
    .bind(guest.account_id.as_uuid())
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(usable, 0);
}

#[tokio::test]
async fn a_ban_ends_a_session_whose_login_overlaps_it() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let guest = server.join(&owner, "Guest").await;
    let ban_path = format!("/api/v1/accounts/{}/ban", guest.account_id);
    let proof = server.proof(&guest.key, ChallengeOperation::Login).await;

    server.gate().arm(0);
    let ban = server.put_empty(&owner, &ban_path);
    tokio::pin!(ban);
    drive_until(&mut ban, server.gate().arrived()).await;

    let login = server
        .open("/api/v1/auth/sessions", &json!({ "proof": proof }))
        .await;
    assert_eq!(login.status, StatusCode::CREATED, "{}", login.text());

    server.gate().release();
    ban.await.empty(StatusCode::NO_CONTENT);

    let sessions: i64 = sqlx::query_scalar(
        "select count(*) from sessions s join devices d on d.id = s.device_id
         where d.account_id = $1",
    )
    .bind(guest.account_id.as_uuid())
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(sessions, 0);
}

#[tokio::test]
async fn a_role_manager_cannot_restore_a_permission_revoked_during_the_request() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let manager = server.join(&owner, "Manager").await;
    server
        .grant_workspace(&owner, &manager, workspace.id, &[])
        .await;
    let role_id = server
        .grant_workspace(
            &owner,
            &manager,
            workspace.id,
            &[W::ManageRoles, W::ExportChannel],
        )
        .await;
    let path = format!("/api/v1/workspaces/{}/roles/{role_id}", workspace.id);

    server.gate().arm(0);
    let narrowed = json!({ "permissions": ["manage_roles"] });
    let revoke = server.patch(&owner, &path, &narrowed);
    tokio::pin!(revoke);
    drive_until(&mut revoke, server.gate().arrived()).await;

    let widened = json!({ "permissions": ["manage_roles", "export_channel"] });
    let restore = server.patch(&manager, &path, &widened);
    tokio::pin!(restore);
    drive_until(&mut restore, server.until_blocked()).await;

    server.gate().release();
    revoke.await.ok::<WorkspaceRole>(StatusCode::OK);
    restore.await.error(ErrorCode::ExceedsOwnPermissions);

    let roles: Vec<WorkspaceRole> = server
        .get(
            &owner,
            &format!("/api/v1/workspaces/{}/roles", workspace.id),
        )
        .await
        .ok(StatusCode::OK);
    let role = roles.iter().find(|role| role.id == role_id).unwrap();
    assert_eq!(role.permissions, [W::ManageRoles]);
}

struct PrivateChannel {
    server: TestServer,
    owner: crate::support::Member,
    outsider: crate::support::Member,
    workspace_id: junto_protocol::ids::WorkspaceId,
    channel: Channel,
}

async fn private_channel() -> PrivateChannel {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let outsider = server.join(&owner, "Outsider").await;
    server
        .grant_workspace(&owner, &outsider, workspace.id, &[W::ViewAuditEntries])
        .await;
    let everyone = server
        .get(
            &owner,
            &format!("/api/v1/workspaces/{}/roles", workspace.id),
        )
        .await
        .ok::<Vec<WorkspaceRole>>(StatusCode::OK)
        .into_iter()
        .find(|role| role.is_everyone)
        .unwrap();
    let channel: Channel = server
        .post(
            &owner,
            &format!("/api/v1/workspaces/{}/channels", workspace.id),
            &json!({ "name": "boardroom", "kind": "text" }),
        )
        .await
        .ok(StatusCode::CREATED);
    set_override(
        &server,
        &owner,
        channel.id,
        everyone.id,
        &[],
        &[W::ViewChannel],
    )
    .await
    .ok::<ChannelOverride>(StatusCode::OK);
    PrivateChannel {
        server,
        owner,
        outsider,
        workspace_id: workspace.id,
        channel,
    }
}

/// Deletes the channel as the deletion route does, in a transaction that holds every reader
/// of the overrides until it commits.
async fn delete_while_readers_wait(
    world: &PrivateChannel,
) -> sqlx::Transaction<'static, sqlx::Postgres> {
    let mut deletion = world.server.db.begin().await.unwrap();
    for statement in [
        "lock table channel_overrides in access exclusive mode",
        "update channels set deleted_at = created_at where id = $1",
        "delete from channel_overrides where channel_id = $1",
    ] {
        sqlx::query(statement)
            .bind(world.channel.id.as_uuid())
            .execute(&mut *deletion)
            .await
            .unwrap();
    }
    sqlx::query(
        "insert into audit_entries
             (id, occurred_at, actor_id, workspace_id, channel_id, action, event)
         values (gen_random_uuid(), now(), $1, $2, $3, 'channel_deleted',
                 jsonb_build_object('action', 'channel_deleted', 'name', 'boardroom'))",
    )
    .bind(world.owner.account_id.as_uuid())
    .bind(world.workspace_id.as_uuid())
    .bind(world.channel.id.as_uuid())
    .execute(&mut *deletion)
    .await
    .unwrap();
    deletion
}

#[tokio::test]
async fn a_private_channel_deleted_during_a_listing_is_not_shown() {
    let world = private_channel().await;
    let server = &world.server;
    let deletion = delete_while_readers_wait(&world).await;

    let path = format!("/api/v1/workspaces/{}/channels", world.workspace_id);
    let listing = server.get(&world.outsider, &path);
    tokio::pin!(listing);
    drive_until(&mut listing, server.until_blocked()).await;
    deletion.commit().await.unwrap();

    let reply = listing.await;
    assert!(!reply.text().contains("boardroom"), "{}", reply.text());
    let list: ChannelList = reply.ok(StatusCode::OK);
    assert!(
        list.channels
            .iter()
            .all(|channel| channel.id != world.channel.id)
    );
}

#[tokio::test]
async fn a_private_channel_deleted_during_an_audit_read_keeps_its_entries_hidden() {
    let world = private_channel().await;
    let server = &world.server;
    let deletion = delete_while_readers_wait(&world).await;

    let path = format!("/api/v1/workspaces/{}/audit-log", world.workspace_id);
    let log = server.get(&world.outsider, &path);
    tokio::pin!(log);
    drive_until(&mut log, server.until_blocked()).await;
    deletion.commit().await.unwrap();

    let reply = log.await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert!(!reply.text().contains("boardroom"), "{}", reply.text());
    assert!(
        !reply.text().contains(&world.channel.id.to_string()),
        "{}",
        reply.text()
    );
}

#[tokio::test]
async fn the_ceiling_on_waiting_join_requests_holds_under_concurrency() {
    let server = TestServer::start_with(|config| {
        config.limits.most_pending_join_requests = 1;
    })
    .await;
    let first_proof = server
        .proof(&DeviceKey::generate(), ChallengeOperation::RequestToJoin)
        .await;
    let second_proof = server
        .proof(&DeviceKey::generate(), ChallengeOperation::RequestToJoin)
        .await;

    server.gate().arm(0);
    let first_body = json!({ "proof": first_proof, "name": "First" });
    let first = server.open("/api/v1/join-requests", &first_body);
    tokio::pin!(first);
    drive_until(&mut first, server.gate().arrived()).await;

    server
        .open(
            "/api/v1/join-requests",
            &json!({ "proof": second_proof, "name": "Second" }),
        )
        .await
        .ok::<JoinRequest>(StatusCode::CREATED);

    server.gate().release();
    first.await.error(ErrorCode::JoinRequestsFull);

    let waiting: i64 =
        sqlx::query_scalar("select count(*) from join_requests where state = 'pending'")
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(waiting, 1);
}

#[tokio::test]
async fn concurrent_channel_edits_keep_the_fields_of_each_other() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let channel: Channel = server
        .post(
            &owner,
            &format!("/api/v1/workspaces/{}/channels", workspace.id),
            &json!({ "name": "records", "kind": "text" }),
        )
        .await
        .ok(StatusCode::CREATED);
    let path = format!("/api/v1/channels/{}", channel.id);

    server.gate().arm(0);
    let setting = json!({ "permanent_messages": true });
    let permanent = server.patch(&owner, &path, &setting);
    tokio::pin!(permanent);
    drive_until(&mut permanent, server.gate().arrived()).await;

    let new_name = json!({ "name": "ledger" });
    let rename = server.patch(&owner, &path, &new_name);
    tokio::pin!(rename);
    drive_until(&mut rename, server.until_blocked()).await;

    server.gate().release();
    permanent.await.ok::<Channel>(StatusCode::OK);
    let renamed: Channel = rename.await.ok(StatusCode::OK);
    assert!(renamed.permanent_messages);

    let (name, permanent_messages): (String, bool) =
        sqlx::query_as("select name, permanent_messages from channels where id = $1")
            .bind(channel.id.as_uuid())
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!((name.as_str(), permanent_messages), ("ledger", true));
}

#[tokio::test]
async fn a_channel_edit_that_overlaps_the_deletion_does_not_succeed() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let channel: Channel = server
        .post(
            &owner,
            &format!("/api/v1/workspaces/{}/channels", workspace.id),
            &json!({ "name": "records", "kind": "text" }),
        )
        .await
        .ok(StatusCode::CREATED);
    let path = format!("/api/v1/channels/{}", channel.id);

    server.gate().arm(0);
    let deletion = server.delete(&owner, &path);
    tokio::pin!(deletion);
    drive_until(&mut deletion, server.gate().arrived()).await;

    let new_name = json!({ "name": "ledger" });
    let rename = server.patch(&owner, &path, &new_name);
    tokio::pin!(rename);
    drive_until(&mut rename, server.until_blocked()).await;

    server.gate().release();
    deletion.await.empty(StatusCode::NO_CONTENT);
    rename.await.error(ErrorCode::NotFound);

    let name: String = sqlx::query_scalar("select name from channels where id = $1")
        .bind(channel.id.as_uuid())
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(name, "records");
}

#[tokio::test]
async fn many_simultaneous_logins_all_succeed() {
    let server = TestServer::start_with(|config| {
        config.limits.join_attempts.attempts = 1_000;
    })
    .await;
    let owner = server.owner().await;
    let mut members = Vec::new();
    for index in 0..24 {
        members.push(server.join(&owner, &format!("Member {index}")).await);
    }
    let logins = members
        .iter()
        .map(|member| server.login_reply(&member.key))
        .collect();
    for reply in join_all(logins).await {
        assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.text());
    }
}

/// The session token of a login is made inside its transaction. A login whose first
/// attempts lose stores one session, and the token of the answer is the one that works
/// (ADR 0009).
#[tokio::test]
async fn a_login_that_runs_again_stores_one_session_whose_token_works() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let sessions = |device_id| {
        sqlx::query_scalar::<_, i64>("select count(*) from sessions where device_id = $1")
            .bind(device_id)
            .fetch_one(&server.db)
    };
    let before = sessions(owner.device_id).await.unwrap();
    let login = json!({ "proof": server.proof(&owner.key, ChallengeOperation::Login).await });
    server.gate().lose_next(2);
    let created: junto_protocol::auth::SessionCreated = server
        .open("/api/v1/auth/sessions", &login)
        .await
        .ok(StatusCode::CREATED);
    assert_eq!(
        server.gate().losses_left(),
        0,
        "the login did not run again"
    );
    assert_eq!(sessions(owner.device_id).await.unwrap(), before + 1);
    let logged_in = crate::support::Member {
        // A request with the token signs nothing.
        key: DeviceKey::generate(),
        account_id: owner.account_id,
        device_id: owner.device_id,
        token: created.token.expose().to_owned(),
    };
    server
        .get(&logged_in, "/api/v1/account")
        .await
        .ok::<junto_protocol::account::OwnAccount>(StatusCode::OK);
}
