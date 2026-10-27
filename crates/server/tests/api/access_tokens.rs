//! Access tokens of administrators (ADR 0203, ADR 0210).

use junto_protocol::ServerPermission;
use junto_protocol::access_token::{
    AccessTokenCreated, AccessTokenInfo, AccessTokenPage, TokenAction,
};
use junto_protocol::audit::{AuditEvent, AuditPage};
use junto_protocol::error::ErrorCode;
use junto_protocol::events::{Event, Level};
use junto_protocol::structure::{Channel, Workspace};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use time::Duration;
use time::format_description::well_known::Rfc3339;

use crate::discovery_support::events;
use crate::support::{DeviceKey, Member, Reply, TestServer, join_all};

const ROUTE: &str = "/api/v1/access-tokens";

impl TestServer {
    pub async fn access_token_reply(
        &self,
        creator: &Member,
        actions: &[TokenAction],
        lifetime: Duration,
    ) -> Reply {
        let expires_at = (self.clock.now() + lifetime).format(&Rfc3339).unwrap();
        self.post(
            creator,
            ROUTE,
            &json!({ "name": "automation", "actions": actions, "expires_at": expires_at }),
        )
        .await
    }

    /// A new access token of `creator` with `actions` that lasts 30 days.
    pub async fn access_token(
        &self,
        creator: &Member,
        actions: &[TokenAction],
    ) -> AccessTokenCreated {
        self.access_token_reply(creator, actions, Duration::days(30))
            .await
            .ok(StatusCode::CREATED)
    }

    /// A request with an access token.
    pub async fn with_token(
        &self,
        method: Method,
        path: &str,
        token: &AccessTokenCreated,
        body: Option<&Value>,
    ) -> Reply {
        self.send(method, path, Some(token.token.expose()), body, None)
            .await
    }

    pub async fn administrator(&self, owner: &Member, name: &str) -> Member {
        let member = self.join(owner, name).await;
        self.grant_server(owner, &member, &[ServerPermission::Administrator])
            .await;
        member
    }
}

async fn token_rows(server: &TestServer) -> i64 {
    sqlx::query_scalar("select count(*) from access_tokens")
        .fetch_one(&server.db)
        .await
        .unwrap()
}

fn listed(reply: &Reply) -> Vec<AccessTokenInfo> {
    reply.ok::<AccessTokenPage>(StatusCode::OK).access_tokens
}

#[tokio::test]
async fn rule_01_creation_returns_the_token_once_and_stores_its_hash() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let created = server
        .access_token(
            &owner,
            &[TokenAction::ReadStructure, TokenAction::ReadAuditLog],
        )
        .await;
    let text = created.token.expose();
    assert!(text.starts_with("jat_"));
    assert_eq!(text.len(), 4 + 43);
    assert_eq!(created.access_token.created_by, owner.account_id);
    assert_eq!(
        created.access_token.actions,
        [TokenAction::ReadStructure, TokenAction::ReadAuditLog]
    );
    assert_eq!(created.access_token.created_at, server.clock.now());

    let (hash, actions): (Vec<u8>, i64) =
        sqlx::query_as("select token_hash, actions from access_tokens where id = $1")
            .bind(created.access_token.id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(
        hash,
        junto_server::secrets::hash_access_token(text).to_vec()
    );
    assert_eq!(actions, 0b1001);
    let stored: String = sqlx::query_scalar("select row::text from access_tokens as row")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert!(!stored.contains(&text[4..]));

    let list = server.get(&owner, ROUTE).await;
    assert_eq!(listed(&list), std::slice::from_ref(&created.access_token));
    assert!(!list.text().contains(&text[4..]));
    assert!(!list.text().contains("token_hash"));

    let other = server
        .access_token(&owner, &[TokenAction::ReadStructure])
        .await;
    assert_ne!(other.token.expose(), text);
}

