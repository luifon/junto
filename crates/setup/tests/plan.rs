//! The plan function: what applying a file changes, computed from two documents.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use junto_protocol::ids::StructureId;
use junto_protocol::structure_file::{StructureChange, StructureFile};
use junto_setup::plan::{Planned, plan};
use serde_json::{Value, json};

fn id(number: u32) -> StructureId {
    format!("00000000-0000-7000-8000-{number:012}")
        .parse()
        .unwrap()
}

fn file(value: Value) -> StructureFile {
    serde_json::from_value(value).unwrap()
}

/// A server with a default workspace 1, its everyone role 10, a role 11, a category 20, a
/// text channel 30 in the category, a voice channel 31 with an override of role 11, and a
/// glossary of two terms.
fn server() -> StructureFile {
    file(json!({
        "format": 1,
        "server_roles": [{ "id": id(90), "name": "Inviters", "permissions": ["create_invites"] }],
        "workspaces": [{
            "id": id(1), "name": "Main", "default": true,
            "roles": [
                { "id": id(10), "name": "everyone", "everyone": true,
                  "permissions": ["view_channel", "lock_room", "run_meeting_mode"] },
                { "id": id(11), "name": "Helpers", "permissions": ["manage_glossary"] },
            ],
            "categories": [{ "id": id(20), "name": "Projects", "position": 0 }],
            "channels": [
                { "id": id(30), "name": "general", "kind": "text", "category": id(20),
                  "position": 0, "permanent_messages": false, "overrides": [] },
                { "id": id(31), "name": "hall", "kind": "voice", "category": null,
                  "position": 1, "permanent_messages": false, "user_limit": null,
                  "hides_occupants": false,
                  "overrides": [{ "role": id(11), "allow": ["mute_members"], "deny": [] }] },
            ],
            "glossary": ["Quorvex", "Lumitrax"],
        }],
    }))
}

fn planned(desired: &StructureFile, current: &StructureFile) -> Vec<Planned> {
    let mut next = 900;
    plan(desired, current, &mut || {
        next += 1;
        id(next)
    })
    .unwrap()
}

fn changes(desired: &StructureFile, current: &StructureFile) -> Vec<Value> {
    planned(desired, current)
        .into_iter()
        .map(|planned| serde_json::to_value(planned.change).unwrap())
        .collect()
}

fn problems(desired: Value, current: &StructureFile) -> String {
    let desired = file(desired);
    plan(&desired, current, &mut || id(999))
        .unwrap_err()
        .to_string()
}

#[test]
fn rule_42_the_export_of_a_server_plans_no_change_for_that_server() {
    let current = server();
    assert_eq!(changes(&current, &current), Vec::<Value>::new());
}

#[test]
fn rule_44_an_entry_is_matched_by_its_identifier_so_a_rename_creates_nothing() {
    let current = server();
    let mut desired = current.clone();
    desired.workspaces[0].name = "Renamed".to_owned();
    desired.workspaces[0].channels[0].name = " roadmap ".to_owned();
    desired.workspaces[0].roles[1].name = "Aides".to_owned();
    desired.workspaces[0].categories[0].name = "Work".to_owned();
    desired.server_roles[0].name = "Hosts".to_owned();
    assert_eq!(
        changes(&desired, &current),
        [
            json!({ "change": "update_server_role", "id": id(90), "name": "Hosts" }),
            json!({ "change": "rename_workspace", "id": id(1), "name": "Renamed" }),
            json!({ "change": "update_workspace_role", "id": id(11), "name": "Aides" }),
            json!({ "change": "update_category", "id": id(20), "name": "Work" }),
            json!({ "change": "update_channel", "id": id(30), "name": "roadmap" }),
        ]
    );
    let lines: Vec<String> = planned(&desired, &current)
        .into_iter()
        .map(|planned| planned.line)
        .collect();
    assert_eq!(
        lines[4],
        format!(
            "~ channel \"general\" ({}): name \"general\" -> \"roadmap\"",
            id(30)
        )
    );
}

#[test]
fn rule_45_an_absent_entry_and_an_absent_field_are_kept_and_a_deletion_is_explicit() {
    let current = server();
    // A file with one channel entry and nothing else of the workspace.
    let partial = file(json!({
        "format": 1,
        "workspaces": [{ "id": id(1), "name": "Main",
            "channels": [{ "id": id(31), "name": "hall", "kind": "voice" }] }],
    }));
    assert_eq!(changes(&partial, &current), Vec::<Value>::new());

    let empty = file(json!({ "format": 1 }));
    assert_eq!(changes(&empty, &current), Vec::<Value>::new());

    let deleting = file(json!({ "format": 1, "delete": { "channels": [id(30), id(77)] } }));
    assert_eq!(
        changes(&deleting, &current),
        [json!({ "change": "delete_channel", "id": id(30) })]
    );
    // An override that the file does not state is kept; one with two empty sets is removed.
    let clearing = file(json!({
        "format": 1,
        "workspaces": [{ "id": id(1), "name": "Main", "channels": [
            { "id": id(31), "name": "hall", "kind": "voice",
              "overrides": [{ "role": id(11) }, { "role": id(10) }] }] }],
    }));
    assert_eq!(
        changes(&clearing, &current),
        [
            json!({ "change": "set_channel_override", "channel": id(31), "role": id(11),
                 "allow": [], "deny": [] })
        ]
    );
}

