//! The role overrides of categories and the sync state of channels in the structure file
//! (format 4, ADR 0212), with the rules of people that a file does not hold.

use junto_protocol::access_token::TokenAction;
use junto_protocol::error::ErrorCode;
use junto_protocol::events::Level;
use junto_protocol::ids::{ChannelId, WorkspaceId, WorkspaceRoleId};
use junto_protocol::structure::{Category, Channel};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::setup_tool::tool;
use crate::support::{Member, Scratch, TestServer};

async fn full_token(server: &TestServer, owner: &Member) -> String {
    let actions = [TokenAction::ReadStructure, TokenAction::ManageStructure];
    let created = server.access_token(owner, &actions).await;
    created.token.expose().to_owned()
}

fn write(scratch: &Scratch, name: &str, content: &Value) -> String {
    let path = scratch.0.join(name);
    std::fs::write(&path, serde_json::to_string_pretty(content).unwrap()).unwrap();
    path.to_str().unwrap().to_owned()
}

async fn export(server: &TestServer, owner: &Member) -> Value {
    let export: Value = server
        .get(owner, "/api/v1/structure")
        .await
        .ok(StatusCode::OK);
    export
}

fn default_workspace(structure: &Value) -> &Value {
    structure["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|workspace| workspace["default"] == json!(true))
        .unwrap()
}

fn entry<'a>(list: &'a Value, name: &str) -> &'a Value {
    list.as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == name)
        .unwrap_or_else(|| panic!("no entry {name} in {list}"))
}

/// The overrides of an entry with the name of each role in place of its identifier, sorted,
/// so that the overrides of two servers can be compared.
fn by_role_name(workspace: &Value, overrides: &Value) -> Vec<(String, Value, Value)> {
    let name_of = |role: &Value| -> String {
        let role = workspace["roles"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["id"] == *role)
            .unwrap_or_else(|| panic!("no role {role}"));
        role["name"].as_str().unwrap().to_owned()
    };
    let mut named: Vec<(String, Value, Value)> = overrides
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            (
                name_of(&entry["role"]),
                entry["allow"].clone(),
                entry["deny"].clone(),
            )
        })
        .collect();
    named.sort_by(|a, b| a.0.cmp(&b.0));
    named
}

struct Source {
    server: TestServer,
    owner: Member,
    workspace_id: WorkspaceId,
    everyone: WorkspaceRoleId,
    helpers: WorkspaceRoleId,
    category: Category,
    synced: Channel,
    apart: Channel,
}

async fn put_rule(server: &TestServer, owner: &Member, path: &str, body: Value) {
    server
        .put(owner, path, &body)
        .await
        .ok::<Value>(StatusCode::OK);
}

/// A server whose category "Projects" has overrides of the everyone role and of the role
/// "Helpers", with a synced channel "plans" and a channel "notes" with its own overrides.
async fn source() -> Source {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace_id = server.default_workspace(&owner).await.id;
    let everyone = server.everyone_role(&owner, workspace_id).await.id;
    let helpers: junto_protocol::roles::WorkspaceRole = server
        .post(
            &owner,
            &format!("/api/v1/workspaces/{workspace_id}/roles"),
            &json!({ "name": "Helpers", "permissions": [] }),
        )
        .await
        .ok(StatusCode::CREATED);
    let category = server.category(&owner, workspace_id, "Projects", 0).await;
    let in_category =
        |name: &'static str| json!({ "name": name, "kind": "text", "category_id": category.id });
    let synced: Channel = server
        .post(
            &owner,
            &format!("/api/v1/workspaces/{workspace_id}/channels"),
            &in_category("plans"),
        )
        .await
        .ok(StatusCode::CREATED);
    let apart: Channel = server
        .post(
            &owner,
            &format!("/api/v1/workspaces/{workspace_id}/channels"),
            &in_category("notes"),
        )
        .await
        .ok(StatusCode::CREATED);
    let category_rules = format!(
        "/api/v1/workspaces/{workspace_id}/categories/{}/rules/roles",
        category.id
    );
    put_rule(
        &server,
        &owner,
        &format!("{category_rules}/{everyone}"),
        json!({ "deny": ["send_messages"] }),
    )
    .await;
    put_rule(
        &server,
        &owner,
        &format!("{category_rules}/{}", helpers.id),
        json!({ "allow": ["manage_messages"] }),
    )
    .await;
    // The first change of the channel copies the rules of the category and detaches it.
    put_rule(
        &server,
        &owner,
        &format!("/api/v1/channels/{}/rules/roles/{}", apart.id, helpers.id),
        json!({ "deny": ["view_channel"] }),
    )
    .await;
    Source {
        server,
        owner,
        workspace_id,
        everyone,
        helpers: helpers.id,
        category,
        synced,
        apart,
    }
}

