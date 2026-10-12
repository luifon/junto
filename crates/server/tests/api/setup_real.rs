//! The binary of the setup tool against two running servers of this repository (ADR 0212).
//!
//! Ignored in the plain run. Start two servers with empty databases, build the tool with
//! `cargo build -p junto-setup`, and give each server as `<address>|<owner code>`:
//!
//! ```sh
//! JUNTO_TEST_SETUP_SOURCE='http://127.0.0.1:8081|XXXX-...' \
//! JUNTO_TEST_SETUP_TARGET='http://127.0.0.1:8082|XXXX-...' \
//!     cargo test -p junto-server --test api setup_real:: -- --ignored
//! ```

use std::path::PathBuf;
use std::process::Command;

use junto_protocol::access_token::AccessTokenCreated;
use junto_protocol::auth::{Challenge, ChallengeOperation, SessionCreated};
use junto_protocol::origin::Origin;
use junto_protocol::structure_file::StructureFile;
use serde_json::{Value, json};

use crate::support::{DeviceKey, Scratch};

struct Running {
    address: String,
    token: String,
}

fn tool_binary() -> PathBuf {
    let test_binary = std::env::current_exe().unwrap();
    let profile = test_binary.parent().unwrap().parent().unwrap();
    let path = profile.join(format!("junto-setup{}", std::env::consts::EXE_SUFFIX));
    assert!(
        path.exists(),
        "{} does not exist. Build it with `cargo build -p junto-setup`.",
        path.display()
    );
    path
}

/// Claims the owner of a server that has none and creates an access token.
async fn prepare(variable: &str) -> Running {
    let value = std::env::var(variable).unwrap_or_else(|_| {
        panic!("{variable} is not set. It is `<address>|<owner code>` of a server without owner.")
    });
    let (address, owner_code) = value.split_once('|').unwrap();
    let origin = Origin::parse(address).unwrap();
    let http = reqwest::Client::new();
    let post = |path: &'static str, token: Option<String>, body: Value| {
        let mut request = http.post(format!("{address}{path}")).json(&body);
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        async move {
            let response = request.send().await.unwrap();
            let status = response.status();
            let text = response.text().await.unwrap();
            assert!(status.is_success(), "{path}: {status} {text}");
            text
        }
    };
    let expires_at = (time::OffsetDateTime::now_utc() + time::Duration::days(1))
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap();
    let key = DeviceKey::generate();
    let challenge: Challenge = serde_json::from_str(
        &post(
            "/api/v1/auth/challenges",
            None,
            json!({ "public_key": key.public(), "operation": ChallengeOperation::ClaimOwner }),
        )
        .await,
    )
    .unwrap();
    let proof = json!({ "challenge_id": challenge.id, "origin": origin,
                        "signature": key.sign_for(&challenge, &origin) });
    let session: SessionCreated = serde_json::from_str(
        &post(
            "/api/v1/owner/claim",
            None,
            json!({ "proof": proof, "owner_code": owner_code, "name": "Owner",
                    "device_name": "setup test" }),
        )
        .await,
    )
    .unwrap();
    let created: AccessTokenCreated = serde_json::from_str(
        &post(
            "/api/v1/access-tokens",
            Some(session.token.expose().to_owned()),
            json!({ "name": "setup test", "actions": ["read_structure", "manage_structure"],
                    "expires_at": expires_at }),
        )
        .await,
    )
    .unwrap_or_else(|error| panic!("{error}"));
    Running {
        address: address.to_owned(),
        token: created.token.expose().to_owned(),
    }
}