#[test]
fn only_the_fields_that_differ_are_sent() {
    let current = server();
    let desired = file(json!({
        "format": 1,
        "server_roles": [{ "id": id(90), "name": "Inviters",
                           "permissions": ["create_invites", "ban_members"] }],
        "workspaces": [{ "id": id(1), "name": "Main",
            "roles": [{ "id": id(11), "name": "Helpers", "permissions": [] }],
            "categories": [{ "id": id(20), "name": "Projects", "position": 4 }],
            "channels": [
                { "id": id(30), "name": "general", "kind": "text", "category": null,
                  "position": 0, "permanent_messages": true },
                { "id": id(31), "name": "hall", "kind": "voice", "user_limit": 12,
                  "hides_occupants": false,
                  "overrides": [{ "role": id(11), "allow": ["mute_members"],
                                  "deny": ["record_calls"] }] },
            ] }],
    }));
    assert_eq!(
        changes(&desired, &current),
        [
            json!({ "change": "update_server_role", "id": id(90),
                    "permissions": ["create_invites", "ban_members"] }),
            json!({ "change": "update_workspace_role", "id": id(11), "permissions": [] }),
            json!({ "change": "update_category", "id": id(20), "position": 4 }),
            json!({ "change": "update_channel", "id": id(30), "category": null,
                    "permanent_messages": true }),
            json!({ "change": "update_call_settings", "id": id(31), "user_limit": 12 }),
            json!({ "change": "set_channel_override", "channel": id(31), "role": id(11),
                    "allow": ["mute_members"], "deny": ["record_calls"] }),
        ]
    );
}

#[test]
fn rule_51_every_change_names_only_entries_that_exist_before_it() {
    let current = server();
    let desired = file(json!({
        "format": 1,
        "server_roles": [{ "id": id(91), "name": "Moderators", "permissions": ["ban_members"] }],
        "workspaces": [
            { "id": id(1), "name": "Main",
              "roles": [{ "id": id(12), "name": "Guests" }],
              "categories": [{ "id": id(21), "name": "Archive" }],
              "channels": [{ "id": id(32), "name": "old", "kind": "text", "category": id(21),
                             "permanent_messages": true,
                             "overrides": [{ "role": id(12), "deny": ["view_channel"] }] }] },
            { "id": id(2), "name": "Team",
              "roles": [
                  { "id": id(13), "name": "all", "everyone": true, "permissions": ["view_channel"] },
                  { "id": id(14), "name": "Leads", "permissions": ["manage_channel"] },
              ],
              "categories": [{ "id": id(22), "name": "Rooms", "position": 2 }],
              "channels": [{ "id": id(33), "name": "room", "kind": "voice", "category": id(22),
                             "user_limit": 5, "hides_occupants": true,
                             "overrides": [{ "role": id(13), "deny": ["view_channel"] },
                                           { "role": id(14), "allow": ["view_channel"] }] }],
              "glossary": ["Tarnwick"] },
        ],
        "delete": { "channels": [id(30)] },
    }));
    let planned = changes(&desired, &current);
    let names: Vec<&str> = planned
        .iter()
        .map(|change| change["change"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "create_server_role",
            "create_workspace_role",
            "create_category",
            "create_channel",
            "update_channel",
            "set_channel_override",
            "create_workspace",
            "update_workspace_role",
            "create_workspace_role",
            "create_category",
            "create_channel",
            "update_call_settings",
            "set_channel_override",
            "set_channel_override",
            "replace_glossary",
            "delete_channel",
        ]
    );
    assert_eq!(
        planned[6],
        json!({ "change": "create_workspace", "id": id(2), "name": "Team",
                "everyone_role": id(13) })
    );
    let mut known: Vec<Value> = [1, 10, 11, 20, 30, 31, 90]
        .map(|number| json!(id(number)))
        .to_vec();
    for change in &planned {
        let creates = change["change"].as_str().unwrap().starts_with("create_");
        for field in ["workspace", "category", "channel", "role"] {
            if let Some(named) = change.get(field).filter(|named| !named.is_null()) {
                assert!(
                    known.contains(named),
                    "{change} names {field} before it exists"
                );
            }
        }
        if creates {
            known.push(change["id"].clone());
            if let Some(everyone) = change.get("everyone_role") {
                known.push(everyone.clone());
            }
        } else if let Some(named) = change.get("id") {
            assert!(
                known.contains(named),
                "{change} changes an entry before it exists"
            );
        }
    }
}

