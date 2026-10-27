//! One table lists every operation that needs a session. For each one, a caller without the
//! permission gets the stated denial and the database is unchanged, and a request without a
//! session gets `unauthenticated`. The table is compared with the generated OpenAPI document,
//! so an operation that is missing here fails a test.

use std::collections::{BTreeMap, BTreeSet};

use junto_protocol::access_token::{AccessTokenCreated, TokenAction};

use junto_protocol::auth::ChallengeOperation;
use junto_protocol::error::ErrorCode;
use junto_protocol::invite::JoinRequest;
use junto_protocol::roles::{ServerRole, WorkspaceRole};
use junto_protocol::structure::{Category, Channel, Workspace};
use junto_protocol::{ServerPermission, WorkspacePermission};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use crate::support::{DeviceKey, Member, TestServer};

#[derive(Debug, Clone, Copy, PartialEq)]
enum Caller {
    /// A member of the default workspace without any role.
    Plain,
    /// An account with every server permission and every workspace permission, not the owner.
    Administrator,
}

struct Operation {
    method: Method,
    template: &'static str,
    path: String,
    body: Value,
    /// The callers that must be denied, each with the code of the denial. Empty for an
    /// operation on the caller's own account, which only a missing session denies.
    denials: Vec<(Caller, ErrorCode)>,
}

struct World {
    /// The directory of the exports of the server.
    _exports: crate::support::Scratch,
    server: TestServer,
    plain: Member,
    administrator: Member,
    operations: Vec<Operation>,
}

