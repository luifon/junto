//! The routes of person and category permission rules, sync, moves and the member access
//! route (ADR 0304, ADR 0306).

use crate::support::{Member, TestServer};
use junto_protocol::error::ErrorCode;
use junto_protocol::events::{Event, Level};
use junto_protocol::ids::{AccountId, WorkspaceId, WorkspaceRoleId};
use junto_protocol::structure::{Category, Channel};
use junto_protocol::{ServerPermission as S, WorkspacePermission as W};
use reqwest::StatusCode;
use serde_json::{Value, json};

struct Place {
    server: TestServer,
    owner: Member,
    workspace: WorkspaceId,
    everyone: WorkspaceRoleId,
}

async fn place() -> Place {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await.id;
    let everyone = server.everyone_role(&owner, workspace).await.id;
    Place {
        server,
        owner,
        workspace,
        everyone,
    }
}

async fn text_channel(place: &Place, name: &str, category: Option<&Category>) -> Channel {
    place
        .server
        .post(
            &place.owner,
            &format!("/api/v1/workspaces/{}/channels", place.workspace),
            &json!({"name": name, "kind": "text", "category_id": category.map(|held| held.id)}),
        )
        .await
        .ok(StatusCode::CREATED)
}

/// A workspace role that nobody holds.
async fn unheld_role(place: &Place, name: &str) -> WorkspaceRoleId {
    let role: Value = place
        .server
        .post(
            &place.owner,
            &format!("/api/v1/workspaces/{}/roles", place.workspace),
            &json!({"name": name, "permissions": []}),
        )
        .await
        .ok(StatusCode::CREATED);
    serde_json::from_value(role["id"].clone()).unwrap()
}

fn channel_rules(channel: &Channel) -> String {
    format!("/api/v1/channels/{}/rules", channel.id)
}

fn channel_role(channel: &Channel, role: WorkspaceRoleId) -> String {
    format!("/api/v1/channels/{}/rules/roles/{role}", channel.id)
}

fn channel_person(channel: &Channel, account: AccountId) -> String {
    format!("/api/v1/channels/{}/rules/people/{account}", channel.id)
}

fn category_rules(category: &Category) -> String {
    format!(
        "/api/v1/workspaces/{}/categories/{}/rules",
        category.workspace_id, category.id
    )
}

fn category_role(category: &Category, role: WorkspaceRoleId) -> String {
    format!("{}/roles/{role}", category_rules(category))
}

fn category_person(category: &Category, account: AccountId) -> String {
    format!("{}/people/{account}", category_rules(category))
}

fn member_permissions(workspace: WorkspaceId, account: AccountId) -> String {
    format!("/api/v1/workspaces/{workspace}/members/{account}/permissions")
}

fn server_permissions(account: AccountId) -> String {
    format!("/api/v1/accounts/{account}/server-permissions")
}

fn subject_of(rule: &Value) -> (String, String) {
    let subject = &rule["subject"];
    let id = subject["role_id"]
        .as_str()
        .or_else(|| subject["account_id"].as_str())
        .unwrap();
    (subject["kind"].as_str().unwrap().to_owned(), id.to_owned())
}

fn rule_of<'a>(rules: &'a Value, kind: &str, id: &str) -> Option<&'a Value> {
    rules["rules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|rule| subject_of(rule) == (kind.to_owned(), id.to_owned()))
}

fn names(rule: &Value, field: &str) -> Vec<String> {
    rule[field]
        .as_array()
        .unwrap()
        .iter()
        .map(|name| name.as_str().unwrap().to_owned())
        .collect()
}

async fn audit_count(server: &TestServer, action: &str) -> i64 {
    sqlx::query_scalar("select count(*) from audit_entries where action = $1")
        .bind(action)
        .fetch_one(&server.db)
        .await
        .unwrap()
}

async fn event_count(server: &TestServer) -> i64 {
    sqlx::query_scalar("select count(*) from events")
        .fetch_one(&server.db)
        .await
        .unwrap()
}

fn has_event(events: &[(u64, Event)], name: &str) -> bool {
    events.iter().any(|(_, event)| event.name() == name)
}

