//! Who receives an event, decided when it is delivered, and the end of a connection when its
//! session ends (ADR 0011, ADR 0107, ADR 0208).

use std::time::Duration;

use junto_protocol::account::RecoveryNotice;
use junto_protocol::auth::ChallengeOperation;
use junto_protocol::events::{CloseCode, EndReason, Event, Level, ServerMessage};
use junto_protocol::ids::{ChannelId, WorkspaceId};
use junto_protocol::invite::JoinRequest;
use junto_protocol::roles::{ServerRole, WorkspaceRole};
use junto_protocol::structure::{Channel, Workspace};
use junto_protocol::{ServerPermission, WorkspacePermission};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::devices::link_second_device;
use crate::stream_support::{Frame, Stream};
use crate::support::{DeviceKey, Member, TestServer};

fn types(events: &[(u64, Event)]) -> Vec<&'static str> {
    events.iter().map(|(_, event)| event.name()).collect()
}

/// Moves the clock past the time in which the setting lines of one person are joined, so
/// that each change of a test has its own system line (ADR 0408).
fn apart(server: &TestServer) {
    server
        .clock
        .advance(junto_server::messages::JOINED_LINES_WITHIN);
}

async fn rename_channel(server: &TestServer, member: &Member, channel_id: ChannelId, name: &str) {
    server
        .patch(
            member,
            &format!("/api/v1/channels/{channel_id}"),
            &json!({ "name": name }),
        )
        .await
        .ok::<Channel>(StatusCode::OK);
}

async fn set_roles(
    server: &TestServer,
    owner: &Member,
    workspace_id: WorkspaceId,
    member: &Member,
    role: &WorkspaceRole,
    held: bool,
) {
    let path = format!(
        "/api/v1/workspaces/{workspace_id}/members/{}/roles/{}",
        member.account_id, role.id
    );
    let reply = if held {
        server.put_empty(owner, &path).await
    } else {
        server.delete(owner, &path).await
    };
    reply.empty(StatusCode::NO_CONTENT);
}

async fn role(
    server: &TestServer,
    owner: &Member,
    workspace_id: WorkspaceId,
    name: &str,
) -> WorkspaceRole {
    server
        .post(
            owner,
            &format!("/api/v1/workspaces/{workspace_id}/roles"),
            &json!({ "name": name }),
        )
        .await
        .ok(StatusCode::CREATED)
}

/// The events that give a system notification reach a background connection, each with the
/// audience that it has on a full connection (ADR 0107, ADR 0742). The start of a call is
/// checked in `calls_state`, with the fake media server.
#[tokio::test]
async fn the_background_level_receives_the_events_of_system_notifications() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let member = server.join(&owner, "Member").await;
    let mut owner_background = server.subscribed(&owner, Level::Background).await;
    let mut background = server.subscribed(&member, Level::Background).await;

    let lab: Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({ "name": "Lab" }))
        .await
        .ok(StatusCode::CREATED);
    server
        .put_empty(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}",
                lab.id, member.account_id
            ),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let proof = server
        .proof(&DeviceKey::generate(), ChallengeOperation::RequestToJoin)
        .await;
    server
        .open(
            "/api/v1/join-requests",
            &json!({ "proof": proof, "name": "Waiting" }),
        )
        .await
        .ok::<JoinRequest>(StatusCode::CREATED);

    let received = server.traffic(&member, &mut background).await;
    let received = types(&received);
    assert!(received.contains(&"workspace_member_added"), "{received:?}");
    // A member without the permission to decide join requests is not told of one.
    assert!(!received.contains(&"join_request_created"), "{received:?}");
    let received = server.traffic(&owner, &mut owner_background).await;
    let received = types(&received);
    assert!(received.contains(&"join_request_created"), "{received:?}");
}

