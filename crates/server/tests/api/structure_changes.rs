//! The structure of the server in one document, its revision, and changes in one request
//! (ADR 0204, ADR 0211).

use junto_protocol::access_token::TokenAction;
use junto_protocol::audit::{AuditEvent, AuditPage};
use junto_protocol::error::ErrorCode;
use junto_protocol::events::{Event, Level};
use junto_protocol::ids::StructureId;
use junto_protocol::roles::WorkspaceRole;
use junto_protocol::structure::{Channel, ChannelKind, ChannelList, Workspace};
use junto_protocol::structure_file::{
    ChannelEntry, StructureApplied, StructureExport, StructureKind, WorkspaceEntry,
};
use junto_protocol::{ServerPermission, WorkspacePermission};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use crate::discovery_support::events;
use crate::support::{Member, Reply, TestServer, drive_until};

const EXPORT: &str = "/api/v1/structure";
const CHANGES: &str = "/api/v1/structure/changes";

fn new_id() -> StructureId {
    StructureId::generate()
}

impl TestServer {
    pub async fn structure(&self, member: &Member) -> StructureExport {
        self.get(member, EXPORT).await.ok(StatusCode::OK)
    }

    pub async fn change_structure(&self, member: &Member, revision: &str, changes: Value) -> Reply {
        self.post(
            member,
            CHANGES,
            &json!({ "revision": revision, "changes": changes }),
        )
        .await
    }
}

/// Reads the revision and requires that it differs from `last`.
async fn changed(server: &TestServer, owner: &Member, last: &mut String, what: &str) {
    let now = server.structure(owner).await.revision;
    assert_ne!(now, *last, "{what} did not change the revision");
    *last = now;
}

/// One text that changes when any row of the structure, of the audit log or of the event
/// log changes.
async fn state(server: &TestServer) -> String {
    let mut state = String::new();
    for table in [
        "server_roles",
        "workspaces",
        "workspace_roles",
        "categories",
        "channels",
        "channel_overrides",
        "structure_identifiers",
        "workspace_members",
        "audit_entries",
        "events",
    ] {
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

fn default_of(export: &StructureExport) -> &WorkspaceEntry {
    export
        .structure
        .workspaces
        .iter()
        .find(|workspace| workspace.default)
        .unwrap()
}

fn channel_named<'a>(workspace: &'a WorkspaceEntry, name: &str) -> &'a ChannelEntry {
    workspace
        .channels
        .iter()
        .find(|channel| channel.name == name)
        .unwrap_or_else(|| panic!("no channel {name} in {:?}", workspace.channels))
}

