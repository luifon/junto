//! The removal of the workspace permission rules of single members (ADR 0301): the cases that
//! `check-upgrade` lists, the refusal of a start while one waits, and the roles that the start
//! makes from the rules once none waits, which keep what every account may do.

use std::collections::{BTreeMap, HashMap};

use axum::http::StatusCode;
use junto_authz::{Request, WorkspacePermission as W, WorkspacePermissions};
use junto_protocol::ids::{AccountId, ChannelId, WorkspaceId, WorkspaceRoleId};
use junto_protocol::structure::Channel;
use junto_server::access::Access;
use junto_server::bootstrap::{self, StartError};
use junto_server::person_rules::{self, Case};
use serde_json::json;

use crate::support::{Member, TestServer, test_config};
use crate::upgrades::forget_the_migrations_after_the_release;

fn set(permissions: &[W]) -> WorkspacePermissions {
    permissions
        .iter()
        .fold(WorkspacePermissions::EMPTY, |set, permission| {
            set.with(*permission)
        })
}

fn bits(permissions: &[W]) -> i64 {
    i64::try_from(set(permissions).bits()).unwrap()
}

fn read(bits: i64) -> WorkspacePermissions {
    W::ALL
        .iter()
        .copied()
        .filter(|permission| bits & i64::try_from(set(&[*permission]).bits()).unwrap() != 0)
        .fold(WorkspacePermissions::EMPTY, |set, permission| {
            set.with(permission)
        })
}

async fn rule(
    server: &TestServer,
    workspace_id: WorkspaceId,
    account_id: AccountId,
    allow: &[W],
    deny: &[W],
) {
    sqlx::query(
        "insert into workspace_person_overrides (workspace_id, account_id, allow, deny)
         values ($1, $2, $3, $4)",
    )
    .bind(workspace_id)
    .bind(account_id)
    .bind(bits(allow))
    .bind(bits(deny))
    .execute(&server.db)
    .await
    .unwrap();
}

async fn remove_rule(server: &TestServer, account_id: AccountId) {
    sqlx::query("delete from workspace_person_overrides where account_id = $1")
        .bind(account_id)
        .execute(&server.db)
        .await
        .unwrap();
}

async fn cases(server: &TestServer) -> Vec<Case> {
    let mut connection = server.db.acquire().await.unwrap();
    person_rules::cases(&mut connection, &test_config().limits)
        .await
        .unwrap()
}

async fn start(server: &TestServer) -> Result<(), StartError> {
    bootstrap::start(
        &server.state.database,
        "Acme",
        &test_config().limits,
        server.clock.now(),
    )
    .await
    .map(|_| ())
}

async fn text_channel(
    server: &TestServer,
    owner: &Member,
    workspace_id: WorkspaceId,
    name: &str,
) -> ChannelId {
    let channel: Channel = server
        .post(
            owner,
            &format!("/api/v1/workspaces/{workspace_id}/channels"),
            &json!({ "name": name, "kind": "text" }),
        )
        .await
        .ok(StatusCode::CREATED);
    channel.id
}

/// A rule of a role, or of the "everyone" role, in a channel.
async fn channel_role_rule(
    server: &TestServer,
    (workspace_id, channel_id): (WorkspaceId, ChannelId),
    role_id: WorkspaceRoleId,
    allow: &[W],
    deny: &[W],
) {
    sqlx::query(
        "insert into channel_overrides (channel_id, role_id, workspace_id, allow, deny)
         values ($1, $2, $3, $4, $5)",
    )
    .bind(channel_id)
    .bind(role_id)
    .bind(workspace_id)
    .bind(bits(allow))
    .bind(bits(deny))
    .execute(&server.db)
    .await
    .unwrap();
    sqlx::query("update channels set permissions_synced = false where id = $1")
        .bind(channel_id)
        .execute(&server.db)
        .await
        .unwrap();
}

async fn channel_person_rule(
    server: &TestServer,
    (workspace_id, channel_id): (WorkspaceId, ChannelId),
    account_id: AccountId,
    allow: &[W],
    deny: &[W],
) {
    sqlx::query(
        "insert into channel_person_overrides (channel_id, account_id, workspace_id, allow, deny)
         values ($1, $2, $3, $4, $5)",
    )
    .bind(channel_id)
    .bind(account_id)
    .bind(workspace_id)
    .bind(bits(allow))
    .bind(bits(deny))
    .execute(&server.db)
    .await
    .unwrap();
}

/// The permissions of each account that is not an administrator, in the workspace (`None`)
/// and in each channel of it.
type Held = BTreeMap<(AccountId, Option<ChannelId>), WorkspacePermissions>;