fn operation(
    method: Method,
    template: &'static str,
    values: &[&dyn std::fmt::Display],
    body: Value,
    denials: &[(Caller, ErrorCode)],
) -> Operation {
    let mut values = values.iter();
    let path = template
        .split('/')
        .map(|segment| {
            if segment.starts_with('{') {
                values.next().unwrap().to_string()
            } else {
                segment.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("/");
    assert!(values.next().is_none(), "{template}");
    Operation {
        method,
        template,
        path,
        body,
        denials: denials.to_vec(),
    }
}

async fn world() -> World {
    let exports = crate::support::Scratch::new();
    let server = TestServer::start_for_calls_with(|config| {
        config.exports_directory = Some(exports.0.clone());
    })
    .await;
    let free = junto_server::storage::FakeDisk::new(crate::support::FREE_DISK);
    server
        .state
        .exports
        .space()
        .use_disk(std::sync::Arc::new(free));
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let plain = server.join(&owner, "Plain").await;
    let target = server.join(&owner, "Target").await;
    let removed = server.join(&owner, "Removed").await;
    let administrator = server.join(&owner, "Administrator").await;
    server
        .grant_server(&owner, &administrator, ServerPermission::ALL)
        .await;
    server
        .grant_workspace(
            &owner,
            &administrator,
            workspace.id,
            WorkspacePermission::ALL,
        )
        .await;

    let closed: Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({ "name": "Closed" }))
        .await
        .ok(StatusCode::CREATED);
    let outsider = server.join_into(&owner, "Outsider", &[closed.id]).await;
    server
        .put_bytes(
            &outsider,
            "/api/v1/account/avatar",
            None,
            junto_server::image::samples::png(8, 8),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    let server_role: ServerRole = server
        .post(
            &owner,
            "/api/v1/server-roles",
            &json!({ "name": "inviters" }),
        )
        .await
        .ok(StatusCode::CREATED);
    let role: WorkspaceRole = server
        .post(
            &owner,
            &format!("/api/v1/workspaces/{}/roles", workspace.id),
            &json!({ "name": "helpers" }),
        )
        .await
        .ok(StatusCode::CREATED);
    let category: Category = server
        .post(
            &owner,
            &format!("/api/v1/workspaces/{}/categories", workspace.id),
            &json!({ "name": "Projects" }),
        )
        .await
        .ok(StatusCode::CREATED);
    let channel: Channel = server
        .post(
            &owner,
            &format!("/api/v1/workspaces/{}/channels", workspace.id),
            &json!({ "name": "plans", "kind": "text" }),
        )
        .await
        .ok(StatusCode::CREATED);
    let permanent: Channel = server
        .post(
            &owner,
            &format!("/api/v1/workspaces/{}/channels", workspace.id),
            &json!({ "name": "records", "kind": "text" }),
        )
        .await
        .ok(StatusCode::CREATED);
    sqlx::query("update channels set permanent_message_count = 1 where id = $1")
        .bind(permanent.id.as_uuid())
        .execute(&server.db)
        .await
        .unwrap();
    // What the deletions of the table delete, so that every other operation keeps its
    // subject.
    let spare_role: WorkspaceRole = server
        .post(
            &owner,
            &format!("/api/v1/workspaces/{}/roles", workspace.id),
            &json!({ "name": "spare" }),
        )
        .await
        .ok(StatusCode::CREATED);
    let spare_server_role: ServerRole = server
        .post(&owner, "/api/v1/server-roles", &json!({ "name": "spare" }))
        .await
        .ok(StatusCode::CREATED);
    let spare_category: Category = server
        .post(
            &owner,
            &format!("/api/v1/workspaces/{}/categories", workspace.id),
            &json!({ "name": "Spare" }),
        )
        .await
        .ok(StatusCode::CREATED);
    // A category without a channel that a member sees is hidden from the member (ADR 0304),
    // so each category holds one visible channel.
    let mut inside = Vec::new();
    for held in [&category, &spare_category] {
        let channel: Channel = server
            .post(
                &owner,
                &format!("/api/v1/workspaces/{}/channels", workspace.id),
                &json!({ "name": "inside", "kind": "text", "category_id": held.id }),
            )
            .await
            .ok(StatusCode::CREATED);
        inside.push(channel.id);
    }
    let synced = inside[0];
    let doomed: Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({ "name": "Doomed" }))
        .await
        .ok(StatusCode::CREATED);
    let shelf: Channel = server
        .post(
            &owner,
            &format!("/api/v1/workspaces/{}/channels", workspace.id),
            &json!({ "name": "shelf", "kind": "text" }),
        )
        .await
        .ok(StatusCode::CREATED);
    let shelved: Channel = server
        .post(
            &owner,
            &format!("/api/v1/workspaces/{}/channels", workspace.id),
            &json!({ "name": "shelved", "kind": "text" }),
        )
        .await
        .ok(StatusCode::CREATED);
    server
        .put_empty(&owner, &format!("/api/v1/channels/{}/archive", shelved.id))
        .await
        .ok::<Value>(StatusCode::OK);
    let invite = server.invite(&owner, json!({ "max_uses": 5 })).await;
    let proof = server
        .proof(&DeviceKey::generate(), ChallengeOperation::RequestToJoin)
        .await;
    let request: JoinRequest = server
        .open(
            "/api/v1/join-requests",
            &json!({ "proof": proof, "name": "Newcomer" }),
        )
        .await
        .ok(StatusCode::CREATED);
    let proof = server
        .proof(&DeviceKey::generate(), ChallengeOperation::RequestToJoin)
        .await;
    let refused: JoinRequest = server
        .open(
            "/api/v1/join-requests",
            &json!({ "proof": proof, "name": "Unwanted" }),
        )
        .await
        .ok(StatusCode::CREATED);

    let call_guest = server.join(&owner, "CallGuest").await;
    let hall = server.voice(&owner, workspace.id, "hall").await;
    let annex = server.voice(&owner, workspace.id, "annex").await;
    let vault = server.voice(&owner, workspace.id, "vault").await;
    server
        .make_private(&owner, workspace.id, vault.id, role.id)
        .await;
    let seat = server.enter_call(&administrator, hall.id).await;
    server.enter_call(&call_guest, hall.id).await;
    server
        .put(
            &administrator,
            "/api/v1/call/screen-share",
            &json!({ "audio": "none" }),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    assert!(server.fake().publish(
        &seat.room,
        &seat.identity,
        junto_server::media::TrackKind::Video,
        junto_server::media::TrackSource::ScreenShare,
    ));
    server.check().await;

    use Caller::{Administrator, Plain};
    use ErrorCode::{Conflict, Forbidden, NotFound, NotInCall, OwnerOnly};
    let outside = [(Plain, NotInCall)];
    let unknown = [(Plain, NotFound), (Administrator, NotFound)];
    let absent = uuid::Uuid::now_v7();
    let (hall_id, guest_id) = (hall.id, call_guest.account_id);
    let forbidden = [(Plain, Forbidden)];
    let token_expiry = (server.clock.now() + time::Duration::days(30))
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap();
    let own: [(Caller, ErrorCode); 0] = [];
    let none = Value::Null;
    let new_key = || json!({ "public_key": DeviceKey::generate().public() });
    let (ws, closed_id, target_id) = (workspace.id, closed.id, target.account_id);

    let mut operations = vec![
        operation(
            Method::PUT,
            "/api/v1/server/logo",
            &[],
            none.clone(),
            &forbidden,
        ),
        operation(
            Method::DELETE,
            "/api/v1/server/logo",
            &[],
            none.clone(),
            &forbidden,
        ),
        operation(
            Method::GET,
            "/api/v1/attachment-limits",
            &[],
            none.clone(),
            &own,
        ),
        operation(
            Method::PUT,
            "/api/v1/attachment-limits",
            &[],
            json!({"bytes_per_file":100_000_000,"files_per_message":10}),
            &forbidden,
        ),
        operation(
            Method::GET,
            "/api/v1/channels/{channel_id}/messages/read",
            &[&absent],
            none.clone(),
            &unknown,
        ),
        operation(
            Method::GET,
            "/api/v1/conversations/{conversation_id}/messages/read",
            &[&absent],
            none.clone(),
            &unknown,
        ),
        operation(
            Method::GET,
            "/api/v1/messages/{message_id}",
            &[&absent],
            none.clone(),
            &unknown,
        ),
        operation(
            Method::PUT,
            "/api/v1/messages/{message_id}",
            &[&absent],
            crate::messages::text("edit"),
            &unknown,
        ),
        operation(
            Method::DELETE,
            "/api/v1/messages/{message_id}",
            &[&absent],
            none.clone(),
            &unknown,
        ),
        operation(
            Method::PUT,
            "/api/v1/messages/{message_id}/reaction",
            &[&absent],
            json!({"emoji":"👍"}),
            &unknown,
        ),
        operation(
            Method::DELETE,
            "/api/v1/messages/{message_id}/reaction/{emoji}",
            &[&absent, &"👍"],
            none.clone(),
            &unknown,
        ),
        operation(
            Method::GET,
            "/api/v1/channels/{channel_id}/messages",
            &[&absent],
            none.clone(),
            &unknown,
        ),
        operation(
            Method::POST,
            "/api/v1/channels/{channel_id}/messages",
            &[&absent],
            crate::messages::text("sent"),
            &unknown,
        ),
        operation(
            Method::PUT,
            "/api/v1/channels/{channel_id}/messages/read",
            &[&absent],
            json!({"position":0}),
            &unknown,
        ),
        operation(
            Method::GET,
            "/api/v1/conversations/{conversation_id}/messages",
            &[&absent],
            none.clone(),
            &unknown,
        ),
        operation(
            Method::POST,
            "/api/v1/conversations/{conversation_id}/messages",
            &[&absent],
            crate::messages::text("sent"),
            &unknown,
        ),
        operation(
            Method::PUT,
            "/api/v1/conversations/{conversation_id}/messages/read",
            &[&absent],
            json!({"position":0}),
            &unknown,
        ),
        operation(
            Method::GET,
            "/api/v1/attachments/{attachment_id}/upload",
            &[&absent],
            none.clone(),
            &unknown,
        ),
        operation(
            Method::DELETE,
            "/api/v1/attachments/{attachment_id}/upload",
            &[&absent],
            none.clone(),
            &unknown,
        ),
        operation(
            Method::POST,
            "/api/v1/attachments/{attachment_id}/complete",
            &[&absent],
            none.clone(),
            &unknown,
        ),
        operation(
            Method::GET,
            "/api/v1/attachments/{attachment_id}/file",
            &[&absent],
            none.clone(),
            &unknown,
        ),
        operation(
            Method::PUT,
            "/api/v1/attachments/{attachment_id}/chunks/{offset}",
            &[&absent, &0],
            json!({"data":"chunk"}),
            &unknown,
        ),
        operation(
            Method::POST,
            "/api/v1/attachments",
            &[],
            json!({"channel_id":absent,"name":"file.bin","bytes":1,"sha256":"00".repeat(32)}),
            &unknown,
        ),
        operation(
            Method::GET,
            "/api/v1/conversations",
            &[],
            none.clone(),
            &own,
        ),
        operation(
            Method::GET,
            "/api/v1/direct-messages/contacts",
            &[],
            none.clone(),
            &own,
        ),
        operation(
            Method::POST,
            "/api/v1/conversations",
            &[],
            json!({"account_id":outsider.account_id}),
            &[(Plain, NotFound)],
        ),
        operation(
            Method::DELETE,
            "/api/v1/auth/session",
            &[],
            none.clone(),
            &own,
        ),
        operation(Method::GET, "/api/v1/account", &[], none.clone(), &own),
        operation(
            Method::PATCH,
            "/api/v1/account",
            &[],
            json!({ "name": "Renamed" }),
            &own,
        ),
        operation(Method::DELETE, "/api/v1/account", &[], none.clone(), &own),
        operation(
            Method::PUT,
            "/api/v1/account/avatar",
            &[],
            none.clone(),
            &own,
        ),
        operation(
            Method::DELETE,
            "/api/v1/account/avatar",
            &[],
            none.clone(),
            &own,
        ),
        operation(
            Method::GET,
            "/api/v1/accounts/{account_id}/profile",
            &[&outsider.account_id],
            none.clone(),
            &[(Plain, NotFound)],
        ),
        operation(
            Method::GET,
            "/api/v1/accounts/{account_id}/avatar",
            &[&outsider.account_id],
            none.clone(),
            &[(Plain, NotFound)],
        ),
        operation(
            Method::GET,
            "/api/v1/account/recovery-notices",
            &[],
            none.clone(),
            &own,
        ),
        operation(
            Method::DELETE,
            "/api/v1/account/recovery-notices/{recovery_id}",
            &[&junto_protocol::ids::RecoveryId::generate()],
            none.clone(),
            &[(Plain, NotFound), (Administrator, NotFound)],
        ),
        operation(
            Method::GET,
            "/api/v1/account/devices",
            &[],
            none.clone(),
            &own,
        ),
        operation(
            Method::PATCH,
            "/api/v1/account/devices/{device_id}",
            &[&target.device_id],
            json!({ "name": "taken" }),
            &[(Plain, NotFound), (Administrator, NotFound)],
        ),
        operation(
            Method::DELETE,
            "/api/v1/account/devices/{device_id}",
            &[&target.device_id],
            none.clone(),
            &[(Plain, NotFound), (Administrator, NotFound)],
        ),
        operation(
            Method::GET,
            "/api/v1/account/recoveries",
            &[],
            none.clone(),
            &own,
        ),
        operation(
            Method::POST,
            "/api/v1/account/device-links/lookup",
            &[],
            json!({ "code": "ZZZZ-ZZZZ" }),
            &own,
        ),
        operation(
            Method::POST,
            "/api/v1/account/device-links/approve",
            &[],
            json!({ "code": "ZZZZ-ZZZZ", "public_key": DeviceKey::generate().public() }),
            &own,
        ),
        operation(Method::POST, "/api/v1/invites", &[], json!({}), &forbidden),
        operation(
            Method::GET,
            "/api/v1/invites",
            &[],
            none.clone(),
            &forbidden,
        ),
        operation(
            Method::DELETE,
            "/api/v1/invites/{invite_id}",
            &[&invite.invite.id],
            none.clone(),
            &forbidden,
        ),
        operation(
            Method::POST,
            "/api/v1/invites/redemption",
            &[],
            json!({ "invite_code": "ZZZZ-ZZZZ-ZZZZ-ZZZZ" }),
            &own,
        ),
        operation(
            Method::GET,
            "/api/v1/join-requests",
            &[],
            none.clone(),
            &forbidden,
        ),
        operation(
            Method::POST,
            "/api/v1/join-requests/{join_request_id}/approval",
            &[&request.id],
            json!({}),
            &forbidden,
        ),
        operation(
            Method::POST,
            "/api/v1/join-requests/{join_request_id}/denial",
            &[&refused.id],
            json!({}),
            &forbidden,
        ),
        operation(
            Method::GET,
            "/api/v1/accounts",
            &[],
            none.clone(),
            &forbidden,
        ),
        operation(
            Method::POST,
            "/api/v1/accounts/{account_id}/removal",
            &[&removed.account_id],
            json!({}),
            &forbidden,
        ),
        operation(
            Method::PUT,
            "/api/v1/accounts/{account_id}/ban",
            &[&target_id],
            none.clone(),
            &forbidden,
        ),
        operation(
            Method::DELETE,
            "/api/v1/accounts/{account_id}/ban",
            &[&target_id],
            none.clone(),
            &forbidden,
        ),
        operation(
            Method::POST,
            "/api/v1/accounts/{account_id}/recovery",
            &[&target_id],
            new_key(),
            &[(Plain, OwnerOnly), (Administrator, OwnerOnly)],
        ),
        operation(
            Method::GET,
            "/api/v1/server-roles",
            &[],
            none.clone(),
            &forbidden,
        ),
        operation(
            Method::POST,
            "/api/v1/server-roles",
            &[],
            json!({ "name": "mine", "permissions": ["administrator"] }),
            &forbidden,
        ),
        operation(
            Method::PATCH,
            "/api/v1/server-roles/positions",
            &[],
            json!({"positions": []}),
            &forbidden,
        ),
        operation(
            Method::PATCH,
            "/api/v1/workspaces/{workspace_id}/roles/positions",
            &[&ws],
            json!({"positions": []}),
            &forbidden,
        ),
        operation(
            Method::GET,
            "/api/v1/account/moderation-notices",
            &[],
            none.clone(),
            &own,
        ),
        operation(
            Method::PATCH,
            "/api/v1/server-roles/{role_id}/permissions",
            &[&server_role.id],
            json!({"permission":"administrator","enabled":true}),
            &forbidden,
        ),
        operation(
            Method::PATCH,
            "/api/v1/workspaces/{workspace_id}/roles/{role_id}/permissions",
            &[&ws, &role.id],
            json!({"permission":"manage_roles","enabled":true}),
            &forbidden,
        ),
        operation(
            Method::PATCH,
            "/api/v1/server-roles/{role_id}",
            &[&server_role.id],
            json!({ "permissions": ["administrator"] }),
            &forbidden,
        ),
        operation(
            Method::PUT,
            "/api/v1/accounts/{account_id}/server-roles/{role_id}",
            &[&plain.account_id, &server_role.id],
            none.clone(),
            &forbidden,
        ),
        operation(
            Method::DELETE,
            "/api/v1/accounts/{account_id}/server-roles/{role_id}",
            &[&administrator.account_id, &server_role.id],
            none.clone(),
            &forbidden,
        ),
        operation(Method::GET, "/api/v1/workspaces", &[], none.clone(), &own),
        operation(
            Method::POST,
            "/api/v1/workspaces",
            &[],
            json!({ "name": "Mine" }),
            &forbidden,
        ),
        operation(
            Method::PATCH,
            "/api/v1/workspaces/{workspace_id}",
            &[&ws],
            json!({ "name": "Mine" }),
            &forbidden,
        ),
        operation(
            Method::GET,
            "/api/v1/workspaces/{workspace_id}/members",
            &[&closed_id],
            none.clone(),
            &[(Plain, NotFound)],
        ),
        operation(
            Method::POST,
            "/api/v1/workspaces/{workspace_id}/members/{account_id}/removal",
            &[&closed_id, &outsider.account_id],
            none.clone(),
            &[(Caller::Plain, NotFound)],
        ),
        // A caller leaves only a workspace of which it is a member, whatever its
        // permissions (ADR 0121): neither caller is a member of this one.
        operation(
            Method::POST,
            "/api/v1/workspaces/{workspace_id}/departure",
            &[&closed_id],
            none.clone(),
            &[(Caller::Plain, NotFound), (Caller::Administrator, NotFound)],
        ),
        operation(
            Method::PUT,
            "/api/v1/workspaces/{workspace_id}/members/{account_id}",
            &[&closed_id, &plain.account_id],
            none.clone(),
            &forbidden,
        ),
        operation(
            Method::GET,
            "/api/v1/workspaces/{workspace_id}/roles",
            &[&closed_id],
            none.clone(),
            &[(Plain, NotFound)],
        ),
        operation(
            Method::POST,
            "/api/v1/workspaces/{workspace_id}/roles",
            &[&ws],
            json!({ "name": "mine" }),
            &forbidden,
        ),
        operation(
            Method::PATCH,
            "/api/v1/workspaces/{workspace_id}/roles/{role_id}",
            &[&ws, &role.id],
            json!({ "permissions": ["manage_roles"] }),
            &forbidden,
        ),
        operation(
            Method::PUT,
            "/api/v1/workspaces/{workspace_id}/members/{account_id}/roles/{role_id}",
            &[&ws, &plain.account_id, &role.id],
            none.clone(),
            &forbidden,
        ),
        operation(
            Method::DELETE,
            "/api/v1/workspaces/{workspace_id}/members/{account_id}/roles/{role_id}",
            &[&ws, &administrator.account_id, &role.id],
            none.clone(),
            &forbidden,
        ),
        operation(
            Method::POST,
            "/api/v1/workspaces/{workspace_id}/categories",
            &[&ws],
            json!({ "name": "mine" }),
            &forbidden,
        ),
        operation(
            Method::PATCH,
            "/api/v1/workspaces/{workspace_id}/categories/{category_id}",
            &[&ws, &category.id],
            json!({ "name": "mine" }),
            &forbidden,
        ),
        operation(
            Method::GET,
            "/api/v1/workspaces/{workspace_id}/channels",
            &[&closed_id],
            none.clone(),
            &[(Plain, NotFound)],
        ),
        operation(
            Method::POST,
            "/api/v1/workspaces/{workspace_id}/channels",
            &[&ws],
            json!({ "name": "mine", "kind": "text" }),
            &forbidden,
        ),
        operation(
            Method::PATCH,
            "/api/v1/channels/{channel_id}",
            &[&channel.id],
            json!({ "name": "mine", "permanent_messages": true }),
            &forbidden,
        ),
        operation(
            Method::PATCH,
            "/api/v1/workspaces/{workspace_id}/channels/positions",
            &[&ws],
            json!({ "moves": [{
                "channel_id": channel.id,
                "from": { "category_id": null, "position": channel.position },
                "to": { "category_id": null, "position": channel.position + 1 },
            }]}),
            &forbidden,
        ),
        operation(
            Method::GET,
            "/api/v1/channels/{channel_id}/rules",
            &[&channel.id],
            none.clone(),
            &forbidden,
        ),
        operation(
            Method::PATCH,
            "/api/v1/channels/{channel_id}/rules/roles/{role_id}",
            &[&channel.id, &role.id],
            json!({"permission":"manage_channel", "state":"allow"}),
            &forbidden,
        ),
        operation(
            Method::PUT,
            "/api/v1/channels/{channel_id}/rules/roles/{role_id}",
            &[&channel.id, &role.id],
            json!({ "allow": ["manage_channel"] }),
            &forbidden,
        ),
        operation(
            Method::PUT,
            "/api/v1/channels/{channel_id}/rules/people/{account_id}",
            &[&channel.id, &target_id],
            json!({ "allow": ["manage_channel"] }),
            &forbidden,
        ),
        operation(
            Method::PATCH,
            "/api/v1/channels/{channel_id}/rules/people/{account_id}",
            &[&channel.id, &target_id],
            json!({"permission":"manage_channel", "state":"allow"}),
            &forbidden,
        ),
        operation(
            Method::POST,
            "/api/v1/channels/{channel_id}/rules/sync",
            &[&synced],
            none.clone(),
            &forbidden,
        ),
        operation(
            Method::GET,
            "/api/v1/workspaces/{workspace_id}/categories/{category_id}/rules",
            &[&ws, &category.id],
            none.clone(),
            &forbidden,
        ),
        operation(
            Method::PUT,
            "/api/v1/workspaces/{workspace_id}/categories/{category_id}/rules/roles/{role_id}",
            &[&ws, &category.id, &role.id],
            json!({ "allow": ["manage_channel"] }),
            &forbidden,
        ),
        operation(
            Method::PATCH,
            "/api/v1/workspaces/{workspace_id}/categories/{category_id}/rules/roles/{role_id}",
            &[&ws, &category.id, &role.id],
            json!({"permission":"manage_channel", "state":"allow"}),
            &forbidden,
        ),
        operation(
            Method::PUT,
            "/api/v1/workspaces/{workspace_id}/categories/{category_id}/rules/people/{account_id}",
            &[&ws, &category.id, &target_id],
            json!({ "allow": ["manage_channel"] }),
            &forbidden,
        ),
        operation(
            Method::PATCH,
            "/api/v1/workspaces/{workspace_id}/categories/{category_id}/rules/people/{account_id}",
            &[&ws, &category.id, &target_id],
            json!({"permission":"manage_channel", "state":"allow"}),
            &forbidden,
        ),
        operation(
            Method::GET,
            "/api/v1/workspaces/{workspace_id}/members/{account_id}/permissions",
            &[&ws, &target_id],
            none.clone(),
            &forbidden,
        ),
        operation(
            Method::PUT,
            "/api/v1/workspaces/{workspace_id}/members/{account_id}/permissions",
            &[&ws, &target_id],
            json!({ "allow": ["manage_channel"] }),
            &forbidden,
        ),
        operation(
            Method::PATCH,
            "/api/v1/workspaces/{workspace_id}/members/{account_id}/permissions",
            &[&ws, &target_id],
            json!({"permission":"manage_channel", "state":"allow"}),
            &forbidden,
        ),
        operation(
            Method::GET,
            "/api/v1/workspaces/{workspace_id}/members/{account_id}/access",
            &[&ws, &target_id],
            none.clone(),
            &forbidden,
        ),
        operation(
            Method::GET,
            "/api/v1/accounts/{account_id}/server-permissions",
            &[&target_id],
            none.clone(),
            &forbidden,
        ),
        operation(
            Method::PUT,
            "/api/v1/accounts/{account_id}/server-permissions",
            &[&target_id],
            json!({ "allow": ["create_invites"] }),
            &forbidden,
        ),
        operation(
            Method::PATCH,
            "/api/v1/accounts/{account_id}/server-permissions",
            &[&target_id],
            json!({"permission":"create_invites", "state":"allow"}),
            &forbidden,
        ),
        operation(
            Method::DELETE,
            "/api/v1/channels/{channel_id}",
            &[&permanent.id],
            none.clone(),
            &[(Plain, OwnerOnly), (Administrator, OwnerOnly)],
        ),
        operation(
            Method::DELETE,
            "/api/v1/channels/{channel_id}",
            &[&channel.id],
            none.clone(),
            &forbidden,
        ),
        operation(
            Method::GET,
            "/api/v1/audit-log",
            &[],
            none.clone(),
            &forbidden,
        ),
        operation(
            Method::GET,
            "/api/v1/workspaces/{workspace_id}/audit-log",
            &[&ws],
            none.clone(),
            &forbidden,
        ),
        operation(Method::GET, "/api/v1/call", &[], none.clone(), &own),
        operation(
            Method::PUT,
            "/api/v1/call/state",
            &[],
            json!({ "muted": true, "deafened": false }),
            &own,
        ),
        operation(Method::PUT, "/api/v1/call/hand", &[], none.clone(), &own),
        operation(Method::DELETE, "/api/v1/call/hand", &[], none.clone(), &own),
        operation(
            Method::PUT,
            "/api/v1/call/speaking",
            &[],
            json!({ "speaking": true }),
            &own,
        ),
        operation(
            Method::GET,
            "/api/v1/channels/{channel_id}/call",
            &[&vault.id],
            none.clone(),
            &[(Plain, NotFound)],
        ),
        operation(
            Method::PATCH,
            "/api/v1/channels/{channel_id}/call/settings",
            &[&hall_id],
            json!({ "user_limit": 50 }),
            &forbidden,
        ),
        operation(
            Method::PUT,
            "/api/v1/channels/{channel_id}/call/lock",
            &[&hall_id],
            none.clone(),
            &outside,
        ),
        operation(
            Method::DELETE,
            "/api/v1/channels/{channel_id}/call/lock",
            &[&hall_id],
            none.clone(),
            &outside,
        ),
        operation(
            Method::POST,
            "/api/v1/channels/{channel_id}/call/entry-requests",
            &[&hall_id],
            none.clone(),
            &[(Plain, Conflict), (Administrator, Conflict)],
        ),
        operation(
            Method::POST,
            "/api/v1/call/entry-requests/{request_id}/answer",
            &[&absent],
            json!({ "accept": true }),
            &unknown,
        ),
        operation(
            Method::DELETE,
            "/api/v1/call/entry-requests/{request_id}",
            &[&absent],
            none.clone(),
            &unknown,
        ),
        operation(
            Method::POST,
            "/api/v1/channels/{channel_id}/call/invitations",
            &[&hall_id],
            json!({ "account_id": plain.account_id }),
            &outside,
        ),
        operation(
            Method::POST,
            "/api/v1/call/invitations/{invitation_id}/acceptance",
            &[&absent],
            none.clone(),
            &unknown,
        ),
        operation(
            Method::POST,
            "/api/v1/call/invitations/{invitation_id}/refusal",
            &[&absent],
            none.clone(),
            &unknown,
        ),
        operation(
            Method::DELETE,
            "/api/v1/call/invitations/{invitation_id}",
            &[&absent],
            none.clone(),
            &unknown,
        ),
        operation(
            Method::POST,
            "/api/v1/channels/{channel_id}/call/participants/{account_id}/move",
            &[&hall_id, &guest_id],
            json!({ "to_channel_id": annex.id }),
            &forbidden,
        ),
        operation(
            Method::POST,
            "/api/v1/call/moves/{move_id}/join",
            &[&absent],
            none.clone(),
            &unknown,
        ),
        operation(
            Method::POST,
            "/api/v1/call/moves/{move_id}/failure",
            &[&absent],
            none.clone(),
            &unknown,
        ),
        operation(
            Method::PUT,
            "/api/v1/channels/{channel_id}/call/meeting",
            &[&hall_id],
            none.clone(),
            &outside,
        ),
        operation(
            Method::DELETE,
            "/api/v1/channels/{channel_id}/call/meeting",
            &[&hall_id],
            none.clone(),
            &outside,
        ),
        operation(
            Method::PUT,
            "/api/v1/channels/{channel_id}/call/recording",
            &[&hall_id],
            json!({}),
            &forbidden,
        ),
        operation(
            Method::DELETE,
            "/api/v1/channels/{channel_id}/call/recording",
            &[&hall_id],
            none.clone(),
            &forbidden,
        ),
        operation(
            Method::PUT,
            "/api/v1/channels/{channel_id}/call/participants/{account_id}/microphone-block",
            &[&hall_id, &guest_id],
            none.clone(),
            &forbidden,
        ),
        operation(
            Method::DELETE,
            "/api/v1/channels/{channel_id}/call/participants/{account_id}/microphone-block",
            &[&hall_id, &guest_id],
            none.clone(),
            &forbidden,
        ),
        operation(
            Method::PUT,
            "/api/v1/channels/{channel_id}/call/participants/{account_id}/sound-block",
            &[&hall_id, &guest_id],
            none.clone(),
            &forbidden,
        ),
        operation(
            Method::DELETE,
            "/api/v1/channels/{channel_id}/call/participants/{account_id}/sound-block",
            &[&hall_id, &guest_id],
            none.clone(),
            &forbidden,
        ),
        operation(
            Method::DELETE,
            "/api/v1/channels/{channel_id}/call/participants/{account_id}/hand",
            &[&hall_id, &guest_id],
            none.clone(),
            &forbidden,
        ),
        operation(
            Method::POST,
            "/api/v1/access-tokens",
            &[],
            json!({ "name": "automation", "actions": ["read_structure"],
                    "expires_at": token_expiry }),
            &forbidden,
        ),
        operation(
            Method::GET,
            "/api/v1/access-tokens",
            &[],
            none.clone(),
            &own,
        ),
        operation(
            Method::DELETE,
            "/api/v1/access-tokens/{access_token_id}",
            &[&absent],
            none.clone(),
            &unknown,
        ),
        operation(
            Method::GET,
            "/api/v1/structure",
            &[],
            none.clone(),
            &forbidden,
        ),
        // The revision is never the current one, so the request of a caller who may send it
        // changes nothing.
        operation(
            Method::POST,
            "/api/v1/structure/changes",
            &[],
            json!({ "revision": "never the current revision", "changes": [] }),
            &[(Plain, Forbidden), (Administrator, Conflict)],
        ),
        operation(
            Method::DELETE,
            "/api/v1/channels/{channel_id}/call/participants/{account_id}",
            &[&hall_id, &guest_id],
            none.clone(),
            &forbidden,
        ),
        operation(
            Method::PUT,
            "/api/v1/call/screen-share",
            &[],
            json!({ "audio": "none" }),
            &own,
        ),
        operation(
            Method::DELETE,
            "/api/v1/call/screen-share",
            &[],
            none.clone(),
            &own,
        ),
        operation(
            Method::POST,
            "/api/v1/direct-calls/{account_id}/refusal",
            &[&outsider.account_id],
            none.clone(),
            &unknown,
        ),
        operation(
            Method::POST,
            "/api/v1/direct-calls/{account_id}/join",
            &[&outsider.account_id],
            none.clone(),
            &[(Plain, NotFound)],
        ),
        operation(
            Method::PUT,
            "/api/v1/direct-messages/{account_id}/mute",
            &[&outsider.account_id],
            json!({}),
            &[(Plain, NotFound)],
        ),
        operation(
            Method::DELETE,
            "/api/v1/direct-messages/{account_id}/mute",
            &[&outsider.account_id],
            none.clone(),
            &[(Plain, NotFound)],
        ),
        operation(
            Method::POST,
            "/api/v1/channels/{channel_id}/call/join",
            &[&vault.id],
            none.clone(),
            &[(Plain, NotFound)],
        ),
        operation(
            Method::GET,
            "/api/v1/workspaces/{workspace_id}/recordings",
            &[&closed_id],
            none.clone(),
            &[(Plain, NotFound)],
        ),
        operation(
            Method::GET,
            "/api/v1/recordings/{recording_id}",
            &[&absent],
            none.clone(),
            &unknown,
        ),
        operation(
            Method::DELETE,
            "/api/v1/recordings/{recording_id}",
            &[&absent],
            none.clone(),
            &unknown,
        ),
        operation(
            Method::GET,
            "/api/v1/recordings/{recording_id}/segments/{segment_id}/file",
            &[&absent, &absent],
            none.clone(),
            &unknown,
        ),
        operation(
            Method::GET,
            "/api/v1/recordings/{recording_id}/transcript",
            &[&absent],
            none.clone(),
            &unknown,
        ),
        operation(
            Method::POST,
            "/api/v1/recordings/{recording_id}/transcript",
            &[&absent],
            none.clone(),
            &unknown,
        ),
        operation(
            Method::GET,
            "/api/v1/recordings/{recording_id}/transcript/lines",
            &[&absent],
            none.clone(),
            &unknown,
        ),
        operation(
            Method::PUT,
            "/api/v1/recordings/{recording_id}/transcript/lines/{line}",
            &[&absent, &0],
            json!({ "text": "Corrected." }),
            &unknown,
        ),
        operation(
            Method::PUT,
            "/api/v1/recordings/{recording_id}/transcript/speakers/{number}",
            &[&absent, &1],
            json!({ "name": "Noor" }),
            &unknown,
        ),
        operation(
            Method::GET,
            "/api/v1/recordings/{recording_id}/transcript/text",
            &[&absent],
            none.clone(),
            &unknown,
        ),
        operation(
            Method::GET,
            "/api/v1/workspaces/{workspace_id}/glossary",
            &[&closed_id],
            none.clone(),
            &[(Plain, NotFound)],
        ),
        operation(
            Method::PUT,
            "/api/v1/workspaces/{workspace_id}/glossary",
            &[&ws],
            json!({ "terms": ["Quorvex"] }),
            &forbidden,
        ),
        operation(Method::DELETE, "/api/v1/call", &[], none.clone(), &own),
        operation(Method::GET, "/api/v1/snapshot", &[], none.clone(), &own),
        operation(Method::GET, "/api/v1/events", &[], none.clone(), &own),
    ];

    operations.extend([
        operation(
            Method::GET,
            "/api/v1/messages/{message_id}/thread/read",
            &[&absent],
            none.clone(),
            &unknown,
        ),
        operation(
            Method::PUT,
            "/api/v1/messages/{message_id}/thread/read",
            &[&absent],
            json!({"position":0}),
            &unknown,
        ),
        operation(
            Method::GET,
            "/api/v1/workspaces/{workspace_id}/recording-retention",
            &[&ws],
            none.clone(),
            &forbidden,
        ),
        operation(
            Method::PUT,
            "/api/v1/workspaces/{workspace_id}/recording-retention",
            &[&ws],
            json!({"days":30}),
            &forbidden,
        ),
        operation(
            Method::GET,
            "/api/v1/messages/{message_id}/thread",
            &[&absent],
            none.clone(),
            &unknown,
        ),
        operation(
            Method::POST,
            "/api/v1/messages/{message_id}/thread",
            &[&absent],
            crate::messages_support::text("reply"),
            &unknown,
        ),
        operation(
            Method::POST,
            "/api/v1/messages/{message_id}/fork",
            &[&absent],
            json!({"name":"fork","kind":"text"}),
            &unknown,
        ),
        operation(
            Method::GET,
            "/api/v1/channels/{channel_id}/pins",
            &[&absent],
            none.clone(),
            &unknown,
        ),
        operation(
            Method::PUT,
            "/api/v1/messages/{message_id}/pin",
            &[&absent],
            none.clone(),
            &unknown,
        ),
        operation(
            Method::DELETE,
            "/api/v1/messages/{message_id}/pin",
            &[&absent],
            none.clone(),
            &unknown,
        ),
        operation(Method::GET, "/api/v1/search", &[], none.clone(), &own),
        operation(
            Method::GET,
            "/api/v1/threads/unread",
            &[],
            none.clone(),
            &own,
        ),
        operation(
            Method::GET,
            "/api/v1/notifications",
            &[],
            none.clone(),
            &own,
        ),
        operation(
            Method::PUT,
            "/api/v1/notifications/{notification_id}/read",
            &[&absent],
            none.clone(),
            &unknown,
        ),
        operation(
            Method::GET,
            "/api/v1/notification-settings/{scope_id}",
            &[&uuid::Uuid::nil()],
            none.clone(),
            &own,
        ),
        operation(
            Method::PUT,
            "/api/v1/notification-settings/{scope_id}",
            &[&uuid::Uuid::nil()],
            json!({}),
            &own,
        ),
        operation(
            Method::GET,
            "/api/v1/workspaces/{workspace_id}/notification-default",
            &[&closed_id],
            none.clone(),
            &[(Plain, NotFound)],
        ),
        operation(
            Method::PUT,
            "/api/v1/workspaces/{workspace_id}/notification-default",
            &[&ws],
            json!({"level":"all"}),
            &forbidden,
        ),
        operation(
            Method::DELETE,
            "/api/v1/workspaces/{workspace_id}/notification-default",
            &[&ws],
            none.clone(),
            &forbidden,
        ),
        operation(
            Method::PUT,
            "/api/v1/notifications/read",
            &[],
            json!({}),
            &own,
        ),
        operation(
            Method::PUT,
            "/api/v1/messages/{message_id}/thread/follow",
            &[&absent],
            json!({"following":true}),
            &unknown,
        ),
        operation(
            Method::DELETE,
            "/api/v1/messages/{message_id}/thread/follow",
            &[&absent],
            none.clone(),
            &unknown,
        ),
        operation(
            Method::GET,
            "/api/v1/channels/{channel_id}/threads",
            &[&absent],
            none.clone(),
            &unknown,
        ),
        operation(
            Method::GET,
            "/api/v1/channels/{channel_id}/permissions",
            &[&absent],
            none.clone(),
            &unknown,
        ),
        operation(
            Method::GET,
            "/api/v1/workspaces/{workspace_id}/file-retention",
            &[&ws],
            none.clone(),
            &forbidden,
        ),
        operation(
            Method::PUT,
            "/api/v1/workspaces/{workspace_id}/file-retention",
            &[&ws],
            json!({"days":30}),
            &forbidden,
        ),
        operation(
            Method::GET,
            "/api/v1/direct-message-retention",
            &[],
            none.clone(),
            &forbidden,
        ),
        operation(
            Method::PUT,
            "/api/v1/direct-message-retention",
            &[],
            json!({"days":30}),
            &forbidden,
        ),
    ]);
    operations.extend([
        operation(
            Method::PUT,
            "/api/v1/channels/{channel_id}/archive",
            &[&shelf.id],
            none.clone(),
            &forbidden,
        ),
        operation(
            Method::DELETE,
            "/api/v1/channels/{channel_id}/archive",
            &[&shelved.id],
            none.clone(),
            &forbidden,
        ),
        operation(
            Method::GET,
            "/api/v1/workspaces/{workspace_id}/archived-channels",
            &[&closed_id],
            none.clone(),
            &[(Plain, NotFound)],
        ),
        operation(
            Method::GET,
            "/api/v1/workspaces/{workspace_id}/archive-period",
            &[&ws],
            none.clone(),
            &forbidden,
        ),
        operation(
            Method::PUT,
            "/api/v1/workspaces/{workspace_id}/archive-period",
            &[&ws],
            json!({ "days": 60 }),
            &forbidden,
        ),
        operation(
            Method::POST,
            "/api/v1/channels/{channel_id}/exports",
            &[&shelf.id],
            none.clone(),
            &forbidden,
        ),
        operation(
            Method::POST,
            "/api/v1/conversations/{conversation_id}/exports",
            &[&absent],
            none.clone(),
            &unknown,
        ),
        operation(Method::GET, "/api/v1/exports", &[], none.clone(), &own),
        operation(
            Method::GET,
            "/api/v1/exports/{export_id}",
            &[&absent],
            none.clone(),
            &unknown,
        ),
        operation(
            Method::DELETE,
            "/api/v1/exports/{export_id}",
            &[&absent],
            none.clone(),
            &unknown,
        ),
        operation(
            Method::GET,
            "/api/v1/exports/{export_id}/file",
            &[&absent],
            none.clone(),
            &unknown,
        ),
        operation(
            Method::DELETE,
            "/api/v1/workspaces/{workspace_id}/roles/{role_id}",
            &[&ws, &spare_role.id],
            none.clone(),
            &forbidden,
        ),
        operation(
            Method::DELETE,
            "/api/v1/server-roles/{role_id}",
            &[&spare_server_role.id],
            none.clone(),
            &forbidden,
        ),
        operation(
            Method::DELETE,
            "/api/v1/workspaces/{workspace_id}/categories/{category_id}",
            &[&ws, &spare_category.id],
            none.clone(),
            &forbidden,
        ),
        operation(
            Method::DELETE,
            "/api/v1/workspaces/{workspace_id}",
            &[&doomed.id],
            none.clone(),
            &forbidden,
        ),
    ]);
    for operation in &mut operations {
        if operation.template == "/api/v1/search" {
            operation.path.push_str("?q=sample");
        }
    }
    World {
        _exports: exports,
        server,
        plain,
        administrator,
        operations,
    }
}

/// One text that changes when any row of any table changes.
async fn database_state(server: &TestServer) -> String {
    let tables: Vec<String> = sqlx::query_scalar(
        "select table_name::text from information_schema.tables
         where table_schema = 'public' and table_name <> '_sqlx_migrations'
         order by table_name",
    )
    .fetch_all(&server.db)
    .await
    .unwrap();
    assert!(tables.len() >= 20, "{tables:?}");
    let mut state = String::new();
    for table in tables {
        let digest: String = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "select coalesce(md5(string_agg(row::text, '|' order by row::text)), 'empty')
             from {table} as row"
        )))
        .fetch_one(&server.db)
        .await
        .unwrap();
        state.push_str(&format!("{table}={digest};"));
    }
    state
}

