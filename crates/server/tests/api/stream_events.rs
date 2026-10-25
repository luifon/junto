//! The events that the changes of the server base record (ADR 0208).

use junto_protocol::account::{AccountStatus, Device, DeviceOrigin};
use junto_protocol::auth::ChallengeOperation;
use junto_protocol::events::{Event, Level};
use junto_protocol::invite::{InviteRevocation, JoinRequest, JoinRequestState, JoinRequestStatus};
use junto_protocol::structure::{Workspace, WorkspaceMember};
use junto_protocol::{ServerPermission, WorkspacePermission};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::devices::request_link;
use crate::support::{DeviceKey, TestServer};

fn events(traffic: Vec<(u64, Event)>) -> Vec<Event> {
    traffic.into_iter().map(|(_, event)| event).collect()
}

async fn kinds(server: &TestServer) -> Vec<String> {
    server
        .log()
        .await
        .into_iter()
        .map(|(_, kind)| kind)
        .collect()
}

#[tokio::test]
async fn the_first_owner_and_a_join_record_the_new_account_and_its_device() {
    let server = TestServer::start().await;
    assert!(server.log().await.is_empty());
    let owner = server.owner().await;
    assert_eq!(
        kinds(&server).await,
        [
            "account_added",
            "device_added",
            "owner_changed",
            "workspace_member_added",
            "permissions_changed",
            "device_logged_in",
        ]
    );
    let workspace = server.default_workspace(&owner).await;
    let mut stream = server.subscribed(&owner, Level::Full).await;

    let invite = server.invite(&owner, json!({ "max_uses": 3 })).await;
    let key = DeviceKey::generate();
    let session: junto_protocol::auth::SessionCreated = server
        .join_reply(&key, invite.code.expose(), "Newcomer")
        .await
        .ok(StatusCode::CREATED);
    let received = events(server.traffic(&owner, &mut stream).await);
    assert_eq!(received.len(), 4, "{received:?}");
    assert_eq!(
        received[0],
        Event::InviteCreated {
            invite: invite.invite.clone()
        }
    );
    assert_eq!(
        received[1],
        Event::InviteUsed {
            invite_id: invite.invite.id,
            uses: 1
        }
    );
    assert!(matches!(
        &received[2],
        Event::AccountAdded { account }
            if account.id == session.account_id
                && account.name == "Newcomer"
                && account.status == AccountStatus::Active
                && !account.is_owner
    ));
    assert_eq!(
        received[3],
        Event::WorkspaceMemberAdded {
            workspace_id: workspace.id,
            member: WorkspaceMember {
                avatar: None,
                account_id: session.account_id,
                name: "Newcomer".to_owned(),
                role_ids: vec![],
            },
        }
    );
}

#[tokio::test]
async fn the_devices_and_sessions_of_an_account_are_told_to_its_own_devices() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let guest = server.join(&owner, "Guest").await;
    let mut stream = server.subscribed(&guest, Level::Background).await;
    let mut of_owner = server.subscribed(&owner, Level::Full).await;

    let key = DeviceKey::generate();
    let link = request_link(&server, &key, "laptop").await;
    let device: Device = server
        .post(
            &guest,
            "/api/v1/account/device-links/approve",
            &json!({ "code": link.code, "public_key": key.public() }),
        )
        .await
        .ok(StatusCode::CREATED);
    let session = server.login(&key).await;
    server
        .patch(
            &guest,
            &format!("/api/v1/account/devices/{}", device.id),
            &json!({ "name": "notebook" }),
        )
        .await
        .ok::<Device>(StatusCode::OK);
    server
        .send::<Value>(
            reqwest::Method::DELETE,
            "/api/v1/auth/session",
            Some(session.token.expose()),
            None,
            None,
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .delete(&guest, &format!("/api/v1/account/devices/{}", device.id))
        .await
        .empty(StatusCode::NO_CONTENT);

    let received = events(server.traffic(&guest, &mut stream).await);
    assert_eq!(
        received[0],
        Event::DeviceAdded {
            device_id: device.id,
            name: "laptop".to_owned(),
            public_key: key.public(),
            origin: DeviceOrigin::Link,
            created_at: device.created_at,
        }
    );
    assert!(matches!(
        received[1],
        Event::DeviceLoggedIn { device_id, .. } if device_id == device.id
    ));
    assert_eq!(
        received[2..],
        [
            Event::DeviceRenamed {
                device_id: device.id,
                name: "notebook".to_owned()
            },
            Event::DeviceLoggedOut {
                device_id: device.id
            },
            Event::DeviceRemoved {
                device_id: device.id
            },
        ]
    );
    assert!(server.traffic(&owner, &mut of_owner).await.is_empty());
}