/// What a member uses in a channel from the effective permissions: nothing without sight of
/// the channel, and no voice without "connect" (ADR 0301).
fn usable(effective: WorkspacePermissions) -> WorkspacePermissions {
    if !effective.contains(W::ViewChannel) {
        return WorkspacePermissions::EMPTY;
    }
    if effective.contains(W::Connect) {
        effective
    } else {
        effective.difference(set(&[
            W::Speak,
            W::Video,
            W::ShareScreen,
            W::UseVoiceActivity,
        ]))
    }
}

/// An evaluation of the rules before release 0.3.0, independent of the server: the
/// "everyone" role and the roles of a member, then the deny and the allow of the rule of the
/// member in the workspace; in a channel, the set of rules of the channel or of its category
/// while synced, in the order "everyone", the denies and the allows of the roles, then the
/// rule of the person.
async fn old_rules(server: &TestServer, workspace_id: WorkspaceId, accounts: &[AccountId]) -> Held {
    let db = &server.db;
    let roles: Vec<(WorkspaceRoleId, i64, bool)> = sqlx::query_as(
        "select id, permissions, is_everyone from workspace_roles where workspace_id = $1",
    )
    .bind(workspace_id)
    .fetch_all(db)
    .await
    .unwrap();
    let everyone = roles.iter().find(|role| role.2).unwrap().0;
    let permissions: HashMap<WorkspaceRoleId, WorkspacePermissions> =
        roles.iter().map(|role| (role.0, read(role.1))).collect();
    let channels: Vec<(ChannelId, Option<junto_protocol::ids::CategoryId>, bool)> = sqlx::query_as(
        "select id, category_id, permissions_synced from channels
             where workspace_id = $1 and deleted_at is null",
    )
    .bind(workspace_id)
    .fetch_all(db)
    .await
    .unwrap();
    let mut held = Held::new();
    for account_id in accounts {
        let own: Vec<WorkspaceRoleId> = sqlx::query_scalar(
            "select role_id from member_roles where workspace_id = $1 and account_id = $2",
        )
        .bind(workspace_id)
        .bind(account_id)
        .fetch_all(db)
        .await
        .unwrap();
        let rule: Option<(i64, i64)> = sqlx::query_as(
            "select allow, deny from workspace_person_overrides
             where workspace_id = $1 and account_id = $2",
        )
        .bind(workspace_id)
        .bind(account_id)
        .fetch_optional(db)
        .await
        .unwrap();
        let (allow, deny) = rule.map_or(
            (WorkspacePermissions::EMPTY, WorkspacePermissions::EMPTY),
            |(allow, deny)| (read(allow), read(deny)),
        );
        let base = own
            .iter()
            .fold(permissions[&everyone], |base, role| {
                base.union(permissions[role])
            })
            .difference(deny)
            .union(allow);
        held.insert((*account_id, None), base);
        for (channel_id, category_id, synced) in &channels {
            let (roles_sql, person_sql, place) = match category_id {
                Some(category_id) if *synced => (
                    "select role_id, allow, deny from category_overrides where category_id = $1",
                    "select allow, deny from category_person_overrides
                     where category_id = $1 and account_id = $2",
                    category_id.as_uuid(),
                ),
                _ => (
                    "select role_id, allow, deny from channel_overrides where channel_id = $1",
                    "select allow, deny from channel_person_overrides
                     where channel_id = $1 and account_id = $2",
                    channel_id.as_uuid(),
                ),
            };
            let role_rules: Vec<(WorkspaceRoleId, i64, i64)> = sqlx::query_as(roles_sql)
                .bind(place)
                .fetch_all(db)
                .await
                .unwrap();
            let person: Option<(i64, i64)> = sqlx::query_as(person_sql)
                .bind(place)
                .bind(account_id)
                .fetch_optional(db)
                .await
                .unwrap();
            let mut effective = base;
            if let Some((_, allow, deny)) = role_rules.iter().find(|rule| rule.0 == everyone) {
                effective = effective.difference(read(*deny)).union(read(*allow));
            }
            let (role_allow, role_deny) =
                role_rules.iter().filter(|rule| own.contains(&rule.0)).fold(
                    (WorkspacePermissions::EMPTY, WorkspacePermissions::EMPTY),
                    |(allow, deny), rule| (allow.union(read(rule.1)), deny.union(read(rule.2))),
                );
            effective = effective.difference(role_deny).union(role_allow);
            if let Some((allow, deny)) = person {
                effective = effective.difference(read(deny)).union(read(allow));
            }
            held.insert((*account_id, Some(*channel_id)), usable(effective));
        }
    }
    held
}