#[test]
fn a_new_workspace_without_an_everyone_entry_gets_an_identifier_for_that_role() {
    let current = server();
    let desired = file(json!({ "format": 1, "workspaces": [{ "id": id(2), "name": "Team" }] }));
    assert_eq!(
        changes(&desired, &current),
        [
            json!({ "change": "create_workspace", "id": id(2), "name": "Team",
                 "everyone_role": id(901) })
        ]
    );
}

#[test]
fn rule_43_the_file_of_another_server_uses_the_default_workspace_and_its_everyone_role() {
    let current = server();
    // The same structure with the identifiers of another server.
    let foreign = file(json!({
        "format": 1,
        "workspaces": [{
            "id": id(501), "name": "Other name", "default": true,
            "roles": [{ "id": id(510), "name": "everyone", "everyone": true,
                        "permissions": ["view_channel"] }],
            "channels": [{ "id": id(530), "name": "lounge", "kind": "text",
                           "overrides": [{ "role": id(510), "deny": ["mention_everyone"] }] }],
        }],
    }));
    assert_eq!(
        changes(&foreign, &current),
        [
            json!({ "change": "rename_workspace", "id": id(1), "name": "Other name" }),
            json!({ "change": "update_workspace_role", "id": id(10),
                    "permissions": ["view_channel"] }),
            json!({ "change": "create_channel", "id": id(530), "workspace": id(1),
                    "kind": "text", "name": "lounge" }),
            json!({ "change": "set_channel_override", "channel": id(530), "role": id(10),
                    "allow": [], "deny": ["mention_everyone"] }),
        ]
    );

    // After that apply, the server exports the new channel under the identifier of the file,
    // and the same file plans nothing.
    let mut applied = current.clone();
    applied.workspaces[0].name = "Other name".to_owned();
    applied.workspaces[0].roles[0].permissions = foreign.workspaces[0].roles[0].permissions.clone();
    let mut lounge = foreign.workspaces[0].channels[0].clone();
    lounge.overrides[0].role = id(10);
    applied.workspaces[0].channels.push(lounge);
    assert_eq!(changes(&foreign, &applied), Vec::<Value>::new());
}

#[test]
fn rule_48_a_file_that_cannot_be_applied_states_every_problem() {
    let current = server();
    let cases = [
        (json!({ "format": 5 }), "the file has the format 5"),
        (
            json!({ "format": 1, "workspaces": [
                { "id": id(1), "name": "Main" }, { "id": id(1), "name": "Again" }] }),
            "two workspace entries have the identifier",
        ),
        (
            json!({ "format": 1, "workspaces": [
                { "id": id(2), "name": "A", "default": true },
                { "id": id(3), "name": "B", "default": true }] }),
            "two workspaces are marked as the default workspace",
        ),
        (
            json!({ "format": 1, "workspaces": [
                { "id": id(1), "name": "Main" }, { "id": id(2), "name": "B", "default": true }] }),
            "another entry has the identifier of the default workspace",
        ),
        (
            json!({ "format": 1, "workspaces": [{ "id": id(2), "name": "Team",
                "channels": [{ "id": id(30), "name": "general", "kind": "text" }] }] }),
            "does not move to another workspace",
        ),
        (
            json!({ "format": 1, "workspaces": [{ "id": id(1), "name": "Main",
                "channels": [{ "id": id(30), "name": "general", "kind": "voice" }] }] }),
            "the kind of a channel does not change",
        ),
        (
            json!({ "format": 1, "workspaces": [{ "id": id(1), "name": "Main",
                "channels": [{ "id": id(34), "name": "new", "kind": "text", "category": id(29) }] }] }),
            "which is no category of its workspace",
        ),
        (
            json!({ "format": 1, "workspaces": [{ "id": id(1), "name": "Main",
                "channels": [{ "id": id(30), "name": "general", "kind": "text",
                               "overrides": [{ "role": id(19), "allow": ["view_channel"] }] }] }] }),
            "which is no role of its workspace",
        ),
        (
            json!({ "format": 1, "workspaces": [{ "id": id(1), "name": "Main",
                "channels": [{ "id": id(30), "name": "general", "kind": "text",
                               "overrides": [{ "role": id(11), "allow": ["view_channel"],
                                               "deny": ["view_channel"] }] }] }] }),
            "a permission in both allow and deny",
        ),
        (
            json!({ "format": 1, "workspaces": [{ "id": id(1), "name": "Main",
                "channels": [{ "id": id(30), "name": "general", "kind": "text",
                               "user_limit": 4 }] }] }),
            "a setting of voice channels",
        ),
        (
            json!({ "format": 1, "workspaces": [{ "id": id(1), "name": "Main",
                "channels": [{ "id": id(30), "name": "general", "kind": "text" }] }],
                "delete": { "channels": [id(30)] } }),
            "is also listed for deletion",
        ),
        (
            json!({ "format": 1, "workspaces": [{ "id": id(1), "name": "   " }] }),
            "has an empty name",
        ),
        (
            json!({ "format": 1, "workspaces": [{ "id": id(1), "name": "Main",
                "roles": [{ "id": id(11), "name": "Helpers", "everyone": true }] }] }),
            "the server has it as another role",
        ),
        (
            json!({ "format": 1, "workspaces": [{ "id": id(1), "name": "Main",
                "roles": [{ "id": id(10), "name": "a", "everyone": true },
                          { "id": id(15), "name": "b", "everyone": true }] }] }),
            "two roles marked as its everyone role",
        ),
    ];
    for (desired, stated) in cases {
        let found = problems(desired.clone(), &current);
        assert!(found.contains(stated), "{desired}: {found}");
    }
    let several = problems(
        json!({ "format": 5, "workspaces": [{ "id": id(1), "name": "" },
                                             { "id": id(1), "name": "x" }] }),
        &current,
    );
    assert_eq!(several.lines().count(), 3, "{several}");
}