async fn send(
    server: &TestServer,
    operation: &Operation,
    token: Option<&str>,
) -> crate::support::Reply {
    if operation.method == Method::PUT && operation.template == "/api/v1/server/logo" {
        let mut request = reqwest::Client::new()
            .put(format!("{}{}", server.base, operation.path))
            .body(junto_server::image::samples::png(8, 8));
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        return TestServer::reply(request).await;
    }
    let body = (!operation.body.is_null()).then_some(&operation.body);
    server
        .send(operation.method.clone(), &operation.path, token, body, None)
        .await
}

#[tokio::test]
async fn the_table_lists_every_operation_that_needs_a_session() {
    let world = world().await;
    let listed: BTreeSet<String> = world
        .operations
        .iter()
        .map(|operation| format!("{} {}", operation.method, operation.template))
        .collect();

    let document: Value = serde_json::from_str(&junto_server::openapi_document().unwrap()).unwrap();
    let mut documented = BTreeSet::new();
    for (path, operations) in document["paths"].as_object().unwrap() {
        for (method, operation) in operations.as_object().unwrap() {
            if operation.get("security").is_some() {
                documented.insert(format!("{} {path}", method.to_uppercase()));
            }
        }
    }
    assert_eq!(listed, documented);
}

#[tokio::test]
async fn a_caller_without_the_permission_is_denied_and_nothing_changes() {
    let world = world().await;
    let mut checked = 0;
    for operation in &world.operations {
        for (caller, code) in &operation.denials {
            let member = match caller {
                Caller::Plain => &world.plain,
                Caller::Administrator => &world.administrator,
            };
            let before = database_state(&world.server).await;
            let reply = send(&world.server, operation, Some(&member.token)).await;
            assert_eq!(
                reply.code(),
                *code,
                "{} {} as {caller:?}: {}",
                operation.method,
                operation.template,
                reply.text()
            );
            assert_eq!(reply.status.as_u16(), code.http_status());
            assert_eq!(
                database_state(&world.server).await,
                before,
                "{} {} as {caller:?} changed the database",
                operation.method,
                operation.template
            );
            checked += 1;
        }
    }
    assert!(checked >= 40, "{checked}");
}