#[tokio::test]
async fn rule_02_creation_needs_the_administrator_permission() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let plain = server.join(&owner, "Plain").await;
    let moderator = server.join(&owner, "Moderator").await;
    let every_other: Vec<ServerPermission> = ServerPermission::ALL
        .iter()
        .copied()
        .filter(|permission| *permission != ServerPermission::Administrator)
        .collect();
    server.grant_server(&owner, &moderator, &every_other).await;

    for caller in [&plain, &moderator] {
        server
            .access_token_reply(caller, &[TokenAction::ReadStructure], Duration::days(1))
            .await
            .error(ErrorCode::Forbidden);
    }
    assert_eq!(token_rows(&server).await, 0);

    let administrator = server.administrator(&owner, "Administrator").await;
    server
        .access_token(&administrator, &[TokenAction::ReadStructure])
        .await;
    assert_eq!(token_rows(&server).await, 1);
}

#[tokio::test]
async fn rule_03_creation_refuses_invalid_requests() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let at = |offset: Duration| (server.clock.now() + offset).format(&Rfc3339).unwrap();
    let valid = at(Duration::days(366));
    let bodies = [
        json!({ "name": "t", "actions": [], "expires_at": valid }),
        json!({ "name": "t", "actions": ["manage_everything"], "expires_at": valid }),
        json!({ "name": "t", "actions": ["unknown"], "expires_at": valid }),
        json!({ "name": "t", "actions": ["read_structure"], "expires_at": at(Duration::ZERO) }),
        json!({ "name": "t", "actions": ["read_structure"],
                "expires_at": at(Duration::days(366) + Duration::seconds(1)) }),
        json!({ "name": "  ", "actions": ["read_structure"], "expires_at": valid }),
        json!({ "name": "a".repeat(65), "actions": ["read_structure"], "expires_at": valid }),
        json!({ "name": "t", "actions": ["read_structure"] }),
    ];
    for body in bodies {
        let reply = server.post(&owner, ROUTE, &body).await;
        assert_eq!(
            reply.code(),
            ErrorCode::InvalidRequest,
            "{body}: {}",
            reply.text()
        );
    }
    assert_eq!(token_rows(&server).await, 0);

    let longest = json!({ "name": "t", "actions": ["read_structure", "read_structure"],
                          "expires_at": valid });
    let created: AccessTokenCreated = server
        .post(&owner, ROUTE, &longest)
        .await
        .ok(StatusCode::CREATED);
    assert_eq!(created.access_token.actions, [TokenAction::ReadStructure]);
}

#[tokio::test]
async fn rule_04_a_creator_holds_at_most_one_hundred_tokens() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let administrator = server.administrator(&owner, "Administrator").await;
    sqlx::query(
        "insert into access_tokens (id, token_hash, created_by, name, actions, created_at, expires_at)
         select gen_random_uuid(), sha256(n::text::bytea), $1, 'filler', 1, $2, $3
         from generate_series(1, 99) as n",
    )
    .bind(owner.account_id)
    .bind(server.clock.now())
    .bind(server.clock.now() + Duration::days(1))
    .execute(&mut server.test_data().await)
    .await
    .unwrap();

    let replies = join_all(vec![
        server.access_token_reply(&owner, &[TokenAction::ReadStructure], Duration::days(1)),
        server.access_token_reply(&owner, &[TokenAction::ReadStructure], Duration::days(1)),
    ])
    .await;
    let mut statuses: Vec<u16> = replies.iter().map(|reply| reply.status.as_u16()).collect();
    statuses.sort_unstable();
    assert_eq!(statuses, [201, 409]);
    let refused = replies.iter().find(|reply| reply.status == 409).unwrap();
    refused.error(ErrorCode::LimitReached);
    assert_eq!(token_rows(&server).await, 100);

    // The ceiling counts the tokens of one creator, and a token that has expired and is not
    // deleted yet still counts.
    server.clock.advance(Duration::days(2));
    server
        .access_token_reply(&owner, &[TokenAction::ReadStructure], Duration::days(1))
        .await
        .error(ErrorCode::LimitReached);
    server
        .access_token(&administrator, &[TokenAction::ReadStructure])
        .await;
}