/// The same permissions as the server evaluates them now (ADR 0301).
async fn evaluated(server: &TestServer, workspace_id: WorkspaceId, accounts: &[AccountId]) -> Held {
    let mut connection = server.db.acquire().await.unwrap();
    let channels: Vec<ChannelId> = sqlx::query_scalar(
        "select id from channels where workspace_id = $1 and deleted_at is null",
    )
    .bind(workspace_id)
    .fetch_all(&mut *connection)
    .await
    .unwrap();
    let mut held = Held::new();
    for account_id in accounts {
        let access = Access::of_account(&mut connection, *account_id)
            .await
            .unwrap()
            .unwrap();
        let scope = access
            .workspace(&mut connection, workspace_id)
            .await
            .unwrap();
        let workspace = W::ALL
            .iter()
            .copied()
            .filter(|permission| {
                access.allows(Request::Workspace {
                    scope: &scope,
                    permission: *permission,
                })
            })
            .fold(WorkspacePermissions::EMPTY, |set, permission| {
                set.with(permission)
            });
        held.insert((*account_id, None), workspace);
        for channel_id in &channels {
            let usable = match access.channel(&mut connection, *channel_id).await {
                Ok((_, scope)) => set(&access.channel_permissions(&scope)),
                Err(_) => WorkspacePermissions::EMPTY,
            };
            held.insert((*account_id, Some(*channel_id)), usable);
        }
    }
    held
}