#[tokio::test]
async fn a_request_without_a_session_is_refused_and_nothing_changes() {
    let world = world().await;
    let before = database_state(&world.server).await;
    for operation in &world.operations {
        for token in [
            None,
            Some("jst_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"),
        ] {
            let reply = send(&world.server, operation, token).await;
            assert_eq!(
                reply.code(),
                ErrorCode::Unauthenticated,
                "{} {}",
                operation.method,
                operation.template
            );
        }
    }
    assert_eq!(database_state(&world.server).await, before);
}

#[tokio::test]
async fn the_denied_operations_succeed_for_a_caller_who_may_do_them() {
    let world = world().await;
    let server = &world.server;
    for operation in &world.operations {
        let denied_to_administrator = operation
            .denials
            .iter()
            .any(|(caller, _)| *caller == Caller::Administrator);
        if operation.denials.is_empty() || denied_to_administrator {
            continue;
        }
        let reply = send(server, operation, Some(&world.administrator.token)).await;
        assert!(
            reply.status.is_success(),
            "{} {} as the administrator: {} {}",
            operation.method,
            operation.template,
            reply.status,
            reply.text()
        );
    }
}

/// The routes that an access token reaches, as ADR 0210 lists them, each with its action.
/// The table of the server must be this list.
fn token_reach() -> BTreeMap<(&'static str, &'static str), TokenAction> {
    use TokenAction::{ManageMembers, ManageStructure, ReadAuditLog, ReadStructure};
    [
        ("GET", "/api/v1/structure", ReadStructure),
        ("GET", "/api/v1/workspaces", ReadStructure),
        (
            "GET",
            "/api/v1/workspaces/{workspace_id}/roles",
            ReadStructure,
        ),
        (
            "GET",
            "/api/v1/workspaces/{workspace_id}/channels",
            ReadStructure,
        ),
        (
            "GET",
            "/api/v1/workspaces/{workspace_id}/members",
            ReadStructure,
        ),
        ("GET", "/api/v1/channels/{channel_id}/rules", ReadStructure),
        (
            "GET",
            "/api/v1/workspaces/{workspace_id}/categories/{category_id}/rules",
            ReadStructure,
        ),
        (
            "POST",
            "/api/v1/channels/{channel_id}/rules/sync",
            ManageStructure,
        ),
        (
            "PUT",
            "/api/v1/workspaces/{workspace_id}/categories/{category_id}/rules/roles/{role_id}",
            ManageStructure,
        ),
        (
            "PATCH",
            "/api/v1/workspaces/{workspace_id}/categories/{category_id}/rules/roles/{role_id}",
            ManageStructure,
        ),
        ("GET", "/api/v1/server-roles", ReadStructure),
        ("POST", "/api/v1/structure/changes", ManageStructure),
        ("POST", "/api/v1/workspaces", ManageStructure),
        (
            "PATCH",
            "/api/v1/workspaces/{workspace_id}",
            ManageStructure,
        ),
        (
            "POST",
            "/api/v1/workspaces/{workspace_id}/categories",
            ManageStructure,
        ),
        (
            "PATCH",
            "/api/v1/workspaces/{workspace_id}/categories/{category_id}",
            ManageStructure,
        ),
        (
            "POST",
            "/api/v1/workspaces/{workspace_id}/channels",
            ManageStructure,
        ),
        ("PATCH", "/api/v1/channels/{channel_id}", ManageStructure),
        (
            "PATCH",
            "/api/v1/channels/{channel_id}/rules/roles/{role_id}",
            ManageStructure,
        ),
        ("DELETE", "/api/v1/channels/{channel_id}", ManageStructure),
        (
            "DELETE",
            "/api/v1/workspaces/{workspace_id}",
            ManageStructure,
        ),
        (
            "DELETE",
            "/api/v1/workspaces/{workspace_id}/categories/{category_id}",
            ManageStructure,
        ),
        (
            "DELETE",
            "/api/v1/workspaces/{workspace_id}/roles/{role_id}",
            ManageStructure,
        ),
        ("DELETE", "/api/v1/server-roles/{role_id}", ManageStructure),
        (
            "PATCH",
            "/api/v1/channels/{channel_id}/call/settings",
            ManageStructure,
        ),
        (
            "PUT",
            "/api/v1/channels/{channel_id}/rules/roles/{role_id}",
            ManageStructure,
        ),
        (
            "POST",
            "/api/v1/workspaces/{workspace_id}/roles",
            ManageStructure,
        ),
        (
            "PATCH",
            "/api/v1/workspaces/{workspace_id}/roles/{role_id}",
            ManageStructure,
        ),
        ("PATCH", "/api/v1/server-roles/positions", ManageStructure),
        (
            "PATCH",
            "/api/v1/workspaces/{workspace_id}/roles/positions",
            ManageStructure,
        ),
        (
            "PATCH",
            "/api/v1/workspaces/{workspace_id}/channels/positions",
            ManageStructure,
        ),
        ("POST", "/api/v1/server-roles", ManageStructure),
        ("PATCH", "/api/v1/server-roles/{role_id}", ManageStructure),
        (
            "PATCH",
            "/api/v1/server-roles/{role_id}/permissions",
            ManageStructure,
        ),
        (
            "PATCH",
            "/api/v1/workspaces/{workspace_id}/roles/{role_id}/permissions",
            ManageStructure,
        ),
        ("GET", "/api/v1/accounts", ManageMembers),
        (
            "POST",
            "/api/v1/accounts/{account_id}/removal",
            ManageMembers,
        ),
        ("PUT", "/api/v1/accounts/{account_id}/ban", ManageMembers),
        (
            "PUT",
            "/api/v1/workspaces/{workspace_id}/members/{account_id}",
            ManageMembers,
        ),
        ("GET", "/api/v1/invites", ManageMembers),
        ("POST", "/api/v1/invites", ManageMembers),
        ("DELETE", "/api/v1/invites/{invite_id}", ManageMembers),
        ("GET", "/api/v1/join-requests", ManageMembers),
        (
            "POST",
            "/api/v1/join-requests/{join_request_id}/approval",
            ManageMembers,
        ),
        (
            "POST",
            "/api/v1/join-requests/{join_request_id}/denial",
            ManageMembers,
        ),
        ("GET", "/api/v1/audit-log", ReadAuditLog),
        (
            "GET",
            "/api/v1/workspaces/{workspace_id}/audit-log",
            ReadAuditLog,
        ),
    ]
    .into_iter()
    .map(|(method, path, action)| ((method, path), action))
    .collect()
}

