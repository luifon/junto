//! The setup tool against a server (ADR 0204, ADR 0212). The commands run in this process
//! through the library of the tool; the binary only passes its arguments to it.

use junto_protocol::access_token::TokenAction;
use junto_protocol::structure::Workspace;
use junto_protocol::structure_file::{StructureExport, StructureFile};
use junto_protocol::transcript::Glossary;
use junto_setup::cli::{FAILURE, Invocation, SUCCESS, USAGE_ERROR, run};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::support::{Member, Scratch, TestServer};

pub struct Ran {
    pub code: u8,
    pub out: String,
    pub err: String,
}

/// Runs one command of the tool against `server` with `token`.
pub async fn tool(server: &TestServer, token: &str, arguments: &[&str]) -> Ran {
    let invocation = Invocation {
        arguments: arguments
            .iter()
            .map(|argument| (*argument).to_owned())
            .collect(),
        token: Some(token.to_owned()),
        server: Some(server.base.clone()),
    };
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = run(invocation, &mut out, &mut err).await;
    Ran {
        code,
        out: String::from_utf8(out).unwrap(),
        err: String::from_utf8(err).unwrap(),
    }
}

impl Ran {
    #[track_caller]
    pub fn succeeded(self) -> Self {
        assert_eq!(self.code, SUCCESS, "out: {}\nerr: {}", self.out, self.err);
        self
    }

    #[track_caller]
    pub fn failed(self, code: u8) -> Self {
        assert_eq!(self.code, code, "out: {}\nerr: {}", self.out, self.err);
        self
    }
}

async fn full_token(server: &TestServer, owner: &Member) -> String {
    let actions = [TokenAction::ReadStructure, TokenAction::ManageStructure];
    let created = server.access_token(owner, &actions).await;
    created.token.expose().to_owned()
}

/// One text that changes when the structure, the audit log or the event log changes.
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
        "glossary_terms",
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

fn write(scratch: &Scratch, name: &str, content: &Value) -> String {
    let path = scratch.0.join(name);
    std::fs::write(&path, serde_json::to_string_pretty(content).unwrap()).unwrap();
    path.to_str().unwrap().to_owned()
}

fn id(number: u32) -> String {
    format!("00000000-0000-7000-8000-{number:012}")
}

/// A file that adds a role, a category, two channels and a second workspace to a server
/// whose default workspace has the identifier `main`.
fn additions(main: &str) -> Value {
    json!({
        "format": 1,
        "server_roles": [{ "id": id(90), "name": "Inviters", "permissions": ["create_invites"] }],
        "workspaces": [
            { "id": main, "name": "Main", "default": true,
              "roles": [{ "id": id(11), "name": "Helpers", "permissions": ["manage_glossary"] }],
              "categories": [{ "id": id(20), "name": "Projects", "position": 2 }],
              "channels": [
                  { "id": id(30), "name": "plans", "kind": "text", "category": id(20),
                    "permanent_messages": true,
                    "overrides": [{ "role": id(11), "allow": ["manage_messages"] }] },
                  { "id": id(31), "name": "hall", "kind": "voice", "user_limit": 6,
                    "hides_occupants": true },
              ] },
            { "id": id(2), "name": "Team",
              "roles": [{ "id": id(13), "name": "all", "everyone": true,
                          "permissions": ["view_channel"] }],
              "channels": [{ "id": id(33), "name": "lobby", "kind": "text",
                             "overrides": [{ "role": id(13), "deny": ["mention_everyone"] }] }] },
        ],
    })
}

#[tokio::test]
async fn rule_40_export_writes_the_structure_of_the_server() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let token = full_token(&server, &owner).await;
    let scratch = Scratch::new();
    let before = state(&server).await;

    let printed = tool(&server, &token, &["export"]).await.succeeded();
    let file: StructureFile = serde_json::from_str(&printed.out).unwrap();
    let export: StructureExport = server
        .get(&owner, "/api/v1/structure")
        .await
        .ok(StatusCode::OK);
    assert_eq!(file, export.structure);
    assert_eq!(file.format, 4);
    assert_eq!(file.workspaces.len(), 1);
    assert_eq!(file.workspaces[0].channels.len(), 2);
    assert!(
        printed
            .err
            .contains(&format!("at the revision {}", export.revision))
    );

    let path = scratch.0.join("structure.json");
    let path = path.to_str().unwrap();
    let written = tool(&server, &token, &["export", "--out", path])
        .await
        .succeeded();
    assert_eq!(written.out, "");
    assert_eq!(std::fs::read_to_string(path).unwrap(), printed.out);
    assert_eq!(state(&server).await, before);
    // The file holds no member, no account and no secret.
    for absent in ["account", "member", "token", &owner.account_id.to_string()] {
        assert!(!printed.out.contains(absent), "{absent}");
    }
}