async fn synced_state(server: &TestServer, owner: &Member, channel: ChannelId) -> bool {
    let rules: Value = server
        .get(owner, &format!("/api/v1/channels/{channel}/rules"))
        .await
        .ok(StatusCode::OK);
    rules["synced"].as_bool().unwrap()
}

#[tokio::test]
async fn format_4_category_overrides_and_sync_states_go_from_one_server_to_another() {
    let source = source().await;
    let (server, owner) = (&source.server, &source.owner);
    let structure = export(server, owner).await["structure"].clone();
    assert_eq!(structure["format"], 4);
    let main = default_workspace(&structure);
    let category = entry(&main["categories"], "Projects");
    assert_eq!(
        by_role_name(main, &category["overrides"]),
        [
            ("Helpers".to_owned(), json!(["manage_messages"]), json!([])),
            ("everyone".to_owned(), json!([]), json!(["send_messages"])),
        ]
    );
    let plans = entry(&main["channels"], "plans");
    assert_eq!(plans["synced"], true);
    assert_eq!(plans["overrides"], json!([]));
    let notes = entry(&main["channels"], "notes");
    assert_eq!(notes["synced"], false);
    assert_eq!(
        by_role_name(main, &notes["overrides"]),
        [
            ("Helpers".to_owned(), json!([]), json!(["view_channel"])),
            ("everyone".to_owned(), json!([]), json!(["send_messages"])),
        ]
    );
    // A channel without a category states no sync state.
    let general = entry(&main["channels"], "general");
    assert!(general.get("synced").is_none(), "{general}");

    let scratch = Scratch::new();
    let file = write(&scratch, "source.json", &structure);
    let target = TestServer::start().await;
    let target_owner = target.owner().await;
    let token = full_token(&target, &target_owner).await;
    let planned = tool(&target, &token, &["plan", &file]).await.succeeded();
    assert!(planned.out.contains("in the category"), "{}", planned.out);
    assert!(
        planned.out.contains("stop the sync with its category"),
        "{}",
        planned.out
    );
    tool(&target, &token, &["apply", &file]).await.succeeded();
    let again = tool(&target, &token, &["apply", &file]).await.succeeded();
    assert!(again.out.starts_with("No changes:"), "{}", again.out);

    let copied = export(&target, &target_owner).await["structure"].clone();
    let copy = default_workspace(&copied);
    assert_eq!(
        by_role_name(copy, &entry(&copy["categories"], "Projects")["overrides"]),
        by_role_name(main, &category["overrides"])
    );
    let copy_plans = entry(&copy["channels"], "plans");
    assert_eq!(copy_plans["synced"], true);
    assert_eq!(copy_plans["overrides"], json!([]));
    let copy_notes = entry(&copy["channels"], "notes");
    assert_eq!(copy_notes["synced"], false);
    assert_eq!(
        by_role_name(copy, &copy_notes["overrides"]),
        by_role_name(main, &notes["overrides"])
    );
    let created: Vec<Channel> = target
        .get(
            &target_owner,
            &format!(
                "/api/v1/workspaces/{}/channels",
                target.default_workspace(&target_owner).await.id
            ),
        )
        .await
        .ok::<junto_protocol::structure::ChannelList>(StatusCode::OK)
        .channels;
    let plans_on_target = created
        .iter()
        .find(|channel| channel.name == "plans")
        .unwrap();
    assert!(synced_state(&target, &target_owner, plans_on_target.id).await);
}