#[tokio::test]
async fn the_reach_of_access_tokens_is_the_stated_list_of_documented_operations() {
    let stated = token_reach();
    let served: BTreeMap<(&str, &str), TokenAction> = junto_server::tokens::REACH
        .iter()
        .map(|(method, path, action)| ((*method, *path), *action))
        .collect();
    assert_eq!(served.len(), junto_server::tokens::REACH.len());
    assert_eq!(served, stated);

    let document: Value = serde_json::from_str(&junto_server::openapi_document().unwrap()).unwrap();
    for (method, path) in stated.keys() {
        let operation = &document["paths"][*path][method.to_lowercase()];
        assert!(operation.get("security").is_some(), "{method} {path}");
    }
    // No route about credentials, the caller's own account and devices, messages, calls,
    // recordings or the event stream.
    let closed = [
        "/api/v1/access-tokens",
        "/api/v1/auth",
        "/api/v1/account/",
        "/api/v1/owner",
        "/api/v1/device-links",
        "/api/v1/messages",
        "/api/v1/conversations",
        "/api/v1/call",
        "/api/v1/direct",
        "/api/v1/recordings",
        "/api/v1/events",
        "/api/v1/snapshot",
        "/api/v1/notification",
        "/api/v1/attachment",
        "/api/v1/search",
        "/api/v1/threads",
    ];
    for (method, path) in stated.keys() {
        let in_a_call = path.contains("/call/") && !path.ends_with("/call/settings");
        let about_content = path.contains("/messages") || path.contains("/recording");
        // A request with a token changes no role assignment and lifts no ban (ADR 0213).
        let assigns_a_role = path.ends_with("/server-roles/{role_id}")
            && path.starts_with("/api/v1/accounts/")
            || path.ends_with("/members/{account_id}/roles/{role_id}")
            || path.ends_with("/members/{account_id}/removal");
        let lifts_a_ban = *method == "DELETE" && path.ends_with("/ban");
        assert!(
            !closed.iter().any(|prefix| path.starts_with(prefix))
                && *path != "/api/v1/account"
                && !path.ends_with("/recovery")
                && !in_a_call
                && !about_content
                && !assigns_a_role
                && !lifts_a_ban,
            "{method} {path} is in the reach of access tokens"
        );
    }
}