#[tokio::test]
async fn rule_05_a_token_follows_the_current_permissions_of_its_creator() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let administrator = server.join(&owner, "Administrator").await;
    let role_id = server
        .grant_server(&owner, &administrator, &[ServerPermission::Administrator])
        .await;
    let token = server
        .access_token(
            &administrator,
            &[TokenAction::ReadStructure, TokenAction::ManageStructure],
        )
        .await;

    let body = json!({ "name": "Created with a token" });
    let created: Workspace = server
        .with_token(Method::POST, "/api/v1/workspaces", &token, Some(&body))
        .await
        .ok(StatusCode::CREATED);
    let members: Vec<Value> = server
        .with_token(
            Method::GET,
            &format!("/api/v1/workspaces/{}/members", created.id),
            &token,
            None,
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(members.len(), 1);
    assert_eq!(members[0]["account_id"], json!(administrator.account_id));

    server
        .delete(
            &owner,
            &format!(
                "/api/v1/accounts/{}/server-roles/{role_id}",
                administrator.account_id
            ),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let workspaces = sqlx::query_scalar::<_, i64>("select count(*) from workspaces");
    let before = workspaces.fetch_one(&server.db).await.unwrap();
    server
        .with_token(Method::POST, "/api/v1/workspaces", &token, Some(&body))
        .await
        .error(ErrorCode::Forbidden);
    server
        .with_token(Method::GET, "/api/v1/structure", &token, None)
        .await
        .error(ErrorCode::Forbidden);
    let after: i64 = sqlx::query_scalar("select count(*) from workspaces")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(after, before);

    // What the creator still may do, the token still does: a member lists the workspaces
    // that it is a member of.
    let seen: Vec<Workspace> = server
        .with_token(Method::GET, "/api/v1/workspaces", &token, None)
        .await
        .ok(StatusCode::OK);
    assert!(seen.iter().any(|workspace| workspace.id == created.id));
}

#[tokio::test]
async fn rule_06_a_token_is_limited_to_its_actions() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reading = server
        .access_token(&owner, &[TokenAction::ReadStructure])
        .await;
    let workspace = server.default_workspace(&owner).await;

    server
        .with_token(Method::GET, "/api/v1/structure", &reading, None)
        .await
        .ok::<Value>(StatusCode::OK);
    let refusals = [
        (
            Method::POST,
            "/api/v1/workspaces".to_owned(),
            json!({ "name": "No" }),
        ),
        (
            Method::POST,
            format!("/api/v1/workspaces/{}/channels", workspace.id),
            json!({ "name": "no", "kind": "text" }),
        ),
        (
            Method::POST,
            "/api/v1/structure/changes".to_owned(),
            json!({ "revision": "", "changes": [] }),
        ),
        (Method::POST, "/api/v1/invites".to_owned(), json!({})),
        (Method::GET, "/api/v1/audit-log".to_owned(), Value::Null),
    ];
    let before = (
        sqlx::query_scalar::<_, i64>("select count(*) from workspaces")
            .fetch_one(&server.db)
            .await
            .unwrap(),
        sqlx::query_scalar::<_, i64>("select count(*) from channels")
            .fetch_one(&server.db)
            .await
            .unwrap(),
        sqlx::query_scalar::<_, i64>("select count(*) from invites")
            .fetch_one(&server.db)
            .await
            .unwrap(),
    );
    for (method, path, body) in refusals {
        let body = (!body.is_null()).then_some(&body);
        let reply = server
            .with_token(method.clone(), &path, &reading, body)
            .await;
        assert_eq!(reply.code(), ErrorCode::Forbidden, "{method} {path}");
        assert!(
            reply.text().contains("does not allow the action"),
            "{}",
            reply.text()
        );
    }
    let after = (
        sqlx::query_scalar::<_, i64>("select count(*) from workspaces")
            .fetch_one(&server.db)
            .await
            .unwrap(),
        sqlx::query_scalar::<_, i64>("select count(*) from channels")
            .fetch_one(&server.db)
            .await
            .unwrap(),
        sqlx::query_scalar::<_, i64>("select count(*) from invites")
            .fetch_one(&server.db)
            .await
            .unwrap(),
    );
    assert_eq!(after, before);

    let auditing = server
        .access_token(&owner, &[TokenAction::ReadAuditLog])
        .await;
    server
        .with_token(Method::GET, "/api/v1/audit-log", &auditing, None)
        .await
        .ok::<AuditPage>(StatusCode::OK);
    server
        .with_token(Method::GET, "/api/v1/structure", &auditing, None)
        .await
        .error(ErrorCode::Forbidden);
}