#[tokio::test]
async fn rule_20_the_export_holds_the_whole_structure_for_an_administrator() {
    let server = TestServer::start_for_calls().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let everyone = server.everyone_role(&owner, workspace.id).await;
    let role = server
        .grant_workspace(
            &owner,
            &owner,
            workspace.id,
            &[WorkspacePermission::ManageRoles],
        )
        .await;
    let category = server.category(&owner, workspace.id, "Projects", 0).await;
    let hidden: Channel = server
        .post(
            &owner,
            &format!("/api/v1/workspaces/{}/channels", workspace.id),
            &json!({ "name": "hidden", "kind": "text", "category_id": category.id }),
        )
        .await
        .ok(StatusCode::CREATED);
    server
        .make_private(&owner, workspace.id, hidden.id, role)
        .await;
    let hall = server.voice(&owner, workspace.id, "hall").await;
    server
        .call_settings(
            &owner,
            hall.id,
            json!({ "user_limit": 8, "hides_occupants": true }),
        )
        .await;
    let gone = server.channel(&owner, workspace.id, "gone").await;
    server
        .delete(&owner, &format!("/api/v1/channels/{}", gone.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    let second: Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({ "name": "Second" }))
        .await
        .ok(StatusCode::CREATED);
    let server_role = server
        .grant_server(&owner, &owner, &[ServerPermission::CreateInvites])
        .await;

    let export = server.structure(&owner).await;
    assert_eq!(export.revision.len(), 32);
    assert_eq!(server.structure(&owner).await, export);
    let file = &export.structure;
    assert_eq!(file.format, 1);
    assert!(file.delete.is_empty());
    assert_eq!(file.server_roles.len(), 1);
    assert_eq!(file.server_roles[0].id.as_uuid(), server_role.as_uuid());
    assert_eq!(
        file.server_roles[0].permissions,
        Some(vec![ServerPermission::CreateInvites])
    );
    assert_eq!(file.workspaces.len(), 2);
    let main = default_of(&export);
    assert_eq!(main.id.as_uuid(), workspace.id.as_uuid());
    assert_eq!(main.roles[0].id.as_uuid(), everyone.id.as_uuid());
    assert!(main.roles[0].everyone);
    assert_eq!(main.roles[1].id.as_uuid(), role.as_uuid());
    assert!(!main.roles[1].everyone);
    assert_eq!(
        main.roles[1].permissions,
        Some(vec![WorkspacePermission::ManageRoles])
    );
    assert_eq!(main.categories.len(), 1);
    assert_eq!(main.categories[0].id.as_uuid(), category.id.as_uuid());
    assert!(main.channels.iter().all(|channel| channel.name != "gone"));

    let secret = channel_named(main, "hidden");
    assert_eq!(secret.kind, ChannelKind::Text);
    assert_eq!(secret.category, Some(Some(main.categories[0].id)));
    assert_eq!((secret.user_limit, secret.hides_occupants), (None, None));
    assert_eq!(secret.overrides.len(), 2);
    let denied = secret
        .overrides
        .iter()
        .find(|entry| entry.role == main.roles[0].id)
        .unwrap();
    assert_eq!(denied.deny, [WorkspacePermission::ViewChannel]);
    let voice = channel_named(main, "hall");
    assert_eq!(voice.kind, ChannelKind::Voice);
    assert_eq!(voice.category, Some(None));
    assert_eq!(
        (voice.user_limit, voice.hides_occupants),
        (Some(Some(8)), Some(true))
    );
    let other = file.workspaces.iter().find(|entry| !entry.default).unwrap();
    assert_eq!(
        (other.id.as_uuid(), other.name.as_str()),
        (second.id.as_uuid(), "Second")
    );
    assert_eq!(other.roles.len(), 1);

    // Every permission except the administrator permission is not enough.
    let manager = server.join(&owner, "Manager").await;
    let others: Vec<ServerPermission> = ServerPermission::ALL
        .iter()
        .copied()
        .filter(|permission| *permission != ServerPermission::Administrator)
        .collect();
    server.grant_server(&owner, &manager, &others).await;
    server
        .grant_workspace(&owner, &manager, workspace.id, WorkspacePermission::ALL)
        .await;
    server
        .get(&manager, EXPORT)
        .await
        .error(ErrorCode::Forbidden);
    let before = state(&server).await;
    server
        .change_structure(&manager, &export.revision, json!([]))
        .await
        .error(ErrorCode::Forbidden);
    server
        .change_structure(
            &manager,
            &export.revision,
            json!([{ "change": "create_channel", "id": new_id(), "workspace": main.id,
                     "kind": "text", "name": "refused" }]),
        )
        .await
        .error(ErrorCode::Forbidden);
    assert_eq!(state(&server).await, before);
}

#[tokio::test]
async fn rule_21_the_revision_counts_every_change_of_the_structure_and_nothing_else() {
    let server = TestServer::start_for_calls().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let member = server.join(&owner, "Member").await;
    let mut last = server.structure(&owner).await.revision;
    let first = last.clone();

    let category = server.category(&owner, workspace.id, "Projects", 0).await;
    changed(&server, &owner, &mut last, "a new category").await;
    server
        .patch(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/categories/{}",
                workspace.id, category.id
            ),
            &json!({ "name": "Renamed" }),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    changed(&server, &owner, &mut last, "a renamed category").await;
    let channel = server.channel(&owner, workspace.id, "plans").await;
    changed(&server, &owner, &mut last, "a new channel").await;
    server
        .patch(
            &owner,
            &format!("/api/v1/channels/{}", channel.id),
            &json!({ "position": 9 }),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    changed(&server, &owner, &mut last, "a moved channel").await;
    let hall = server.voice(&owner, workspace.id, "hall").await;
    changed(&server, &owner, &mut last, "a new voice channel").await;
    server
        .call_settings(&owner, hall.id, json!({ "user_limit": 4 }))
        .await;
    changed(&server, &owner, &mut last, "a user limit").await;
    let role: WorkspaceRole = server
        .post(
            &owner,
            &format!("/api/v1/workspaces/{}/roles", workspace.id),
            &json!({ "name": "helpers" }),
        )
        .await
        .ok(StatusCode::CREATED);
    changed(&server, &owner, &mut last, "a new workspace role").await;
    server
        .patch(
            &owner,
            &format!("/api/v1/workspaces/{}/roles/{}", workspace.id, role.id),
            &json!({ "permissions": ["manage_glossary"] }),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    changed(&server, &owner, &mut last, "changed role permissions").await;
    let overrides = format!("/api/v1/channels/{}/overrides/{}", channel.id, role.id);
    server
        .put(&owner, &overrides, &json!({ "deny": ["view_channel"] }))
        .await
        .ok::<Value>(StatusCode::OK);
    changed(&server, &owner, &mut last, "a new override").await;
    server
        .put(&owner, &overrides, &json!({ "allow": ["view_channel"] }))
        .await
        .ok::<Value>(StatusCode::OK);
    changed(&server, &owner, &mut last, "a changed override").await;
    server
        .put(&owner, &overrides, &json!({}))
        .await
        .ok::<Value>(StatusCode::OK);
    changed(&server, &owner, &mut last, "a removed override").await;
    let server_role = server.grant_server(&owner, &member, &[]).await;
    changed(&server, &owner, &mut last, "a new server role").await;
    server
        .patch(
            &owner,
            &format!("/api/v1/server-roles/{server_role}"),
            &json!({ "name": "renamed" }),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    changed(&server, &owner, &mut last, "a renamed server role").await;
    let second: Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({ "name": "Second" }))
        .await
        .ok(StatusCode::CREATED);
    changed(&server, &owner, &mut last, "a new workspace").await;
    server
        .patch(
            &owner,
            &format!("/api/v1/workspaces/{}", second.id),
            &json!({ "name": "Renamed" }),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    changed(&server, &owner, &mut last, "a renamed workspace").await;
    server
        .delete(&owner, &format!("/api/v1/channels/{}", channel.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    changed(&server, &owner, &mut last, "a deleted channel").await;

    // What is not structure leaves the revision: members, role assignments, messages, a
    // permanent message, a call, a request that changes no value, and a refused request.
    let text = server.channel(&owner, workspace.id, "talk").await;
    server
        .patch(
            &owner,
            &format!("/api/v1/channels/{}", text.id),
            &json!({ "permanent_messages": true }),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    let steady = server.structure(&owner).await.revision;
    server.join(&owner, "Another").await;
    server
        .put_empty(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}/roles/{}",
                workspace.id, member.account_id, role.id
            ),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .post(
            &owner,
            &format!("/api/v1/channels/{}/messages", text.id),
            &crate::messages_support::text("a permanent line"),
        )
        .await
        .ok::<Value>(StatusCode::CREATED);
    let counted: i64 =
        sqlx::query_scalar("select permanent_message_count from channels where id = $1")
            .bind(text.id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(counted, 1);
    server.enter_call(&owner, hall.id).await;
    server
        .patch(
            &owner,
            &format!("/api/v1/channels/{}", text.id),
            &json!({ "name": "talk" }),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    server
        .post(
            &member,
            &format!("/api/v1/workspaces/{}/channels", workspace.id),
            &json!({ "name": "refused", "kind": "text" }),
        )
        .await
        .error(ErrorCode::Forbidden);
    assert_eq!(server.structure(&owner).await.revision, steady);

    // Equal structures have equal revisions: a change that is taken back gives the revision
    // from before it.
    let rename = |name: &'static str| {
        let (server, owner) = (&server, &owner);
        let path = format!("/api/v1/channels/{}", text.id);
        async move {
            server
                .patch(owner, &path, &json!({ "name": name }))
                .await
                .ok::<Value>(StatusCode::OK);
        }
    };
    rename("other").await;
    changed(&server, &owner, &mut last, "a renamed channel").await;
    rename("talk").await;
    assert_eq!(server.structure(&owner).await.revision, steady);
    assert_ne!(steady, first);
}

fn setup_changes(workspace: StructureId, ids: &[StructureId; 6]) -> Value {
    let [role, category, text, voice, second, second_everyone] = ids;
    json!([
        { "change": "create_server_role", "id": new_id(), "name": "Inviters",
          "permissions": ["create_invites"] },
        { "change": "create_workspace_role", "id": role, "workspace": workspace,
          "name": "Helpers", "permissions": ["manage_glossary"] },
        { "change": "create_category", "id": category, "workspace": workspace,
          "name": "Projects", "position": 3 },
        { "change": "create_channel", "id": text, "workspace": workspace, "kind": "text",
          "name": "plans", "category": category },
        { "change": "update_channel", "id": text, "permanent_messages": true, "position": 7 },
        { "change": "create_channel", "id": voice, "workspace": workspace, "kind": "voice",
          "name": "hall" },
        { "change": "update_call_settings", "id": voice, "user_limit": 6,
          "hides_occupants": true },
        { "change": "set_channel_override", "channel": text, "role": role,
          "allow": ["manage_messages"], "deny": [] },
        { "change": "create_workspace", "id": second, "name": "Second",
          "everyone_role": second_everyone },
        { "change": "update_workspace_role", "id": second_everyone,
          "permissions": ["view_channel"] },
        { "change": "create_channel", "id": new_id(), "workspace": second, "kind": "text",
          "name": "lobby" },
        { "change": "rename_workspace", "id": workspace, "name": "Main" },
        { "change": "update_category", "id": category, "name": "Work" },
    ])
}

#[tokio::test]
async fn rule_22_changes_are_applied_together_with_their_audit_entries_and_events() {
    let server = TestServer::start_for_calls().await;
    let owner = server.owner().await;
    let before = server.structure(&owner).await;
    let workspace = default_of(&before).id;
    let cursor = server.snapshot(&owner).await.cursor;
    let ids = [new_id(), new_id(), new_id(), new_id(), new_id(), new_id()];
    let [role, category, text, voice, second, second_everyone] = ids;

    let applied: StructureApplied = server
        .change_structure(&owner, &before.revision, setup_changes(workspace, &ids))
        .await
        .ok(StatusCode::OK);
    let after = server.structure(&owner).await;
    assert_eq!(applied.revision, after.revision);
    assert_ne!(after.revision, before.revision);
    let kinds: Vec<_> = applied
        .created
        .iter()
        .map(|entry| (entry.kind, entry.entry))
        .collect();
    assert_eq!(
        kinds[1..],
        [
            (StructureKind::WorkspaceRole, role),
            (StructureKind::Category, category),
            (StructureKind::Channel, text),
            (StructureKind::Channel, voice),
            (StructureKind::Workspace, second),
            (StructureKind::WorkspaceRole, second_everyone),
            (StructureKind::Channel, kinds[7].1),
        ]
    );
    // The server chose the identifiers of the objects, as for every object.
    for entry in &applied.created {
        assert_ne!(entry.id, entry.entry.as_uuid());
        assert_eq!(entry.id.get_version_num(), 7);
    }

    let main = default_of(&after);
    assert_eq!(main.name, "Main");
    assert_eq!(main.categories.len(), 1);
    assert_eq!(
        (main.categories[0].id, main.categories[0].name.as_str()),
        (category, "Work")
    );
    assert_eq!(main.categories[0].position, Some(3));
    let helpers = main.roles.iter().find(|entry| entry.id == role).unwrap();
    assert_eq!(
        helpers.permissions,
        Some(vec![WorkspacePermission::ManageGlossary])
    );
    let plans = channel_named(main, "plans");
    assert_eq!(plans.id, text);
    assert_eq!(plans.category, Some(Some(category)));
    assert_eq!(
        (plans.position, plans.permanent_messages),
        (Some(7), Some(true))
    );
    assert_eq!(plans.overrides.len(), 1);
    assert_eq!(plans.overrides[0].role, role);
    assert_eq!(
        plans.overrides[0].allow,
        [WorkspacePermission::ManageMessages]
    );
    let hall = channel_named(main, "hall");
    assert_eq!(hall.id, voice);
    assert_eq!(
        (hall.user_limit, hall.hides_occupants),
        (Some(Some(6)), Some(true))
    );
    let other = after
        .structure
        .workspaces
        .iter()
        .find(|entry| entry.id == second)
        .unwrap();
    assert_eq!(other.name, "Second");
    assert_eq!(other.roles.len(), 1);
    assert_eq!(other.roles[0].id, second_everyone);
    assert!(other.roles[0].everyone);
    assert_eq!(
        other.roles[0].permissions,
        Some(vec![WorkspacePermission::ViewChannel])
    );
    assert_eq!(other.channels.len(), 1);
    assert_eq!(after.structure.server_roles.len(), 1);

    // The single routes show the same objects under the identifiers of the server.
    let created_id = |entry: StructureId| {
        applied
            .created
            .iter()
            .find(|created| created.entry == entry)
            .unwrap()
            .id
    };
    let list: ChannelList = server
        .get(&owner, &format!("/api/v1/workspaces/{}/channels", main.id))
        .await
        .ok(StatusCode::OK);
    let listed = list
        .channels
        .iter()
        .find(|channel| channel.name == "plans")
        .unwrap();
    assert_eq!(listed.id.as_uuid(), created_id(text));
    let workspaces: Vec<Workspace> = server
        .get(&owner, "/api/v1/workspaces")
        .await
        .ok(StatusCode::OK);
    assert!(
        workspaces
            .iter()
            .any(|entry| entry.id.as_uuid() == created_id(second))
    );

    let received = events(&server, &owner, cursor, Level::Full).await;
    let names: Vec<&str> = received
        .iter()
        .map(Event::name)
        .filter(|name| *name != "permissions_changed" && *name != "message_changed")
        .collect();
    assert_eq!(
        names,
        [
            "server_role_created",
            "workspace_role_created",
            "category_created",
            "channel_created",
            "channel_updated",
            "channel_created",
            "channel_updated",
            "channel_override_set",
            "workspace_created",
            "workspace_role_created",
            "workspace_member_added",
            "workspace_role_updated",
            "channel_created",
            "workspace_updated",
            "category_updated",
        ]
    );

    let server_log: AuditPage = server
        .get(&owner, "/api/v1/audit-log")
        .await
        .ok(StatusCode::OK);
    let mut actions: Vec<&str> = server_log
        .entries
        .iter()
        .map(|entry| entry.event.action())
        .filter(|action| *action != "owner_claimed")
        .collect();
    actions.reverse();
    assert_eq!(
        actions,
        [
            "server_role_created",
            "workspace_created",
            "workspace_renamed"
        ]
    );
    let log: AuditPage = server
        .get(&owner, &format!("/api/v1/workspaces/{}/audit-log", main.id))
        .await
        .ok(StatusCode::OK);
    let mut actions: Vec<&str> = log
        .entries
        .iter()
        .map(|entry| entry.event.action())
        .collect();
    actions.reverse();
    assert_eq!(
        actions,
        [
            "workspace_role_created",
            "category_created",
            "channel_created",
            "channel_updated",
            "channel_created",
            "channel_call_settings_updated",
            "channel_override_set",
            "category_updated",
        ]
    );
    assert!(
        log.entries
            .iter()
            .all(|entry| entry.actor_id == Some(owner.account_id))
    );
    assert!(matches!(
        &log.entries.last().unwrap().event,
        AuditEvent::WorkspaceRoleCreated { role_id, .. } if role_id.as_uuid() == created_id(role)
    ));
}

#[tokio::test]
async fn rule_23_a_stale_revision_changes_nothing() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let planned = server.structure(&owner).await;
    let workspace = default_of(&planned).id;
    server
        .channel(&owner, WorkspaceIdOf(workspace).into(), "after-the-plan")
        .await;
    let before = state(&server).await;
    let change = json!([{ "change": "create_channel", "id": new_id(), "workspace": workspace,
                          "kind": "text", "name": "planned" }]);
    let other_revision = "0".repeat(32);
    for revision in [planned.revision.as_str(), other_revision.as_str(), "", "1"] {
        let reply = server
            .change_structure(&owner, revision, change.clone())
            .await;
        reply.error(ErrorCode::Conflict);
        assert!(reply.text().contains("Make a new plan"), "{}", reply.text());
    }
    // Also a request without changes states whether its revision is current.
    server
        .change_structure(&owner, &planned.revision, json!([]))
        .await
        .error(ErrorCode::Conflict);
    assert_eq!(state(&server).await, before);

    let current = server.structure(&owner).await.revision;
    let unchanged: StructureApplied = server
        .change_structure(&owner, &current, json!([]))
        .await
        .ok(StatusCode::OK);
    assert_eq!(
        (&unchanged.revision, unchanged.created.len()),
        (&current, 0)
    );
    assert_eq!(state(&server).await, before);
    server
        .change_structure(&owner, &current, change)
        .await
        .ok::<StructureApplied>(StatusCode::OK);
}

struct WorkspaceIdOf(StructureId);

impl From<WorkspaceIdOf> for junto_protocol::ids::WorkspaceId {
    fn from(entry: WorkspaceIdOf) -> Self {
        Self::from_uuid(entry.0.as_uuid())
    }
}

#[tokio::test]
async fn rule_24_one_failing_change_applies_nothing_and_names_its_number() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let export = server.structure(&owner).await;
    let workspace = default_of(&export).id;
    let general = default_of(&export).channels[0].id;
    let good = json!({ "change": "create_channel", "id": new_id(), "workspace": workspace,
                       "kind": "text", "name": "first" });
    let failing = [
        (
            json!({ "change": "update_channel", "id": new_id(), "name": "absent" }),
            ErrorCode::InvalidRequest,
            "does not exist",
        ),
        (
            json!({ "change": "create_channel", "id": new_id(), "workspace": new_id(),
                    "kind": "text", "name": "nowhere" }),
            ErrorCode::InvalidRequest,
            "does not exist",
        ),
        (
            json!({ "change": "create_category", "id": new_id(), "workspace": workspace,
                    "name": "   " }),
            ErrorCode::InvalidRequest,
            "name is empty",
        ),
        (
            json!({ "change": "set_channel_override", "channel": general, "role": new_id(),
                    "allow": ["view_channel"] }),
            ErrorCode::InvalidRequest,
            "does not exist",
        ),
        (
            json!({ "change": "set_channel_override", "channel": general,
                    "role": default_of(&export).roles[0].id,
                    "allow": ["view_channel"], "deny": ["view_channel"] }),
            ErrorCode::InvalidRequest,
            "both allow and deny",
        ),
        (
            json!({ "change": "update_call_settings", "id": general, "user_limit": 3 }),
            ErrorCode::Conflict,
            "voice channels",
        ),
        (
            json!({ "change": "create_channel", "id": general, "workspace": workspace,
                    "kind": "text", "name": "taken" }),
            ErrorCode::Conflict,
            "is in use",
        ),
    ];
    let before = state(&server).await;
    for (change, code, stated) in failing {
        let reply = server
            .change_structure(&owner, &export.revision, json!([good, good_twin(), change]))
            .await;
        assert_eq!(reply.code(), code, "{change}: {}", reply.text());
        let text = reply.text();
        assert!(text.contains("change 3 ("), "{text}");
        assert!(text.contains(change["change"].as_str().unwrap()), "{text}");
        assert!(text.contains(stated), "{text}");
        assert!(text.contains("nothing was changed"), "{text}");
        assert_eq!(state(&server).await, before, "{change}");
    }
    // A body that is no list of known changes is refused whole.
    for body in [
        json!({ "revision": export.revision, "changes": [{ "change": "delete_workspace",
                "id": workspace }] }),
        json!({ "revision": export.revision }),
        json!({ "changes": [] }),
    ] {
        server
            .post(&owner, CHANGES, &body)
            .await
            .error(ErrorCode::InvalidRequest);
    }
    assert_eq!(state(&server).await, before);
}

fn good_twin() -> Value {
    json!({ "change": "create_server_role", "id": new_id(), "name": "second" })
}

#[tokio::test]
async fn rule_25_each_change_keeps_the_limits_of_its_own_route() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let administrator = server.administrator(&owner, "Administrator").await;
    let export = server.structure(&owner).await;
    let workspace = default_of(&export);
    let records = server
        .channel(&owner, WorkspaceIdOf(workspace.id).into(), "records")
        .await;
    sqlx::query("update channels set permanent_message_count = 1 where id = $1")
        .bind(records.id)
        .execute(&server.db)
        .await
        .unwrap();
    let revision = server.structure(&owner).await.revision;
    let changes = json!([
        { "change": "create_category", "id": new_id(), "workspace": workspace.id,
          "name": "kept out" },
        { "change": "delete_channel", "id": records.id },
    ]);
    let token = server
        .access_token(&owner, &[TokenAction::ManageStructure])
        .await;
    let before = state(&server).await;
    let reply = server
        .change_structure(&administrator, &revision, changes.clone())
        .await;
    reply.error(ErrorCode::OwnerOnly);
    assert!(
        reply.text().contains("change 2 (delete_channel)"),
        "{}",
        reply.text()
    );
    assert_eq!(state(&server).await, before);

    // The token of the owner is not the owner (ADR 0210); the session of the owner is.
    let body = json!({ "revision": revision, "changes": changes });
    server
        .with_token(Method::POST, CHANGES, &token, Some(&body))
        .await
        .error(ErrorCode::OwnerOnly);
    assert_eq!(state(&server).await, before);
    server
        .change_structure(&owner, &revision, changes)
        .await
        .ok::<StructureApplied>(StatusCode::OK);

    // The ceiling of roles of a workspace counts inside the request: the workspace has five
    // roles less than the ceiling, and the sixth new role of one request passes it.
    let most = i64::from(server.state.config.limits.most_roles_per_workspace);
    sqlx::query(
        "insert into workspace_roles (id, workspace_id, name, permissions, created_at)
         select gen_random_uuid(), $1, 'filler ' || n, 0, $2
         from generate_series(1, $3) as n",
    )
    .bind(workspace.id.as_uuid())
    .bind(server.clock.now())
    .bind(most - 6)
    .execute(&server.db)
    .await
    .unwrap();
    let revision = server.structure(&owner).await.revision;
    let roles: Vec<Value> = (0..6)
        .map(|index| {
            json!({ "change": "create_workspace_role", "id": new_id(),
                    "workspace": workspace.id, "name": format!("role {index}") })
        })
        .collect();
    let before = state(&server).await;
    let reply = server
        .change_structure(&owner, &revision, json!(roles))
        .await;
    reply.error(ErrorCode::LimitReached);
    assert!(
        reply.text().contains("change 6 (create_workspace_role)"),
        "{}",
        reply.text()
    );
    assert_eq!(state(&server).await, before);
    server
        .change_structure(&owner, &revision, json!(roles[..5]))
        .await
        .ok::<StructureApplied>(StatusCode::OK);
}

#[tokio::test]
async fn rule_26_an_entry_keeps_the_identifier_that_created_it() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let export = server.structure(&owner).await;
    let workspace = default_of(&export).id;
    let general = default_of(&export).channels[0].id;
    let (plans, category) = (new_id(), new_id());
    let applied: StructureApplied = server
        .change_structure(
            &owner,
            &export.revision,
            json!([
                { "change": "create_category", "id": category, "workspace": workspace,
                  "name": "Projects" },
                { "change": "create_channel", "id": plans, "workspace": workspace,
                  "kind": "text", "name": "plans" },
            ]),
        )
        .await
        .ok(StatusCode::OK);
    let own_id = applied.created[1].id;

    // A later request names the channel by the identifier of its entry, and also by the
    // identifier that the server gave the object.
    let renamed: StructureApplied = server
        .change_structure(
            &owner,
            &applied.revision,
            json!([
                { "change": "update_channel", "id": plans, "name": "renamed",
                  "category": category },
                { "change": "update_channel", "id": own_id, "position": 4 },
            ]),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(renamed.created, []);
    let export = server.structure(&owner).await;
    let main = default_of(&export);
    assert_eq!(main.channels.len(), 3);
    let channel = channel_named(main, "renamed");
    assert_eq!((channel.id, channel.position), (plans, Some(4)));
    assert_eq!(channel.category, Some(Some(category)));

    // An identifier that names an object is not given to a new entry of the same kind: the
    // identifier of an entry, of an object, and of a deleted channel.
    server
        .change_structure(
            &owner,
            &export.revision,
            json!([{ "change": "delete_channel", "id": plans }]),
        )
        .await
        .ok::<StructureApplied>(StatusCode::OK);
    let revision = server.structure(&owner).await.revision;
    let before = state(&server).await;
    for taken in [plans.as_uuid(), own_id, general.as_uuid()] {
        let reply = server
            .change_structure(
                &owner,
                &revision,
                json!([{ "change": "create_channel", "id": taken, "workspace": workspace,
                         "kind": "text", "name": "again" }]),
            )
            .await;
        reply.error(ErrorCode::Conflict);
        assert!(reply.text().contains("is in use"), "{}", reply.text());
    }
    // The same identifier twice in one request.
    let twice = new_id();
    let entry = json!({ "change": "create_workspace_role", "id": twice, "workspace": workspace,
                        "name": "twice" });
    server
        .change_structure(&owner, &revision, json!([entry, entry]))
        .await
        .error(ErrorCode::Conflict);
    assert_eq!(state(&server).await, before);
    // Identifiers are kept per kind: the identifier of a channel entry can name a category.
    server
        .change_structure(
            &owner,
            &revision,
            json!([{ "change": "create_category", "id": plans, "workspace": workspace,
                     "name": "same text, other kind" }]),
        )
        .await
        .ok::<StructureApplied>(StatusCode::OK);
}

#[tokio::test]
async fn rule_27_a_request_has_at_most_two_hundred_changes() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let export = server.structure(&owner).await;
    let general = default_of(&export).channels[0].id;
    let rename = |count: usize| -> Vec<Value> {
        (0..count)
            .map(|index| {
                json!({ "change": "update_channel", "id": general,
                                 "name": format!("name {index}") })
            })
            .collect()
    };
    let before = state(&server).await;
    server
        .change_structure(&owner, &export.revision, json!(rename(201)))
        .await
        .error(ErrorCode::InvalidRequest);
    assert_eq!(state(&server).await, before);
    server
        .change_structure(&owner, &export.revision, json!(rename(200)))
        .await
        .ok::<StructureApplied>(StatusCode::OK);
    let export = server.structure(&owner).await;
    assert_eq!(default_of(&export).channels[0].name, "name 199");
}

#[tokio::test]
async fn rule_28_of_two_requests_with_one_revision_one_is_applied() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let export = server.structure(&owner).await;
    let workspace = default_of(&export).id;
    let change = |name: &str| {
        json!([{ "change": "create_channel", "id": new_id(), "workspace": workspace,
                 "kind": "text", "name": name }])
    };
    // The first request has done its work and waits before its commit. The second reads the
    // same structure, is applied and commits. The first then loses, runs again, and finds a
    // structure with another revision.
    let (one, two) = (change("first"), change("second"));
    server.gate().arm(0);
    let mut first = Box::pin(server.change_structure(&owner, &export.revision, one));
    drive_until(&mut first, server.gate().arrived()).await;
    server
        .change_structure(&owner, &export.revision, two)
        .await
        .ok::<StructureApplied>(StatusCode::OK);
    server.gate().release();
    let lost = first.await;
    lost.error(ErrorCode::Conflict);
    assert!(lost.text().contains("Make a new plan"), "{}", lost.text());

    // A change through a single route that commits first makes the request stale as well.
    let revision = server.structure(&owner).await.revision;
    let three = change("third");
    server.gate().arm(0);
    let mut planned = Box::pin(server.change_structure(&owner, &revision, three));
    drive_until(&mut planned, server.gate().arrived()).await;
    server
        .channel(&owner, WorkspaceIdOf(workspace).into(), "single")
        .await;
    server.gate().release();
    planned.await.error(ErrorCode::Conflict);
    let names: Vec<String> = default_of(&server.structure(&owner).await)
        .channels
        .iter()
        .map(|channel| channel.name.clone())
        .collect();
    for (name, exists) in [
        ("first", false),
        ("second", true),
        ("third", false),
        ("single", true),
    ] {
        assert_eq!(
            names.contains(&name.to_owned()),
            exists,
            "{name}: {names:?}"
        );
    }
}

fn node_types(plan: &Value, found: &mut Vec<(String, String)>) {
    if let Some(node) = plan["Node Type"].as_str() {
        let relation = plan["Relation Name"].as_str().unwrap_or_default();
        found.push((node.to_owned(), relation.to_owned()));
    }
    if let Some(children) = plan["Plans"].as_array() {
        for child in children {
            node_types(child, found);
        }
    }
}

/// The number of statements that one export runs on the connection of the requests.
async fn export_statements(
    server: &TestServer,
    owner: &Member,
    observer: &mut sqlx::PgConnection,
    role: i64,
) -> i64 {
    use crate::messages_queries::{since, statements};
    let before = statements(&mut *observer, role).await;
    server.structure(owner).await;
    since(statements(observer, role).await, before)
        .values()
        .sum()
}

#[tokio::test]
async fn rule_29_the_export_runs_a_fixed_number_of_statements_and_skips_deleted_channels() {
    use junto_server::structure as statements;
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    sqlx::query("create extension pg_stat_statements")
        .execute(&server.db)
        .await
        .unwrap();
    let (mut held, role) = crate::messages_queries::measure_requests(&server).await;
    server.structure(&owner).await;
    let small = export_statements(&server, &owner, &mut held[0], role).await;

    sqlx::query(
        "insert into workspaces (id, name, created_at)
         select gen_random_uuid(), 'w' || n, $1 from generate_series(1, 40) as n",
    )
    .bind(server.clock.now())
    .execute(&server.db)
    .await
    .unwrap();
    sqlx::query(
        "insert into channels (id, workspace_id, kind, name, position, created_at, deleted_at)
         select gen_random_uuid(), $1, 'text', 'c' || n, n, $2,
                case when n > 300 then $2 end
         from generate_series(1, 3000) as n",
    )
    .bind(workspace.id)
    .bind(server.clock.now())
    .execute(&server.db)
    .await
    .unwrap();
    sqlx::query("analyze").execute(&server.db).await.unwrap();
    let large = export_statements(&server, &owner, &mut held[0], role).await;
    assert_eq!(large, small);
    assert!(small <= 14, "{small}");
    drop(held);
    let export = server.structure(&owner).await;
    assert_eq!(export.structure.workspaces.len(), 41);
    assert_eq!(default_of(&export).channels.len(), 302);

    let mut connection = server.db.acquire().await.unwrap();
    sqlx::query("set enable_seqscan = off")
        .execute(&mut *connection)
        .await
        .unwrap();
    let plan: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "explain (format json) {}",
        statements::EXPORT_CHANNELS
    )))
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    let mut nodes = Vec::new();
    node_types(&plan[0]["Plan"], &mut nodes);
    let on_channels: Vec<&(String, String)> = nodes
        .iter()
        .filter(|(_, relation)| relation == "channels")
        .collect();
    assert_eq!(on_channels.len(), 1, "{plan}");
    assert_ne!(on_channels[0].0, "Seq Scan", "{plan}");
    assert!(plan.to_string().contains("channels_workspace"), "{plan}");
}

