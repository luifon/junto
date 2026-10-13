//! Proofs of the audit of the whole server: permissions across two rules. Each test fails
//! while its defect exists.

use junto_protocol::access_token::TokenAction;
use junto_protocol::account::AccountSummary;
use junto_protocol::auth::SessionCreated;
use junto_protocol::invite::InviteCreated;
use junto_protocol::structure::Workspace;
use junto_protocol::{ServerPermission, WorkspacePermission};
use reqwest::Method;

use crate::discovery_support::*;
use crate::stream_support::Stream;
use crate::support::DeviceKey;

/// ADR 0210 states that a token that leaks gives no access to message content, whatever its
/// creator may read. A token with the one action `manage_members` creates an invite, and
/// gives the account that joins with it a server role of its creator. That account then
/// reads a private channel.
#[tokio::test]
async fn audit_a_token_with_manage_members_does_not_lead_to_message_content() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let administrator = server.administrator(&owner, "Administrator").await;
    let workspace = server.default_workspace(&owner).await;
    let private = channel(&server, &owner, "text").await;
    let insiders = server
        .grant_workspace(&owner, &owner, workspace.id, &[])
        .await;
    server
        .make_private(&owner, workspace.id, private.id, insiders)
        .await;
    send(&server, &owner, &private, "the merger closes in March").await;
    let token = server
        .access_token(&administrator, &[TokenAction::ManageMembers])
        .await;

    let invite: InviteCreated = server
        .with_token(Method::POST, "/api/v1/invites", &token, Some(&json!({})))
        .await
        .ok(StatusCode::CREATED);
    let key = DeviceKey::generate();
    let session: SessionCreated = server
        .join_reply(&key, invite.code.expose(), "Holder")
        .await
        .ok(StatusCode::CREATED);
    let holder = crate::support::member(key, session);
    let history = format!("/api/v1/channels/{}/messages", private.id);
    server
        .get(&holder, &history)
        .await
        .error(ErrorCode::NotFound);

    let accounts: Vec<AccountSummary> = server
        .with_token(Method::GET, "/api/v1/accounts", &token, None)
        .await
        .ok(StatusCode::OK);
    let role_id = accounts
        .iter()
        .find(|account| account.id == administrator.account_id)
        .unwrap()
        .server_role_ids[0];
    let assigned = server
        .with_token(
            Method::PUT,
            &format!(
                "/api/v1/accounts/{}/server-roles/{role_id}",
                holder.account_id
            ),
            &token,
            None,
        )
        .await;
    let read = server.get(&holder, &history).await;
    assert_eq!(
        read.status,
        StatusCode::NOT_FOUND,
        "the assignment with the token answered {} and the holder of the token now reads \
         the private channel: {}",
        assigned.status,
        read.text()
    );
}