async fn hide_from_everyone(place: &Place, channel: &Channel) {
    place
        .server
        .put(
            &place.owner,
            &channel_role(channel, place.everyone),
            &json!({"allow": [], "deny": ["view_channel"]}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
}

#[tokio::test]
async fn channel_rules_list_role_and_person_rules_and_the_sync_state() {
    let place = place().await;
    let reader = place.server.join(&place.owner, "Reader").await;
    let channel = text_channel(&place, "Notes", None).await;
    let role: Value = place
        .server
        .put(
            &place.owner,
            &channel_role(&channel, place.everyone),
            &json!({"allow": [], "deny": ["add_reactions"]}),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(role["subject"]["kind"], "role");
    let person: Value = place
        .server
        .patch(
            &place.owner,
            &channel_person(&channel, reader.account_id),
            &json!({"permission": "attach_files", "state": "deny"}),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(person["subject"]["kind"], "person");
    assert_eq!(names(&person, "deny"), ["attach_files"]);
    let rules: Value = place
        .server
        .get(&place.owner, &channel_rules(&channel))
        .await
        .ok(StatusCode::OK);
    assert_eq!(rules["synced"], false);
    assert_eq!(rules["category_id"], Value::Null);
    let everyone = place.everyone.to_string();
    let held = rule_of(&rules, "role", &everyone).unwrap();
    assert_eq!(names(held, "deny"), ["add_reactions"]);
    let held = rule_of(&rules, "person", &reader.account_id.to_string()).unwrap();
    assert_eq!(names(held, "deny"), ["attach_files"]);
    assert_eq!(
        audit_count(&place.server, "channel_person_rule_set").await,
        1
    );

    // The person rule decides what the reader can use in the channel.
    let permissions: Value = place
        .server
        .get(
            &reader,
            &format!("/api/v1/channels/{}/permissions", channel.id),
        )
        .await
        .ok(StatusCode::OK);
    let held: Vec<String> = names(&permissions, "permissions");
    assert!(!held.contains(&"attach_files".to_owned()), "{held:?}");
    assert!(held.contains(&"send_messages".to_owned()), "{held:?}");

    // Two empty sets remove the rule.
    place
        .server
        .put(
            &place.owner,
            &channel_person(&channel, reader.account_id),
            &json!({"allow": [], "deny": []}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    let rules: Value = place
        .server
        .get(&place.owner, &channel_rules(&channel))
        .await
        .ok(StatusCode::OK);
    assert!(rule_of(&rules, "person", &reader.account_id.to_string()).is_none());
}

#[tokio::test]
async fn hidden_and_absent_places_and_subjects_answer_not_found() {
    let place = place().await;
    let manager = place.server.join(&place.owner, "Manager").await;
    place
        .server
        .grant_workspace(&place.owner, &manager, place.workspace, &[W::ManageChannel])
        .await;
    let hidden = text_channel(&place, "Hidden", None).await;
    hide_from_everyone(&place, &hidden).await;
    let visible = text_channel(&place, "Visible", None).await;
    let absent = Channel {
        id: junto_protocol::ids::ChannelId::generate(),
        ..visible.clone()
    };
    for channel in [&hidden, &absent] {
        place
            .server
            .get(&manager, &channel_rules(channel))
            .await
            .error(ErrorCode::NotFound);
        place
            .server
            .put(
                &manager,
                &channel_role(channel, place.everyone),
                &json!({"allow": [], "deny": ["add_reactions"]}),
            )
            .await
            .error(ErrorCode::NotFound);
        place
            .server
            .post(
                &manager,
                &format!("{}/sync", channel_rules(channel)),
                &json!({}),
            )
            .await
            .error(ErrorCode::NotFound);
    }
    // An absent account and an account that is not a member are the same answer.
    let other: Value = place
        .server
        .post(
            &place.owner,
            "/api/v1/workspaces",
            &json!({"name": "Other"}),
        )
        .await
        .ok(StatusCode::CREATED);
    let other: WorkspaceId = serde_json::from_value(other["id"].clone()).unwrap();
    let elsewhere: Channel = place
        .server
        .post(
            &place.owner,
            &format!("/api/v1/workspaces/{other}/channels"),
            &json!({"name": "Elsewhere", "kind": "text"}),
        )
        .await
        .ok(StatusCode::CREATED);
    place
        .server
        .patch(
            &manager,
            &channel_person(&visible, AccountId::generate()),
            &json!({"permission": "add_reactions", "state": "deny"}),
        )
        .await
        .error(ErrorCode::NotFound);
    place
        .server
        .patch(
            &place.owner,
            &channel_person(&elsewhere, manager.account_id),
            &json!({"permission": "add_reactions", "state": "deny"}),
        )
        .await
        .error(ErrorCode::NotFound);
    for (workspace, account) in [
        (place.workspace, AccountId::generate()),
        (other, manager.account_id),
    ] {
        place
            .server
            .get(&place.owner, &member_permissions(workspace, account))
            .await
            .error(ErrorCode::NotFound);
    }
    let foreign = place.server.everyone_role(&place.owner, other).await.id;
    place
        .server
        .put(
            &manager,
            &channel_role(&visible, foreign),
            &json!({"allow": [], "deny": ["add_reactions"]}),
        )
        .await
        .error(ErrorCode::NotFound);
    // A category that holds no channel that the caller sees is hidden like an absent one.
    let category = place
        .server
        .category(&place.owner, place.workspace, "Private", 0)
        .await;
    let reader = place.server.join(&place.owner, "Reader").await;
    let absent_category = Category {
        id: junto_protocol::ids::CategoryId::generate(),
        ..category.clone()
    };
    for held in [&category, &absent_category] {
        place
            .server
            .get(&reader, &category_rules(held))
            .await
            .error(ErrorCode::NotFound);
        place
            .server
            .put(
                &reader,
                &category_role(held, place.everyone),
                &json!({"allow": ["view_channel"], "deny": []}),
            )
            .await
            .error(ErrorCode::NotFound);
    }
}

#[tokio::test]
async fn rules_need_manage_channel_in_the_place() {
    let place = place().await;
    let reader = place.server.join(&place.owner, "Reader").await;
    let category = place
        .server
        .category(&place.owner, place.workspace, "Plans", 0)
        .await;
    let channel = text_channel(&place, "Notes", Some(&category)).await;
    let forbidden = |reply: crate::support::Reply| reply.error(ErrorCode::Forbidden);
    forbidden(place.server.get(&reader, &channel_rules(&channel)).await);
    forbidden(
        place
            .server
            .put(
                &reader,
                &channel_person(&channel, reader.account_id),
                &json!({"allow": ["manage_messages"], "deny": []}),
            )
            .await,
    );
    forbidden(
        place
            .server
            .post(
                &reader,
                &format!("{}/sync", channel_rules(&channel)),
                &json!({}),
            )
            .await,
    );
    forbidden(place.server.get(&reader, &category_rules(&category)).await);
    forbidden(
        place
            .server
            .patch(
                &reader,
                &category_person(&category, reader.account_id),
                &json!({"permission": "manage_messages", "state": "allow"}),
            )
            .await,
    );
}

#[tokio::test]
async fn a_person_rule_needs_rank_above_the_person_or_the_person() {
    let place = place().await;
    let manager = place.server.join(&place.owner, "Manager").await;
    let reader = place.server.join(&place.owner, "Reader").await;
    place
        .server
        .grant_workspace(&place.owner, &manager, place.workspace, &[W::ManageChannel])
        .await;
    let category = place
        .server
        .category(&place.owner, place.workspace, "Plans", 0)
        .await;
    let channel = text_channel(&place, "Notes", Some(&category)).await;
    let deny = json!({"allow": [], "deny": ["add_reactions"]});
    for path in [
        channel_person(&channel, place.owner.account_id),
        category_person(&category, place.owner.account_id),
    ] {
        place
            .server
            .put(&manager, &path, &deny)
            .await
            .error(ErrorCode::Forbidden);
    }
    for account in [manager.account_id, reader.account_id] {
        place
            .server
            .put(&manager, &channel_person(&channel, account), &deny)
            .await
            .ok::<Value>(StatusCode::OK);
        place
            .server
            .put(&manager, &category_person(&category, account), &deny)
            .await
            .ok::<Value>(StatusCode::OK);
    }
}

#[tokio::test]
async fn a_rule_grants_nothing_that_the_caller_does_not_hold() {
    let place = place().await;
    let manager = place.server.join(&place.owner, "Manager").await;
    let reader = place.server.join(&place.owner, "Reader").await;
    place
        .server
        .grant_workspace(&place.owner, &manager, place.workspace, &[W::ManageChannel])
        .await;
    let category = place
        .server
        .category(&place.owner, place.workspace, "Plans", 0)
        .await;
    let channel = text_channel(&place, "Notes", None).await;
    let child = text_channel(&place, "Child", Some(&category)).await;
    let allow = json!({"allow": ["manage_messages"], "deny": []});
    for path in [
        channel_person(&channel, reader.account_id),
        channel_role(&channel, place.everyone),
        category_person(&category, reader.account_id),
        category_role(&category, place.everyone),
        channel_person(&child, reader.account_id),
    ] {
        place
            .server
            .put(&manager, &path, &allow)
            .await
            .error(ErrorCode::ExceedsOwnPermissions);
    }
    // Lifting a deny is a grant as well; adding a deny is not.
    place
        .server
        .put(
            &place.owner,
            &category_person(&category, reader.account_id),
            &json!({"allow": [], "deny": ["manage_messages"]}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    place
        .server
        .patch(
            &manager,
            &category_person(&category, reader.account_id),
            &json!({"permission": "manage_messages", "state": "inherit"}),
        )
        .await
        .error(ErrorCode::ExceedsOwnPermissions);
    place
        .server
        .patch(
            &manager,
            &category_person(&category, reader.account_id),
            &json!({"permission": "add_reactions", "state": "deny"}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    // A rule cannot give the caller a channel that it cannot see (ADR 0304).
    let hidden = text_channel(&place, "Hidden", None).await;
    hide_from_everyone(&place, &hidden).await;
    place
        .server
        .put(
            &manager,
            &channel_person(&hidden, manager.account_id),
            &json!({"allow": ["view_channel"], "deny": []}),
        )
        .await
        .error(ErrorCode::NotFound);
}

#[tokio::test]
async fn a_rule_that_changes_nothing_writes_nothing_and_keeps_the_channel_synced() {
    let place = place().await;
    let reader = place.server.join(&place.owner, "Reader").await;
    let category = place
        .server
        .category(&place.owner, place.workspace, "Plans", 0)
        .await;
    let channel = text_channel(&place, "Notes", Some(&category)).await;
    let rule = json!({"allow": [], "deny": ["add_reactions"]});
    place
        .server
        .put(
            &place.owner,
            &category_person(&category, reader.account_id),
            &rule,
        )
        .await
        .ok::<Value>(StatusCode::OK);
    let entries: i64 = sqlx::query_scalar("select count(*) from audit_entries")
        .fetch_one(&place.server.db)
        .await
        .unwrap();
    let events = event_count(&place.server).await;
    for path in [
        category_person(&category, reader.account_id),
        channel_person(&channel, reader.account_id),
    ] {
        place
            .server
            .put(&place.owner, &path, &rule)
            .await
            .ok::<Value>(StatusCode::OK);
    }
    place
        .server
        .patch(
            &place.owner,
            &member_permissions(place.workspace, reader.account_id),
            &json!({"permission": "add_reactions", "state": "inherit"}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    place
        .server
        .patch(
            &place.owner,
            &server_permissions(reader.account_id),
            &json!({"permission": "create_invites", "state": "inherit"}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    let after: i64 = sqlx::query_scalar("select count(*) from audit_entries")
        .fetch_one(&place.server.db)
        .await
        .unwrap();
    assert_eq!(after, entries);
    assert_eq!(event_count(&place.server).await, events);
    let rules: Value = place
        .server
        .get(&place.owner, &channel_rules(&channel))
        .await
        .ok(StatusCode::OK);
    assert_eq!(rules["synced"], true);
}

#[tokio::test]
async fn the_first_change_detaches_and_sync_takes_the_category_rules_again() {
    let place = place().await;
    let manager = place.server.join(&place.owner, "Manager").await;
    let reader = place.server.join(&place.owner, "Reader").await;
    place
        .server
        .grant_workspace(&place.owner, &manager, place.workspace, &[W::ManageChannel])
        .await;
    let category = place
        .server
        .category(&place.owner, place.workspace, "Plans", 0)
        .await;
    let channel = text_channel(&place, "Notes", Some(&category)).await;
    let guests = unheld_role(&place, "Guests").await;
    place
        .server
        .put(
            &place.owner,
            &category_role(&category, guests),
            &json!({"allow": [], "deny": ["add_reactions"]}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    let rules: Value = place
        .server
        .get(&manager, &channel_rules(&channel))
        .await
        .ok(StatusCode::OK);
    assert_eq!(rules["synced"], true);
    assert_eq!(rules["category_id"], json!(category.id));
    assert!(rule_of(&rules, "role", &guests.to_string()).is_some());

    let mut stream = place.server.subscribed(&manager, Level::Full).await;
    place
        .server
        .patch(
            &manager,
            &channel_person(&channel, reader.account_id),
            &json!({"permission": "attach_files", "state": "deny"}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    let events = place.server.traffic(&manager, &mut stream).await;
    assert!(has_event(&events, "channel_rules_synced"), "{events:?}");
    assert!(has_event(&events, "permission_rule_set"), "{events:?}");
    let rules: Value = place
        .server
        .get(&manager, &channel_rules(&channel))
        .await
        .ok(StatusCode::OK);
    assert_eq!(rules["synced"], false);
    assert!(rule_of(&rules, "role", &guests.to_string()).is_some());
    assert!(rule_of(&rules, "person", &reader.account_id.to_string()).is_some());
    assert_eq!(audit_count(&place.server, "channel_rules_synced").await, 1);

    // A later change of the category does not reach the detached channel.
    place
        .server
        .put(
            &place.owner,
            &category_role(&category, guests),
            &json!({"allow": [], "deny": ["send_messages"]}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    let rules: Value = place
        .server
        .get(&manager, &channel_rules(&channel))
        .await
        .ok(StatusCode::OK);
    let held = rule_of(&rules, "role", &guests.to_string()).unwrap();
    assert_eq!(names(held, "deny"), ["add_reactions"]);

    let synced: Value = place
        .server
        .post(
            &manager,
            &format!("{}/sync", channel_rules(&channel)),
            &json!({}),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(synced["synced"], true);
    let held = rule_of(&synced, "role", &guests.to_string()).unwrap();
    assert_eq!(names(held, "deny"), ["send_messages"]);
    assert!(rule_of(&synced, "person", &reader.account_id.to_string()).is_none());
    let own: i64 = sqlx::query_scalar(
        "select (select count(*) from channel_overrides where channel_id = $1)
              + (select count(*) from channel_person_overrides where channel_id = $1)",
    )
    .bind(channel.id)
    .fetch_one(&place.server.db)
    .await
    .unwrap();
    assert_eq!(own, 0);
    let events = place.server.traffic(&manager, &mut stream).await;
    assert!(has_event(&events, "channel_rules_synced"), "{events:?}");
    assert_eq!(audit_count(&place.server, "channel_rules_synced").await, 2);

    // Sync again of a synced channel writes nothing; a channel without a category has none.
    place
        .server
        .post(
            &manager,
            &format!("{}/sync", channel_rules(&channel)),
            &json!({}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    assert_eq!(audit_count(&place.server, "channel_rules_synced").await, 2);
    let alone = text_channel(&place, "Alone", None).await;
    place
        .server
        .post(
            &manager,
            &format!("{}/sync", channel_rules(&alone)),
            &json!({}),
        )
        .await
        .error(ErrorCode::InvalidRequest);
}

#[tokio::test]
async fn sync_grants_nothing_that_the_caller_does_not_hold() {
    let place = place().await;
    let manager = place.server.join(&place.owner, "Manager").await;
    place
        .server
        .grant_workspace(&place.owner, &manager, place.workspace, &[W::ManageChannel])
        .await;
    let category = place
        .server
        .category(&place.owner, place.workspace, "Plans", 0)
        .await;
    let channel = text_channel(&place, "Notes", Some(&category)).await;
    place
        .server
        .put(
            &place.owner,
            &category_role(&category, place.everyone),
            &json!({"allow": ["manage_messages"], "deny": []}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    place
        .server
        .put(
            &place.owner,
            &channel_role(&channel, place.everyone),
            &json!({"allow": [], "deny": []}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    place
        .server
        .post(
            &manager,
            &format!("{}/sync", channel_rules(&channel)),
            &json!({}),
        )
        .await
        .error(ErrorCode::ExceedsOwnPermissions);
    let rules: Value = place
        .server
        .get(&place.owner, &channel_rules(&channel))
        .await
        .ok(StatusCode::OK);
    assert_eq!(rules["synced"], false);
    place
        .server
        .post(
            &place.owner,
            &format!("{}/sync", channel_rules(&channel)),
            &json!({}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
}

#[tokio::test]
async fn a_category_rule_reaches_its_synced_channels() {
    let place = place().await;
    let reader = place.server.join(&place.owner, "Reader").await;
    let category = place
        .server
        .category(&place.owner, place.workspace, "Plans", 0)
        .await;
    let channel = text_channel(&place, "Notes", Some(&category)).await;
    let mut stream = place.server.subscribed(&reader, Level::Full).await;
    let rule: Value = place
        .server
        .patch(
            &place.owner,
            &category_person(&category, reader.account_id),
            &json!({"permission": "view_channel", "state": "deny"}),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(names(&rule, "deny"), ["view_channel"]);
    let held: Value = place
        .server
        .get(&place.owner, &category_rules(&category))
        .await
        .ok(StatusCode::OK);
    assert!(rule_of(&held, "person", &reader.account_id.to_string()).is_some());
    place
        .server
        .get(
            &reader,
            &format!("/api/v1/channels/{}/messages", channel.id),
        )
        .await
        .error(ErrorCode::NotFound);
    let events = place.server.traffic(&reader, &mut stream).await;
    assert!(has_event(&events, "permissions_changed"), "{events:?}");
    assert!(!has_event(&events, "permission_rule_set"), "{events:?}");
    assert_eq!(
        audit_count(&place.server, "category_person_rule_set").await,
        1
    );
    place
        .server
        .put(
            &place.owner,
            &category_role(&category, place.everyone),
            &json!({"allow": [], "deny": ["add_reactions"]}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    assert_eq!(
        audit_count(&place.server, "category_role_rule_set").await,
        1
    );
    let rules: Value = place
        .server
        .get(&place.owner, &channel_rules(&channel))
        .await
        .ok(StatusCode::OK);
    assert_eq!(rules["synced"], true);
    assert!(rule_of(&rules, "role", &place.everyone.to_string()).is_some());
    assert!(rule_of(&rules, "person", &reader.account_id.to_string()).is_some());
}

#[tokio::test]
async fn rule_events_reach_only_the_people_who_manage_the_place() {
    let place = place().await;
    let manager = place.server.join(&place.owner, "Manager").await;
    let reader = place.server.join(&place.owner, "Reader").await;
    let bystander = place.server.join(&place.owner, "Bystander").await;
    place
        .server
        .grant_workspace(&place.owner, &manager, place.workspace, &[W::ManageChannel])
        .await;
    let category = place
        .server
        .category(&place.owner, place.workspace, "Plans", 0)
        .await;
    let channel = text_channel(&place, "Notes", Some(&category)).await;
    let mut managing = place.server.subscribed(&manager, Level::Full).await;
    let mut reading = place.server.subscribed(&reader, Level::Full).await;
    let mut watching = place.server.subscribed(&bystander, Level::Full).await;
    let deny = json!({"allow": [], "deny": ["attach_files"]});
    place
        .server
        .put(
            &place.owner,
            &category_person(&category, reader.account_id),
            &deny,
        )
        .await
        .ok::<Value>(StatusCode::OK);
    place
        .server
        .put(
            &place.owner,
            &channel_person(&channel, reader.account_id),
            &json!({"allow": [], "deny": ["add_reactions"]}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    let managed = place.server.traffic(&manager, &mut managing).await;
    let set = managed
        .iter()
        .filter(|(_, event)| event.name() == "permission_rule_set")
        .count();
    assert_eq!(set, 2, "{managed:?}");
    assert!(has_event(&managed, "channel_rules_synced"), "{managed:?}");
    let read = place.server.traffic(&reader, &mut reading).await;
    assert!(!has_event(&read, "permission_rule_set"), "{read:?}");
    assert!(!has_event(&read, "channel_rules_synced"), "{read:?}");
    assert!(has_event(&read, "permissions_changed"), "{read:?}");
    // A person whom no rule names gets no refresh.
    let watched = place.server.traffic(&bystander, &mut watching).await;
    assert!(!has_event(&watched, "permission_rule_set"), "{watched:?}");
    assert!(!has_event(&watched, "permissions_changed"), "{watched:?}");
}

#[tokio::test]
async fn a_synced_channel_that_moves_takes_the_rules_of_its_new_category() {
    let place = place().await;
    let manager = place.server.join(&place.owner, "Manager").await;
    place
        .server
        .grant_workspace(&place.owner, &manager, place.workspace, &[W::ManageChannel])
        .await;
    let first = place
        .server
        .category(&place.owner, place.workspace, "First", 0)
        .await;
    let second = place
        .server
        .category(&place.owner, place.workspace, "Second", 1)
        .await;
    let channel = text_channel(&place, "Notes", Some(&first)).await;
    let other = text_channel(&place, "Other", Some(&first)).await;
    let guests = unheld_role(&place, "Guests").await;
    for (category, denied) in [(&first, "add_reactions"), (&second, "attach_files")] {
        place
            .server
            .put(
                &place.owner,
                &category_role(category, guests),
                &json!({"allow": [], "deny": [denied]}),
            )
            .await
            .ok::<Value>(StatusCode::OK);
    }
    let mut stream = place.server.subscribed(&manager, Level::Full).await;
    place
        .server
        .patch(
            &manager,
            &format!("/api/v1/channels/{}", channel.id),
            &json!({"category_id": second.id}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    let rules: Value = place
        .server
        .get(&manager, &channel_rules(&channel))
        .await
        .ok(StatusCode::OK);
    assert_eq!(rules["synced"], true);
    assert_eq!(rules["category_id"], json!(second.id));
    let held = rule_of(&rules, "role", &guests.to_string()).unwrap();
    assert_eq!(names(held, "deny"), ["attach_files"]);
    let events = place.server.traffic(&manager, &mut stream).await;
    assert!(has_event(&events, "channel_rules_synced"), "{events:?}");

    // A synced channel that leaves its category keeps the rules it had, as its own.
    place
        .server
        .patch(
            &manager,
            &format!("/api/v1/workspaces/{}/channels/positions", place.workspace),
            &json!({"moves": [{
                "channel_id": other.id,
                "from": {"category_id": first.id, "position": other.position},
                "to": {"category_id": null, "position": other.position},
            }]}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let rules: Value = place
        .server
        .get(&manager, &channel_rules(&other))
        .await
        .ok(StatusCode::OK);
    assert_eq!(rules["synced"], false);
    let held = rule_of(&rules, "role", &guests.to_string()).unwrap();
    assert_eq!(names(held, "deny"), ["add_reactions"]);
}

#[tokio::test]
async fn a_move_that_would_grant_beyond_the_mover_is_refused() {
    let place = place().await;
    let manager = place.server.join(&place.owner, "Manager").await;
    place
        .server
        .grant_workspace(&place.owner, &manager, place.workspace, &[W::ManageChannel])
        .await;
    let first = place
        .server
        .category(&place.owner, place.workspace, "First", 0)
        .await;
    let second = place
        .server
        .category(&place.owner, place.workspace, "Second", 1)
        .await;
    let channel = text_channel(&place, "Notes", Some(&first)).await;
    place
        .server
        .put(
            &place.owner,
            &category_role(&second, place.everyone),
            &json!({"allow": ["manage_messages"], "deny": []}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    place
        .server
        .patch(
            &manager,
            &format!("/api/v1/channels/{}", channel.id),
            &json!({"category_id": second.id}),
        )
        .await
        .error(ErrorCode::ExceedsOwnPermissions);
    place
        .server
        .patch(
            &manager,
            &format!("/api/v1/workspaces/{}/channels/positions", place.workspace),
            &json!({"moves": [{
                "channel_id": channel.id,
                "from": {"category_id": first.id, "position": channel.position},
                "to": {"category_id": second.id, "position": channel.position},
            }]}),
        )
        .await
        .error(ErrorCode::ExceedsOwnPermissions);
    let stored: Option<junto_protocol::ids::CategoryId> =
        sqlx::query_scalar("select category_id from channels where id = $1")
            .bind(channel.id)
            .fetch_one(&place.server.db)
            .await
            .unwrap();
    assert_eq!(stored, Some(first.id));
    place
        .server
        .patch(
            &place.owner,
            &format!("/api/v1/channels/{}", channel.id),
            &json!({"category_id": second.id}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
}

#[tokio::test]
async fn deleting_a_category_tells_its_managers_that_its_channels_detached() {
    let place = place().await;
    let category = place
        .server
        .category(&place.owner, place.workspace, "Plans", 0)
        .await;
    let channel = text_channel(&place, "Notes", Some(&category)).await;
    let mut stream = place.server.subscribed(&place.owner, Level::Full).await;
    place
        .server
        .delete(
            &place.owner,
            &format!(
                "/api/v1/workspaces/{}/categories/{}",
                place.workspace, category.id
            ),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let events = place.server.traffic(&place.owner, &mut stream).await;
    assert!(
        events
            .iter()
            .any(|(_, event)| event.name() == "channel_rules_synced"
                && serde_json::to_value(event).unwrap()["channel_id"] == json!(channel.id)),
        "{events:?}"
    );
}

#[tokio::test]
async fn workspace_person_permissions_follow_rank_and_the_ceiling_in_every_channel() {
    let place = place().await;
    let manager = place.server.join(&place.owner, "Manager").await;
    let reader = place.server.join(&place.owner, "Reader").await;
    let bystander = place.server.join(&place.owner, "Bystander").await;
    let managers = place
        .server
        .grant_workspace(
            &place.owner,
            &manager,
            place.workspace,
            &[W::ManageRoles, W::ManageMessages],
        )
        .await;
    let path = member_permissions(place.workspace, reader.account_id);
    place
        .server
        .get(&bystander, &path)
        .await
        .error(ErrorCode::Forbidden);
    place
        .server
        .put(
            &manager,
            &member_permissions(place.workspace, place.owner.account_id),
            &json!({"allow": [], "deny": ["add_reactions"]}),
        )
        .await
        .error(ErrorCode::Forbidden);
    place
        .server
        .put(
            &manager,
            &member_permissions(place.workspace, manager.account_id),
            &json!({"allow": [], "deny": ["add_reactions"]}),
        )
        .await
        .error(ErrorCode::Forbidden);
    place
        .server
        .put(
            &manager,
            &path,
            &json!({"allow": ["manage_channel"], "deny": []}),
        )
        .await
        .error(ErrorCode::ExceedsOwnPermissions);
    // The manager is denied manage messages in one channel, where the change would add it.
    let channel = text_channel(&place, "Notes", None).await;
    place
        .server
        .put(
            &place.owner,
            &channel_role(&channel, managers),
            &json!({"allow": [], "deny": ["manage_messages"]}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    place
        .server
        .patch(
            &manager,
            &path,
            &json!({"permission": "manage_messages", "state": "allow"}),
        )
        .await
        .error(ErrorCode::ExceedsOwnPermissions);
    // Where the reader does not gain it, the channel does not count.
    place
        .server
        .put(
            &place.owner,
            &channel_person(&channel, reader.account_id),
            &json!({"allow": [], "deny": ["manage_messages"]}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    let mut managing = place.server.subscribed(&manager, Level::Full).await;
    let mut reading = place.server.subscribed(&reader, Level::Full).await;
    let mut watching = place.server.subscribed(&bystander, Level::Full).await;
    let set: Value = place
        .server
        .patch(
            &manager,
            &path,
            &json!({"permission": "manage_messages", "state": "allow"}),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(names(&set, "allow"), ["manage_messages"]);
    let held: Value = place.server.get(&manager, &path).await.ok(StatusCode::OK);
    assert_eq!(held, set);
    let managed = place.server.traffic(&manager, &mut managing).await;
    assert!(
        has_event(&managed, "workspace_person_permissions_set"),
        "{managed:?}"
    );
    let read = place.server.traffic(&reader, &mut reading).await;
    assert!(has_event(&read, "permissions_changed"), "{read:?}");
    assert!(
        !has_event(&read, "workspace_person_permissions_set"),
        "{read:?}"
    );
    let watched = place.server.traffic(&bystander, &mut watching).await;
    assert!(!has_event(&watched, "permissions_changed"), "{watched:?}");
    assert_eq!(
        audit_count(&place.server, "workspace_person_permissions_set").await,
        1
    );
    // The person deny of the workspace takes what a role gives.
    place
        .server
        .put(
            &manager,
            &path,
            &json!({"allow": [], "deny": ["send_messages"]}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    let permissions: Value = place
        .server
        .get(
            &reader,
            &format!("/api/v1/channels/{}/permissions", channel.id),
        )
        .await
        .ok(StatusCode::OK);
    assert!(!names(&permissions, "permissions").contains(&"send_messages".to_owned()));
}

#[tokio::test]
async fn server_person_permissions_follow_the_administrator_and_owner_limits() {
    let place = place().await;
    let higher = place.server.join(&place.owner, "Higher").await;
    let admin = place.server.join(&place.owner, "Admin").await;
    let reader = place.server.join(&place.owner, "Reader").await;
    place
        .server
        .grant_server(&place.owner, &higher, &[S::Administrator])
        .await;
    place
        .server
        .grant_server(&place.owner, &admin, &[S::Administrator])
        .await;
    let path = server_permissions(reader.account_id);
    place
        .server
        .get(&reader, &path)
        .await
        .error(ErrorCode::Forbidden);
    place
        .server
        .get(&admin, &server_permissions(AccountId::generate()))
        .await
        .error(ErrorCode::NotFound);
    place
        .server
        .put(
            &admin,
            &server_permissions(AccountId::generate()),
            &json!({"allow": ["create_invites"], "deny": []}),
        )
        .await
        .error(ErrorCode::NotFound);
    place
        .server
        .put(
            &place.owner,
            &server_permissions(place.owner.account_id),
            &json!({"allow": [], "deny": ["administrator"]}),
        )
        .await
        .error(ErrorCode::InvalidRequest);
    place
        .server
        .put(
            &admin,
            &server_permissions(higher.account_id),
            &json!({"allow": [], "deny": ["create_invites"]}),
        )
        .await
        .error(ErrorCode::Forbidden);
    place
        .server
        .put(
            &higher,
            &server_permissions(admin.account_id),
            &json!({"allow": [], "deny": ["administrator"]}),
        )
        .await
        .error(ErrorCode::OwnerOnly);

    let mut administering = place.server.subscribed(&higher, Level::Full).await;
    let mut reading = place.server.subscribed(&reader, Level::Full).await;
    let set: Value = place
        .server
        .put(
            &admin,
            &path,
            &json!({"allow": ["create_invites"], "deny": []}),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(names(&set, "allow"), ["create_invites"]);
    place
        .server
        .post(&reader, "/api/v1/invites", &json!({}))
        .await
        .ok::<Value>(StatusCode::CREATED);
    let administered = place.server.traffic(&higher, &mut administering).await;
    assert!(
        has_event(&administered, "server_person_permissions_set"),
        "{administered:?}"
    );
    let read = place.server.traffic(&reader, &mut reading).await;
    assert!(has_event(&read, "permissions_changed"), "{read:?}");
    assert!(
        !has_event(&read, "server_person_permissions_set"),
        "{read:?}"
    );
    assert_eq!(
        audit_count(&place.server, "server_person_permissions_set").await,
        1
    );

    // The owner denies the administrator permission of an administrator.
    place
        .server
        .patch(
            &place.owner,
            &server_permissions(admin.account_id),
            &json!({"permission": "administrator", "state": "deny"}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    place
        .server
        .get(&admin, &path)
        .await
        .error(ErrorCode::Forbidden);
}

#[tokio::test]
async fn the_access_of_a_member_lists_only_channels_that_the_caller_sees() {
    let place = place().await;
    let manager = place.server.join(&place.owner, "Manager").await;
    let reader = place.server.join(&place.owner, "Reader").await;
    let bystander = place.server.join(&place.owner, "Bystander").await;
    let managers = place
        .server
        .grant_workspace(&place.owner, &manager, place.workspace, &[W::ManageRoles])
        .await;
    let open = text_channel(&place, "Open", None).await;
    let hidden = text_channel(&place, "Hidden", None).await;
    place
        .server
        .put(
            &place.owner,
            &channel_role(&hidden, managers),
            &json!({"allow": [], "deny": ["view_channel"]}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    place
        .server
        .put(
            &place.owner,
            &channel_person(&open, reader.account_id),
            &json!({"allow": [], "deny": ["send_messages"]}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    let path = format!(
        "/api/v1/workspaces/{}/members/{}/access",
        place.workspace, reader.account_id
    );
    place
        .server
        .get(&bystander, &path)
        .await
        .error(ErrorCode::Forbidden);
    place
        .server
        .get(
            &manager,
            &format!(
                "/api/v1/workspaces/{}/members/{}/access",
                place.workspace,
                AccountId::generate()
            ),
        )
        .await
        .error(ErrorCode::NotFound);
    let listed: Vec<Value> = place.server.get(&manager, &path).await.ok(StatusCode::OK);
    let ids: Vec<&str> = listed
        .iter()
        .map(|held| held["channel_id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&open.id.to_string().as_str()), "{listed:?}");
    assert!(!ids.contains(&hidden.id.to_string().as_str()), "{listed:?}");
    let in_open = listed
        .iter()
        .find(|held| held["channel_id"] == json!(open.id))
        .unwrap();
    let held = names(in_open, "permissions");
    assert!(held.contains(&"view_channel".to_owned()), "{held:?}");
    assert!(!held.contains(&"send_messages".to_owned()), "{held:?}");
    // The owner sees the hidden channel and what the reader holds there.
    let listed: Vec<Value> = place
        .server
        .get(&place.owner, &path)
        .await
        .ok(StatusCode::OK);
    assert!(
        listed
            .iter()
            .any(|held| held["channel_id"] == json!(hidden.id))
    );
}

#[tokio::test]
async fn a_place_holds_a_bounded_number_of_person_rules() {
    let server =
        TestServer::start_with(|config| config.limits.most_person_rules_per_place = 1).await;
    let owner = server.owner().await;
    let first = server.join(&owner, "First").await;
    let second = server.join(&owner, "Second").await;
    let workspace = server.default_workspace(&owner).await.id;
    let category = server.category(&owner, workspace, "Plans", 0).await;
    let channel: Channel = server
        .post(
            &owner,
            &format!("/api/v1/workspaces/{workspace}/channels"),
            &json!({"name": "Notes", "kind": "text"}),
        )
        .await
        .ok(StatusCode::CREATED);
    let deny = json!({"allow": [], "deny": ["add_reactions"]});
    for (first_path, second_path) in [
        (
            channel_person(&channel, first.account_id),
            channel_person(&channel, second.account_id),
        ),
        (
            category_person(&category, first.account_id),
            category_person(&category, second.account_id),
        ),
    ] {
        server
            .put(&owner, &first_path, &deny)
            .await
            .ok::<Value>(StatusCode::OK);
        // Changing the held rule is no new rule.
        server
            .patch(
                &owner,
                &first_path,
                &json!({"permission": "attach_files", "state": "deny"}),
            )
            .await
            .ok::<Value>(StatusCode::OK);
        server
            .put(&owner, &second_path, &deny)
            .await
            .error(ErrorCode::LimitReached);
    }
}
