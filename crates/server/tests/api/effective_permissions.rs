//! The permissions of the account in the snapshot and in the route of one channel
//! (ADR 0310). Every set is what the permission function decides now.

use junto_protocol::access_token::TokenAction;
use junto_protocol::events::Level;
use junto_protocol::message::Message;
use junto_protocol::snapshot::WorkspaceState;
use junto_protocol::structure::ChannelPermissions;
use junto_protocol::{ServerPermission, WorkspacePermission};
use reqwest::Method;

use crate::discovery_support::*;

fn of(server_state: &junto_protocol::snapshot::Snapshot) -> &WorkspaceState {
    &server_state.workspaces[0]
}

fn entry(state: &WorkspaceState, channel: &Channel) -> Option<Vec<WorkspacePermission>> {
    state
        .channel_permissions
        .iter()
        .find(|entry| entry.channel_id == channel.id)
        .map(|entry| entry.permissions.clone())
}

/// The set of the account in a channel as a client reads it from the snapshot.
fn held(state: &WorkspaceState, channel: &Channel) -> Vec<WorkspacePermission> {
    entry(state, channel).unwrap_or_else(|| state.permissions.clone())
}

async fn set_override(
    server: &TestServer,
    manager: &Member,
    channel: &Channel,
    role_id: WorkspaceRoleId,
    body: Value,
) {
    server
        .put(
            manager,
            &format!("/api/v1/channels/{}/overrides/{role_id}", channel.id),
            &body,
        )
        .await
        .ok::<Value>(StatusCode::OK);
}

/// Hides a channel from every member who holds no other role with sight of it.
async fn hide(server: &TestServer, manager: &Member, workspace_id: WorkspaceId, channel: &Channel) {
    let everyone = server.everyone_role(manager, workspace_id).await;
    set_override(
        server,
        manager,
        channel,
        everyone.id,
        json!({ "deny": ["view_channel"] }),
    )
    .await;
}

async fn route(
    server: &TestServer,
    member: &Member,
    channel_id: ChannelId,
) -> crate::support::Reply {
    server
        .get(
            member,
            &format!("/api/v1/channels/{channel_id}/permissions"),
        )
        .await
}

#[tokio::test]
async fn the_snapshot_states_the_permissions_of_the_workspace() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let member = server.join(&owner, "Member").await;
    let workspace = server.default_workspace(&owner).await;

    let plain = server.snapshot(&member).await;
    assert_eq!(
        of(&plain).permissions,
        [
            WorkspacePermission::ViewChannel,
            WorkspacePermission::LockRoom,
            WorkspacePermission::RunMeetingMode
        ]
    );

    server
        .grant_workspace(
            &owner,
            &member,
            workspace.id,
            &[WorkspacePermission::ManageMessages],
        )
        .await;
    let granted = server.snapshot(&member).await;
    assert_eq!(
        of(&granted).permissions,
        [
            WorkspacePermission::ViewChannel,
            WorkspacePermission::ManageMessages,
            WorkspacePermission::LockRoom,
            WorkspacePermission::RunMeetingMode
        ]
    );
    assert!(of(&granted).channel_permissions.is_empty());
}