#[tokio::test]
async fn rule_41_42_plan_changes_nothing_and_a_second_apply_changes_nothing() {
    let server = TestServer::start_for_calls().await;
    let owner = server.owner().await;
    let token = full_token(&server, &owner).await;
    let scratch = Scratch::new();
    let main = server.default_workspace(&owner).await;
    let path = write(&scratch, "structure.json", &additions(&main.id.to_string()));

    let before = state(&server).await;
    let planned = tool(&server, &token, &["plan", &path]).await.succeeded();
    assert_eq!(state(&server).await, before);
    assert!(planned.out.contains("13 changes"), "{}", planned.out);
    for line in [
        "+ server role \"Inviters\"",
        "+ role \"Helpers\"",
        "+ category \"Projects\"",
        "+ text channel \"plans\"",
        "permanent messages on",
        "+ voice channel \"hall\"",
        "user limit 6, hides occupants true",
        "allow manage_messages, deny none",
        "+ workspace \"Team\"",
        "the everyone role of the new workspace",
        "+ text channel \"lobby\"",
        "allow none, deny mention_everyone",
    ] {
        assert!(
            planned.out.contains(line),
            "{line} is not in:\n{}",
            planned.out
        );
    }

    let applied = tool(&server, &token, &["apply", &path]).await.succeeded();
    assert!(
        applied.out.contains("Applied 13 changes."),
        "{}",
        applied.out
    );
    assert_ne!(state(&server).await, before);
    let export: StructureExport = server
        .get(&owner, "/api/v1/structure")
        .await
        .ok(StatusCode::OK);
    let team = export
        .structure
        .workspaces
        .iter()
        .find(|entry| entry.name == "Team")
        .unwrap();
    assert_eq!(team.id.to_string(), id(2));
    assert_eq!(team.roles[0].id.to_string(), id(13));
    assert_eq!(team.channels[0].overrides[0].role.to_string(), id(13));
    let main_entry = export
        .structure
        .workspaces
        .iter()
        .find(|entry| entry.default)
        .unwrap();
    assert_eq!(main_entry.channels.len(), 4);
    let hall = main_entry
        .channels
        .iter()
        .find(|entry| entry.name == "hall")
        .unwrap();
    assert_eq!(
        (hall.user_limit, hall.hides_occupants),
        (Some(Some(6)), Some(true))
    );

    let after = state(&server).await;
    for command in ["plan", "apply"] {
        let again = tool(&server, &token, &[command, &path]).await.succeeded();
        assert!(again.out.starts_with("No changes:"), "{}", again.out);
        assert_eq!(state(&server).await, after, "{command}");
    }
}