#[test]
fn a_plan_is_the_same_for_the_same_documents() {
    let current = server();
    let mut desired = current.clone();
    desired.workspaces[0].channels[1].position = Some(9);
    let first = planned(&desired, &current);
    assert_eq!(first, planned(&desired, &current));
    assert!(matches!(
        first[0].change,
        StructureChange::UpdateChannel { .. }
    ));
}

#[test]
fn review_4_two_entries_cannot_stand_for_one_role() {
    let current = server();
    // The everyone role under its identifier, and a second entry with the mark.
    let found = problems(
        json!({ "format": 1, "workspaces": [{ "id": id(1), "name": "Main",
            "roles": [{ "id": id(10), "name": "everyone" },
                      { "id": id(16), "name": "all", "everyone": true }] }] }),
        &current,
    );
    assert!(found.contains("stand for the same role"), "{found}");
    // An entry with the mark whose identifier is a role of another workspace.
    let mut two = current.clone();
    two.workspaces.push(
        serde_json::from_value(json!({
            "id": id(2), "name": "Team",
            "roles": [{ "id": id(50), "name": "everyone", "everyone": true, "permissions": [] },
                      { "id": id(51), "name": "Leads", "permissions": [] }],
        }))
        .unwrap(),
    );
    let found = problems(
        json!({ "format": 1, "workspaces": [{ "id": id(1), "name": "Main",
            "roles": [{ "id": id(51), "name": "everyone", "everyone": true }] }] }),
        &two,
    );
    assert!(
        found.contains("does not move to another workspace"),
        "{found}"
    );
}

#[test]
fn review2_two_overrides_of_a_channel_cannot_stand_for_one_role() {
    let current = server();
    // The everyone role under the identifier of the file and under its own identifier.
    let found = problems(
        json!({ "format": 1, "workspaces": [{ "id": id(1), "name": "Main",
            "roles": [{ "id": id(16), "name": "everyone", "everyone": true }],
            "channels": [{ "id": id(30), "name": "general", "kind": "text", "overrides": [
                { "role": id(10), "allow": ["view_channel"] },
                { "role": id(16), "deny": ["view_channel"] }] }] }] }),
        &current,
    );
    assert!(
        found.contains("two overrides that stand for the same role"),
        "{found}"
    );
}