#[tokio::test]
async fn an_override_of_a_held_role_changes_the_set_of_one_channel() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let member = server.join(&owner, "Member").await;
    let workspace = server.default_workspace(&owner).await;
    let denied = server.channel(&owner, workspace.id, "denied").await;
    let allowed = server.channel(&owner, workspace.id, "allowed").await;
    let plain = server.channel(&owner, workspace.id, "plain").await;
    let role = server
        .grant_workspace(
            &owner,
            &member,
            workspace.id,
            &[WorkspacePermission::ManageMessages],
        )
        .await;
    set_override(
        &server,
        &owner,
        &denied,
        role,
        json!({ "deny": ["manage_messages"] }),
    )
    .await;
    set_override(
        &server,
        &owner,
        &allowed,
        role,
        json!({ "allow": ["manage_channel"] }),
    )
    .await;

    let snapshot = server.snapshot(&member).await;
    let state = of(&snapshot);
    assert!(
        state
            .permissions
            .contains(&WorkspacePermission::ManageMessages)
    );
    assert!(
        !state
            .permissions
            .contains(&WorkspacePermission::ManageChannel)
    );
    let in_denied = entry(state, &denied).expect("an entry for the channel with the deny");
    assert!(!in_denied.contains(&WorkspacePermission::ManageMessages));
    assert!(in_denied.contains(&WorkspacePermission::ViewChannel));
    let in_allowed = entry(state, &allowed).expect("an entry for the channel with the allow");
    assert!(in_allowed.contains(&WorkspacePermission::ManageChannel));
    assert!(in_allowed.contains(&WorkspacePermission::ManageMessages));
    assert_eq!(entry(state, &plain), None);
}

#[tokio::test]
async fn a_channel_with_the_set_of_the_workspace_has_no_entry() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let member = server.join(&owner, "Member").await;
    let other = server.join(&owner, "Other").await;
    let workspace = server.default_workspace(&owner).await;
    let channel = server.channel(&owner, workspace.id, "plans").await;
    // An override of a role that the member does not hold changes nothing for the member.
    let role = server
        .grant_workspace(&owner, &other, workspace.id, &[])
        .await;
    set_override(
        &server,
        &owner,
        &channel,
        role,
        json!({ "allow": ["manage_channel"] }),
    )
    .await;

    let snapshot = server.snapshot(&member).await;
    assert!(of(&snapshot).channel_permissions.is_empty());
    let theirs = server.snapshot(&other).await;
    assert_eq!(of(&theirs).channel_permissions.len(), 1);
}

#[tokio::test]
async fn an_administrator_holds_every_permission_and_has_no_entry() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let administrator = server.join(&owner, "Administrator").await;
    server
        .grant_server(&owner, &administrator, &[ServerPermission::Administrator])
        .await;
    let workspace = server.default_workspace(&owner).await;
    let channel = server.channel(&owner, workspace.id, "private").await;
    hide(&server, &owner, workspace.id, &channel).await;

    for account in [&owner, &administrator] {
        let snapshot = server.snapshot(account).await;
        assert_eq!(of(&snapshot).permissions, WorkspacePermission::ALL);
        assert!(of(&snapshot).channel_permissions.is_empty());
        assert!(
            of(&snapshot)
                .channels
                .iter()
                .any(|seen| seen.id == channel.id)
        );
    }
}

#[tokio::test]
async fn a_channel_that_the_account_cannot_view_has_no_entry() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let member = server.join(&owner, "Member").await;
    let workspace = server.default_workspace(&owner).await;
    let channel = server.channel(&owner, workspace.id, "private").await;
    hide(&server, &owner, workspace.id, &channel).await;

    let snapshot = server.snapshot(&member).await;
    let state = of(&snapshot);
    assert!(state.channels.iter().all(|seen| seen.id != channel.id));
    assert_eq!(entry(state, &channel), None);
    assert!(state.channel_permissions.is_empty());
}