/// ADR 0304 states that a person with "manage roles" cannot use a role to enter a channel.
/// A role whose override denies "view channel" hides a channel from its holders. A holder
/// who also has "manage roles" takes that role from the own account and reads the channel.
#[tokio::test]
async fn audit_a_member_with_manage_roles_does_not_enter_a_channel_by_dropping_a_role() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let manager = server.join(&owner, "Manager").await;
    let workspace = server.default_workspace(&owner).await;
    let closed = channel(&server, &owner, "text").await;
    send(&server, &owner, &closed, "the merger closes in March").await;

    let restricted = server
        .grant_workspace(&owner, &manager, workspace.id, &[])
        .await;
    server
        .put(
            &owner,
            &format!("/api/v1/channels/{}/overrides/{restricted}", closed.id),
            &json!({"allow":[],"deny":["view_channel"]}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    server
        .grant_workspace(
            &owner,
            &manager,
            workspace.id,
            &[WorkspacePermission::ManageRoles],
        )
        .await;
    let history = format!("/api/v1/channels/{}/messages", closed.id);
    server
        .get(&manager, &history)
        .await
        .error(ErrorCode::NotFound);

    let dropped = server
        .delete(
            &manager,
            &format!(
                "/api/v1/workspaces/{}/members/{}/roles/{restricted}",
                workspace.id, manager.account_id
            ),
        )
        .await;
    let read = server.get(&manager, &history).await;
    assert_eq!(
        read.status,
        StatusCode::NOT_FOUND,
        "the removal of the role answered {} and the member now reads the channel: {}",
        dropped.status,
        read.text()
    );
}

async fn set_override(
    server: &TestServer,
    owner: &Member,
    channel: &Channel,
    role: WorkspaceRoleId,
    rule: Value,
) {
    server
        .put(
            owner,
            &format!("/api/v1/channels/{}/overrides/{role}", channel.id),
            &rule,
        )
        .await
        .ok::<Value>(StatusCode::OK);
}

async fn roles_held(server: &TestServer, member: &Member) -> i64 {
    sqlx::query_scalar("select count(*) from member_roles where account_id = $1")
        .bind(member.account_id)
        .fetch_one(&server.db)
        .await
        .unwrap()
}

/// The same rule with another member as the target: a holder of "manage roles" does not
/// give a member back what a role denies, unless the holder has that permission in the
/// channel of the override. A caller who cannot see the channel holds nothing in it. An
/// administrator takes every role.
#[tokio::test]
async fn a_role_is_taken_from_a_member_only_by_a_caller_who_holds_what_its_overrides_deny() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let manager = server.join(&owner, "Manager").await;
    let target = server.join(&owner, "Target").await;
    let workspace = server.default_workspace(&owner).await;
    let hidden = channel(&server, &owner, "text").await;
    let open = channel(&server, &owner, "text").await;
    let managing = [WorkspacePermission::ManageRoles];
    server
        .grant_workspace(&owner, &manager, workspace.id, &managing)
        .await;
    let insiders = server
        .grant_workspace(&owner, &owner, workspace.id, &[])
        .await;
    server
        .make_private(&owner, workspace.id, hidden.id, insiders)
        .await;
    let role_of_target = || server.grant_workspace(&owner, &target, workspace.id, &[]);
    let path = |role: WorkspaceRoleId| {
        format!(
            "/api/v1/workspaces/{}/members/{}/roles/{role}",
            workspace.id, target.account_id
        )
    };

    // A deny in a channel that the caller cannot see.
    let unseen = role_of_target().await;
    let rule = json!({"allow":[],"deny":["lock_room"]});
    set_override(&server, &owner, &hidden, unseen, rule).await;
    // A deny of a permission that the caller does not hold in a channel that it sees.
    let muted = role_of_target().await;
    let rule = json!({"allow":[],"deny":["manage_messages"]});
    set_override(&server, &owner, &open, muted, rule).await;
    // A role whose override only allows.
    let allowed = role_of_target().await;
    let rule = json!({"allow":["manage_messages"],"deny":[]});
    set_override(&server, &owner, &open, allowed, rule).await;

    for role in [unseen, muted] {
        server
            .delete(&manager, &path(role))
            .await
            .error(ErrorCode::ExceedsOwnPermissions);
    }
    assert_eq!(roles_held(&server, &target).await, 3);
    server
        .delete(&manager, &path(allowed))
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(roles_held(&server, &target).await, 2);

    // The caller who holds the denied permission in the channel takes the role.
    let messages = [WorkspacePermission::ManageMessages];
    server
        .grant_workspace(&owner, &manager, workspace.id, &messages)
        .await;
    server
        .delete(&manager, &path(muted))
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .delete(&manager, &path(unseen))
        .await
        .error(ErrorCode::ExceedsOwnPermissions);
    server
        .delete(&owner, &path(unseen))
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(roles_held(&server, &target).await, 0);
}

/// ADR 0213: a request with an access token gives no workspace role. A role whose override
/// allows "view channel" in a private channel would let the account that the token let in
/// read that channel.
#[tokio::test]
async fn a_token_with_manage_members_gives_no_workspace_role() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let administrator = server.administrator(&owner, "Administrator").await;
    let holder = server.join(&owner, "Holder").await;
    let workspace = server.default_workspace(&owner).await;
    let private = channel(&server, &owner, "text").await;
    let insiders = server
        .grant_workspace(&owner, &owner, workspace.id, &[])
        .await;
    server
        .make_private(&owner, workspace.id, private.id, insiders)
        .await;
    send(&server, &owner, &private, "the merger closes in March").await;
    let token = server.access_token(&administrator, TokenAction::ALL).await;

    let assignment = format!(
        "/api/v1/workspaces/{}/members/{}/roles/{insiders}",
        workspace.id, holder.account_id
    );
    let refused = server
        .with_token(Method::PUT, &assignment, &token, None)
        .await;
    refused.error(ErrorCode::Forbidden);
    assert!(refused.text().contains("needs a session"));
    server
        .get(
            &holder,
            &format!("/api/v1/channels/{}/messages", private.id),
        )
        .await
        .error(ErrorCode::NotFound);
    // The same request with the session of the creator is allowed.
    server
        .put_empty(&administrator, &assignment)
        .await
        .empty(StatusCode::NO_CONTENT);
}