#[test]
fn rule_53_a_file_deletes_only_what_it_lists_in_an_order_that_the_server_takes() {
    let mut current = server();
    current.workspaces.push(
        file(json!({ "format": 1, "workspaces": [{
            "id": id(2), "name": "Side",
            "roles": [{ "id": id(12), "name": "everyone", "everyone": true }],
            "categories": [{ "id": id(21), "name": "Old" }],
            "channels": [{ "id": id(32), "name": "side", "kind": "text" }] }] }))
        .workspaces
        .remove(0),
    );
    let deleting = file(json!({ "format": 1, "delete": {
        "server_roles": [id(90), id(98)],
        "workspaces": [id(2), id(97)],
        "roles": [id(11), id(96)],
        "categories": [id(20), id(95)],
        "channels": [id(31), id(94)],
    } }));
    let planned = planned(&deleting, &current);
    let sent: Vec<Value> = planned
        .iter()
        .map(|planned| serde_json::to_value(&planned.change).unwrap())
        .collect();
    // Channels first, then categories and roles, then workspaces, then server roles. An
    // entry that the server does not have is no change.
    assert_eq!(
        sent,
        [
            json!({ "change": "delete_channel", "id": id(31) }),
            json!({ "change": "delete_category", "id": id(20) }),
            json!({ "change": "delete_workspace_role", "id": id(11) }),
            json!({ "change": "delete_workspace", "id": id(2) }),
            json!({ "change": "delete_server_role", "id": id(90) }),
        ]
    );
    let lines: Vec<&str> = planned
        .iter()
        .map(|planned| planned.line.as_str())
        .collect();
    assert!(lines[1].starts_with("- category \"Projects\""), "{lines:?}");
    assert!(lines[1].contains("its channels stay"), "{lines:?}");
    assert!(lines[2].starts_with("- role \"Helpers\""), "{lines:?}");
    assert!(lines[3].starts_with("- workspace \"Side\""), "{lines:?}");
    assert!(
        lines[3].contains("1 roles, 1 categories and 1 channels"),
        "{lines:?}"
    );
    assert!(
        lines[4].starts_with("- server role \"Inviters\""),
        "{lines:?}"
    );

    // A file that holds an entry lists nothing of it for deletion, and no entry names what
    // the file deletes.
    let contradicting = problems(
        json!({ "format": 1,
            "server_roles": [{ "id": id(90), "name": "Inviters" }],
            "workspaces": [{ "id": id(1), "name": "Main",
                "roles": [{ "id": id(11), "name": "Helpers" }],
                "categories": [{ "id": id(20), "name": "Projects" }],
                "channels": [
                    { "id": id(30), "name": "general", "kind": "text", "category": id(22),
                      "overrides": [{ "role": id(13), "allow": ["view_channel"] }] }] }],
            "delete": { "server_roles": [id(90)], "workspaces": [id(1)], "roles": [id(11), id(13)],
                        "categories": [id(20), id(22), id(22)] } }),
        &current,
    );
    for stated in [
        format!(
            "the server role {} is an entry and is also listed for deletion",
            id(90)
        ),
        format!(
            "the workspace {} is an entry and is also listed for deletion",
            id(1)
        ),
        format!(
            "the role {} is an entry and is also listed for deletion",
            id(11)
        ),
        format!(
            "the category {} is an entry and is also listed for deletion",
            id(20)
        ),
        format!(
            "names the category {}, which is listed for deletion",
            id(22)
        ),
        format!(
            "an override of the role {}, which is listed for deletion",
            id(13)
        ),
        format!("the category {} is listed for deletion twice", id(22)),
    ] {
        assert!(contradicting.contains(&stated), "{stated}\n{contradicting}");
    }

    // The server deletes no everyone role and not its default workspace.
    let refused = problems(
        json!({ "format": 1, "delete": { "roles": [id(10)], "workspaces": [id(1)] } }),
        &current,
    );
    assert!(refused.contains("is the everyone role"), "{refused}");
    assert!(refused.contains("is the default workspace"), "{refused}");
}

#[test]
fn role_order_positions_are_atomic_and_unchanged_roles_are_omitted() {
    let first = "00000000-0000-7000-8000-000000000001";
    let second = "00000000-0000-7000-8000-000000000002";
    let third = "00000000-0000-7000-8000-000000000003";
    let current: junto_protocol::structure_file::StructureFile = serde_json::from_value(serde_json::json!({
        "format":1,"server_roles":[{"id":first,"name":"first","position":3},{"id":second,"name":"second","position":2},{"id":third,"name":"third","position":1}]
    })).unwrap();
    let mut desired = current.clone();
    desired.server_roles[1].position = Some(1);
    desired.server_roles[2].position = Some(2);
    let plan = junto_setup::plan::plan(
        &desired,
        &current,
        &mut junto_protocol::ids::StructureId::generate,
    )
    .unwrap();
    assert_eq!(plan.len(), 1);
    let junto_protocol::structure_file::StructureChange::ReorderServerRoles { positions } =
        &plan[0].change
    else {
        panic!("expected one reorder")
    };
    assert_eq!(positions.len(), 2);
    assert!(positions.iter().all(|p| p.id.to_string() != first));
    assert!(
        junto_setup::plan::plan(
            &desired,
            &desired,
            &mut junto_protocol::ids::StructureId::generate
        )
        .unwrap()
        .is_empty()
    );
    desired.server_roles[0].position = Some(1);
    assert!(
        junto_setup::plan::plan(
            &desired,
            &current,
            &mut junto_protocol::ids::StructureId::generate
        )
        .is_err()
    );
}