#[tokio::test]
async fn format_4_applying_a_file_keeps_the_rules_of_people() {
    let source = source().await;
    let (server, owner) = (&source.server, &source.owner);
    let member = server.join(owner, "Member").await;
    let workspace = source.workspace_id;
    let person = member.account_id;
    let category_person = format!(
        "/api/v1/workspaces/{workspace}/categories/{}/rules/people/{person}",
        source.category.id
    );
    put_rule(
        server,
        owner,
        &category_person,
        json!({ "allow": ["manage_messages"] }),
    )
    .await;
    let channel_person = format!("/api/v1/channels/{}/rules/people/{person}", source.apart.id);
    put_rule(
        server,
        owner,
        &channel_person,
        json!({ "deny": ["attach_files"] }),
    )
    .await;
    let workspace_person = format!("/api/v1/workspaces/{workspace}/members/{person}/permissions");
    put_rule(
        server,
        owner,
        &workspace_person,
        json!({ "deny": ["add_reactions"] }),
    )
    .await;
    let server_person = format!("/api/v1/accounts/{person}/server-permissions");
    put_rule(
        server,
        owner,
        &server_person,
        json!({ "allow": ["create_invites"] }),
    )
    .await;
    let read = |path: String| async move {
        let rules: Value = server.get(owner, &path).await.ok(StatusCode::OK);
        rules
    };
    let people_before = (
        read(category_person.replace(&format!("/people/{person}"), "")).await,
        read(workspace_person.clone()).await,
        read(server_person.clone()).await,
    );

    let mut structure = export(server, owner).await["structure"].clone();
    let text = structure.to_string();
    assert!(!text.contains(&person.to_string()), "{text}");
    assert!(!text.contains("person"), "{text}");
    // The file adds an override of the category, detaches the synced channel with an
    // override of its own, and syncs the other channel again.
    let everyone = source.everyone.to_string();
    let helpers = source.helpers.to_string();
    let main = structure["workspaces"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|workspace| workspace["default"] == json!(true))
        .unwrap();
    for category in main["categories"].as_array_mut().unwrap() {
        if category["name"] == "Projects" {
            category["overrides"] = json!([
                { "role": everyone, "deny": ["send_messages", "attach_files"] },
                { "role": helpers, "allow": ["manage_messages"] },
            ]);
        }
    }
    for channel in main["channels"].as_array_mut().unwrap() {
        if channel["name"] == "plans" {
            channel["synced"] = json!(false);
            channel["overrides"] = json!([
                { "role": everyone, "deny": ["send_messages", "attach_files"] },
                { "role": helpers, "allow": ["manage_messages", "mention_everyone"] },
            ]);
        }
    }
    let scratch = Scratch::new();
    let file = write(&scratch, "changed.json", &structure);
    let token = full_token(server, owner).await;
    tool(server, &token, &["apply", &file]).await.succeeded();

    assert!(!synced_state(server, owner, source.synced.id).await);
    let plans: Value = read(format!("/api/v1/channels/{}/rules", source.synced.id)).await;
    let person_rule = |rules: &Value| -> Option<Value> {
        rules["rules"]
            .as_array()
            .unwrap()
            .iter()
            .find(|rule| rule["subject"]["kind"] == "person")
            .cloned()
    };
    // The detached channel keeps the rule of the person that it took from its category.
    assert_eq!(
        person_rule(&plans).map(|rule| rule["allow"].clone()),
        Some(json!(["manage_messages"]))
    );
    let notes: Value = read(format!("/api/v1/channels/{}/rules", source.apart.id)).await;
    assert_eq!(
        person_rule(&notes).map(|rule| rule["deny"].clone()),
        Some(json!(["attach_files"]))
    );
    let category_rules: Value =
        read(category_person.replace(&format!("/people/{person}"), "")).await;
    assert_eq!(person_rule(&category_rules), person_rule(&people_before.0));
    assert_eq!(read(workspace_person.clone()).await, people_before.1);
    assert_eq!(read(server_person.clone()).await, people_before.2);

    // Syncing the channel with its own rule of a person again replaces that rule with the
    // rules of the category, as the sync route does.
    let mut structure = export(server, owner).await["structure"].clone();
    let main = structure["workspaces"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|workspace| workspace["default"] == json!(true))
        .unwrap();
    for channel in main["channels"].as_array_mut().unwrap() {
        if channel["name"] == "notes" {
            channel["synced"] = json!(true);
            channel["overrides"] = json!([]);
        }
    }
    let file = write(&scratch, "synced.json", &structure);
    let planned = tool(server, &token, &["plan", &file]).await.succeeded();
    assert!(
        planned
            .out
            .contains("the rules of people in it are replaced"),
        "{}",
        planned.out
    );
    tool(server, &token, &["apply", &file]).await.succeeded();
    assert!(synced_state(server, owner, source.apart.id).await);
    let notes: Value = read(format!("/api/v1/channels/{}/rules", source.apart.id)).await;
    assert_eq!(
        person_rule(&notes).map(|rule| rule["allow"].clone()),
        Some(json!(["manage_messages"]))
    );
}