/// ADR 0213: a request with an access token takes no role from an account. A workspace
/// role whose override denies "view channel" hides a channel from its holders, and a
/// server role can be the one that keeps a permission from nobody: both are changes of
/// what an account reads or does, which need a session.
#[tokio::test]
async fn a_token_with_manage_members_takes_no_role_from_an_account() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let administrator = server.administrator(&owner, "Administrator").await;
    let holder = server.join(&owner, "Holder").await;
    let workspace = server.default_workspace(&owner).await;
    let closed = channel(&server, &owner, "text").await;
    send(&server, &owner, &closed, "the merger closes in March").await;
    let restricted = server
        .grant_workspace(&owner, &holder, workspace.id, &[])
        .await;
    server
        .put(
            &owner,
            &format!("/api/v1/channels/{}/overrides/{restricted}", closed.id),
            &json!({"allow":[],"deny":["view_channel"]}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    let server_role = server
        .grant_server(&owner, &holder, &[ServerPermission::CreateInvites])
        .await;
    let token = server.access_token(&administrator, TokenAction::ALL).await;

    let paths = [
        format!(
            "/api/v1/workspaces/{}/members/{}/roles/{restricted}",
            workspace.id, holder.account_id
        ),
        format!(
            "/api/v1/accounts/{}/server-roles/{server_role}",
            holder.account_id
        ),
    ];
    for path in &paths {
        let refused = server.with_token(Method::DELETE, path, &token, None).await;
        refused.error(ErrorCode::Forbidden);
        assert!(refused.text().contains("needs a session"), "{path}");
    }
    server
        .get(&holder, &format!("/api/v1/channels/{}/messages", closed.id))
        .await
        .error(ErrorCode::NotFound);
    let held: (i64, i64) = sqlx::query_as(
        "select (select count(*) from member_roles where account_id = $1),
                (select count(*) from account_server_roles where account_id = $1)",
    )
    .bind(holder.account_id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(held, (1, 1));
    for path in &paths {
        server
            .delete(&administrator, path)
            .await
            .empty(StatusCode::NO_CONTENT);
    }
}

/// ADR 0213: a request with an access token bans an account and does not lift a ban. An
/// unban gives an account back its memberships and its roles (ADR 0113).
#[tokio::test]
async fn a_token_with_manage_members_lifts_no_ban() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let administrator = server.administrator(&owner, "Administrator").await;
    let banned = server.join(&owner, "Banned").await;
    let token = server.access_token(&administrator, TokenAction::ALL).await;
    let ban = format!("/api/v1/accounts/{}/ban", banned.account_id);
    server
        .with_token(Method::PUT, &ban, &token, None)
        .await
        .empty(StatusCode::NO_CONTENT);

    let refused = server.with_token(Method::DELETE, &ban, &token, None).await;
    refused.error(ErrorCode::Forbidden);
    assert!(refused.text().contains("needs a session"));
    let status: String = sqlx::query_scalar("select status from accounts where id = $1")
        .bind(banned.account_id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(status, "banned");
    server
        .challenge_reply(
            &banned.key.public(),
            junto_protocol::auth::ChallengeOperation::Login,
        )
        .await
        .error(ErrorCode::AccountBanned);
    server
        .delete(&administrator, &ban)
        .await
        .empty(StatusCode::NO_CONTENT);
}

async fn conversation(server: &TestServer, opener: &Member, other: &Member) -> String {
    let opened: Conversation = server
        .post(
            opener,
            "/api/v1/conversations",
            &json!({ "account_id": other.account_id }),
        )
        .await
        .ok(StatusCode::OK);
    format!("/api/v1/conversations/{}/messages", opened.id)
}

/// The notices `permissions_changed` for the server that `stream` received since the last
/// call.
async fn notices_of(server: &TestServer, member: &Member, stream: &mut Stream) -> usize {
    server
        .traffic(member, stream)
        .await
        .into_iter()
        .filter(|(_, event)| *event == Event::PermissionsChanged { workspace_id: None })
        .count()
}

/// ADR 0208: a change that can alter what an account may see or do records the notice
/// `permissions_changed`. A ban and an unban of one account close and open its direct
/// conversations for the other accounts (ADR 0307). Each of them is told, at every level,
/// and an account without such a conversation is not.
#[tokio::test]
async fn a_ban_and_an_unban_tell_the_other_account_of_a_direct_conversation() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let ann = server.join(&owner, "Ann").await;
    let ben = server.join(&owner, "Ben").await;
    let dee = server.join(&owner, "Dee").await;
    let history = conversation(&server, &ann, &ben).await;
    let mut of_ben = server.subscribed(&ben, Level::Background).await;
    let mut of_dee = server.subscribed(&dee, Level::Full).await;
    let ban = format!("/api/v1/accounts/{}/ban", ann.account_id);

    server
        .put_empty(&owner, &ban)
        .await
        .empty(StatusCode::NO_CONTENT);
    server.get(&ben, &history).await.error(ErrorCode::NotFound);
    assert_eq!(notices_of(&server, &ben, &mut of_ben).await, 1);

    server
        .delete(&owner, &ban)
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .get(&ben, &history)
        .await
        .ok::<MessagePage>(StatusCode::OK);
    assert_eq!(notices_of(&server, &ben, &mut of_ben).await, 1);
    assert_eq!(notices_of(&server, &dee, &mut of_dee).await, 0);
}

/// The same for two accounts without a common workspace, which reach each other while one
/// of them administers the server (ADR 0307): the other account is told when that
/// permission is taken, when it is given again, and when the account is removed. A removal
/// tells the members of the workspaces of the account, and this account is in none of them.
#[tokio::test]
async fn a_change_of_the_administrator_permission_and_a_removal_tell_the_other_account() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let board: Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({"name":"board"}))
        .await
        .ok(StatusCode::CREATED);
    let ada = server.join(&owner, "Ada").await;
    let administers = server
        .grant_server(&owner, &ada, &[ServerPermission::Administrator])
        .await;
    let cy = server.join_into(&owner, "Cy", &[board.id]).await;
    let history = conversation(&server, &ada, &cy).await;
    let mut of_cy = server.subscribed(&cy, Level::Background).await;
    let role = format!(
        "/api/v1/accounts/{}/server-roles/{administers}",
        ada.account_id
    );

    server
        .delete(&owner, &role)
        .await
        .empty(StatusCode::NO_CONTENT);
    server.get(&cy, &history).await.error(ErrorCode::NotFound);
    assert_eq!(notices_of(&server, &cy, &mut of_cy).await, 1);

    server
        .put_empty(&owner, &role)
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .get(&cy, &history)
        .await
        .ok::<MessagePage>(StatusCode::OK);
    assert_eq!(notices_of(&server, &cy, &mut of_cy).await, 1);

    // A change of the permissions of the role that the account holds.
    let server_role = format!("/api/v1/server-roles/{administers}");
    for (permissions, reached) in [(json!([]), false), (json!(["administrator"]), true)] {
        server
            .patch(&owner, &server_role, &json!({ "permissions": permissions }))
            .await
            .ok::<Value>(StatusCode::OK);
        assert_eq!(server.get(&cy, &history).await.status.is_success(), reached);
        assert_eq!(notices_of(&server, &cy, &mut of_cy).await, 1);
    }

    server
        .post(
            &owner,
            &format!("/api/v1/accounts/{}/removal", ada.account_id),
            &json!({}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    server.get(&cy, &history).await.error(ErrorCode::NotFound);
    assert_eq!(notices_of(&server, &cy, &mut of_cy).await, 1);
}

/// The deletion of a server role with the administrator permission tells the accounts that
/// have a direct conversation with a holder of the role.
#[tokio::test]
async fn the_deletion_of_an_administrator_role_tells_the_other_account() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let board: Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({"name":"board"}))
        .await
        .ok(StatusCode::CREATED);
    let ada = server.join(&owner, "Ada").await;
    let administers = server
        .grant_server(&owner, &ada, &[ServerPermission::Administrator])
        .await;
    let cy = server.join_into(&owner, "Cy", &[board.id]).await;
    let history = conversation(&server, &ada, &cy).await;
    let mut of_cy = server.subscribed(&cy, Level::Background).await;

    server
        .delete(&owner, &format!("/api/v1/server-roles/{administers}"))
        .await
        .empty(StatusCode::NO_CONTENT);
    server.get(&cy, &history).await.error(ErrorCode::NotFound);
    assert_eq!(notices_of(&server, &cy, &mut of_cy).await, 1);
}