#[tokio::test]
async fn rule_07_an_expired_token_stops_and_is_deleted() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let short: AccessTokenCreated = server
        .access_token_reply(&owner, &[TokenAction::ReadStructure], Duration::hours(1))
        .await
        .ok(StatusCode::CREATED);
    let long = server
        .access_token(&owner, &[TokenAction::ReadStructure])
        .await;

    server
        .clock
        .advance(Duration::hours(1) - Duration::seconds(1));
    server
        .with_token(Method::GET, "/api/v1/structure", &short, None)
        .await
        .ok::<Value>(StatusCode::OK);
    server.clock.advance(Duration::seconds(1));
    server
        .with_token(Method::GET, "/api/v1/structure", &short, None)
        .await
        .error(ErrorCode::Unauthenticated);
    assert_eq!(
        listed(&server.get(&owner, ROUTE).await),
        std::slice::from_ref(&long.access_token)
    );

    assert_eq!(token_rows(&server).await, 2);
    junto_server::maintenance::run_once(&server.state)
        .await
        .unwrap();
    assert_eq!(token_rows(&server).await, 1);
    server
        .with_token(Method::GET, "/api/v1/structure", &long, None)
        .await
        .ok::<Value>(StatusCode::OK);
}

#[tokio::test]
async fn rule_08_the_creator_and_an_administrator_revoke_and_others_find_nothing() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let first = server.administrator(&owner, "First").await;
    let second = server.administrator(&owner, "Second").await;
    let plain = server.join(&owner, "Plain").await;
    let own = server
        .access_token(&first, &[TokenAction::ReadStructure])
        .await;
    let other = server
        .access_token(&first, &[TokenAction::ReadStructure])
        .await;
    let route = |token: &AccessTokenCreated| format!("{ROUTE}/{}", token.access_token.id);

    let absent = server
        .delete(&plain, &format!("{ROUTE}/{}", uuid::Uuid::now_v7()))
        .await;
    absent.error(ErrorCode::NotFound);
    let hidden = server.delete(&plain, &route(&own)).await;
    assert_eq!(hidden.observable(), absent.observable());
    assert_eq!(token_rows(&server).await, 2);
    server
        .with_token(Method::GET, "/api/v1/structure", &own, None)
        .await
        .ok::<Value>(StatusCode::OK);

    server
        .delete(&first, &route(&own))
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .with_token(Method::GET, "/api/v1/structure", &own, None)
        .await
        .error(ErrorCode::Unauthenticated);
    server
        .delete(&first, &route(&own))
        .await
        .error(ErrorCode::NotFound);

    server
        .delete(&second, &route(&other))
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .with_token(Method::GET, "/api/v1/structure", &other, None)
        .await
        .error(ErrorCode::Unauthenticated);
    assert_eq!(token_rows(&server).await, 0);

    // A creator who lost the administrator permission still revokes the own token.
    let demoted = server.join(&owner, "Demoted").await;
    let role_id = server
        .grant_server(&owner, &demoted, &[ServerPermission::Administrator])
        .await;
    let kept = server
        .access_token(&demoted, &[TokenAction::ReadStructure])
        .await;
    server
        .delete(
            &owner,
            &format!(
                "/api/v1/accounts/{}/server-roles/{role_id}",
                demoted.account_id
            ),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(
        listed(&server.get(&demoted, ROUTE).await),
        std::slice::from_ref(&kept.access_token)
    );
    server
        .delete(&demoted, &route(&kept))
        .await
        .empty(StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn rule_09_tokens_end_with_the_access_of_their_creator() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let banned = server.administrator(&owner, "Banned").await;
    let removed = server.administrator(&owner, "Removed").await;
    let recovered = server.administrator(&owner, "Recovered").await;
    let kept = server
        .access_token(&owner, &[TokenAction::ReadStructure])
        .await;
    let of_banned = server
        .access_token(&banned, &[TokenAction::ReadStructure])
        .await;
    let of_removed = server
        .access_token(&removed, &[TokenAction::ReadStructure])
        .await;
    let of_recovered = server
        .access_token(&recovered, &[TokenAction::ReadStructure])
        .await;
    let cursor = server.snapshot(&owner).await.cursor;

    let ban = format!("/api/v1/accounts/{}/ban", banned.account_id);
    server
        .put_empty(&owner, &ban)
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .post(
            &owner,
            &format!("/api/v1/accounts/{}/removal", removed.account_id),
            &json!({}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let proof_key = DeviceKey::generate();
    server
        .post(
            &owner,
            &format!("/api/v1/accounts/{}/recovery", recovered.account_id),
            &json!({ "public_key": proof_key.public() }),
        )
        .await
        .ok::<Value>(StatusCode::CREATED);

    for token in [&of_banned, &of_removed, &of_recovered] {
        server
            .with_token(Method::GET, "/api/v1/structure", token, None)
            .await
            .error(ErrorCode::Unauthenticated);
    }
    assert_eq!(token_rows(&server).await, 1);
    server
        .with_token(Method::GET, "/api/v1/structure", &kept, None)
        .await
        .ok::<Value>(StatusCode::OK);

    server
        .delete(&owner, &ban)
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .with_token(Method::GET, "/api/v1/structure", &of_banned, None)
        .await
        .error(ErrorCode::Unauthenticated);

    // A token whose row exists does not work while its creator is not active.
    let inactive = server.administrator(&owner, "Inactive").await;
    let of_inactive = server
        .access_token(&inactive, &[TokenAction::ReadStructure])
        .await;
    for status in ["banned", "removed"] {
        sqlx::query("update accounts set status = $2 where id = $1")
            .bind(inactive.account_id)
            .bind(status)
            .execute(&server.db)
            .await
            .unwrap();
        server
            .with_token(Method::GET, "/api/v1/structure", &of_inactive, None)
            .await
            .error(ErrorCode::Unauthenticated);
    }

    let ended: Vec<_> = events(&server, &owner, cursor, Level::Full)
        .await
        .into_iter()
        .filter_map(|event| match event {
            Event::AccessTokensOfCreatorRevoked { created_by } => Some(created_by),
            _ => None,
        })
        .collect();
    assert_eq!(
        ended,
        [banned.account_id, removed.account_id, recovered.account_id]
    );
}

#[tokio::test]
async fn rule_10_a_token_of_the_owner_is_not_the_owner() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let token = server
        .access_token(
            &owner,
            &[TokenAction::ReadStructure, TokenAction::ManageStructure],
        )
        .await;
    let channel = |name: &'static str| {
        let (server, owner) = (&server, &owner);
        async move {
            server
                .post(
                    owner,
                    &format!("/api/v1/workspaces/{}/channels", workspace.id),
                    &json!({ "name": name, "kind": "text" }),
                )
                .await
                .ok::<Channel>(StatusCode::CREATED)
        }
    };
    let plain = channel("plain").await;
    let permanent = channel("records").await;
    sqlx::query("update channels set permanent_message_count = 1 where id = $1")
        .bind(permanent.id)
        .execute(&server.db)
        .await
        .unwrap();

    // The owner holds no server role: the token has the administrator permission from the
    // position of its creator.
    server
        .with_token(Method::GET, "/api/v1/structure", &token, None)
        .await
        .ok::<Value>(StatusCode::OK);
    server
        .with_token(
            Method::DELETE,
            &format!("/api/v1/channels/{}", plain.id),
            &token,
            None,
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .with_token(
            Method::DELETE,
            &format!("/api/v1/channels/{}", permanent.id),
            &token,
            None,
        )
        .await
        .error(ErrorCode::OwnerOnly);
    let deleted: bool =
        sqlx::query_scalar("select deleted_at is not null from channels where id = $1")
            .bind(permanent.id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert!(!deleted);
    server
        .delete(&owner, &format!("/api/v1/channels/{}", permanent.id))
        .await
        .empty(StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn rule_12_a_token_cannot_manage_tokens() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let token = server.access_token(&owner, TokenAction::ALL).await;
    let body = json!({ "name": "child", "actions": ["read_structure"],
                       "expires_at": "2999-01-01T00:00:00Z" });
    let own = format!("{ROUTE}/{}", token.access_token.id);
    for (method, path, body) in [
        (Method::POST, ROUTE, Some(&body)),
        (Method::GET, ROUTE, None),
        (Method::DELETE, own.as_str(), None),
    ] {
        let reply = server.with_token(method, path, &token, body).await;
        reply.error(ErrorCode::Forbidden);
        assert!(reply.text().contains("needs a session"), "{}", reply.text());
    }
    assert_eq!(token_rows(&server).await, 1);
}

#[tokio::test]
async fn rule_13_an_administrator_lists_every_token_and_another_account_its_own() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let demoted = server.join(&owner, "Demoted").await;
    let role_id = server
        .grant_server(&owner, &demoted, &[ServerPermission::Administrator])
        .await;
    let plain = server.join(&owner, "Plain").await;
    let mut all = Vec::new();
    for creator in [&owner, &demoted, &owner, &demoted, &owner] {
        let created = server
            .access_token(creator, &[TokenAction::ReadStructure])
            .await;
        all.push(created.access_token);
    }
    all.reverse();
    server
        .delete(
            &owner,
            &format!(
                "/api/v1/accounts/{}/server-roles/{role_id}",
                demoted.account_id
            ),
        )
        .await
        .empty(StatusCode::NO_CONTENT);

    assert_eq!(listed(&server.get(&owner, ROUTE).await), all);
    let own: Vec<AccessTokenInfo> = all
        .iter()
        .filter(|token| token.created_by == demoted.account_id)
        .cloned()
        .collect();
    assert_eq!(own.len(), 2);
    assert_eq!(listed(&server.get(&demoted, ROUTE).await), own);
    assert_eq!(listed(&server.get(&plain, ROUTE).await), []);

    let first: AccessTokenPage = server
        .get(&owner, &format!("{ROUTE}?limit=2"))
        .await
        .ok(StatusCode::OK);
    assert_eq!(first.access_tokens, all[..2]);
    assert_eq!(first.next_before, Some(all[1].id));
    let rest: AccessTokenPage = server
        .get(&owner, &format!("{ROUTE}?limit=200&before={}", all[1].id))
        .await
        .ok(StatusCode::OK);
    assert_eq!(rest.access_tokens, all[2..]);
    assert_eq!(rest.next_before, None);
    // A cursor of a token of another account does not widen the list.
    let narrowed: AccessTokenPage = server
        .get(&demoted, &format!("{ROUTE}?before={}", all[0].id))
        .await
        .ok(StatusCode::OK);
    assert_eq!(narrowed.access_tokens, own);
    for limit in ["0", "201", "x"] {
        server
            .get(&owner, &format!("{ROUTE}?limit={limit}"))
            .await
            .error(ErrorCode::InvalidRequest);
    }
}

#[tokio::test]
async fn rule_14_audit_entries_name_the_token_of_a_request() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let token = server
        .access_token(
            &owner,
            &[TokenAction::ManageStructure, TokenAction::ReadAuditLog],
        )
        .await;
    let token_id = token.access_token.id;
    server
        .with_token(
            Method::POST,
            "/api/v1/workspaces",
            &token,
            Some(&json!({ "name": "With a token" })),
        )
        .await
        .ok::<Workspace>(StatusCode::CREATED);
    server
        .post(
            &owner,
            "/api/v1/workspaces",
            &json!({ "name": "With a session" }),
        )
        .await
        .ok::<Workspace>(StatusCode::CREATED);
    server
        .delete(&owner, &format!("{ROUTE}/{token_id}"))
        .await
        .empty(StatusCode::NO_CONTENT);

    // The setting that names the token ends with its transaction: the next transaction on
    // the same connection writes entries without a token.
    let mut connection = server.db.acquire().await.unwrap();
    sqlx::query("begin")
        .execute(&mut *connection)
        .await
        .unwrap();
    sqlx::query("select set_config($1, $2, true)")
        .bind(junto_server::audit::ACCESS_TOKEN_SETTING)
        .bind(token_id.to_string())
        .execute(&mut *connection)
        .await
        .unwrap();
    sqlx::query("commit")
        .execute(&mut *connection)
        .await
        .unwrap();
    let left: Option<String> = sqlx::query_scalar("select nullif(current_setting($1, true), '')")
        .bind(junto_server::audit::ACCESS_TOKEN_SETTING)
        .fetch_one(&mut *connection)
        .await
        .unwrap();
    assert_eq!(left, None);
    drop(connection);

    let page: AuditPage = server
        .get(&owner, "/api/v1/audit-log")
        .await
        .ok(StatusCode::OK);
    let relevant: Vec<_> = page
        .entries
        .iter()
        .filter(|entry| {
            matches!(
                entry.event,
                AuditEvent::AccessTokenCreated { .. }
                    | AuditEvent::AccessTokenRevoked { .. }
                    | AuditEvent::WorkspaceCreated { .. }
            )
        })
        .map(|entry| (entry.event.clone(), entry.actor_id, entry.access_token_id))
        .collect();
    let by_owner = Some(owner.account_id);
    assert_eq!(relevant.len(), 4);
    assert_eq!(
        relevant[0],
        (
            AuditEvent::AccessTokenRevoked {
                access_token_id: token_id,
                created_by: owner.account_id
            },
            by_owner,
            None
        )
    );
    assert!(
        matches!(&relevant[1], (AuditEvent::WorkspaceCreated { name, .. }, actor, None)
            if name == "With a session" && *actor == by_owner)
    );
    assert!(
        matches!(&relevant[2], (AuditEvent::WorkspaceCreated { name, .. }, actor, used)
            if name == "With a token" && *actor == by_owner && *used == Some(token_id))
    );
    assert_eq!(
        relevant[3],
        (
            AuditEvent::AccessTokenCreated {
                access_token_id: token_id,
                name: "automation".to_owned(),
                actions: vec![TokenAction::ManageStructure, TokenAction::ReadAuditLog],
                expires_at: token.access_token.expires_at,
            },
            by_owner,
            None
        )
    );
    assert!(
        !server
            .get(&owner, "/api/v1/audit-log")
            .await
            .text()
            .contains(&token.token.expose()[4..])
    );
}

#[tokio::test]
async fn rule_15_token_events_reach_administrators_only() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let administrator = server.administrator(&owner, "Administrator").await;
    let plain = server.join(&owner, "Plain").await;
    let cursor = server.snapshot(&plain).await.cursor;
    let created = server
        .access_token(&owner, &[TokenAction::ReadStructure])
        .await;
    server
        .delete(&owner, &format!("{ROUTE}/{}", created.access_token.id))
        .await
        .empty(StatusCode::NO_CONTENT);

    let about_tokens = |events: Vec<Event>| -> Vec<Event> {
        events
            .into_iter()
            .filter(|event| event.name().starts_with("access_token"))
            .collect()
    };
    let expected = [
        Event::AccessTokenCreated {
            access_token: created.access_token.clone(),
        },
        Event::AccessTokenRevoked {
            access_token_id: created.access_token.id,
        },
    ];
    for receiver in [&owner, &administrator] {
        let received = about_tokens(events(&server, receiver, cursor, Level::Full).await);
        assert_eq!(received, expected);
    }
    assert_eq!(
        about_tokens(events(&server, &plain, cursor, Level::Full).await),
        []
    );
    let background = about_tokens(events(&server, &administrator, cursor, Level::Background).await);
    assert_eq!(background, []);
}

fn uses_index(plan: &Value, index: &str) -> bool {
    plan["Index Name"].as_str() == Some(index)
        || plan["Plans"]
            .as_array()
            .is_some_and(|children| children.iter().any(|child| uses_index(child, index)))
}

fn scans_sequentially(plan: &Value) -> bool {
    plan["Node Type"].as_str() == Some("Seq Scan")
        || plan["Plans"]
            .as_array()
            .is_some_and(|children| children.iter().any(scans_sequentially))
}

#[tokio::test]
async fn rule_16_the_statements_of_tokens_read_through_indexes() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    sqlx::query(
        "insert into access_tokens (id, token_hash, created_by, name, actions, created_at, expires_at)
         select gen_random_uuid(), sha256(n::text::bytea), $1, 'filler', 1, $2, $3
         from generate_series(1, 2000) as n",
    )
    .bind(owner.account_id)
    .bind(server.clock.now())
    .bind(server.clock.now() + Duration::days(1))
    .execute(&mut server.test_data().await)
    .await
    .unwrap();
    sqlx::query("analyze access_tokens")
        .execute(&server.db)
        .await
        .unwrap();
    let mut connection = server.db.acquire().await.unwrap();
    sqlx::query("set enable_seqscan = off")
        .execute(&mut *connection)
        .await
        .unwrap();
    let now = server.clock.now();
    let nil = uuid::Uuid::nil();
    let explain = |statement: &str| format!("explain (format json) {statement}");

    let list_all: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(explain(
        junto_server::routes::access_tokens::LIST_ALL,
    )))
    .bind(now)
    .bind(Option::<uuid::Uuid>::None)
    .bind(51_i64)
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert!(
        uses_index(&list_all[0]["Plan"], "access_tokens_pkey"),
        "{list_all}"
    );
    assert!(!scans_sequentially(&list_all[0]["Plan"]), "{list_all}");

    let list_own: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(explain(
        junto_server::routes::access_tokens::LIST_OWN,
    )))
    .bind(now)
    .bind(Option::<uuid::Uuid>::None)
    .bind(51_i64)
    .bind(nil)
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert!(
        uses_index(&list_own[0]["Plan"], "access_tokens_creator"),
        "{list_own}"
    );
    assert!(!scans_sequentially(&list_own[0]["Plan"]), "{list_own}");

    for (statement, index) in [
        (
            "select id from access_tokens where token_hash = $1 and expires_at > $2 \
             and $3::uuid is not null",
            "access_tokens_token_hash_key",
        ),
        (
            "select id from access_tokens where expires_at <= $2 and $1::bytea is not null \
             and $3::uuid is not null limit 500",
            "access_tokens_expiry",
        ),
        (
            "select count(*) from access_tokens where created_by = $3 and $1::bytea is not null \
             and $2::timestamptz is not null",
            "access_tokens_creator",
        ),
    ] {
        let plan: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(explain(statement)))
            .bind(vec![0u8; 32])
            .bind(now)
            .bind(nil)
            .fetch_one(&mut *connection)
            .await
            .unwrap_or_else(|error| panic!("{statement}: {error}"));
        assert!(uses_index(&plan[0]["Plan"], index), "{statement}: {plan}");
        assert!(!scans_sequentially(&plan[0]["Plan"]), "{statement}: {plan}");
    }
}

