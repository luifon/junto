//! Proofs of the audit of the whole server: answers that tell about a thing that the caller
//! cannot see. Each test fails while its defect exists.

use junto_protocol::ServerPermission;
use junto_protocol::invite::Invite;
use junto_protocol::roles::WorkspaceRole;
use junto_protocol::structure::Workspace;

use crate::discovery_support::*;
use crate::support::DeviceKey;

/// ADR 0205: a hidden object and an absent object get the same answer. The cursor of the
/// notice list is checked against the stored notice only, so the answer changes when the
/// message of a notice is deleted in a channel that the account can no longer see.
#[tokio::test]
async fn audit_a_notice_cursor_does_not_tell_whether_a_hidden_message_still_exists() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    let noticed = mention(&server, &owner, &channel, &reader).await;
    assert_eq!(notices(&server, &reader).await.len(), 1);
    let everyone = server.everyone_role(&owner, channel.workspace_id).await;
    server
        .put(
            &owner,
            &format!("/api/v1/channels/{}/overrides/{}", channel.id, everyone.id),
            &json!({"allow":[],"deny":["view_channel"]}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    assert!(notices(&server, &reader).await.is_empty());

    let page = format!("/api/v1/notifications?before={}", noticed.id);
    let while_it_exists = server.get(&reader, &page).await;
    server
        .delete(&owner, &message(noticed.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    let after_its_deletion = server.get(&reader, &page).await;
    assert_eq!(
        while_it_exists.observable(),
        after_its_deletion.observable(),
        "the answer tells that the message was deleted"
    );
}

/// The cursor of the notice list is a notice that the caller reads now. A notice of a
/// channel that the caller no longer sees gets the complete answer of a cursor that never
/// existed, and a notice that the caller reads is still a cursor.
#[tokio::test]
async fn a_notice_that_the_caller_cannot_read_is_no_cursor_of_the_notice_list() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let open = channel(&server, &owner, "text").await;
    let closing = channel(&server, &owner, "text").await;
    let older = mention(&server, &owner, &open, &reader).await;
    server.clock.advance(time::Duration::seconds(1));
    let hidden = mention(&server, &owner, &closing, &reader).await;
    server.clock.advance(time::Duration::seconds(1));
    let newer = mention(&server, &owner, &open, &reader).await;
    let page = |cursor: uuid::Uuid| format!("/api/v1/notifications?before={cursor}");

    let listed: NotificationPage = server
        .get(&reader, &page(hidden.id.as_uuid()))
        .await
        .ok(StatusCode::OK);
    let ids = |page: &NotificationPage| -> Vec<MessageId> {
        page.notifications
            .iter()
            .map(|notice| notice.message_id)
            .collect()
    };
    assert_eq!(ids(&listed), [older.id]);

    let everyone = server.everyone_role(&owner, closing.workspace_id).await;
    server
        .put(
            &owner,
            &format!("/api/v1/channels/{}/overrides/{}", closing.id, everyone.id),
            &json!({"allow":[],"deny":["view_channel"]}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    let of_hidden = server.get(&reader, &page(hidden.id.as_uuid())).await;
    let of_absent = server.get(&reader, &page(uuid::Uuid::now_v7())).await;
    of_hidden.error(ErrorCode::InvalidRequest);
    assert_eq!(of_hidden.observable(), of_absent.observable());
    // The notice of another account is no cursor either.
    let of_other = server.get(&owner, &page(newer.id.as_uuid())).await;
    assert_eq!(of_other.observable(), of_absent.observable());

    let listed: NotificationPage = server
        .get(&reader, &page(newer.id.as_uuid()))
        .await
        .ok(StatusCode::OK);
    assert_eq!(ids(&listed), [older.id]);
}

/// The list of invites returns the workspaces of every invite to a holder of
/// `create_invites`, also the workspaces that this holder is not a member of and gets
/// `not_found` for.
#[tokio::test]
async fn audit_the_invite_list_does_not_name_a_workspace_that_its_reader_cannot_see() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let inviter = server.join(&owner, "Inviter").await;
    server
        .grant_server(&owner, &inviter, &[ServerPermission::CreateInvites])
        .await;
    let board: Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({"name":"board"}))
        .await
        .ok(StatusCode::CREATED);
    server
        .invite(&owner, json!({ "workspace_ids": [board.id] }))
        .await;
    server
        .get(
            &inviter,
            &format!("/api/v1/workspaces/{}/channels", board.id),
        )
        .await
        .error(ErrorCode::NotFound);

    let listed = server.get(&inviter, "/api/v1/invites").await.text();
    assert!(
        !listed.contains(&board.id.to_string()),
        "the list names a workspace that the reader cannot see: {listed}"
    );
}

async fn board(server: &TestServer, owner: &Member) -> Workspace {
    server
        .post(owner, "/api/v1/workspaces", &json!({"name":"board"}))
        .await
        .ok(StatusCode::CREATED)
}

async fn inviter(
    server: &TestServer,
    owner: &Member,
    name: &str,
    workspaces: &[WorkspaceId],
) -> Member {
    let member = server.join_into(owner, name, workspaces).await;
    server
        .grant_server(owner, &member, &[ServerPermission::CreateInvites])
        .await;
    member
}

async fn listed_invites(server: &TestServer, reader: &Member) -> Vec<InviteId> {
    let invites: Vec<Invite> = server
        .get(reader, "/api/v1/invites")
        .await
        .ok(StatusCode::OK);
    invites.into_iter().map(|invite| invite.id).collect()
}

fn of_invites(events: Vec<(u64, Event)>) -> Vec<&'static str> {
    events
        .iter()
        .map(|(_, event)| event.name())
        .filter(|name| name.starts_with("invite_"))
        .collect()
}

/// ADR 0111: an invite is revoked by a holder of `create_invites` who reads it. For a
/// holder who cannot see a workspace of the invite, the invite does not exist: the complete
/// answer is the one for an identifier that no invite has, and the invite stays valid.
#[tokio::test]
async fn an_invite_is_revoked_only_by_an_account_that_reads_it() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let home = server.default_workspace(&owner).await;
    let board = board(&server, &owner).await;
    let outside = inviter(&server, &owner, "Outside", &[home.id]).await;
    let inside = inviter(&server, &owner, "Inside", &[home.id, board.id]).await;
    let invite = server
        .invite(&owner, json!({ "workspace_ids": [home.id, board.id] }))
        .await
        .invite;
    let path = |id: InviteId| format!("/api/v1/invites/{id}");

    let hidden = server.delete(&outside, &path(invite.id)).await;
    let absent = server.delete(&outside, &path(InviteId::generate())).await;
    hidden.error(ErrorCode::NotFound);
    assert_eq!(hidden.observable(), absent.observable());
    assert!(!listed_invites(&server, &outside).await.contains(&invite.id));
    let revoked: bool =
        sqlx::query_scalar("select revoked_at is not null from invites where id = $1")
            .bind(invite.id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert!(!revoked);

    assert!(listed_invites(&server, &inside).await.contains(&invite.id));
    server
        .delete(&inside, &path(invite.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    // The repeated request of a reader is answered as before, and the one of an account
    // that does not read the invite stays `not_found`.
    server
        .delete(&inside, &path(invite.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    let again = server.delete(&outside, &path(invite.id)).await;
    assert_eq!(again.observable(), absent.observable());
}

/// ADR 0208: the events of an invite go to the accounts that read the invite. A holder of
/// `create_invites` outside a workspace of the invite receives none of them, also not in a
/// replay; a holder who sees every workspace of the invite receives all three.
#[tokio::test]
async fn the_events_of_an_invite_reach_only_the_accounts_that_read_it() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let home = server.default_workspace(&owner).await;
    let board = board(&server, &owner).await;
    let outside = inviter(&server, &owner, "Outside", &[home.id]).await;
    let inside = inviter(&server, &owner, "Inside", &[home.id, board.id]).await;
    let cursor = server.snapshot(&outside).await.cursor;
    let mut outside_stream = server.subscribed(&outside, Level::Full).await;
    let mut inside_stream = server.subscribed(&inside, Level::Full).await;

    for workspace_ids in [vec![board.id], vec![home.id]] {
        let created = server
            .invite(
                &owner,
                json!({ "workspace_ids": workspace_ids, "max_uses": 2 }),
            )
            .await;
        server
            .join_reply(&DeviceKey::generate(), created.code.expose(), "Guest")
            .await
            .ok::<Value>(StatusCode::CREATED);
        server
            .delete(&owner, &format!("/api/v1/invites/{}", created.invite.id))
            .await
            .empty(StatusCode::NO_CONTENT);
    }

    let all = ["invite_created", "invite_used", "invite_revoked"];
    let seen = of_invites(server.traffic(&inside, &mut inside_stream).await);
    assert_eq!(seen, [all, all].concat());
    let seen = of_invites(server.traffic(&outside, &mut outside_stream).await);
    assert_eq!(seen, all);
    let mut replay = server.stream(&outside).await;
    replay.subscribe(Level::Full, Some(cursor)).await;
    let replayed = server.traffic(&outside, &mut replay).await;
    assert_eq!(of_invites(replayed.clone()), all);
    let text = serde_json::to_string(&replayed).unwrap();
    assert!(!text.contains(&board.id.to_string()), "{text}");
}

/// ADR 0111: the account that created an invite reads it while it holds `create_invites`,
/// also when it no longer sees a workspace of the invite. It names nothing that this
/// account did not name itself.
#[tokio::test]
async fn the_creator_of_an_invite_reads_it_without_sight_of_its_workspaces() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let board = board(&server, &owner).await;
    let creator = server.join(&owner, "Creator").await;
    let administrator = server
        .grant_server(&owner, &creator, &[ServerPermission::Administrator])
        .await;
    let invite = server
        .invite(&creator, json!({ "workspace_ids": [board.id] }))
        .await
        .invite;
    server
        .grant_server(&owner, &creator, &[ServerPermission::CreateInvites])
        .await;
    server
        .delete(
            &owner,
            &format!(
                "/api/v1/accounts/{}/server-roles/{administrator}",
                creator.account_id
            ),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .get(
            &creator,
            &format!("/api/v1/workspaces/{}/channels", board.id),
        )
        .await
        .error(ErrorCode::NotFound);

    let mut stream = server.subscribed(&creator, Level::Full).await;
    assert!(listed_invites(&server, &creator).await.contains(&invite.id));
    server
        .delete(&creator, &format!("/api/v1/invites/{}", invite.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    let seen = of_invites(server.traffic(&creator, &mut stream).await);
    assert_eq!(seen, ["invite_revoked"]);
}

/// The routes that give a server role or a membership refuse an account that is not
/// active. The route that gives a workspace role checks the membership only, and a banned
/// account keeps its memberships (ADR 0113), so it gets the role and holds it at an unban.
#[tokio::test]
async fn audit_a_workspace_role_is_not_given_to_a_banned_account() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let target = server.join(&owner, "Target").await;
    let workspace = server.default_workspace(&owner).await;
    let role: WorkspaceRole = server
        .post(
            &owner,
            &format!("/api/v1/workspaces/{}/roles", workspace.id),
            &json!({ "name": "moderators", "permissions": ["manage_messages"] }),
        )
        .await
        .ok(StatusCode::CREATED);
    server
        .put_empty(
            &owner,
            &format!("/api/v1/accounts/{}/ban", target.account_id),
        )
        .await
        .empty(StatusCode::NO_CONTENT);

    let given = server
        .put_empty(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}/roles/{}",
                workspace.id, target.account_id, role.id
            ),
        )
        .await;
    let held: i64 = sqlx::query_scalar("select count(*) from member_roles where account_id = $1")
        .bind(target.account_id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(
        held, 0,
        "the banned account got the role; the answer was {}",
        given.status
    );
}

/// The refusal for a banned member is `conflict`, as for a server role. An account that is
/// not a member of the workspace gets `not_found` whatever its state, so the route does not
/// tell the state of an account outside the workspace. After the unban the role is given.
#[tokio::test]
async fn a_workspace_role_is_refused_for_a_member_that_is_not_active() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let board = board(&server, &owner).await;
    let member = server.join_into(&owner, "Member", &[board.id]).await;
    let stranger = server.join(&owner, "Stranger").await;
    let role: WorkspaceRole = server
        .post(
            &owner,
            &format!("/api/v1/workspaces/{}/roles", board.id),
            &json!({ "name": "moderators", "permissions": ["manage_messages"] }),
        )
        .await
        .ok(StatusCode::CREATED);
    let assignment = |account: &Member| {
        format!(
            "/api/v1/workspaces/{}/members/{}/roles/{}",
            board.id, account.account_id, role.id
        )
    };
    for account in [&member, &stranger] {
        let ban = format!("/api/v1/accounts/{}/ban", account.account_id);
        server
            .put_empty(&owner, &ban)
            .await
            .empty(StatusCode::NO_CONTENT);
    }

    server
        .put_empty(&owner, &assignment(&member))
        .await
        .error(ErrorCode::Conflict);
    server
        .put_empty(&owner, &assignment(&stranger))
        .await
        .error(ErrorCode::NotFound);
    assert_eq!(server.count("member_roles").await, 0);

    let ban = format!("/api/v1/accounts/{}/ban", member.account_id);
    server
        .delete(&owner, &ban)
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .put_empty(&owner, &assignment(&member))
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(server.count("member_roles").await, 1);
}

/// The default of the notifications of one workspace is a setting of that workspace. Its
/// change is told to the accounts that see the workspace, and to no account outside it.
#[tokio::test]
async fn a_change_of_the_notification_default_of_a_workspace_reaches_its_members_only() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let home = server.default_workspace(&owner).await;
    let board = board(&server, &owner).await;
    let inside = server.join_into(&owner, "Inside", &[board.id]).await;
    let outside = server.join_into(&owner, "Outside", &[home.id]).await;
    let mut of_inside = server.subscribed(&inside, Level::Full).await;
    let mut of_outside = server.subscribed(&outside, Level::Full).await;

    server
        .put(
            &owner,
            &format!("/api/v1/workspaces/{}/notification-default", board.id),
            &json!({ "level": "all" }),
        )
        .await
        .ok::<Value>(StatusCode::OK);

    let changes = |events: Vec<(u64, Event)>| {
        events
            .into_iter()
            .filter(|(_, event)| *event == Event::NotificationSettingsChanged {})
            .count()
    };
    assert_eq!(changes(server.traffic(&inside, &mut of_inside).await), 1);
    assert_eq!(changes(server.traffic(&outside, &mut of_outside).await), 0);
}

/// ADR 0305: an audit entry refers to an invite by its identifier and states the account
/// that created the invite. The entry of a revocation named the invite only.
#[tokio::test]
async fn audit_the_entry_of_a_revoked_invite_states_the_account_that_created_the_invite() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let inviter = server.join(&owner, "Inviter").await;
    server
        .grant_server(&owner, &inviter, &[ServerPermission::CreateInvites])
        .await;
    let created = server.invite(&inviter, json!({})).await;
    server
        .delete(&owner, &format!("/api/v1/invites/{}", created.invite.id))
        .await
        .empty(StatusCode::NO_CONTENT);

    let log: Value = server
        .get(&owner, "/api/v1/audit-log?limit=200")
        .await
        .ok(StatusCode::OK);
    let revoked = log["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["event"]["action"] == "invite_revoked")
        .expect("the entry of the revocation");
    assert_eq!(revoked["actor_id"], json!(owner.account_id));
    assert_eq!(
        revoked["event"]["invite_created_by"],
        json!(inviter.account_id),
        "the entry does not state who created the invite: {revoked}"
    );
}

/// ADR 0111: an event about an invite is for the accounts that read the invite. The events
/// that were stored before that rule went to every holder of `create_invites`. Migration
/// 0013 gives them the audience of their invite, so an inviter who cannot see a workspace
/// of an invite does not replay its events from the kept log.
#[tokio::test]
async fn audit_the_stored_events_of_invites_get_the_audience_of_their_invite() {
    use junto_protocol::events::{Event, Level};

    let server = TestServer::start().await;
    let owner = server.owner().await;
    let inviter = server.join(&owner, "Inviter").await;
    server
        .grant_server(&owner, &inviter, &[ServerPermission::CreateInvites])
        .await;
    let board: Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({"name":"board"}))
        .await
        .ok(StatusCode::CREATED);
    let before = server.snapshot(&inviter).await.cursor;
    let created = server
        .invite(&owner, json!({ "workspace_ids": [board.id] }))
        .await;
    server
        .delete(&owner, &format!("/api/v1/invites/{}", created.invite.id))
        .await
        .empty(StatusCode::NO_CONTENT);

    // The state before the rule: both events for every holder of `create_invites`.
    let earlier = "alter table events disable trigger events_no_change;
         update events set audience = '{\"to\":\"server_permission\",\"permission\":\"create_invites\"}'
         where kind in ('invite_created', 'invite_revoked');
         alter table events enable trigger events_no_change;";
    sqlx::raw_sql(earlier).execute(&server.db).await.unwrap();
    let replayed = |member: &'static str| {
        let member = if member == "owner" { &owner } else { &inviter };
        let server = &server;
        async move {
            let mut stream = server.stream(member).await;
            stream.subscribe(Level::Full, Some(before)).await;
            server
                .traffic(member, &mut stream)
                .await
                .into_iter()
                .filter(|(_, event)| {
                    matches!(
                        event,
                        Event::InviteCreated { .. } | Event::InviteRevoked { .. }
                    )
                })
                .count()
        }
    };
    assert_eq!(replayed("inviter").await, 2);

    let migration = include_str!("../../migrations/0013_limits_after_audit.sql");
    let start = migration
        .find("alter table events disable trigger")
        .unwrap();
    sqlx::raw_sql(sqlx::AssertSqlSafe(migration[start..].to_owned()))
        .execute(&server.db)
        .await
        .unwrap();
    assert_eq!(replayed("inviter").await, 0);
    assert_eq!(replayed("owner").await, 2);
    let stored: Vec<Value> = sqlx::query_scalar(
        "select audience from events
         where kind in ('invite_created', 'invite_revoked')
           and coalesce(payload -> 'invite' ->> 'id', payload ->> 'invite_id') = $1",
    )
    .bind(created.invite.id.to_string())
    .fetch_all(&server.db)
    .await
    .unwrap();
    let expected = json!({
        "to": "invite",
        "created_by": owner.account_id,
        "workspace_ids": [board.id],
    });
    assert_eq!(stored, [expected.clone(), expected]);
}