#[tokio::test]
async fn rule_30_a_new_workspace_has_its_creator_as_member_and_a_named_everyone_role() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let administrator = server.administrator(&owner, "Administrator").await;
    let export = server.structure(&owner).await;
    let (workspace, everyone, channel) = (new_id(), new_id(), new_id());
    let applied: StructureApplied = server
        .change_structure(
            &administrator,
            &export.revision,
            json!([
                { "change": "create_workspace", "id": workspace, "name": "Team",
                  "everyone_role": everyone },
                { "change": "create_channel", "id": channel, "workspace": workspace,
                  "kind": "text", "name": "private" },
                { "change": "set_channel_override", "channel": channel, "role": everyone,
                  "deny": ["view_channel"] },
            ]),
        )
        .await
        .ok(StatusCode::OK);
    let workspace_id = applied.created[0].id;
    let members: Vec<Value> = server
        .get(
            &administrator,
            &format!("/api/v1/workspaces/{workspace_id}/members"),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(members.len(), 1);
    assert_eq!(members[0]["account_id"], json!(administrator.account_id));
    let roles: Vec<WorkspaceRole> = server
        .get(
            &administrator,
            &format!("/api/v1/workspaces/{workspace_id}/roles"),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(roles.len(), 1);
    assert!(roles[0].is_everyone);
    assert_eq!(roles[0].id.as_uuid(), applied.created[1].id);
    let export = server.structure(&owner).await;
    let team = export
        .structure
        .workspaces
        .iter()
        .find(|entry| entry.id == workspace)
        .unwrap();
    assert_eq!(team.roles[0].id, everyone);
    assert_eq!(team.channels[0].overrides[0].role, everyone);
    assert_eq!(
        team.channels[0].overrides[0].deny,
        [WorkspacePermission::ViewChannel]
    );

    // The "everyone" identifier of a new workspace must be unused as well.
    let reply = server
        .change_structure(
            &owner,
            &export.revision,
            json!([{ "change": "create_workspace", "id": new_id(), "name": "Other",
                     "everyone_role": everyone }]),
        )
        .await;
    reply.error(ErrorCode::Conflict);
}

#[tokio::test]
async fn rule_31_the_database_refuses_broken_structure_state() {
    let server = TestServer::start().await;
    for statement in [
        "insert into structure_identifiers (kind, identifier, object_id)
         values ('message', gen_random_uuid(), gen_random_uuid())",
        "insert into access_tokens (id, token_hash, created_by, name, actions, created_at, expires_at)
         select gen_random_uuid(), sha256('a'), id, 'n', 0, now(), now() + interval '1 day'
         from accounts limit 1",
        "insert into access_tokens (id, token_hash, created_by, name, actions, created_at, expires_at)
         select gen_random_uuid(), sha256('b'), id, 'n', 16, now(), now() + interval '1 day'
         from accounts limit 1",
        "insert into access_tokens (id, token_hash, created_by, name, actions, created_at, expires_at)
         select gen_random_uuid(), 'short', id, 'n', 1, now(), now() + interval '1 day'
         from accounts limit 1",
    ] {
        let owner_exists: i64 = sqlx::query_scalar("select count(*) from accounts")
            .fetch_one(&server.db)
            .await
            .unwrap();
        if owner_exists == 0 {
            server.owner().await;
        }
        let refused = sqlx::query(statement).execute(&server.db).await;
        assert!(refused.is_err(), "{statement}");
    }
    let pair = "insert into structure_identifiers (kind, identifier, object_id)
                values ('channel', $1, $2)";
    let (entry, object) = (uuid::Uuid::now_v7(), uuid::Uuid::now_v7());
    sqlx::query(pair)
        .bind(entry)
        .bind(object)
        .execute(&server.db)
        .await
        .unwrap();
    for (entry, object) in [
        (entry, uuid::Uuid::now_v7()),
        (uuid::Uuid::now_v7(), object),
    ] {
        let refused = sqlx::query(pair)
            .bind(entry)
            .bind(object)
            .execute(&server.db)
            .await;
        assert!(refused.is_err());
    }
}

#[tokio::test]
async fn review_3_a_change_beside_a_request_is_ordered_after_it_or_makes_it_stale() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let export = server.structure(&owner).await;
    let workspace = default_of(&export).id;
    let category = new_id();
    let planned = json!([{ "change": "create_category", "id": category,
                           "workspace": workspace, "name": "Planned" }]);
    // The request has done its work and waits before its commit. A single route renames a
    // server role, which the request does not change, and commits first.
    let role = server.grant_server(&owner, &owner, &[]).await;
    let revision = server.structure(&owner).await.revision;
    server.gate().arm(0);
    let mut request = Box::pin(server.change_structure(&owner, &revision, planned));
    drive_until(&mut request, server.gate().arrived()).await;
    server
        .patch(
            &owner,
            &format!("/api/v1/server-roles/{role}"),
            &json!({ "name": "renamed meanwhile" }),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    server.gate().release();
    let reply = request.await;

    // PostgreSQL either lets both commit, in the order of the request first and the rename
    // after it, or makes the request run again, and it then finds another revision. In both
    // cases the request was applied to the structure of its revision or not at all.
    let after = server.structure(&owner).await;
    assert_eq!(after.structure.server_roles[0].name, "renamed meanwhile");
    let created = default_of(&after)
        .categories
        .iter()
        .any(|entry| entry.id == category);
    if reply.status == StatusCode::OK {
        let applied: StructureApplied = reply.ok(StatusCode::OK);
        assert!(created);
        // The revision in the answer is the one after the request alone.
        assert_ne!(applied.revision, revision);
        assert_ne!(applied.revision, after.revision);
    } else {
        reply.error(ErrorCode::Conflict);
        assert!(!created);
    }
}