#[tokio::test]
async fn the_rules_of_single_members_become_roles_only_when_no_case_waits() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let ana = server.join(&owner, "Ana").await;
    let bruno = server.join(&owner, "Bruno").await;
    let cara = server.join(&owner, "Cara").await;
    let dana = server.join(&owner, "Dana").await;
    let eli = server.join(&owner, "Eli").await;
    let workspace = server.default_workspace(&owner).await;
    let reactors = server
        .grant_workspace(&owner, &bruno, workspace.id, &[W::AddReactions])
        .await;
    let plans = text_channel(&server, &owner, workspace.id, "plans").await;
    let everyone: WorkspaceRoleId = sqlx::query_scalar(
        "select id from workspace_roles where workspace_id = $1 and is_everyone",
    )
    .bind(workspace.id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    let before: i32 = sqlx::query_scalar("select position from workspace_roles where id = $1")
        .bind(reactors)
        .fetch_one(&server.db)
        .await
        .unwrap();
    forget_the_migrations_after_the_release(&server).await;

    // Ana gains nothing: her rule allows, and its deny takes what no role gives her. Bruno's
    // rule denies what a role grants him. Cara holds "manage roles" by her rule and would
    // stand above Ana and Eli, who stand at her level now. Dana left the workspace and keeps her
    // rule. Eli has no rule. The channel "plans" has rules of its own.
    rule(
        &server,
        workspace.id,
        ana.account_id,
        &[W::MentionEveryone, W::AttachFiles],
        &[W::ManageMessages],
    )
    .await;
    rule(
        &server,
        workspace.id,
        bruno.account_id,
        &[],
        &[W::AddReactions],
    )
    .await;
    rule(
        &server,
        workspace.id,
        cara.account_id,
        &[W::ManageRoles],
        &[],
    )
    .await;
    rule(&server, workspace.id, dana.account_id, &[W::Connect], &[]).await;
    sqlx::query("delete from workspace_members where account_id = $1")
        .bind(dana.account_id)
        .execute(&server.db)
        .await
        .unwrap();
    let place = (workspace.id, plans);
    channel_role_rule(&server, place, everyone, &[], &[W::AttachFiles]).await;
    channel_role_rule(&server, place, reactors, &[W::MentionEveryone], &[]).await;
    channel_person_rule(&server, place, eli.account_id, &[W::ManageMessages], &[]).await;

    let listed = cases(&server).await;
    assert_eq!(listed.len(), 3, "{listed:#?}");
    assert!(listed.contains(&Case::Gains {
        member: "Bruno".to_owned(),
        workspace: workspace.name.clone(),
        permissions: vec![W::AddReactions],
    }));
    assert!(listed.contains(&Case::Outranks {
        member: "Cara".to_owned(),
        workspace: workspace.name.clone(),
        over: vec!["Ana".to_owned(), "Eli".to_owned()],
        permissions: vec![W::ManageRoles],
    }));
    assert!(listed.contains(&Case::Former {
        account: "Dana".to_owned(),
        workspace: workspace.name.clone(),
        allow: vec![W::Connect],
        deny: Vec::new(),
    }));
    for case in &listed {
        assert!(!case.line().is_empty());
    }

    // The start refuses while a case waits, and changes nothing.
    let refused = start(&server).await;
    assert!(
        matches!(refused, Err(StartError::PersonRules(3))),
        "{refused:?}"
    );
    let rules: i64 = sqlx::query_scalar("select count(*) from workspace_person_overrides")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(rules, 4);

    // The administrator handles each case; the start then turns Ana's rule into a role, and
    // every account keeps what the old rules gave it, in the workspace and in each channel.
    for handled in [&bruno, &cara, &dana] {
        remove_rule(&server, handled.account_id).await;
    }
    assert!(cases(&server).await.is_empty());
    let accounts = [
        ana.account_id,
        bruno.account_id,
        cara.account_id,
        eli.account_id,
    ];
    let expected = old_rules(&server, workspace.id, &accounts).await;
    assert!(expected[&(ana.account_id, None)].contains(W::MentionEveryone));
    assert!(!expected[&(ana.account_id, Some(plans))].contains(W::AttachFiles));
    start(&server).await.unwrap();
    assert_eq!(evaluated(&server, workspace.id, &accounts).await, expected);
    let table: bool =
        sqlx::query_scalar("select to_regclass('public.workspace_person_overrides') is not null")
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert!(!table);
    let made: (String, i64, i32) = sqlx::query_as(
        "select r.name, r.permissions, r.position from workspace_roles r
         join member_roles m on m.role_id = r.id
         where m.account_id = $1 and r.workspace_id = $2",
    )
    .bind(ana.account_id)
    .bind(workspace.id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(
        made,
        (
            person_rules::generated_role_name("Ana"),
            bits(&[W::MentionEveryone, W::AttachFiles]),
            1
        )
    );
    // The role of now stands above the new one, in the same order.
    let after: i32 = sqlx::query_scalar("select position from workspace_roles where id = $1")
        .bind(reactors)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert!(after > 1 && before >= 1, "{before} {after}");
    // A second start finds nothing to do.
    assert!(cases(&server).await.is_empty());
    start(&server).await.unwrap();
}

/// A member without a role who gets the role of a rule stands above the members without a
/// role: with "manage channel", also by a rule of a channel, the member would set their
/// rules in channels and categories (ADR 0309).
#[tokio::test]
async fn a_member_who_would_act_on_people_by_a_made_role_waits() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let finn = server.join(&owner, "Finn").await;
    let gail = server.join(&owner, "Gail").await;
    let hana = server.join(&owner, "Hana").await;
    let workspace = server.default_workspace(&owner).await;
    let plans = text_channel(&server, &owner, workspace.id, "plans").await;
    forget_the_migrations_after_the_release(&server).await;

    // Finn's rule allows "mention everyone"; a rule of "plans" gives him "manage channel".
    rule(
        &server,
        workspace.id,
        finn.account_id,
        &[W::MentionEveryone],
        &[],
    )
    .await;
    channel_person_rule(
        &server,
        (workspace.id, plans),
        finn.account_id,
        &[W::ManageChannel],
        &[],
    )
    .await;
    let listed = cases(&server).await;
    assert_eq!(
        listed,
        [Case::Outranks {
            member: "Finn".to_owned(),
            workspace: workspace.name.clone(),
            over: vec!["Gail".to_owned(), "Hana".to_owned()],
            permissions: vec![W::ManageChannel],
        }]
    );
    assert!(listed[0].line().contains(W::ManageChannel.name()));
    assert!(matches!(
        start(&server).await,
        Err(StartError::PersonRules(1))
    ));

    // Without "manage channel" in "plans", the role acts on nobody.
    sqlx::query("delete from channel_person_overrides where account_id = $1")
        .bind(finn.account_id)
        .execute(&server.db)
        .await
        .unwrap();
    assert!(cases(&server).await.is_empty());
    let _ = (gail, hana);
}

/// A ban keeps the memberships and the roles of an account, which an unban restores: a
/// banned member whose rule denies what a role grants would gain it (ADR 0005).
#[tokio::test]
async fn a_banned_member_is_compared_with_the_roles_that_an_unban_restores() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let ivo = server.join(&owner, "Ivo").await;
    let workspace = server.default_workspace(&owner).await;
    server
        .grant_workspace(&owner, &ivo, workspace.id, &[W::ManageChannel])
        .await;
    server
        .put_empty(&owner, &format!("/api/v1/accounts/{}/ban", ivo.account_id))
        .await
        .empty(StatusCode::NO_CONTENT);
    forget_the_migrations_after_the_release(&server).await;
    rule(
        &server,
        workspace.id,
        ivo.account_id,
        &[],
        &[W::ManageChannel],
    )
    .await;
    assert_eq!(
        cases(&server).await,
        [Case::Gains {
            member: "Ivo".to_owned(),
            workspace: workspace.name.clone(),
            permissions: vec![W::ManageChannel],
        }]
    );
}