/// A server in format 4: the category 20 has an override of role 11, the channel 30 is synced
/// with it, a text channel 32 in it has its own overrides, and a channel 33 has no category.
fn synced_value() -> Value {
    json!({
        "format": 4,
        "workspaces": [{
            "id": id(1), "name": "Main", "default": true,
            "roles": [
                { "id": id(10), "name": "everyone", "everyone": true,
                  "permissions": ["view_channel"] },
                { "id": id(11), "name": "Helpers", "permissions": [] },
            ],
            "categories": [{ "id": id(20), "name": "Projects", "position": 0,
                "overrides": [{ "role": id(11), "allow": ["manage_messages"], "deny": [] }] }],
            "channels": [
                { "id": id(30), "name": "general", "kind": "text", "category": id(20),
                  "position": 0, "permanent_messages": false, "synced": true,
                  "overrides": [] },
                { "id": id(32), "name": "notes", "kind": "text", "category": id(20),
                  "position": 1, "permanent_messages": false, "synced": false,
                  "overrides": [{ "role": id(10), "allow": [], "deny": ["view_channel"] }] },
                { "id": id(33), "name": "loose", "kind": "text", "category": null,
                  "position": 2, "permanent_messages": false, "overrides": [] },
            ],
        }],
    })
}

fn lines(desired: &StructureFile, current: &StructureFile) -> Vec<String> {
    planned(desired, current)
        .into_iter()
        .map(|planned| planned.line)
        .collect()
}

#[test]
fn format_4_an_export_with_category_overrides_and_sync_states_plans_no_change() {
    let current = file(synced_value());
    assert_eq!(changes(&current, &current), Vec::<Value>::new());
    for format in 1..=4 {
        let mut older = current.clone();
        older.format = format;
        assert_eq!(changes(&older, &current), Vec::<Value>::new(), "{format}");
    }
}

#[test]
fn format_4_a_category_override_is_planned_against_the_one_of_the_server() {
    let current = file(synced_value());
    let mut desired = synced_value();
    desired["workspaces"][0]["categories"][0]["overrides"] = json!([
        { "role": id(11), "allow": ["manage_messages"] },
        { "role": id(10), "deny": ["send_messages"] },
    ]);
    let desired_file = file(desired.clone());
    assert_eq!(
        changes(&desired_file, &current),
        [
            json!({ "change": "set_category_override", "category": id(20), "role": id(10),
                 "allow": [], "deny": ["send_messages"] })
        ]
    );
    assert_eq!(
        lines(&desired_file, &current),
        [format!(
            "~ override of the role {} in the category \"Projects\" ({}): allow none, deny \
             send_messages",
            id(10),
            id(20)
        )]
    );
    desired["workspaces"][0]["categories"][0]["overrides"] = json!([{ "role": id(11) }]);
    assert_eq!(
        changes(&file(desired), &current),
        [
            json!({ "change": "set_category_override", "category": id(20), "role": id(11),
                 "allow": [], "deny": [] })
        ]
    );
}

#[test]
fn format_4_a_channel_is_synced_again_or_keeps_its_own_overrides() {
    let current = file(synced_value());
    let mut desired = synced_value();
    // The channel with its own overrides is synced again: one change, and no override.
    desired["workspaces"][0]["channels"][1]["synced"] = json!(true);
    desired["workspaces"][0]["channels"][1]["overrides"] = json!([]);
    // The synced channel keeps the override of the category and adds one of its own.
    desired["workspaces"][0]["channels"][0]["synced"] = json!(false);
    desired["workspaces"][0]["channels"][0]["overrides"] = json!([
        { "role": id(11), "allow": ["manage_messages"] },
        { "role": id(10), "deny": ["attach_files"] },
    ]);
    let desired_file = file(desired.clone());
    assert_eq!(
        changes(&desired_file, &current),
        [
            json!({ "change": "set_channel_sync", "channel": id(30), "synced": false }),
            json!({ "change": "set_channel_override", "channel": id(30), "role": id(10),
                    "allow": [], "deny": ["attach_files"] }),
            json!({ "change": "set_channel_sync", "channel": id(32), "synced": true }),
        ]
    );
    let lines = lines(&desired_file, &current);
    assert!(
        lines[0].contains("stop the sync with its category"),
        "{lines:?}"
    );
    assert!(
        lines[2].contains("the rules of people in it are replaced"),
        "{lines:?}"
    );

    // An override that the category gives and that the entry does not state is removed
    // after the channel stops the sync, so that the channel has the overrides of the entry.
    desired["workspaces"][0]["channels"][0]["overrides"] =
        json!([{ "role": id(10), "deny": ["attach_files"] }]);
    let planned = changes(&file(desired), &current);
    assert_eq!(
        planned[..3],
        [
            json!({ "change": "set_channel_sync", "channel": id(30), "synced": false }),
            json!({ "change": "set_channel_override", "channel": id(30), "role": id(10),
                    "allow": [], "deny": ["attach_files"] }),
            json!({ "change": "set_channel_override", "channel": id(30), "role": id(11),
                    "allow": [], "deny": [] }),
        ]
    );
}