#[tokio::test]
async fn rule_17_an_unknown_token_and_a_token_in_another_form_are_refused() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let token = server
        .access_token(&owner, &[TokenAction::ReadStructure])
        .await;
    let text = token.token.expose();
    let mut altered = text.to_owned();
    altered.pop();
    altered.push(if text.ends_with('A') { 'B' } else { 'A' });
    let as_session = format!("jst_{}", &text[4..]);
    let session_as_token = format!("jat_{}", &owner.token[4..]);
    for (credential, code) in [
        (altered.as_str(), ErrorCode::Unauthenticated),
        ("jat_", ErrorCode::Unauthenticated),
        (as_session.as_str(), ErrorCode::Unauthenticated),
        (session_as_token.as_str(), ErrorCode::Unauthenticated),
        (&text[4..], ErrorCode::Unauthenticated),
    ] {
        let reply = server
            .send::<Value>(
                Method::GET,
                "/api/v1/structure",
                Some(credential),
                None,
                None,
            )
            .await;
        assert_eq!(reply.code(), code, "{credential}");
    }
    server
        .with_token(Method::GET, "/api/v1/structure", &token, None)
        .await
        .ok::<Value>(StatusCode::OK);
}

#[tokio::test]
async fn review_1_a_caller_who_may_not_see_a_token_does_not_wait_for_its_row() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let plain = server.join(&owner, "Plain").await;
    let token = server
        .access_token(&owner, &[TokenAction::ReadStructure])
        .await;
    let absent = server
        .delete(&plain, &format!("{ROUTE}/{}", uuid::Uuid::now_v7()))
        .await;

    // Another transaction holds the row of the token, as a revocation in progress does.
    let mut holder = server.db.acquire().await.unwrap();
    sqlx::query("begin").execute(&mut *holder).await.unwrap();
    sqlx::query("select id from access_tokens where id = $1 for update")
        .bind(token.access_token.id)
        .execute(&mut *holder)
        .await
        .unwrap();
    let started = std::time::Instant::now();
    let hidden = server
        .delete(&plain, &format!("{ROUTE}/{}", token.access_token.id))
        .await;
    let waited = started.elapsed();
    sqlx::query("rollback").execute(&mut *holder).await.unwrap();
    assert_eq!(hidden.observable(), absent.observable());
    // The wait for a lock ends after 5 seconds; a request that does not wait is far below.
    let lock_wait = server.state.config.limits.database.lock.unsigned_abs();
    assert!(waited < lock_wait / 2, "{waited:?}");
    assert_eq!(token_rows(&server).await, 1);
}