fn tool(server: &Running, arguments: &[&str]) -> (bool, String, String) {
    let output = Command::new(tool_binary())
        .args(arguments)
        .env_clear()
        .env("JUNTO_SERVER", &server.address)
        .env("JUNTO_ACCESS_TOKEN", &server.token)
        .output()
        .unwrap();
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[track_caller]
fn succeeded(ran: (bool, String, String)) -> String {
    assert!(ran.0, "out: {}\nerr: {}", ran.1, ran.2);
    ran.1
}

#[tokio::test]
#[ignore = "needs two running servers; see the head of this file"]
async fn export_plan_and_apply_between_two_running_servers() {
    let source = prepare("JUNTO_TEST_SETUP_SOURCE").await;
    let target = prepare("JUNTO_TEST_SETUP_TARGET").await;
    let scratch = Scratch::new();
    let path = |name: &str| scratch.0.join(name).to_str().unwrap().to_owned();
    let id = |number: u32| format!("00000000-0000-7000-8000-{number:012}");

    // The source gets a structure from a file that a person wrote.
    let first: StructureFile =
        serde_json::from_str(&succeeded(tool(&source, &["export"]))).unwrap();
    let main = first
        .workspaces
        .iter()
        .find(|entry| entry.default)
        .unwrap()
        .id;
    let written = json!({
        "format": 1,
        "server_roles": [{ "id": id(90), "name": "Inviters", "permissions": ["create_invites"] }],
        "workspaces": [
            { "id": main, "name": "Main",
              "roles": [{ "id": id(11), "name": "Helpers", "permissions": ["manage_glossary"] }],
              "categories": [{ "id": id(20), "name": "Projects" }],
              "channels": [{ "id": id(30), "name": "plans", "kind": "text", "category": id(20),
                             "overrides": [{ "role": id(11), "allow": ["manage_messages"] }] }] },
            { "id": id(2), "name": "Team",
              "channels": [{ "id": id(33), "name": "lobby", "kind": "text" }] },
        ],
    });
    std::fs::write(path("written.json"), written.to_string()).unwrap();
    let planned = succeeded(tool(&source, &["plan", &path("written.json")]));
    assert!(planned.contains("+ workspace \"Team\""), "{planned}");
    let unchanged: StructureFile =
        serde_json::from_str(&succeeded(tool(&source, &["export"]))).unwrap();
    assert_eq!(unchanged, first, "plan changed the server");
    let applied = succeeded(tool(&source, &["apply", &path("written.json")]));
    assert!(applied.contains("Applied"), "{applied}");
    let again = succeeded(tool(&source, &["apply", &path("written.json")]));
    assert!(again.starts_with("No changes:"), "{again}");

    // The export of the source is applied to the empty target, twice.
    succeeded(tool(&source, &["export", "--out", &path("exported.json")]));
    let before: StructureFile =
        serde_json::from_str(&succeeded(tool(&target, &["export"]))).unwrap();
    let planned = succeeded(tool(
        &target,
        &["plan", &path("exported.json"), "--out", &path("plan.json")],
    ));
    assert!(planned.contains("+ text channel \"plans\""), "{planned}");
    let unchanged: StructureFile =
        serde_json::from_str(&succeeded(tool(&target, &["export"]))).unwrap();
    assert_eq!(unchanged, before, "plan changed the server");
    let applied = succeeded(tool(&target, &["apply", "--plan", &path("plan.json")]));
    assert!(applied.contains("Applied"), "{applied}");
    let again = succeeded(tool(&target, &["apply", &path("exported.json")]));
    assert!(again.starts_with("No changes:"), "{again}");
    let stale = tool(&target, &["apply", "--plan", &path("plan.json")]);
    assert!(!stale.0, "a plan of an earlier revision was applied");
    assert!(stale.2.contains("applied none"), "{}", stale.2);

    let result: StructureFile =
        serde_json::from_str(&succeeded(tool(&target, &["export"]))).unwrap();
    let names = |file: &StructureFile| -> Vec<String> {
        let mut names: Vec<String> = file
            .workspaces
            .iter()
            .flat_map(|workspace| {
                workspace
                    .channels
                    .iter()
                    .map(move |channel| format!("{}/{}", workspace.name, channel.name))
            })
            .collect();
        names.sort();
        names
    };
    let source_file: StructureFile =
        serde_json::from_str(&std::fs::read_to_string(path("exported.json")).unwrap()).unwrap();
    for name in names(&source_file) {
        assert!(
            names(&result).contains(&name),
            "{name} is not on the target"
        );
    }
    assert_eq!(result.server_roles, source_file.server_roles);
    // An address that is not the server of the plan, and a wrong token, are refused.
    let wrong = Running {
        address: target.address.clone(),
        token: "jat_wrong".to_owned(),
    };
    assert!(!tool(&wrong, &["export"]).0);
}