#[tokio::test]
async fn rule_43_the_export_of_one_server_is_applied_to_an_empty_server() {
    let source = TestServer::start_for_calls().await;
    let source_owner = source.owner().await;
    let source_token = full_token(&source, &source_owner).await;
    let scratch = Scratch::new();
    let main = source.default_workspace(&source_owner).await;
    let first = write(&scratch, "first.json", &additions(&main.id.to_string()));
    tool(&source, &source_token, &["apply", &first])
        .await
        .succeeded();
    let exported = scratch.0.join("exported.json");
    let exported = exported.to_str().unwrap();
    tool(&source, &source_token, &["export", "--out", exported])
        .await
        .succeeded();

    let target = TestServer::start_for_calls().await;
    let target_owner = target.owner().await;
    let target_token = full_token(&target, &target_owner).await;
    let before = state(&target).await;
    let planned = tool(&target, &target_token, &["plan", exported])
        .await
        .succeeded();
    assert_eq!(state(&target).await, before);
    assert!(planned.out.contains("changes"), "{}", planned.out);
    tool(&target, &target_token, &["apply", exported])
        .await
        .succeeded();

    let after = state(&target).await;
    let again = tool(&target, &target_token, &["apply", exported])
        .await
        .succeeded();
    assert!(again.out.starts_with("No changes:"), "{}", again.out);
    assert_eq!(state(&target).await, after);

    // The target has the entries of the source under the identifiers of the file, in its own
    // default workspace, and keeps its own two first channels.
    let source_file: StructureFile =
        serde_json::from_str(&std::fs::read_to_string(exported).unwrap()).unwrap();
    let target_export: StructureExport = target
        .get(&target_owner, "/api/v1/structure")
        .await
        .ok(StatusCode::OK);
    let target_file = target_export.structure;
    assert_eq!(target_file.workspaces.len(), 2);
    assert_eq!(target_file.server_roles, source_file.server_roles);
    let default_of = |file: &StructureFile| {
        file.workspaces
            .iter()
            .find(|entry| entry.default)
            .unwrap()
            .clone()
    };
    let (source_main, target_main) = (default_of(&source_file), default_of(&target_file));
    assert_ne!(target_main.id, source_main.id);
    assert_eq!(target_main.name, source_main.name);
    assert_eq!(target_main.channels.len(), source_main.channels.len() + 2);
    for channel in &source_main.channels {
        let copy = target_main
            .channels
            .iter()
            .find(|entry| entry.id == channel.id)
            .unwrap();
        assert_eq!((&copy.name, copy.kind), (&channel.name, channel.kind));
        assert_eq!(copy.permanent_messages, channel.permanent_messages);
        assert_eq!(
            (copy.user_limit, copy.hides_occupants),
            (channel.user_limit, channel.hides_occupants)
        );
        assert_eq!(copy.category, channel.category);
        assert_eq!(copy.overrides.len(), channel.overrides.len());
    }
    assert_eq!(target_main.categories, source_main.categories);
    let helpers = |entry: &junto_protocol::structure_file::WorkspaceEntry| {
        entry
            .roles
            .iter()
            .find(|role| role.name == "Helpers")
            .cloned()
            .unwrap()
    };
    assert_eq!(helpers(&target_main), helpers(&source_main));
    let team = |file: &StructureFile| {
        file.workspaces
            .iter()
            .find(|entry| !entry.default)
            .unwrap()
            .clone()
    };
    assert_eq!(team(&target_file), team(&source_file));
    // The identifiers of the objects are the ones that the target generated.
    let workspaces: Vec<Workspace> = target
        .get(&target_owner, "/api/v1/workspaces")
        .await
        .ok(StatusCode::OK);
    assert!(
        workspaces
            .iter()
            .all(|workspace| workspace.id.to_string() != id(2))
    );
}

#[tokio::test]
async fn rule_44_45_a_rename_keeps_the_entry_and_only_a_listed_channel_is_deleted() {
    let server = TestServer::start_for_calls().await;
    let owner = server.owner().await;
    let token = full_token(&server, &owner).await;
    let scratch = Scratch::new();
    let main = server.default_workspace(&owner).await;
    let main_id = main.id.to_string();
    let path = write(&scratch, "structure.json", &additions(&main_id));
    tool(&server, &token, &["apply", &path]).await.succeeded();
    let channels = || async {
        sqlx::query_scalar::<_, i64>("select count(*) from channels where deleted_at is null")
            .fetch_one(&server.db)
            .await
            .unwrap()
    };
    let before = channels().await;

    let smaller = json!({
        "format": 1,
        "workspaces": [{ "id": main_id, "name": "Main",
            "channels": [{ "id": id(30), "name": "roadmap", "kind": "text" }] }],
    });
    let path = write(&scratch, "smaller.json", &smaller);
    let renamed = tool(&server, &token, &["apply", &path]).await.succeeded();
    assert!(renamed.out.contains("1 change"), "{}", renamed.out);
    assert!(
        renamed.out.contains("name \"plans\" -> \"roadmap\""),
        "{}",
        renamed.out
    );
    assert_eq!(channels().await, before);

    let deleting = json!({ "format": 1, "delete": { "channels": [id(31)] } });
    let path = write(&scratch, "deleting.json", &deleting);
    let deleted = tool(&server, &token, &["apply", &path]).await.succeeded();
    assert!(
        deleted.out.contains("- channel \"hall\""),
        "{}",
        deleted.out
    );
    assert_eq!(channels().await, before - 1);
    let again = tool(&server, &token, &["apply", &path]).await.succeeded();
    assert!(again.out.starts_with("No changes:"), "{}", again.out);
}