#[tokio::test]
async fn the_background_level_receives_control_events_only() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let member = server.join(&owner, "Member").await;
    let workspace = server.default_workspace(&owner).await;
    let helpers = role(&server, &owner, workspace.id, "helpers").await;
    let mut full = server.subscribed(&member, Level::Full).await;
    let mut background = server.subscribed(&member, Level::Background).await;

    server.category(&owner, workspace.id, "Projects", 0).await;
    let channel = server.channel(&owner, workspace.id, "plans").await;
    rename_channel(&server, &owner, channel.id, "roadmap").await;
    set_roles(&server, &owner, workspace.id, &member, &helpers, true).await;
    let mut in_full = full.events(7).await;
    let mut in_background = background.events(1).await;
    server
        .delete(&owner, &format!("/api/v1/channels/{}", channel.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .patch(
            &owner,
            &format!("/api/v1/workspaces/{}", workspace.id),
            &json!({ "name": "Renamed" }),
        )
        .await
        .ok::<Workspace>(StatusCode::OK);

    in_background.extend(server.traffic(&member, &mut background).await);
    assert_eq!(
        types(&in_background),
        ["permissions_changed", "channel_deleted"]
    );
    assert_eq!(
        in_background[0].1,
        Event::PermissionsChanged {
            workspace_id: Some(workspace.id)
        }
    );
    in_full.extend(server.traffic(&member, &mut full).await);
    assert_eq!(
        types(&in_full),
        [
            "category_created",
            "message_changed",
            "channel_created",
            "message_changed",
            "channel_updated",
            "workspace_member_roles_changed",
            "permissions_changed",
            "channel_deleted",
            "workspace_updated",
            "device_renamed",
        ]
    );
    // The index of an event in its change counts the events that the connection receives,
    // so the same event is compared by its change (ADR 0010).
    let by_change = |events: &[(u64, Event)]| -> Vec<(i64, Event)> {
        events
            .iter()
            .map(|(id, event)| (junto_server::events::log::change_of(*id), event.clone()))
            .collect()
    };
    let background_events = by_change(&in_background);
    let shared: Vec<(i64, Event)> = by_change(&in_full)
        .into_iter()
        .filter(|event| background_events.contains(event))
        .collect();
    assert_eq!(shared.len(), 2);
}

/// ADR 0510: a background connection whose account has a seat in a call receives every
/// event of the channel of that call. The connection reads the seat from the record.
#[tokio::test]
async fn an_account_in_a_call_receives_every_event_of_the_channel_of_the_call() {
    let scene = crate::calls_support::Scene::start().await;
    let (server, owner, member) = (&scene.server, &scene.owner, &scene.ann);
    let call = scene.voice.id;
    let other = server.channel(owner, scene.workspace_id, "other").await;
    let mut background = server.subscribed(member, Level::Background).await;

    rename_channel(server, owner, call, "call-1").await;
    assert!(server.traffic(member, &mut background).await.is_empty());

    server.enter_call(member, call).await;
    server.traffic(member, &mut background).await;
    rename_channel(server, owner, other.id, "other-2").await;
    rename_channel(server, owner, call, "call-2").await;
    server
        .category(owner, scene.workspace_id, "Projects", 0)
        .await;
    let during = server.traffic(member, &mut background).await;
    assert_eq!(types(&during), ["message_changed", "channel_updated"]);
    assert!(matches!(
        &during[1].1,
        Event::ChannelUpdated { channel } if channel.id == call
    ));

    server.leave_call(member).await;
    server.check().await;
    server.traffic(member, &mut background).await;
    rename_channel(server, owner, call, "call-3").await;
    assert!(server.traffic(member, &mut background).await.is_empty());
}

struct Private {
    server: TestServer,
    owner: Member,
    insider: Member,
    outsider: Member,
    workspace: Workspace,
    role: WorkspaceRole,
    channel: Channel,
}

/// A channel that only the holders of one role see. The insider holds the role.
async fn private_channel() -> Private {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let insider = server.join(&owner, "Insider").await;
    let outsider = server.join(&owner, "Outsider").await;
    let workspace = server.default_workspace(&owner).await;
    let role = role(&server, &owner, workspace.id, "team").await;
    set_roles(&server, &owner, workspace.id, &insider, &role, true).await;
    let channel = server.channel(&owner, workspace.id, "secret-plans").await;
    server
        .make_private(&owner, workspace.id, channel.id, role.id)
        .await;
    Private {
        server,
        owner,
        insider,
        outsider,
        workspace,
        role,
        channel,
    }
}

#[tokio::test]
async fn a_permission_removed_while_events_wait_hides_them() {
    let world = private_channel().await;
    let (server, owner) = (&world.server, &world.owner);
    let second = server.join(owner, "Second").await;
    set_roles(
        server,
        owner,
        world.workspace.id,
        &second,
        &world.role,
        true,
    )
    .await;
    let mut losing = server.subscribed(&world.insider, Level::Full).await;
    let mut keeping = server.subscribed(&second, Level::Full).await;

    server.state.streams.hold();
    for name in ["secret-1", "secret-2", "secret-3"] {
        rename_channel(server, owner, world.channel.id, name).await;
        apart(server);
    }
    assert_eq!(losing.within(Duration::from_millis(200)).await, None);
    set_roles(
        server,
        owner,
        world.workspace.id,
        &world.insider,
        &world.role,
        false,
    )
    .await;
    server.state.streams.release();

    let lost = server.traffic(&world.insider, &mut losing).await;
    assert_eq!(
        types(&lost),
        ["workspace_member_roles_changed", "permissions_changed"]
    );
    assert!(!losing.raw.join("\n").contains("secret-"));
    let kept = server.traffic(&second, &mut keeping).await;
    assert_eq!(
        types(&kept),
        [
            "message_changed",
            "channel_updated",
            "message_changed",
            "channel_updated",
            "message_changed",
            "channel_updated",
            "workspace_member_roles_changed"
        ]
    );
}

#[tokio::test]
async fn an_event_that_is_not_written_in_time_is_decided_again() {
    const CHANGES: usize = 400;
    let server = TestServer::start_tuned(
        |config| {
            config.limits.stream.batch = 400;
            config.limits.stream.authorization_validity = time::Duration::milliseconds(500);
            config.limits.stream.retry_delay = time::Duration::milliseconds(50);
            config.limits.stream.send_timeout = time::Duration::seconds(30);
            config.limits.stream.poll_interval = time::Duration::milliseconds(50);
        },
        Some(8 * 1024),
    )
    .await;
    let owner = server.owner().await;
    let insider = server.join(&owner, "Insider").await;
    let workspace = server.default_workspace(&owner).await;
    let team = role(&server, &owner, workspace.id, "team").await;
    set_roles(&server, &owner, workspace.id, &insider, &team, true).await;
    let channel = server.channel(&owner, workspace.id, "secret-plans").await;
    server
        .make_private(&owner, workspace.id, channel.id, team.id)
        .await;
    let mut slow = server.narrow_stream(&insider, 8 * 1024).await;
    slow.subscribe(Level::Full, None).await;

    server
        .large_events(
            json!({
                "to": "channel",
                "workspace_id": workspace.id,
                "channel_id": channel.id,
                "permission": "view_channel",
            }),
            json!({ "type": "channel_updated", "channel": channel }),
            &["channel", "name"],
            CHANGES,
        )
        .await;
    tokio::time::sleep(Duration::from_millis(1_200)).await;
    set_roles(&server, &owner, workspace.id, &insider, &team, false).await;
    tokio::time::sleep(Duration::from_millis(800)).await;

    let received = server.traffic(&insider, &mut slow).await;
    let written_before: Vec<&(u64, Event)> = received
        .iter()
        .filter(|(_, event)| matches!(event, Event::ChannelUpdated { .. }))
        .collect();
    assert!(!written_before.is_empty());
    assert!(
        written_before.len() < CHANGES / 2,
        "{}",
        written_before.len()
    );
    assert_eq!(
        types(&received[written_before.len()..]),
        ["workspace_member_roles_changed", "permissions_changed"]
    );
}

/// An event about a channel that is delivered after the deletion of the channel reaches
/// nobody: a delivery decides with the state of its moment (ADR 0011). The deletion reaches
/// the accounts that saw the channel.
#[tokio::test]
async fn events_about_a_channel_that_wait_until_after_its_deletion_reach_nobody() {
    let world = private_channel().await;
    let (server, owner) = (&world.server, &world.owner);
    let channel_id = world.channel.id;
    let other_role = role(server, owner, world.workspace.id, "guests").await;
    let mut inside = server.subscribed(&world.insider, Level::Full).await;
    let mut outside = server.subscribed(&world.outsider, Level::Full).await;
    let mut managing = server.subscribed(owner, Level::Full).await;

    server.state.streams.hold();
    rename_channel(server, owner, channel_id, "secret-renamed").await;
    server
        .put(
            owner,
            &format!("/api/v1/channels/{channel_id}/overrides/{}", other_role.id),
            &json!({ "deny": ["manage_messages"] }),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    server
        .delete(owner, &format!("/api/v1/channels/{channel_id}"))
        .await
        .empty(StatusCode::NO_CONTENT);
    server.state.streams.release();

    let to_insider = server.traffic(&world.insider, &mut inside).await;
    assert_eq!(types(&to_insider), ["channel_deleted"]);
    let to_manager = server.traffic(owner, &mut managing).await;
    assert_eq!(types(&to_manager), ["channel_deleted"]);
    assert!(!inside.raw.join("\n").contains("secret-renamed"));
    server.traffic(&world.outsider, &mut outside).await;
    let traffic = outside.raw.join("\n");
    assert!(!traffic.contains(&channel_id.to_string()), "{traffic}");
    assert!(!traffic.contains("secret"), "{traffic}");
}

#[tokio::test]
async fn a_permission_granted_while_events_wait_shows_them() {
    let world = private_channel().await;
    let (server, owner) = (&world.server, &world.owner);
    let mut stream = server.subscribed(&world.outsider, Level::Full).await;

    server.state.streams.hold();
    rename_channel(server, owner, world.channel.id, "secret-1").await;
    set_roles(
        server,
        owner,
        world.workspace.id,
        &world.outsider,
        &world.role,
        true,
    )
    .await;
    server.state.streams.release();

    let received = server.traffic(&world.outsider, &mut stream).await;
    assert_eq!(
        types(&received),
        [
            "message_changed",
            "channel_updated",
            "workspace_member_roles_changed",
            "permissions_changed"
        ]
    );
}

#[tokio::test]
async fn nothing_about_a_private_channel_reaches_an_account_that_cannot_see_it() {
    let world = private_channel().await;
    let (server, owner) = (&world.server, &world.owner);
    let channel_id = world.channel.id;
    let other_role = role(server, owner, world.workspace.id, "guests").await;

    let mut inside = server.subscribed(&world.insider, Level::Full).await;
    let mut outside = server.stream(&world.outsider).await;
    outside
        .subscribe(Level::Full, Some(outside.cursor(0)))
        .await;
    let mut outside_background = server.stream(&world.outsider).await;
    outside_background
        .subscribe(Level::Background, Some(outside_background.cursor(0)))
        .await;
    let mut managing = server.subscribed(owner, Level::Full).await;

    rename_channel(server, owner, channel_id, "secret-renamed").await;
    apart(server);
    server
        .patch(
            owner,
            &format!("/api/v1/channels/{channel_id}"),
            &json!({ "permanent_messages": true, "position": 9 }),
        )
        .await
        .ok::<Channel>(StatusCode::OK);
    server
        .put(
            owner,
            &format!("/api/v1/channels/{channel_id}/overrides/{}", other_role.id),
            &json!({ "deny": ["manage_messages"] }),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    // The second change records one system line: the permanent messages that it turns on.
    // A change of the position has no line (ADR 0402, ADR 0408).
    let mut to_manager = managing.events(6).await;
    let mut to_insider = inside.events(5).await;
    server
        .delete(owner, &format!("/api/v1/channels/{channel_id}"))
        .await
        .empty(StatusCode::NO_CONTENT);

    to_insider.extend(server.traffic(&world.insider, &mut inside).await);
    assert_eq!(
        types(&to_insider),
        [
            "message_changed",
            "channel_updated",
            "message_changed",
            "channel_updated",
            "message_changed",
            "channel_deleted"
        ]
    );
    to_manager.extend(server.traffic(owner, &mut managing).await);
    assert_eq!(
        types(&to_manager),
        [
            "message_changed",
            "channel_updated",
            "message_changed",
            "channel_updated",
            "message_changed",
            "channel_override_set",
            "channel_deleted"
        ]
    );

    let to_outsider = server.traffic(&world.outsider, &mut outside).await;
    let to_background = server
        .traffic(&world.outsider, &mut outside_background)
        .await;
    for (events, stream) in [
        (&to_outsider, &outside),
        (&to_background, &outside_background),
    ] {
        let traffic = stream.raw.join("\n");
        assert!(!traffic.contains(&channel_id.to_string()), "{traffic}");
        assert!(!traffic.contains("secret"), "{traffic}");
        for (_, event) in events {
            assert!(
                !matches!(
                    event,
                    Event::ChannelCreated { channel } | Event::ChannelUpdated { channel }
                        if channel.id == channel_id
                ),
                "{event:?}"
            );
        }
    }
    let notice = Event::PermissionsChanged {
        workspace_id: Some(world.workspace.id),
    };
    let notices: Vec<&Event> = to_outsider
        .iter()
        .map(|(_, event)| event)
        .filter(|event| matches!(event, Event::PermissionsChanged { .. }))
        .collect();
    assert_eq!(notices, [&notice, &notice]);
    assert_eq!(
        types(&to_background),
        [
            "device_added",
            "permissions_changed",
            "device_logged_in",
            "permissions_changed",
            "device_renamed",
        ]
    );
}

#[tokio::test]
async fn the_events_of_a_workspace_reach_its_members_and_administrators_only() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let stranger = server.join(&owner, "Stranger").await;
    let administrator = server.join(&owner, "Administrator").await;
    server
        .grant_server(&owner, &administrator, &[ServerPermission::Administrator])
        .await;
    let closed: Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({ "name": "Closed" }))
        .await
        .ok(StatusCode::CREATED);
    let newcomer = server.join(&owner, "Newcomer").await;
    let mut outside = server.subscribed(&stranger, Level::Full).await;
    let mut overseeing = server.subscribed(&administrator, Level::Full).await;
    let mut inside = server.subscribed(&owner, Level::Full).await;

    server.category(&owner, closed.id, "Hidden", 0).await;
    let channel = server.channel(&owner, closed.id, "hidden-channel").await;
    let helpers = role(&server, &owner, closed.id, "hidden-role").await;
    server
        .put_empty(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}",
                closed.id, newcomer.account_id
            ),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    set_roles(&server, &owner, closed.id, &newcomer, &helpers, true).await;
    let mut to_owner = inside.events(6).await;
    let mut to_administrator = overseeing.events(6).await;
    server
        .delete(&owner, &format!("/api/v1/channels/{}", channel.id))
        .await
        .empty(StatusCode::NO_CONTENT);

    let expected = [
        "category_created",
        "message_changed",
        "channel_created",
        "workspace_role_created",
        "workspace_member_added",
        "workspace_member_roles_changed",
        "channel_deleted",
    ];
    to_owner.extend(server.traffic(&owner, &mut inside).await);
    assert_eq!(types(&to_owner), expected);
    to_administrator.extend(server.traffic(&administrator, &mut overseeing).await);
    assert_eq!(types(&to_administrator), expected);
    assert!(server.traffic(&stranger, &mut outside).await.is_empty());
    let traffic = outside.raw.join("\n");
    assert!(!traffic.contains(&closed.id.to_string()));
    assert!(!traffic.to_lowercase().contains("hidden"));
}