/// The sets are no second rule: a permission is in the set of a channel when the route
/// that needs it accepts the account, and absent when the route refuses.
#[tokio::test]
async fn the_stated_sets_agree_with_what_the_routes_decide() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let member = server.join(&owner, "Member").await;
    let workspace = server.default_workspace(&owner).await;
    let open = server.channel(&owner, workspace.id, "open").await;
    let closed = server.channel(&owner, workspace.id, "closed").await;
    let role = server
        .grant_workspace(
            &owner,
            &member,
            workspace.id,
            &[
                WorkspacePermission::ManageMessages,
                WorkspacePermission::ManageChannel,
            ],
        )
        .await;
    set_override(
        &server,
        &owner,
        &closed,
        role,
        json!({ "deny": ["manage_messages", "manage_channel"] }),
    )
    .await;
    let snapshot = server.snapshot(&member).await;
    let state = of(&snapshot);

    for channel in [&open, &closed] {
        let set = held(state, channel);
        let message = send(&server, &owner, channel, "of the owner").await;
        let deleted = server.delete(&member, &message_path(&message)).await;
        assert_eq!(
            deleted.status == StatusCode::NO_CONTENT,
            set.contains(&WorkspacePermission::ManageMessages),
            "{}: {}",
            channel.name,
            deleted.status
        );
        let renamed = server
            .patch(
                &member,
                &format!("/api/v1/channels/{}", channel.id),
                &json!({ "name": format!("{}-renamed", channel.name) }),
            )
            .await;
        assert_eq!(
            renamed.status == StatusCode::OK,
            set.contains(&WorkspacePermission::ManageChannel),
            "{}: {}",
            channel.name,
            renamed.status
        );
    }
    assert!(held(state, &open).contains(&WorkspacePermission::ManageMessages));
    assert!(!held(state, &closed).contains(&WorkspacePermission::ManageMessages));
}

fn message_path(message: &Message) -> String {
    message_route(message.id)
}

fn message_route(id: MessageId) -> String {
    message(id)
}

#[tokio::test]
async fn the_route_states_the_set_of_one_channel_also_when_it_is_archived() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let member = server.join(&owner, "Member").await;
    let workspace = server.default_workspace(&owner).await;
    let channel = server.channel(&owner, workspace.id, "plans").await;
    let role = server
        .grant_workspace(&owner, &member, workspace.id, &[])
        .await;
    set_override(
        &server,
        &owner,
        &channel,
        role,
        json!({ "allow": ["manage_channel"] }),
    )
    .await;

    let stated: ChannelPermissions = route(&server, &member, channel.id).await.ok(StatusCode::OK);
    assert_eq!(stated.channel_id, channel.id);
    let snapshot = server.snapshot(&member).await;
    assert_eq!(
        Some(stated.permissions.clone()),
        entry(of(&snapshot), &channel)
    );

    server
        .put_empty(&member, &format!("/api/v1/channels/{}/archive", channel.id))
        .await
        .ok::<Value>(StatusCode::OK);
    let archived = server.snapshot(&member).await;
    assert!(
        of(&archived)
            .channels
            .iter()
            .all(|seen| seen.id != channel.id)
    );
    assert_eq!(entry(of(&archived), &channel), None);
    let again: ChannelPermissions = route(&server, &member, channel.id).await.ok(StatusCode::OK);
    assert_eq!(again.permissions, stated.permissions);
    assert!(
        again
            .permissions
            .contains(&WorkspacePermission::ManageChannel)
    );
}

#[tokio::test]
async fn the_route_answers_not_found_for_a_channel_that_the_account_cannot_view() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let member = server.join(&owner, "Member").await;
    let workspace = server.default_workspace(&owner).await;
    let hidden = server.channel(&owner, workspace.id, "hidden").await;
    hide(&server, &owner, workspace.id, &hidden).await;
    let deleted = server.channel(&owner, workspace.id, "deleted").await;
    server
        .delete(&owner, &format!("/api/v1/channels/{}", deleted.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    let elsewhere: junto_protocol::structure::Workspace = server
        .post(
            &owner,
            "/api/v1/workspaces",
            &json!({ "name": "Elsewhere" }),
        )
        .await
        .ok(StatusCode::CREATED);
    let foreign = server.channel(&owner, elsewhere.id, "foreign").await;

    route(&server, &owner, hidden.id)
        .await
        .ok::<ChannelPermissions>(StatusCode::OK);
    for channel_id in [hidden.id, deleted.id, foreign.id, ChannelId::generate()] {
        let reply = route(&server, &member, channel_id).await;
        reply.error(ErrorCode::NotFound);
        assert!(!reply.text().contains("permissions"), "{}", reply.text());
    }
}

#[tokio::test]
async fn an_access_token_does_not_read_the_permissions_of_a_channel() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let channel = server.channel(&owner, workspace.id, "plans").await;
    let token = server
        .access_token(
            &owner,
            &[
                TokenAction::ReadStructure,
                TokenAction::ManageStructure,
                TokenAction::ManageMembers,
                TokenAction::ReadAuditLog,
            ],
        )
        .await;
    for path in [
        format!("/api/v1/channels/{}/permissions", channel.id),
        format!("/api/v1/channels/{}/threads", channel.id),
        "/api/v1/snapshot".to_owned(),
    ] {
        server
            .with_token(Method::GET, &path, &token, None)
            .await
            .error(ErrorCode::Forbidden);
    }
}