#[tokio::test]
async fn rule_53_a_file_deletes_only_what_it_lists() {
    let server = TestServer::start_for_calls().await;
    let owner = server.owner().await;
    let token = full_token(&server, &owner).await;
    let scratch = Scratch::new();
    let main = server.default_workspace(&owner).await;
    let main_id = main.id.to_string();
    let path = write(&scratch, "structure.json", &additions(&main_id));
    tool(&server, &token, &["apply", &path]).await.succeeded();
    let side = json!({ "format": 1, "workspaces": [{ "id": id(2), "name": "Side",
        "channels": [{ "id": id(40), "name": "side", "kind": "text" }] }] });
    let path = write(&scratch, "side.json", &side);
    tool(&server, &token, &["apply", &path]).await.succeeded();
    let export = server.structure(&owner).await;
    assert_eq!(export.structure.workspaces.len(), 2);
    let named = |export: &junto_protocol::structure_file::StructureExport| {
        let main = export
            .structure
            .workspaces
            .iter()
            .find(|workspace| workspace.default)
            .unwrap();
        (
            export.structure.server_roles.len(),
            main.roles.len(),
            main.categories.len(),
            main.channels.len(),
        )
    };
    let (server_roles, roles, categories, channels) = named(&export);
    assert!(server_roles >= 1 && roles >= 2 && categories >= 1);

    // The file names one entry of each kind. `plan` changes nothing.
    let deleting = json!({ "format": 1, "delete": {
        "server_roles": [export.structure.server_roles[0].id],
        "roles": [id(11)],
        "categories": [id(20)],
        "workspaces": [id(2)],
    } });
    let path = write(&scratch, "deleting.json", &deleting);
    let before = state(&server).await;
    let planned = tool(&server, &token, &["plan", &path]).await.succeeded();
    for line in [
        "- category ",
        "- role ",
        "- workspace \"Side\"",
        "- server role ",
    ] {
        assert!(planned.out.contains(line), "{line}: {}", planned.out);
    }
    assert_eq!(state(&server).await, before);

    let applied = tool(&server, &token, &["apply", &path]).await.succeeded();
    assert!(applied.out.contains("4 change"), "{}", applied.out);
    let export = server.structure(&owner).await;
    assert_eq!(export.structure.workspaces.len(), 1);
    assert_eq!(
        named(&export),
        (server_roles - 1, roles - 1, categories - 1, channels)
    );
    let again = tool(&server, &token, &["apply", &path]).await.succeeded();
    assert!(again.out.starts_with("No changes:"), "{}", again.out);

    // The tool refuses a file that deletes the default workspace, before any request that
    // changes the server.
    let default = json!({ "format": 1, "delete": { "workspaces": [main_id] } });
    let path = write(&scratch, "default.json", &default);
    let before = state(&server).await;
    let refused = tool(&server, &token, &["apply", &path])
        .await
        .failed(FAILURE);
    assert!(refused.err.contains("default workspace"), "{}", refused.err);
    assert_eq!(state(&server).await, before);
}