#[test]
fn format_4_a_new_channel_in_a_category_is_synced_unless_its_entry_says_otherwise() {
    let current = file(synced_value());
    let mut desired = synced_value();
    desired["workspaces"][0]["channels"] = json!([
        { "id": id(40), "name": "taken", "kind": "text", "category": id(20), "synced": true },
        { "id": id(41), "name": "apart", "kind": "text", "category": id(20), "synced": false,
          "overrides": [{ "role": id(10), "deny": ["view_channel"] }] },
    ]);
    assert_eq!(
        changes(&file(desired), &current),
        [
            json!({ "change": "create_channel", "id": id(40), "workspace": id(1),
                    "kind": "text", "name": "taken", "category": id(20) }),
            json!({ "change": "create_channel", "id": id(41), "workspace": id(1),
                    "kind": "text", "name": "apart", "category": id(20) }),
            json!({ "change": "set_channel_sync", "channel": id(41), "synced": false }),
            json!({ "change": "set_channel_override", "channel": id(41), "role": id(10),
                    "allow": [], "deny": ["view_channel"] }),
            json!({ "change": "set_channel_override", "channel": id(41), "role": id(11),
                    "allow": [], "deny": [] }),
        ]
    );
}

#[test]
fn format_4_a_channel_without_a_category_that_moves_into_one_states_every_override() {
    let current = file(synced_value());
    let mut desired = synced_value();
    desired["workspaces"][0]["channels"] = json!([
        { "id": id(33), "name": "loose", "kind": "text", "category": id(20), "synced": false,
          "overrides": [{ "role": id(10), "allow": ["attach_files"] }] },
    ]);
    // The export does not tell whether the channel takes the overrides of the category when
    // it moves, so every override of the entry is sent; the server does not change an
    // override that it holds already.
    assert_eq!(
        changes(&file(desired), &current),
        [
            json!({ "change": "update_channel", "id": id(33), "category": id(20) }),
            json!({ "change": "set_channel_sync", "channel": id(33), "synced": false }),
            json!({ "change": "set_channel_override", "channel": id(33), "role": id(10),
                    "allow": ["attach_files"], "deny": [] }),
            json!({ "change": "set_channel_override", "channel": id(33), "role": id(11),
                    "allow": [], "deny": [] }),
        ]
    );
}

#[test]
fn format_4_a_format_2_override_of_a_synced_channel_is_compared_with_its_category() {
    let current = file(synced_value());
    let mut desired = synced_value();
    desired["format"] = json!(2);
    desired["workspaces"][0]["channels"][0]
        .as_object_mut()
        .unwrap()
        .remove("synced");
    desired["workspaces"][0]["channels"][0]["overrides"] =
        json!([{ "role": id(11), "allow": ["manage_messages"] }]);
    assert_eq!(
        changes(&file(desired.clone()), &current),
        Vec::<Value>::new()
    );
    desired["workspaces"][0]["channels"][0]["overrides"] =
        json!([{ "role": id(11), "allow": ["mention_everyone"] }]);
    assert_eq!(
        changes(&file(desired), &current),
        [
            json!({ "change": "set_channel_override", "channel": id(30), "role": id(11),
                 "allow": ["mention_everyone"], "deny": [] })
        ]
    );
}