#[tokio::test]
async fn joining_further_workspaces_and_join_requests_are_recorded() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let guest = server.join(&owner, "Guest").await;
    let second: Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({ "name": "Second" }))
        .await
        .ok(StatusCode::CREATED);
    let third: Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({ "name": "Third" }))
        .await
        .ok(StatusCode::CREATED);
    let mut of_guest = server.subscribed(&guest, Level::Background).await;
    let mut of_owner = server.subscribed(&owner, Level::Full).await;

    let invite = server
        .invite(&owner, json!({ "workspace_ids": [second.id] }))
        .await;
    server
        .post(
            &guest,
            "/api/v1/invites/redemption",
            &json!({ "invite_code": invite.code }),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    server
        .put_empty(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}",
                third.id, guest.account_id
            ),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .put_empty(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}",
                third.id, guest.account_id
            ),
        )
        .await
        .empty(StatusCode::NO_CONTENT);

    assert_eq!(
        events(server.traffic(&guest, &mut of_guest).await),
        [
            Event::PermissionsChanged {
                workspace_id: Some(second.id)
            },
            Event::PermissionsChanged {
                workspace_id: Some(third.id)
            },
        ]
    );
    let member = WorkspaceMember {
        avatar: None,
        account_id: guest.account_id,
        name: "Guest".to_owned(),
        role_ids: vec![],
    };
    assert_eq!(
        events(server.traffic(&owner, &mut of_owner).await),
        [
            Event::InviteCreated {
                invite: invite.invite.clone()
            },
            Event::InviteUsed {
                invite_id: invite.invite.id,
                uses: 1
            },
            Event::WorkspaceMemberAdded {
                workspace_id: second.id,
                member: member.clone()
            },
            Event::WorkspaceMemberAdded {
                workspace_id: third.id,
                member
            },
        ]
    );

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
    let approved: JoinRequestStatus = server
        .post(
            &owner,
            &format!("/api/v1/join-requests/{}/approval", request.id),
            &json!({}),
        )
        .await
        .ok(StatusCode::OK);
    let received = events(server.traffic(&owner, &mut of_owner).await);
    assert_eq!(
        received[0],
        Event::JoinRequestCreated {
            join_request: request.clone()
        }
    );
    assert!(matches!(&received[1], Event::AccountAdded { account } if account.name == "Asking"));
    assert!(matches!(&received[2], Event::WorkspaceMemberAdded { .. }));
    assert_eq!(
        received[3],
        Event::JoinRequestDecided {
            join_request_id: request.id,
            state: JoinRequestState::Approved,
            account_id: approved.account_id,
        }
    );
    assert_eq!(received.len(), 4);
}

#[tokio::test]
async fn moderation_and_the_roles_of_the_server_are_recorded() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let guest = server.join(&owner, "Guest").await;
    let workspace = server.default_workspace(&owner).await;
    let role_id = server
        .grant_server(&owner, &guest, &[ServerPermission::CreateInvites])
        .await;
    server.invite(&guest, json!({})).await;
    let mut stream = server.subscribed(&owner, Level::Full).await;
    let account = format!("/api/v1/accounts/{}", guest.account_id);

    server
        .put_empty(&owner, &format!("{account}/ban"))
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .delete(&owner, &format!("{account}/ban"))
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .delete(&owner, &format!("{account}/server-roles/{role_id}"))
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .put_empty(&owner, &format!("{account}/server-roles/{role_id}"))
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .post(&owner, &format!("{account}/removal"), &json!({}))
        .await
        .empty(StatusCode::NO_CONTENT);

    let status = |status| Event::AccountStatusChanged {
        account_id: guest.account_id,
        status,
    };
    let roles = |server_role_ids| Event::AccountServerRolesChanged {
        account_id: guest.account_id,
        server_role_ids,
    };
    assert_eq!(
        events(server.traffic(&owner, &mut stream).await),
        [
            status(AccountStatus::Banned),
            Event::InvitesOfCreatorRevoked {
                created_by: guest.account_id,
                revocation: InviteRevocation::CreatorBanned,
            },
            status(AccountStatus::Active),
            roles(vec![]),
            roles(vec![role_id]),
            status(AccountStatus::Removed),
            Event::WorkspaceMemberRemoved {
                workspace_id: workspace.id,
                account_id: guest.account_id,
            },
            roles(vec![]),
        ]
    );
}