#[tokio::test]
async fn rule_46_a_plan_of_an_earlier_revision_is_refused() {
    let server = TestServer::start_for_calls().await;
    let owner = server.owner().await;
    let token = full_token(&server, &owner).await;
    let scratch = Scratch::new();
    let main = server.default_workspace(&owner).await;
    let path = write(&scratch, "structure.json", &additions(&main.id.to_string()));
    let plan = scratch.0.join("plan.json");
    let plan = plan.to_str().unwrap();

    tool(&server, &token, &["plan", &path, "--out", plan])
        .await
        .succeeded();
    let stored: Value = serde_json::from_str(&std::fs::read_to_string(plan).unwrap()).unwrap();
    assert_eq!(stored["server"], server.base);
    assert_eq!(stored["changes"].as_array().unwrap().len(), 13);
    let revision = stored["revision"].as_str().unwrap().to_owned();

    server.channel(&owner, main.id, "made-after-the-plan").await;
    let before = state(&server).await;
    let refused = tool(&server, &token, &["apply", "--plan", plan])
        .await
        .failed(FAILURE);
    assert!(refused.err.contains("conflict"), "{}", refused.err);
    assert!(
        refused.err.contains("applied none of the 13 changes"),
        "{}",
        refused.err
    );
    assert!(refused.err.contains("Run `plan` again"), "{}", refused.err);
    assert_eq!(state(&server).await, before);

    // A plan of the current revision is applied, and it is applied once.
    tool(&server, &token, &["plan", &path, "--out", plan])
        .await
        .succeeded();
    let stored: Value = serde_json::from_str(&std::fs::read_to_string(plan).unwrap()).unwrap();
    assert_ne!(stored["revision"].as_str().unwrap(), revision);
    tool(&server, &token, &["apply", "--plan", plan])
        .await
        .succeeded();
    let after = state(&server).await;
    tool(&server, &token, &["apply", "--plan", plan])
        .await
        .failed(FAILURE);
    assert_eq!(state(&server).await, after);

    // A plan names its server.
    let other = TestServer::start().await;
    let other_owner = other.owner().await;
    let other_token = full_token(&other, &other_owner).await;
    let before = state(&other).await;
    let wrong = tool(&other, &other_token, &["apply", "--plan", plan])
        .await
        .failed(FAILURE);
    assert!(wrong.err.contains("was computed for"), "{}", wrong.err);
    assert_eq!(state(&other).await, before);
}

#[tokio::test]
async fn rule_47_a_failed_apply_states_that_nothing_was_applied() {
    let server = TestServer::start_for_calls().await;
    let owner = server.owner().await;
    let token = full_token(&server, &owner).await;
    let scratch = Scratch::new();
    let main = server.default_workspace(&owner).await;
    let records = server.channel(&owner, main.id, "records").await;
    sqlx::query("update channels set permanent_message_count = 1 where id = $1")
        .bind(records.id)
        .execute(&server.db)
        .await
        .unwrap();
    // The last change is refused: only the owner deletes a channel with permanent messages,
    // and a token is never the owner.
    let mut file = additions(&main.id.to_string());
    file["delete"] = json!({ "channels": [records.id] });
    let path = write(&scratch, "structure.json", &file);
    let before = state(&server).await;
    let failed = tool(&server, &token, &["apply", &path])
        .await
        .failed(FAILURE);
    assert!(failed.err.contains("owner_only"), "{}", failed.err);
    assert!(
        failed.err.contains("change 14 (delete_channel)"),
        "{}",
        failed.err
    );
    assert!(
        failed
            .err
            .contains("The server applied none of the 14 changes."),
        "{}",
        failed.err
    );
    assert_eq!(state(&server).await, before);

    // A token that only reads exports and plans, and its apply is refused whole.
    let reading = server
        .access_token(&owner, &[TokenAction::ReadStructure])
        .await;
    let reading = reading.token.expose();
    let path = write(&scratch, "additions.json", &additions(&main.id.to_string()));
    tool(&server, reading, &["export"]).await.succeeded();
    tool(&server, reading, &["plan", &path]).await.succeeded();
    let before = state(&server).await;
    let refused = tool(&server, reading, &["apply", &path])
        .await
        .failed(FAILURE);
    assert!(refused.err.contains("forbidden"), "{}", refused.err);
    assert!(refused.err.contains("manage_structure"), "{}", refused.err);
    assert!(
        refused.err.contains("applied none of the 13 changes"),
        "{}",
        refused.err
    );
    assert_eq!(state(&server).await, before);
}