#[tokio::test]
async fn format_4_the_changes_run_the_checks_audit_entries_and_events_of_the_rule_routes() {
    let source = source().await;
    let (server, owner) = (&source.server, &source.owner);
    let revision = || async move {
        export(server, owner).await["revision"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let cursor = server.snapshot(owner).await.cursor;
    let changes = json!([
        { "change": "set_category_override", "category": source.category.id,
          "role": source.everyone, "allow": [], "deny": ["add_reactions"] },
        { "change": "set_channel_sync", "channel": source.synced.id, "synced": false },
        { "change": "set_channel_sync", "channel": source.apart.id, "synced": true },
    ]);
    server
        .change_structure(owner, &revision().await, changes)
        .await
        .ok::<Value>(StatusCode::OK);
    assert!(!synced_state(server, owner, source.synced.id).await);
    assert!(synced_state(server, owner, source.apart.id).await);
    let category: Value = server
        .get(
            owner,
            &format!(
                "/api/v1/workspaces/{}/categories/{}/rules",
                source.workspace_id, source.category.id
            ),
        )
        .await
        .ok(StatusCode::OK);
    let everyone_rule = category["rules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|rule| rule["subject"]["role_id"] == json!(source.everyone))
        .cloned()
        .unwrap();
    assert_eq!(everyone_rule["deny"], json!(["add_reactions"]));

    let actions: Vec<String> = sqlx::query_scalar(
        "select action from audit_entries where action in
             ('category_role_rule_set', 'channel_rules_synced')
         order by occurred_at, action",
    )
    .fetch_all(&server.db)
    .await
    .unwrap();
    assert!(
        actions
            .iter()
            .filter(|action| *action == "category_role_rule_set")
            .count()
            >= 3,
        "{actions:?}"
    );
    assert!(
        actions
            .iter()
            .filter(|action| *action == "channel_rules_synced")
            .count()
            >= 3,
        "{actions:?}"
    );
    let names: Vec<&str> = crate::discovery_support::events(server, owner, cursor, Level::Full)
        .await
        .iter()
        .map(junto_protocol::events::Event::name)
        .filter(|name| *name == "permission_rule_set" || *name == "channel_rules_synced")
        .collect();
    assert_eq!(
        names,
        [
            "permission_rule_set",
            "channel_rules_synced",
            "channel_rules_synced"
        ]
    );

    // A change that names a role of another workspace, or that syncs a channel without a
    // category, ends the request with the number of the change, and nothing is changed.
    let other: junto_protocol::structure::Workspace = server
        .post(owner, "/api/v1/workspaces", &json!({ "name": "Other" }))
        .await
        .ok(StatusCode::CREATED);
    let foreign = server.everyone_role(owner, other.id).await.id;
    let general = server
        .get(
            owner,
            &format!("/api/v1/workspaces/{}/channels", source.workspace_id),
        )
        .await
        .ok::<junto_protocol::structure::ChannelList>(StatusCode::OK)
        .channels
        .into_iter()
        .find(|channel| channel.category_id.is_none())
        .unwrap();
    let current = revision().await;
    for (change, code) in [
        (
            json!({ "change": "set_category_override", "category": source.category.id,
                    "role": foreign, "allow": ["view_channel"] }),
            ErrorCode::InvalidRequest,
        ),
        (
            json!({ "change": "set_channel_sync", "channel": general.id, "synced": true }),
            ErrorCode::InvalidRequest,
        ),
        (
            json!({ "change": "set_channel_sync", "channel": general.id, "synced": false }),
            ErrorCode::InvalidRequest,
        ),
    ] {
        let reply = server
            .change_structure(owner, &current, json!([change.clone()]))
            .await;
        reply.error(code);
        assert!(
            reply.text().contains(&format!(
                "change 1 ({})",
                change["change"].as_str().unwrap()
            )),
            "{}",
            reply.text()
        );
        assert_eq!(revision().await, current);
    }

    // Only an administrator uses the route; a member who manages channels does not.
    let manager = server.join(owner, "Manager").await;
    server
        .grant_workspace(
            owner,
            &manager,
            source.workspace_id,
            &[junto_protocol::WorkspacePermission::ManageChannel],
        )
        .await;
    let current = revision().await;
    server
        .change_structure(
            &manager,
            &current,
            json!([{ "change": "set_category_override", "category": source.category.id,
                     "role": source.helpers, "allow": ["view_channel"] }]),
        )
        .await
        .error(ErrorCode::Forbidden);
}

#[tokio::test]
async fn format_4_a_file_of_format_2_is_compared_with_what_a_synced_channel_uses() {
    let source = source().await;
    let (server, owner) = (&source.server, &source.owner);
    let token = full_token(server, owner).await;
    let workspace = export(server, owner).await["structure"].clone();
    let main = default_workspace(&workspace);
    let plans = entry(&main["channels"], "plans");
    let format_2 = |overrides: Value| {
        json!({ "format": 2, "workspaces": [{ "id": main["id"], "name": main["name"],
            "channels": [{ "id": plans["id"], "name": "plans", "kind": "text",
                           "overrides": overrides }] }] })
    };
    let scratch = Scratch::new();

    // The override of the file equals the one that the channel takes from its category.
    let same = write(
        &scratch,
        "same.json",
        &format_2(json!([{ "role": source.helpers, "allow": ["manage_messages"] }])),
    );
    let planned = tool(server, &token, &["plan", &same]).await.succeeded();
    assert!(planned.out.starts_with("No changes:"), "{}", planned.out);
    assert!(synced_state(server, owner, source.synced.id).await);

    // Another override detaches the channel, which keeps the other rules of its category.
    let other = write(
        &scratch,
        "other.json",
        &format_2(json!([{ "role": source.helpers, "allow": ["mention_everyone"] }])),
    );
    tool(server, &token, &["apply", &other]).await.succeeded();
    assert!(!synced_state(server, owner, source.synced.id).await);
    let after = export(server, owner).await["structure"].clone();
    let after_main = default_workspace(&after);
    let plans = entry(&after_main["channels"], "plans");
    assert_eq!(plans["synced"], false);
    assert_eq!(
        by_role_name(after_main, &plans["overrides"]),
        [
            ("Helpers".to_owned(), json!(["mention_everyone"]), json!([])),
            ("everyone".to_owned(), json!([]), json!(["send_messages"])),
        ]
    );
    let again = tool(server, &token, &["apply", &other]).await.succeeded();
    assert!(again.out.starts_with("No changes:"), "{}", again.out);
}