#[tokio::test]
async fn token_workspace_role_deletion_checks_rank_and_denies_or_the_structure_administrator_gate()
{
    for structured in [false, true] {
        let server = TestServer::start().await;
        let owner = server.owner().await;
        let manager = server.join(&owner, "Manager").await;
        let administrator = server
            .grant_server(&owner, &manager, &[ServerPermission::Administrator])
            .await;
        let token = server.access_token(&manager, TokenAction::ALL).await;
        server
            .delete(
                &owner,
                &format!(
                    "/api/v1/accounts/{}/server-roles/{administrator}",
                    manager.account_id
                ),
            )
            .await
            .empty(StatusCode::NO_CONTENT);
        let workspace = server.default_workspace(&owner).await;
        let role_route = format!("/api/v1/workspaces/{}/roles", workspace.id);
        let high: junto_protocol::roles::WorkspaceRole = server
            .post(
                &owner,
                &role_route,
                &json!({"name":"Higher","permissions":[]}),
            )
            .await
            .ok(StatusCode::CREATED);
        server
            .grant_workspace(
                &owner,
                &manager,
                workspace.id,
                &[WorkspacePermission::ManageRoles],
            )
            .await;
        let low: junto_protocol::roles::WorkspaceRole = server
            .post(
                &owner,
                &role_route,
                &json!({"name":"Lower","permissions":[]}),
            )
            .await
            .ok(StatusCode::CREATED);
        let channel = server.channel(&owner, workspace.id, "text").await;
        server
            .put(
                &owner,
                &format!("/api/v1/channels/{}/overrides/{}", channel.id, low.id),
                &json!({"deny":["mute_members"]}),
            )
            .await
            .ok::<Value>(StatusCode::OK);
        for (role, expected) in [
            (high.id, Some(ErrorCode::Forbidden)),
            (low.id, Some(ErrorCode::ExceedsOwnPermissions)),
            (low.id, None),
        ] {
            if expected.is_none() {
                if structured {
                    server
                        .grant_server(&owner, &manager, &[ServerPermission::Administrator])
                        .await;
                }
                server
                    .grant_workspace(
                        &owner,
                        &manager,
                        workspace.id,
                        &[WorkspacePermission::MuteMembers],
                    )
                    .await;
            }
            let reply = if structured {
                let revision = server.structure(&owner).await.revision;
                server.with_token(Method::POST,"/api/v1/structure/changes",&token,Some(&json!({"revision":revision,"changes":[{"change":"delete_workspace_role","id":role}]}))).await
            } else {
                server
                    .with_token(
                        Method::DELETE,
                        &format!("{role_route}/{role}"),
                        &token,
                        None,
                    )
                    .await
            };
            if let Some(code) = expected {
                reply.error(if structured {
                    ErrorCode::Forbidden
                } else {
                    code
                });
            } else if structured {
                reply.ok::<Value>(StatusCode::OK);
            } else {
                reply.empty(StatusCode::NO_CONTENT);
            }
        }
    }
}

#[tokio::test]
async fn token_role_definition_deletion_never_bypasses_server_rank() {
    for structured in [false, true] {
        let server = TestServer::start().await;
        let owner = server.owner().await;
        let high: junto_protocol::roles::ServerRole = server
            .post(
                &owner,
                "/api/v1/server-roles",
                &json!({"name":"Higher","permissions":[]}),
            )
            .await
            .ok(StatusCode::CREATED);
        let admin = server.administrator(&owner, "Administrator").await;
        let token = server.access_token(&admin, TokenAction::ALL).await;
        let reply = if structured {
            let revision = server.structure(&owner).await.revision;
            server.with_token(Method::POST,"/api/v1/structure/changes",&token,Some(&json!({"revision":revision,"changes":[{"change":"delete_server_role","id":high.id}]}))).await
        } else {
            server
                .with_token(
                    Method::DELETE,
                    &format!("/api/v1/server-roles/{}", high.id),
                    &token,
                    None,
                )
                .await
        };
        reply.error(ErrorCode::Forbidden);
    }
}