#[tokio::test]
async fn rule_48_a_file_that_cannot_be_applied_is_refused_before_any_change() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let token = full_token(&server, &owner).await;
    let scratch = Scratch::new();
    let main = server.default_workspace(&owner).await.id.to_string();
    let before = state(&server).await;
    let cases = [
        (json!({ "format": 5 }), "the file has the format 5"),
        (json!({ "format": 1, "workspace": [] }), "unknown field"),
        (json!({ "workspaces": [] }), "missing field `format`"),
        (
            json!({ "format": 1, "workspaces": [{ "id": main, "name": "Main", "channels": [
                { "id": id(30), "name": "a", "kind": "text" },
                { "id": id(30), "name": "b", "kind": "text" }] }] }),
            "two channel entries have the identifier",
        ),
        (
            json!({ "format": 1, "workspaces": [{ "id": main, "name": "Main", "channels": [
                { "id": id(30), "name": "a", "kind": "video" }] }] }),
            "unknown variant",
        ),
    ];
    for (file, stated) in cases {
        let path = write(&scratch, "broken.json", &file);
        for command in ["plan", "apply"] {
            let refused = tool(&server, &token, &[command, &path])
                .await
                .failed(FAILURE);
            assert!(refused.err.contains(stated), "{file}: {}", refused.err);
            assert_eq!(refused.out, "");
        }
    }
    let missing = scratch.0.join("absent.json");
    let refused = tool(&server, &token, &["apply", missing.to_str().unwrap()])
        .await
        .failed(FAILURE);
    assert!(
        refused.err.contains("cannot read the structure file"),
        "{}",
        refused.err
    );
    assert_eq!(state(&server).await, before);
}

#[tokio::test]
async fn rule_49_the_token_and_the_address_are_given_safely() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let token = full_token(&server, &owner).await;
    let scratch = Scratch::new();
    let run_with = |arguments: Vec<&str>, token: Option<&str>, address: Option<&str>| {
        let invocation = Invocation {
            arguments: arguments.into_iter().map(str::to_owned).collect(),
            token: token.map(str::to_owned),
            server: address.map(str::to_owned),
        };
        async move {
            let (mut out, mut err) = (Vec::new(), Vec::new());
            let code = run(invocation, &mut out, &mut err).await;
            (
                code,
                String::from_utf8(out).unwrap(),
                String::from_utf8(err).unwrap(),
            )
        }
    };
    let base = server.base.as_str();

    let (code, _, err) = run_with(vec!["export"], None, Some(base)).await;
    assert_eq!(code, USAGE_ERROR);
    assert!(err.contains("no access token"), "{err}");
    let (code, _, err) = run_with(vec!["export", "--token", &token], None, Some(base)).await;
    assert_eq!(code, USAGE_ERROR);
    assert!(err.contains("never given on the command line"), "{err}");
    assert!(!err.contains(&token[4..]));
    let (code, _, err) = run_with(vec!["export"], Some(&token), None).await;
    assert_eq!(code, USAGE_ERROR);
    assert!(err.contains("no server address"), "{err}");
    for address in ["http://chat.example.test", "http://192.0.2.7:8080"] {
        let (code, _, err) = run_with(vec!["export"], Some(&token), Some(address)).await;
        assert_eq!(code, FAILURE);
        assert!(err.contains("over https only"), "{err}");
    }
    let (code, _, err) = run_with(vec!["export"], Some(&token), Some("chat.example.test")).await;
    assert_eq!(code, FAILURE);
    assert!(err.contains("an origin is written as"), "{err}");

    let file = scratch.0.join("token");
    std::fs::write(&file, format!("{token}\n")).unwrap();
    let from_file = vec![
        "export",
        "--token-file",
        file.to_str().unwrap(),
        "--server",
        base,
    ];
    let (code, out, _) = run_with(from_file, None, None).await;
    assert_eq!(code, SUCCESS);
    assert!(out.contains("\"format\": 4"));

    let (code, _, err) = run_with(vec!["export"], Some("jat_unknown"), Some(base)).await;
    assert_eq!(code, FAILURE);
    assert!(err.contains("unauthenticated"), "{err}");
    for arguments in [
        vec![],
        vec!["remove"],
        vec!["plan"],
        vec!["apply"],
        vec!["export", "x"],
    ] {
        let (code, _, err) = run_with(arguments, Some(&token), Some(base)).await;
        assert_eq!(code, USAGE_ERROR);
        assert!(err.contains("usage: junto-setup"), "{err}");
    }
    let (code, out, _) = run_with(vec!["--help"], None, None).await;
    assert_eq!(code, SUCCESS);
    assert!(out.contains("usage: junto-setup"));
    // The help states that a file holds no rule of a person and that apply keeps them.
    assert!(out.contains("no rule of\na person"), "{out}");
    assert!(out.contains("the rules of people on the server"), "{out}");
}