#[test]
fn format_4_a_file_with_a_wrong_sync_state_or_category_override_is_refused() {
    let current = file(synced_value());
    let cases = [
        (
            json!({ "format": 4, "workspaces": [{ "id": id(1), "name": "Main",
                "channels": [{ "id": id(30), "name": "general", "kind": "text",
                    "synced": true, "overrides": [{ "role": id(11), "allow": ["view_channel"] }] }] }] }),
            "is synced with its category and states overrides",
        ),
        (
            json!({ "format": 4, "workspaces": [{ "id": id(1), "name": "Main",
                "channels": [{ "id": id(30), "name": "general", "kind": "text",
                    "category": null, "synced": true }] }] }),
            "is synced with its category and has no category",
        ),
        (
            json!({ "format": 4, "workspaces": [{ "id": id(1), "name": "Main",
                "channels": [{ "id": id(33), "name": "loose", "kind": "text",
                    "synced": true }] }] }),
            "is synced with its category and has no category",
        ),
        (
            json!({ "format": 4, "workspaces": [{ "id": id(1), "name": "Main",
                "categories": [{ "id": id(20), "name": "Projects",
                    "overrides": [{ "role": id(19), "allow": ["view_channel"] }] }] }] }),
            "which is no role of its workspace",
        ),
        (
            json!({ "format": 4, "workspaces": [{ "id": id(1), "name": "Main",
                "categories": [{ "id": id(20), "name": "Projects",
                    "overrides": [{ "role": id(11), "allow": ["view_channel"],
                                    "deny": ["view_channel"] }] }] }] }),
            "in the category",
        ),
        (
            json!({ "format": 4, "workspaces": [{ "id": id(1), "name": "Main",
                "categories": [{ "id": id(20), "name": "Projects",
                    "overrides": [{ "role": id(11), "allow": ["view_channel"] },
                                  { "role": id(11), "deny": ["view_channel"] }] }] }] }),
            "two overrides of the role",
        ),
        (
            json!({ "format": 4, "workspaces": [{ "id": id(1), "name": "Main",
                "categories": [{ "id": id(20), "name": "Projects",
                    "overrides": [{ "role": id(11), "allow": ["view_channel"] }] }] }],
                "delete": { "roles": [id(11)] } }),
            "which is listed for deletion",
        ),
    ];
    for (desired, stated) in cases {
        let found = problems(desired.clone(), &current);
        assert!(found.contains(stated), "{desired}: {found}");
    }
    for format in [3, 4] {
        let accepted = file(json!({ "format": format }));
        assert_eq!(changes(&accepted, &current), Vec::<Value>::new());
    }
    assert!(problems(json!({ "format": 5 }), &current).contains("the formats 1 to 4"));
}
#[test]
fn a_glossary_that_differs_replaces_the_whole_glossary_after_the_channels_of_its_workspace() {
    let current = server();
    let desired = file(json!({
        "format": 3,
        "workspaces": [
            { "id": id(1), "name": "Main",
              "channels": [{ "id": id(34), "name": "notes", "kind": "text" }],
              "glossary": [" Lumitrax ", "Ysolde Varkonen"] },
            { "id": id(2), "name": "Team", "glossary": ["Tarnwick", "Quorvex"] },
        ],
    }));
    assert_eq!(
        changes(&desired, &current),
        [
            json!({ "change": "create_channel", "id": id(34), "workspace": id(1),
                    "kind": "text", "name": "notes" }),
            json!({ "change": "replace_glossary", "workspace": id(1),
                    "terms": ["Lumitrax", "Ysolde Varkonen"] }),
            json!({ "change": "create_workspace", "id": id(2), "name": "Team",
                    "everyone_role": id(901) }),
            json!({ "change": "replace_glossary", "workspace": id(2),
                    "terms": ["Tarnwick", "Quorvex"] }),
        ]
    );
    let lines: Vec<String> = planned(&desired, &current)
        .into_iter()
        .map(|planned| planned.line)
        .collect();
    assert_eq!(
        lines[1],
        format!(
            "~ glossary of the workspace \"Main\" ({}): add \"Ysolde Varkonen\"; remove \
             \"Quorvex\"; terms in order of priority: \"Lumitrax\", \"Ysolde Varkonen\"",
            id(1)
        )
    );
    assert_eq!(
        lines[3],
        format!(
            "~ glossary of the workspace \"Team\" ({}): add \"Tarnwick\", \"Quorvex\"; remove \
             none; terms in order of priority: \"Tarnwick\", \"Quorvex\"",
            id(2)
        )
    );

    // A new order of priority is a change; the same terms with space around them are not.
    let reordered = file(json!({ "format": 3, "workspaces": [
        { "id": id(1), "name": "Main", "glossary": ["Lumitrax", "Quorvex"] }] }));
    assert_eq!(
        changes(&reordered, &current),
        [json!({ "change": "replace_glossary", "workspace": id(1),
                 "terms": ["Lumitrax", "Quorvex"] })]
    );
    let lines: Vec<String> = planned(&reordered, &current)
        .into_iter()
        .map(|planned| planned.line)
        .collect();
    assert!(
        lines[0].contains("add none; remove none; terms in order of priority"),
        "{lines:?}"
    );
    let spaced = file(json!({ "format": 3, "workspaces": [
        { "id": id(1), "name": "Main", "glossary": [" Quorvex", "Lumitrax "] }] }));
    assert_eq!(changes(&spaced, &current), Vec::<Value>::new());
}

#[test]
fn an_absent_glossary_is_kept_and_an_empty_one_removes_every_term() {
    let current = server();
    let absent = file(json!({ "format": 3, "workspaces": [
        { "id": id(1), "name": "Main" }, { "id": id(2), "name": "Team" }] }));
    assert_eq!(
        changes(&absent, &current),
        [
            json!({ "change": "create_workspace", "id": id(2), "name": "Team",
                 "everyone_role": id(901) })
        ]
    );
    let empty = file(json!({ "format": 3, "workspaces": [
        { "id": id(1), "name": "Main", "glossary": [] },
        { "id": id(2), "name": "Team", "glossary": [] }] }));
    assert_eq!(
        changes(&empty, &current),
        [
            json!({ "change": "replace_glossary", "workspace": id(1), "terms": [] }),
            json!({ "change": "create_workspace", "id": id(2), "name": "Team",
                    "everyone_role": id(901) }),
        ]
    );
    // The file of another server sets the glossary of the default workspace of this one.
    let foreign = file(json!({ "format": 3, "workspaces": [
        { "id": id(501), "name": "Main", "default": true, "glossary": ["Tarnwick"] }] }));
    assert_eq!(
        changes(&foreign, &current),
        [json!({ "change": "replace_glossary", "workspace": id(1), "terms": ["Tarnwick"] })]
    );
}