#[tokio::test]
async fn an_access_token_is_refused_on_every_route_outside_its_reach() {
    let world = world().await;
    let server = &world.server;
    let reach = token_reach();
    let complete = server
        .access_token(&world.administrator, TokenAction::ALL)
        .await;
    let mut lacking: BTreeMap<TokenAction, AccessTokenCreated> = BTreeMap::new();
    for action in TokenAction::ALL {
        let others: Vec<TokenAction> = TokenAction::ALL
            .iter()
            .copied()
            .filter(|other| other != action)
            .collect();
        lacking.insert(
            *action,
            server.access_token(&world.administrator, &others).await,
        );
    }
    let cannot_read_positions = server
        .access_token(
            &world.administrator,
            &[TokenAction::ManageStructure, TokenAction::ReadAuditLog],
        )
        .await;
    let unknown = "jat_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

    let before = database_state(server).await;
    let (mut closed, mut open) = (0, BTreeSet::new());
    for operation in &world.operations {
        let named = format!("{} {}", operation.method, operation.template);
        match reach.get(&(operation.method.as_str(), operation.template)) {
            None => {
                for token in [complete.token.expose(), unknown] {
                    let reply = send(server, operation, Some(token)).await;
                    assert_eq!(
                        reply.code(),
                        ErrorCode::Forbidden,
                        "{named}: {}",
                        reply.text()
                    );
                    assert!(
                        reply.text().contains("needs a session"),
                        "{named}: {}",
                        reply.text()
                    );
                }
                closed += 1;
            }
            Some(action) => {
                let token = if operation.method == Method::GET
                    && operation.template == "/api/v1/server-roles"
                {
                    cannot_read_positions.token.expose()
                } else {
                    lacking[action].token.expose()
                };
                let reply = send(server, operation, Some(token)).await;
                assert_eq!(
                    reply.code(),
                    ErrorCode::Forbidden,
                    "{named}: {}",
                    reply.text()
                );
                assert!(
                    reply
                        .text()
                        .contains(&format!("does not allow the action {}", action.name())),
                    "{named}: {}",
                    reply.text()
                );
                let reply = send(server, operation, Some(unknown)).await;
                assert_eq!(reply.code(), ErrorCode::Unauthenticated, "{named}");
                open.insert(named);
            }
        }
    }
    assert_eq!(database_state(server).await, before);
    assert_eq!(open.len(), reach.len());
    assert!(closed >= 110, "{closed}");
}