#[tokio::test]
async fn rule_52_a_plan_with_more_changes_than_one_request_takes_is_not_sent() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let token = full_token(&server, &owner).await;
    let scratch = Scratch::new();
    let main = server.default_workspace(&owner).await.id.to_string();
    let channels: Vec<Value> = (0..201)
        .map(|number| json!({ "id": id(1000 + number), "name": format!("c{number}"), "kind": "text" }))
        .collect();
    let file = json!({ "format": 1,
                       "workspaces": [{ "id": main, "name": "Acme", "channels": channels }] });
    let path = write(&scratch, "large.json", &file);
    let before = state(&server).await;
    tool(&server, &token, &["plan", &path]).await.succeeded();
    let refused = tool(&server, &token, &["apply", &path])
        .await
        .failed(FAILURE);
    assert!(refused.err.contains("201 changes"), "{}", refused.err);
    assert!(refused.err.contains("at most 200"), "{}", refused.err);
    assert_eq!(state(&server).await, before);
}

/// A server that answers the reading request of the tool with an empty structure and the
/// request of changes with `status` and `body`.
async fn answering(status: u16, body: &'static str) -> String {
    use axum::http::StatusCode as Status;
    use axum::routing::{get, post};
    let export = json!({ "revision": "r", "structure": { "format": 1 } }).to_string();
    let router = axum::Router::new()
        .route("/api/v1/structure", get(move || async move { export }))
        .route(
            "/api/v1/structure/changes",
            post(move || async move { (Status::from_u16(status).unwrap(), body) }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await });
    address
}

#[tokio::test]
async fn review_5_an_answer_that_cannot_be_read_is_not_reported_as_nothing_applied() {
    let scratch = Scratch::new();
    let file = json!({ "format": 1, "server_roles": [{ "id": id(90), "name": "Inviters" }] });
    let path = write(&scratch, "structure.json", &file);
    for (status, body) in [
        (200, "applied, in a form that the tool does not read"),
        (200, "{}"),
        (502, "<html>bad gateway</html>"),
    ] {
        let invocation = Invocation {
            arguments: vec!["apply".to_owned(), path.clone()],
            token: Some("jat_any".to_owned()),
            server: Some(answering(status, body).await),
        };
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run(invocation, &mut out, &mut err).await;
        let err = String::from_utf8(err).unwrap();
        assert_eq!(code, FAILURE, "{status} {body}: {err}");
        assert!(!err.contains("applied none"), "{status} {body}: {err}");
        assert!(
            err.contains("cannot tell whether the server applied"),
            "{status} {body}: {err}"
        );
        assert!(err.contains("run `plan` again"), "{status} {body}: {err}");
    }
    // An error body of the API is a refusal, and nothing was applied.
    let refusal = r#"{"error":{"code":"forbidden","message":"no"}}"#;
    let invocation = Invocation {
        arguments: vec!["apply".to_owned(), path.clone()],
        token: Some("jat_any".to_owned()),
        server: Some(answering(403, refusal).await),
    };
    let (mut out, mut err) = (Vec::new(), Vec::new());
    assert_eq!(run(invocation, &mut out, &mut err).await, FAILURE);
    let err = String::from_utf8(err).unwrap();
    assert!(
        err.contains("The server applied none of the 1 change."),
        "{err}"
    );
}

#[tokio::test]
async fn review_6_a_saved_plan_without_changes_is_checked_against_the_revision() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let token = full_token(&server, &owner).await;
    let scratch = Scratch::new();
    let main = server.default_workspace(&owner).await;
    let nothing = json!({ "format": 1 });
    let path = write(&scratch, "nothing.json", &nothing);
    let plan = scratch.0.join("plan.json");
    let plan = plan.to_str().unwrap();
    tool(&server, &token, &["plan", &path, "--out", plan])
        .await
        .succeeded();
    let before = state(&server).await;
    let current = tool(&server, &token, &["apply", "--plan", plan])
        .await
        .succeeded();
    assert!(current.out.contains("no changes"), "{}", current.out);
    assert_eq!(state(&server).await, before);

    server.channel(&owner, main.id, "made-after-the-plan").await;
    let stale = tool(&server, &token, &["apply", "--plan", plan])
        .await
        .failed(FAILURE);
    assert!(stale.err.contains("conflict"), "{}", stale.err);
    assert!(stale.err.contains("Run `plan` again"), "{}", stale.err);
}