#[tokio::test]
async fn the_events_of_the_server_reach_the_holders_of_their_permission_only() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let plain = server.join(&owner, "Plain").await;
    let inviter = server.join(&owner, "Inviter").await;
    let approver = server.join(&owner, "Approver").await;
    let moderator = server.join(&owner, "Moderator").await;
    server
        .grant_server(&owner, &inviter, &[ServerPermission::CreateInvites])
        .await;
    server
        .grant_server(&owner, &approver, &[ServerPermission::ApproveJoinRequests])
        .await;
    server
        .grant_server(&owner, &moderator, &[ServerPermission::BanMembers])
        .await;
    let mut to_plain = server.subscribed(&plain, Level::Full).await;
    let mut to_inviter = server.subscribed(&inviter, Level::Full).await;
    let mut to_approver = server.subscribed(&approver, Level::Full).await;
    let mut to_moderator = server.subscribed(&moderator, Level::Full).await;
    let mut to_owner = server.subscribed(&owner, Level::Full).await;

    let invite = server.invite(&inviter, json!({ "max_uses": 2 })).await;
    let proof = server
        .proof(&DeviceKey::generate(), ChallengeOperation::RequestToJoin)
        .await;
    let request: JoinRequest = server
        .open(
            "/api/v1/join-requests",
            &json!({ "proof": proof, "name": "Asking" }),
        )
        .await
        .ok(StatusCode::CREATED);
    server
        .post(
            &approver,
            &format!("/api/v1/join-requests/{}/denial", request.id),
            &json!({}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    server
        .delete(&inviter, &format!("/api/v1/invites/{}", invite.invite.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    let role: ServerRole = server
        .post(&owner, "/api/v1/server-roles", &json!({ "name": "quiet" }))
        .await
        .ok(StatusCode::CREATED);
    server
        .put_empty(
            &owner,
            &format!(
                "/api/v1/accounts/{}/server-roles/{}",
                plain.account_id, role.id
            ),
        )
        .await
        .empty(StatusCode::NO_CONTENT);

    assert_eq!(
        types(&server.traffic(&plain, &mut to_plain).await),
        ["permissions_changed"]
    );
    assert_eq!(
        types(&server.traffic(&inviter, &mut to_inviter).await),
        ["invite_created", "invite_revoked"]
    );
    assert_eq!(
        types(&server.traffic(&approver, &mut to_approver).await),
        ["join_request_created", "join_request_decided"]
    );
    assert_eq!(
        types(&server.traffic(&moderator, &mut to_moderator).await),
        [
            "server_role_positions_shifted",
            "server_role_created",
            "account_server_roles_changed"
        ]
    );
    assert_eq!(
        types(&server.traffic(&owner, &mut to_owner).await),
        [
            "invite_created",
            "join_request_created",
            "join_request_decided",
            "invite_revoked",
            "server_role_positions_shifted",
            "server_role_created",
            "account_server_roles_changed",
        ]
    );
    let traffic = to_plain.raw.join("\n");
    assert!(!traffic.contains(&invite.invite.id.to_string()));
    assert!(!traffic.contains("Asking"));
}

#[tokio::test]
async fn a_change_of_a_role_tells_its_holders_that_their_permissions_changed() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let holder = server.join(&owner, "Holder").await;
    let other = server.join(&owner, "Other").await;
    let workspace = server.default_workspace(&owner).await;
    let helpers = role(&server, &owner, workspace.id, "helpers").await;
    set_roles(&server, &owner, workspace.id, &holder, &helpers, true).await;
    let everyone = server.everyone_role(&owner, workspace.id).await;
    let server_role_id = server
        .grant_server(&owner, &holder, &[ServerPermission::CreateInvites])
        .await;
    let mut to_holder = server.subscribed(&holder, Level::Background).await;
    let mut to_other = server.subscribed(&other, Level::Background).await;
    let roles = format!("/api/v1/workspaces/{}/roles", workspace.id);

    server
        .patch(
            &owner,
            &format!("{roles}/{}", helpers.id),
            &json!({ "name": "renamed" }),
        )
        .await
        .ok::<WorkspaceRole>(StatusCode::OK);
    assert!(server.traffic(&holder, &mut to_holder).await.is_empty());

    server
        .patch(
            &owner,
            &format!("{roles}/{}", helpers.id),
            &json!({ "permissions": [WorkspacePermission::ManageMessages] }),
        )
        .await
        .ok::<WorkspaceRole>(StatusCode::OK);
    let in_workspace = Event::PermissionsChanged {
        workspace_id: Some(workspace.id),
    };
    let on_server = Event::PermissionsChanged { workspace_id: None };
    let notices = |events: Vec<(u64, Event)>| -> Vec<Event> {
        events.into_iter().map(|(_, event)| event).collect()
    };
    assert_eq!(
        notices(server.traffic(&holder, &mut to_holder).await),
        std::slice::from_ref(&in_workspace)
    );
    assert!(server.traffic(&other, &mut to_other).await.is_empty());

    server
        .patch(
            &owner,
            &format!("{roles}/{}", everyone.id),
            &json!({ "permissions": [WorkspacePermission::ViewChannel] }),
        )
        .await
        .ok::<WorkspaceRole>(StatusCode::OK);
    server
        .patch(
            &owner,
            &format!("/api/v1/server-roles/{server_role_id}"),
            &json!({ "permissions": [ServerPermission::ApproveJoinRequests] }),
        )
        .await
        .ok::<ServerRole>(StatusCode::OK);
    assert_eq!(
        notices(server.traffic(&holder, &mut to_holder).await),
        [in_workspace.clone(), on_server]
    );
    assert_eq!(
        notices(server.traffic(&other, &mut to_other).await),
        [in_workspace]
    );
}

async fn until_connections(server: &TestServer, count: usize) {
    for _ in 0..500 {
        if server.state.streams.connections() == count {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!(
        "{} connections stayed open",
        server.state.streams.connections()
    );
}

async fn ended(stream: &mut Stream) -> (Vec<ServerMessage>, Option<CloseCode>) {
    let (messages, code) = stream.until_closed().await;
    let last: Vec<ServerMessage> = messages
        .into_iter()
        .filter(|message| matches!(message, ServerMessage::Ended { .. }))
        .collect();
    (last, code)
}

fn end(reason: EndReason) -> (Vec<ServerMessage>, Option<CloseCode>) {
    (
        vec![ServerMessage::Ended {
            reason,
            recovery: None,
        }],
        Some(CloseCode::Ended),
    )
}

#[tokio::test]
async fn a_ban_and_a_removal_close_every_connection_of_the_account() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let banned = server.join(&owner, "Banned").await;
    let removed = server.join(&owner, "Removed").await;
    let mut bystander = server.subscribed(&owner, Level::Full).await;

    let mut subscribed = server.subscribed(&banned, Level::Full).await;
    let mut background = server.subscribed(&banned, Level::Background).await;
    server.state.streams.hold();
    server
        .put_empty(
            &owner,
            &format!("/api/v1/accounts/{}/ban", banned.account_id),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(ended(&mut subscribed).await, end(EndReason::Banned));
    assert_eq!(ended(&mut background).await, end(EndReason::Banned));
    server.state.streams.release();

    let mut idle = server.stream(&removed).await;
    let mut listening = server.subscribed(&removed, Level::Full).await;
    server
        .post(
            &owner,
            &format!("/api/v1/accounts/{}/removal", removed.account_id),
            &json!({}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(ended(&mut idle).await, end(EndReason::Removed));
    assert_eq!(ended(&mut listening).await, end(EndReason::Removed));

    assert_eq!(
        types(&server.traffic(&owner, &mut bystander).await),
        [
            "account_status_changed",
            "account_status_changed",
            "workspace_member_removed"
        ]
    );
    until_connections(&server, 1).await;
    let refused = server.open_stream(Some(&banned.token)).await.err().unwrap();
    assert_eq!(refused.0, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn the_end_of_a_session_closes_its_connections_and_no_other() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let second = link_second_device(&server, &owner).await;
    let third = link_second_device(&server, &owner).await;
    let other_session = server.login(&second.key).await;

    let mut staying = server.subscribed(&owner, Level::Full).await;
    let mut logged_out = server.subscribed(&second, Level::Full).await;
    let mut same_device = server
        .open_stream(Some(other_session.token.expose()))
        .await
        .unwrap();
    same_device.subscribe(Level::Full, None).await;
    let mut removed = server.stream(&third).await;

    server
        .delete(&second, "/api/v1/auth/session")
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(ended(&mut logged_out).await, end(EndReason::SessionEnded));

    server
        .delete(
            &owner,
            &format!("/api/v1/account/devices/{}", third.device_id),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(ended(&mut removed).await, end(EndReason::DeviceRemoved));

    assert_eq!(
        types(&server.traffic(&owner, &mut staying).await),
        ["device_logged_out", "device_removed"]
    );
    assert_eq!(
        types(&server.traffic(&owner, &mut same_device).await),
        ["device_logged_out", "device_removed", "device_renamed"]
    );
    until_connections(&server, 2).await;
}

#[tokio::test]
async fn a_session_that_expires_closes_its_connection() {
    let server = TestServer::start_with(|config| {
        config.limits.stream.heartbeat = time::Duration::milliseconds(100);
        config.limits.stream.client_frames.attempts = 10_000;
    })
    .await;
    let owner = server.owner().await;
    let mut subscribed = server.subscribed(&owner, Level::Full).await;
    let mut idle = server.stream(&owner).await;

    server.clock.advance(time::Duration::days(29));
    assert_eq!(idle.within(Duration::from_millis(400)).await, None);
    server.clock.advance(time::Duration::days(2));
    assert_eq!(ended(&mut subscribed).await, end(EndReason::SessionEnded));
    assert_eq!(ended(&mut idle).await, end(EndReason::SessionEnded));
}

#[tokio::test]
async fn a_recovery_tells_the_other_devices_through_the_stream() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let guest = server.join(&owner, "Guest").await;
    let second = link_second_device(&server, &guest).await;
    let mut first_stream = server.subscribed(&guest, Level::Full).await;
    let mut second_stream = server.subscribed(&second, Level::Background).await;
    let start = server.snapshot(&guest).await.cursor;

    let new_key = DeviceKey::generate();
    let notice: RecoveryNotice = server
        .post(
            &owner,
            &format!("/api/v1/accounts/{}/recovery", guest.account_id),
            &json!({ "public_key": new_key.public(), "device_name": "new" }),
        )
        .await
        .ok(StatusCode::CREATED);
    let told = (
        vec![ServerMessage::Ended {
            reason: EndReason::Recovered,
            recovery: Some(notice.clone()),
        }],
        Some(CloseCode::Ended),
    );
    assert_eq!(ended(&mut first_stream).await, told);
    assert_eq!(ended(&mut second_stream).await, told);

    let session = server.login(&guest.key).await;
    assert_eq!(session.recoveries, std::slice::from_ref(&notice));
    let mut again = server
        .open_stream(Some(session.token.expose()))
        .await
        .unwrap();
    again.subscribe(Level::Background, Some(start)).await;
    let (_, added) = again.event().await;
    assert!(matches!(
        added,
        Event::DeviceAdded { device_id, .. } if Some(device_id) == notice.device_id
    ));
    assert_eq!(
        again.event().await.1,
        Event::AccountRecovered { recovery: notice }
    );
    assert!(matches!(
        again.event().await.1,
        Event::DeviceLoggedIn { device_id, .. } if device_id == guest.device_id
    ));
}

#[tokio::test]
async fn an_ended_connection_receives_no_event_that_waited() {
    let world = private_channel().await;
    let (server, owner) = (&world.server, &world.owner);
    let mut stream = server.subscribed(&world.insider, Level::Full).await;

    server.state.streams.hold();
    rename_channel(server, owner, world.channel.id, "secret-1").await;
    server
        .category(owner, world.workspace.id, "Projects", 0)
        .await;
    server
        .put_empty(
            owner,
            &format!("/api/v1/accounts/{}/ban", world.insider.account_id),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    server.state.streams.release();

    assert_eq!(
        stream.until_closed().await,
        (
            vec![ServerMessage::Ended {
                reason: EndReason::Banned,
                recovery: None
            }],
            Some(CloseCode::Ended)
        )
    );
    assert!(matches!(stream.next().await, Frame::Closed(_)));
}