#[tokio::test]
async fn a_changed_override_is_in_the_next_snapshot() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let member = server.join(&owner, "Member").await;
    let workspace = server.default_workspace(&owner).await;
    let channel = server.channel(&owner, workspace.id, "plans").await;
    let role = server
        .grant_workspace(&owner, &member, workspace.id, &[])
        .await;
    let before = server.snapshot(&member).await;
    assert_eq!(entry(of(&before), &channel), None);

    set_override(
        &server,
        &owner,
        &channel,
        role,
        json!({ "allow": ["manage_messages"] }),
    )
    .await;
    let told = events(&server, &member, before.cursor, Level::Background).await;
    assert!(
        told.contains(&Event::PermissionsChanged {
            workspace_id: Some(workspace.id)
        }),
        "{told:?}"
    );
    let after = server.snapshot(&member).await;
    assert!(
        entry(of(&after), &channel)
            .expect("an entry after the override")
            .contains(&WorkspacePermission::ManageMessages)
    );

    set_override(&server, &owner, &channel, role, json!({})).await;
    let removed = server.snapshot(&member).await;
    assert_eq!(entry(of(&removed), &channel), None);
}

#[tokio::test]
async fn a_fork_with_copied_overrides_tells_the_holders_to_load_a_snapshot() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let member = server.join(&owner, "Member").await;
    let outsider = server.join(&owner, "Outsider").await;
    let workspace = server.default_workspace(&owner).await;
    let source = server.channel(&owner, workspace.id, "source").await;
    let open = server.channel(&owner, workspace.id, "open").await;
    let role = server
        .grant_workspace(&owner, &member, workspace.id, &[])
        .await;
    // The source is private: the role sees it and manages its messages.
    hide(&server, &owner, workspace.id, &source).await;
    set_override(
        &server,
        &owner,
        &source,
        role,
        json!({ "allow": ["view_channel", "manage_messages"] }),
    )
    .await;
    let root = send(&server, &owner, &source, "root").await;
    reply(&server, &owner, root.id, "reply").await;
    let plain_root = send(&server, &owner, &open, "root").await;
    reply(&server, &owner, plain_root.id, "reply").await;
    let notice = Event::PermissionsChanged {
        workspace_id: Some(workspace.id),
    };

    // A fork of a channel without overrides has the list of the workspace: no notice.
    let before = server.snapshot(&member).await;
    let plain_fork = fork(&server, &owner, plain_root.id, "text").await;
    let told = events(&server, &member, before.cursor, Level::Full).await;
    assert!(
        told.iter().any(
            |event| matches!(event, Event::ChannelCreated { channel } if channel.id == plain_fork.id)
        ),
        "{told:?}"
    );
    assert!(!told.contains(&notice), "{told:?}");

    let before = server.snapshot(&member).await;
    let outside = server.snapshot(&outsider).await;
    let forked = fork(&server, &owner, root.id, "text").await;
    let told = events(&server, &member, before.cursor, Level::Full).await;
    assert!(
        told.iter().any(
            |event| matches!(event, Event::ChannelCreated { channel } if channel.id == forked.id)
        ),
        "{told:?}"
    );
    assert!(told.contains(&notice), "{told:?}");
    // An account that does not see the fork learns nothing of it.
    let others = events(&server, &outsider, outside.cursor, Level::Full).await;
    assert!(others.is_empty(), "{others:?}");

    let after = server.snapshot(&member).await;
    assert!(
        entry(of(&after), &forked)
            .expect("an entry for the fork")
            .contains(&WorkspacePermission::ManageMessages)
    );
}