#[tokio::test]
async fn rule_54_a_file_sets_the_glossary_that_it_states_and_keeps_an_absent_one() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let token = full_token(&server, &owner).await;
    let scratch = Scratch::new();
    let main = server.default_workspace(&owner).await;
    let route = format!("/api/v1/workspaces/{}/glossary", main.id);
    server
        .put(&owner, &route, &json!({ "terms": ["Quorvex", "Lumitrax"] }))
        .await
        .ok::<Glossary>(StatusCode::OK);
    let glossary = || {
        let (server, owner, route) = (&server, &owner, &route);
        async move {
            server
                .get(owner, route)
                .await
                .ok::<Glossary>(StatusCode::OK)
                .terms
        }
    };

    // The export states the glossary in its order of priority.
    let printed = tool(&server, &token, &["export"]).await.succeeded();
    let exported: Value = serde_json::from_str(&printed.out).unwrap();
    assert_eq!(
        exported["workspaces"][0]["glossary"],
        json!(["Quorvex", "Lumitrax"])
    );

    // A file without a glossary keeps the glossary of the server.
    let main_id = main.id.to_string();
    let absent = write(
        &scratch,
        "absent.json",
        &json!({ "format": 3, "workspaces": [
            { "id": main_id, "name": "Renamed", "default": true }] }),
    );
    let applied = tool(&server, &token, &["apply", &absent]).await.succeeded();
    assert!(applied.out.contains("Applied 1 change."), "{}", applied.out);
    assert!(!applied.out.contains("glossary"), "{}", applied.out);
    assert_eq!(glossary().await, ["Quorvex", "Lumitrax"]);

    // A file with a glossary replaces the whole list, through a plan that states it.
    let stated = write(
        &scratch,
        "stated.json",
        &json!({ "format": 3, "workspaces": [
            { "id": main_id, "name": "Renamed",
              "glossary": ["Lumitrax", "Ysolde Varkonen"] }] }),
    );
    let plan = scratch.0.join("plan.json");
    let plan = plan.to_str().unwrap();
    let before = state(&server).await;
    let planned = tool(&server, &token, &["plan", &stated, "--out", plan])
        .await
        .succeeded();
    assert_eq!(state(&server).await, before);
    assert!(
        planned.out.contains(&format!(
            "~ glossary of the workspace \"Renamed\" ({main_id}): add \"Ysolde Varkonen\"; \
             remove \"Quorvex\"; terms in order of priority: \"Lumitrax\", \"Ysolde Varkonen\""
        )),
        "{}",
        planned.out
    );

    // A change of the glossary after the plan makes the plan stale.
    server
        .put(&owner, &route, &json!({ "terms": ["Quorvex"] }))
        .await
        .ok::<Glossary>(StatusCode::OK);
    let before = state(&server).await;
    let refused = tool(&server, &token, &["apply", "--plan", plan])
        .await
        .failed(FAILURE);
    assert!(refused.err.contains("Run `plan` again"), "{}", refused.err);
    assert_eq!(state(&server).await, before);

    let applied = tool(&server, &token, &["apply", &stated]).await.succeeded();
    assert!(applied.out.contains("Applied 1 change."), "{}", applied.out);
    assert_eq!(glossary().await, ["Lumitrax", "Ysolde Varkonen"]);
    let after = state(&server).await;
    let again = tool(&server, &token, &["apply", &stated]).await.succeeded();
    assert!(again.out.starts_with("No changes:"), "{}", again.out);
    assert_eq!(state(&server).await, after);

    // A glossary that the server refuses applies nothing of the file.
    let refused_file = write(
        &scratch,
        "refused.json",
        &json!({ "format": 3, "workspaces": [
            { "id": main_id, "name": "Other", "glossary": ["Quorvex", "Quorvex"] }] }),
    );
    let refused = tool(&server, &token, &["apply", &refused_file])
        .await
        .failed(FAILURE);
    assert!(
        refused.err.contains("change 2 (replace_glossary)"),
        "{}",
        refused.err
    );
    assert!(
        refused.err.contains("applied none of the 2 changes"),
        "{}",
        refused.err
    );
    assert_eq!(state(&server).await, after);
}
