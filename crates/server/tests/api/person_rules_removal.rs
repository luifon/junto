//! The removal of the workspace permission rules of single members (ADR 0301): the cases that
//! `check-upgrade` lists, the refusal of a start while one waits, and the roles that the start
//! makes from the rules once none waits.

use junto_authz::{Request, WorkspacePermission as W, WorkspacePermissions};
use junto_protocol::ids::{AccountId, WorkspaceId};
use junto_server::access::Access;
use junto_server::bootstrap::{self, StartError};
use junto_server::person_rules::{self, Case};

use crate::support::{TestServer, test_config};
use crate::upgrades::forget_the_newest_migration;

fn bits(permissions: &[W]) -> i64 {
    let set = permissions
        .iter()
        .fold(WorkspacePermissions::EMPTY, |set, permission| {
            set.with(*permission)
        });
    i64::try_from(set.bits()).unwrap()
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

/// What an account holds in a workspace by the everyone role and its roles, as the server
/// evaluates it (ADR 0301).
async fn held(
    server: &TestServer,
    workspace_id: WorkspaceId,
    account_id: AccountId,
) -> WorkspacePermissions {
    let mut connection = server.db.acquire().await.unwrap();
    let access = Access::of_account(&mut connection, account_id)
        .await
        .unwrap()
        .unwrap();
    let scope = access
        .workspace(&mut connection, workspace_id)
        .await
        .unwrap();
    W::ALL
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
        })
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

#[tokio::test]
async fn the_rules_of_single_members_become_roles_only_when_no_case_waits() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let ana = server.join(&owner, "Ana").await;
    let bruno = server.join(&owner, "Bruno").await;
    let cara = server.join(&owner, "Cara").await;
    let dana = server.join(&owner, "Dana").await;
    let workspace = server.default_workspace(&owner).await;
    let reactors = server
        .grant_workspace(&owner, &bruno, workspace.id, &[W::AddReactions])
        .await;
    let before: i32 = sqlx::query_scalar("select position from workspace_roles where id = $1")
        .bind(reactors)
        .fetch_one(&server.db)
        .await
        .unwrap();
    forget_the_newest_migration(&server).await;

    // Ana gains nothing: her rule only allows. Bruno's rule denies what a role grants him.
    // Cara holds "manage roles" by her rule and would stand above the role of Ana, who stands
    // at her level now. Dana left the workspace and keeps her rule.
    rule(
        &server,
        workspace.id,
        ana.account_id,
        &[W::MentionEveryone],
        &[],
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
        over: vec!["Ana".to_owned()],
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

    // The administrator handles each case; the start then turns Ana's rule into a role.
    for handled in [&bruno, &cara, &dana] {
        remove_rule(&server, handled.account_id).await;
    }
    assert!(cases(&server).await.is_empty());
    let expected = held(&server, workspace.id, ana.account_id)
        .await
        .union(WorkspacePermissions::EMPTY.with(W::MentionEveryone));
    start(&server).await.unwrap();
    // What Ana may do is the same: the everyone role, her roles and the allows of her rule.
    assert_eq!(held(&server, workspace.id, ana.account_id).await, expected);
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
            bits(&[W::MentionEveryone]),
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