#[tokio::test]
async fn a_channel_override_is_told_to_the_managers_of_the_channel() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let channel = server.channel(&owner, workspace.id, "plans").await;
    let everyone = server.everyone_role(&owner, workspace.id).await;
    let mut stream = server.subscribed(&owner, Level::Full).await;
    let path = format!("/api/v1/channels/{}/overrides/{}", channel.id, everyone.id);
    let deny = json!({ "deny": [WorkspacePermission::ManageMessages] });

    server
        .put(&owner, &path, &deny)
        .await
        .ok::<Value>(StatusCode::OK);
    // Each change has its own system line (ADR 0408).
    server
        .clock
        .advance(junto_server::messages::JOINED_LINES_WITHIN);
    server
        .put(&owner, &path, &deny)
        .await
        .ok::<Value>(StatusCode::OK);
    // Each change has its own system line (ADR 0408).
    server
        .clock
        .advance(junto_server::messages::JOINED_LINES_WITHIN);
    server
        .put(&owner, &path, &json!({}))
        .await
        .ok::<Value>(StatusCode::OK);

    let set = |deny: Vec<WorkspacePermission>| Event::ChannelOverrideSet {
        workspace_id: workspace.id,
        channel_id: channel.id,
        role_id: everyone.id,
        allow: vec![],
        deny,
    };
    let notice = Event::PermissionsChanged {
        workspace_id: Some(workspace.id),
    };
    let history: junto_protocol::message::MessagePage = server
        .get(&owner, &format!("/api/v1/channels/{}/messages", channel.id))
        .await
        .ok(StatusCode::OK);
    let changes: Vec<_> = history
        .messages
        .iter()
        .filter(|m| {
            m.system
                .as_ref()
                .is_some_and(|s| s.action == "channel_override_set")
        })
        .map(|m| Event::MessageChanged {
            place: m.place,
            message_id: m.id,
            position: m.position,
            deleted: false,
        })
        .collect();
    assert_eq!(changes.len(), 3);
    assert_eq!(
        events(server.traffic(&owner, &mut stream).await),
        [
            changes[0].clone(),
            set(vec![WorkspacePermission::ManageMessages]),
            notice.clone(),
            changes[1].clone(),
            set(vec![WorkspacePermission::ManageMessages]),
            changes[2].clone(),
            set(vec![]),
            notice,
        ]
    );
}

#[tokio::test]
async fn the_host_command_that_assigns_the_owner_reaches_the_connected_clients() {
    let server = TestServer::start_with(|config| {
        config.limits.stream.poll_interval = time::Duration::milliseconds(50);
    })
    .await;
    let owner = server.owner().await;
    let guest = server.join(&owner, "Guest").await;
    let mut of_owner = server.subscribed(&owner, Level::Background).await;
    let mut of_guest = server.subscribed(&guest, Level::Full).await;

    let command = server
        .host_command(&["assign-owner", &guest.account_id.to_string()])
        .await;
    assert!(command.success, "{}", command.stderr);

    let on_server = Event::PermissionsChanged { workspace_id: None };
    assert_eq!(
        events(server.traffic(&owner, &mut of_owner).await),
        std::slice::from_ref(&on_server)
    );
    assert_eq!(
        events(server.traffic(&guest, &mut of_guest).await),
        [
            Event::OwnerChanged {
                account_id: guest.account_id,
                previous_owner_id: Some(owner.account_id),
            },
            on_server,
        ]
    );
    assert!(server.snapshot(&guest).await.account.is_owner);
}