/// An administrator who is no member of a workspace holds every permission in it. The
/// creation of the workspace tells the accounts that see it to load their lists.
#[tokio::test]
async fn a_new_workspace_tells_the_accounts_that_see_it_to_load_a_snapshot() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let administrator = server.join(&owner, "Administrator").await;
    server
        .grant_server(&owner, &administrator, &[ServerPermission::Administrator])
        .await;
    let member = server.join(&owner, "Member").await;
    let seen = server.snapshot(&administrator).await;
    let unseen = server.snapshot(&member).await;

    let created: junto_protocol::structure::Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({ "name": "Second" }))
        .await
        .ok(StatusCode::CREATED);
    let notice = Event::PermissionsChanged {
        workspace_id: Some(created.id),
    };
    let told = events(&server, &administrator, seen.cursor, Level::Background).await;
    assert_eq!(told, [notice]);
    assert!(
        events(&server, &member, unseen.cursor, Level::Full)
            .await
            .is_empty()
    );
    let after = server.snapshot(&administrator).await;
    let second = after
        .workspaces
        .iter()
        .find(|state| state.workspace.id == created.id)
        .expect("the new workspace");
    assert_eq!(second.permissions, WorkspacePermission::ALL);
}

#[tokio::test]
async fn a_restored_channel_with_overrides_tells_the_holders_to_load_a_snapshot() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let member = server.join(&owner, "Member").await;
    let workspace = server.default_workspace(&owner).await;
    let channel = server.channel(&owner, workspace.id, "plans").await;
    let untouched = server.channel(&owner, workspace.id, "untouched").await;
    let role = server
        .grant_workspace(&owner, &member, workspace.id, &[])
        .await;
    set_override(
        &server,
        &owner,
        &channel,
        role,
        json!({ "allow": ["manage_messages"] }),
    )
    .await;
    for archived in [&channel, &untouched] {
        server
            .put_empty(&owner, &format!("/api/v1/channels/{}/archive", archived.id))
            .await
            .ok::<Value>(StatusCode::OK);
    }
    let notice = Event::PermissionsChanged {
        workspace_id: Some(workspace.id),
    };

    let before = server.snapshot(&member).await;
    server
        .delete(
            &owner,
            &format!("/api/v1/channels/{}/archive", untouched.id),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    let plain = events(&server, &member, before.cursor, Level::Full).await;
    assert!(!plain.contains(&notice), "{plain:?}");

    let before = server.snapshot(&member).await;
    server
        .delete(&owner, &format!("/api/v1/channels/{}/archive", channel.id))
        .await
        .ok::<Value>(StatusCode::OK);
    let told = events(&server, &member, before.cursor, Level::Full).await;
    assert!(told.contains(&notice), "{told:?}");
    let after = server.snapshot(&member).await;
    assert!(
        entry(of(&after), &channel)
            .expect("an entry for the restored channel")
            .contains(&WorkspacePermission::ManageMessages)
    );
}

/// The route of one channel counts against the limit of reads of the account, 120 in a
/// minute, as the other reads of a channel do (ADR 0408).
#[tokio::test]
async fn the_route_of_one_channel_has_the_limit_of_reads() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let channel = server.channel(&owner, workspace.id, "plans").await;
    for _ in 0..120 {
        route(&server, &owner, channel.id)
            .await
            .ok::<ChannelPermissions>(StatusCode::OK);
    }
    route(&server, &owner, channel.id)
        .await
        .error(ErrorCode::RateLimited);
}
