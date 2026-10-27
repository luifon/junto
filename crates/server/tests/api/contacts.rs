//! The accounts that a caller may contact (ADR 0307): the list from which a person chooses
//! an account that is in no member list the person can open.

use std::collections::BTreeSet;

use junto_protocol::ServerPermission;
use junto_protocol::account::{AccountProfile, OwnAccount};
use junto_protocol::error::ErrorCode;
use junto_protocol::events::{Event, Level};
use junto_protocol::ids::{AccountId, ServerRoleId};
use junto_protocol::message::{ContactPage, Conversation};
use junto_protocol::structure::Workspace;
use reqwest::StatusCode;
use serde_json::json;

use crate::support::{Member, TestServer};

const CONTACTS: &str = "/api/v1/direct-messages/contacts";

async fn contacts(server: &TestServer, caller: &Member, query: &str) -> ContactPage {
    server
        .get(caller, &format!("{CONTACTS}{query}"))
        .await
        .ok(StatusCode::OK)
}

async fn reached(server: &TestServer, caller: &Member) -> BTreeSet<AccountId> {
    let page = contacts(server, caller, "").await;
    assert!(page.next_after.is_none());
    page.accounts.iter().map(|account| account.id).collect()
}

fn ids(members: &[&Member]) -> BTreeSet<AccountId> {
    members.iter().map(|member| member.account_id).collect()
}

/// The accounts of the people in the tests: Ann and Bea share the default workspace, Carl
/// and Eve the second one, Frank is alone in a third one, and Eve administers the server.
struct People {
    server: TestServer,
    owner: Member,
    ann: Member,
    bea: Member,
    carl: Member,
    eve: Member,
    /// The server role that gives Eve the administrator permission.
    eve_role: ServerRoleId,
    frank: Member,
}

async fn people() -> People {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = |name: &'static str| {
        let (server, owner) = (&server, &owner);
        async move {
            let created: Workspace = server
                .post(owner, "/api/v1/workspaces", &json!({ "name": name }))
                .await
                .ok(StatusCode::CREATED);
            created.id
        }
    };
    let second = workspace("Second").await;
    let third = workspace("Third").await;
    let ann = server.join(&owner, "Ann").await;
    let bea = server.join(&owner, "bea").await;
    let carl = server.join_into(&owner, "Carl", &[second]).await;
    let eve = server.join_into(&owner, "Eve", &[second]).await;
    let eve_role = server
        .grant_server(&owner, &eve, &[ServerPermission::Administrator])
        .await;
    let frank = server.join_into(&owner, "Frank", &[third]).await;
    People {
        server,
        owner,
        ann,
        bea,
        carl,
        eve,
        eve_role,
        frank,
    }
}

#[tokio::test]
async fn a_member_reaches_the_accounts_of_its_workspaces_and_the_administrators_only() {
    let People {
        server,
        owner,
        ann,
        bea,
        carl,
        eve,
        eve_role,
        frank,
    } = people().await;

    // Ann shares a workspace with Bea and reaches the two administrators; Carl, who shares
    // nothing with Ann and does not administer the server, is not listed.
    assert_eq!(reached(&server, &ann).await, ids(&[&owner, &bea, &eve]));
    assert_eq!(reached(&server, &carl).await, ids(&[&owner, &eve]));
    // Frank shares a workspace with nobody: the list holds only what the rule allows him,
    // the administrators.
    assert_eq!(reached(&server, &frank).await, ids(&[&owner, &eve]));

    // An administrator reaches every active account but itself.
    assert_eq!(
        reached(&server, &eve).await,
        ids(&[&owner, &ann, &bea, &carl, &frank])
    );
    assert_eq!(
        reached(&server, &owner).await,
        ids(&[&ann, &bea, &carl, &eve, &frank])
    );

    // Every listed account takes a conversation, and an account that is not listed is
    // refused as before.
    for account in reached(&server, &frank).await {
        server
            .post(
                &frank,
                "/api/v1/conversations",
                &json!({ "account_id": account }),
            )
            .await
            .ok::<Conversation>(StatusCode::OK);
    }
    server
        .post(
            &frank,
            "/api/v1/conversations",
            &json!({ "account_id": ann.account_id }),
        )
        .await
        .error(ErrorCode::NotFound);

    // A banned account is reached by nobody.
    server
        .put_empty(&owner, &format!("/api/v1/accounts/{}/ban", bea.account_id))
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(reached(&server, &ann).await, ids(&[&owner, &eve]));
    assert!(!reached(&server, &owner).await.contains(&bea.account_id));

    // The administrator permission is what reaches beyond the workspaces: without it, Eve
    // is reached by the accounts of the second workspace only.
    server
        .delete(
            &owner,
            &format!(
                "/api/v1/accounts/{}/server-roles/{eve_role}",
                eve.account_id
            ),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(reached(&server, &frank).await, ids(&[&owner]));
    assert_eq!(reached(&server, &eve).await, ids(&[&owner, &carl]));
}

#[tokio::test]
async fn the_contacts_are_found_by_a_part_of_the_name_in_pages_of_names() {
    let People {
        server,
        owner,
        ann,
        bea,
        eve,
        ..
    } = people().await;

    // A part of the name, whatever its case.
    let found = contacts(&server, &owner, "?q=%20B%20").await;
    assert_eq!(
        found
            .accounts
            .iter()
            .map(|account| account.id)
            .collect::<Vec<_>>(),
        [bea.account_id]
    );
    assert_eq!(found.accounts[0].name, "bea");
    assert!(contacts(&server, &ann, "?q=carl").await.accounts.is_empty());

    // Pages of one account follow the names in lower case, and the last page names no
    // next one.
    let mut names = Vec::new();
    let mut after = None::<String>;
    loop {
        let query = match &after {
            Some(after) => format!("?limit=1&after={after}"),
            None => "?limit=1".to_owned(),
        };
        let page = contacts(&server, &eve, &query).await;
        assert!(page.accounts.len() <= 1);
        names.extend(
            page.accounts
                .iter()
                .map(|account| account.name.to_lowercase()),
        );
        match page.next_after {
            Some(next) => after = Some(next),
            None => break,
        }
    }
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);
    assert_eq!(names.len(), 5);

    // A bad limit, a long part and an unknown cursor are refused.
    for query in [
        "?limit=0".to_owned(),
        "?limit=101".to_owned(),
        format!("?q={}", "a".repeat(65)),
        "?after=%25%25".to_owned(),
    ] {
        server
            .get(&owner, &format!("{CONTACTS}{query}"))
            .await
            .error(ErrorCode::InvalidRequest);
    }
}

/// An account sees the profile of every account that it may write to (ADR 0119,
/// ADR 0307), from the routes and from the events, and no longer when the contact ends.
#[tokio::test]
async fn the_profile_of_an_administrator_is_seen_by_the_accounts_that_may_write_to_it() {
    let People {
        server,
        owner,
        ann,
        carl,
        eve,
        eve_role,
        ..
    } = people().await;
    let profile = |member: &Member| format!("/api/v1/accounts/{}/profile", member.account_id);
    let avatar = |member: &Member| format!("/api/v1/accounts/{}/avatar", member.account_id);
    server
        .put_bytes(
            &eve,
            "/api/v1/account/avatar",
            None,
            junto_server::image::samples::png(8, 8),
        )
        .await
        .ok::<junto_protocol::account::Avatar>(StatusCode::OK);

    // Ann shares no workspace with Eve, and Eve administers the server.
    let seen: AccountProfile = server.get(&ann, &profile(&eve)).await.ok(StatusCode::OK);
    assert_eq!(seen.name, "Eve");
    assert_eq!(server.get(&ann, &avatar(&eve)).await.status, StatusCode::OK);
    server
        .get(&ann, &profile(&carl))
        .await
        .error(ErrorCode::NotFound);
    let mut to_ann = server.subscribed(&ann, Level::Full).await;
    server
        .patch(&eve, "/api/v1/account", &json!({ "name": "Evelyn" }))
        .await
        .ok::<OwnAccount>(StatusCode::OK);
    assert!(matches!(
        to_ann.event().await.1,
        Event::AccountProfileUpdated { profile } if profile.name == "Evelyn"
    ));

    // Without the administrator permission Eve is unrelated to Ann again.
    server
        .delete(
            &owner,
            &format!(
                "/api/v1/accounts/{}/server-roles/{eve_role}",
                eve.account_id
            ),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .get(&ann, &profile(&eve))
        .await
        .error(ErrorCode::NotFound);
    server
        .get(&ann, &avatar(&eve))
        .await
        .error(ErrorCode::NotFound);
    server
        .patch(&eve, "/api/v1/account", &json!({ "name": "Eve" }))
        .await
        .ok::<OwnAccount>(StatusCode::OK);
    assert!(server.traffic(&ann, &mut to_ann).await.is_empty());

    // An administrator that is removed is reached by nobody, and its profile is seen by
    // nobody who reached it only as an administrator.
    server
        .grant_server(&owner, &eve, &[ServerPermission::Administrator])
        .await;
    server
        .get(&ann, &profile(&eve))
        .await
        .ok::<AccountProfile>(StatusCode::OK);
    server
        .post(
            &owner,
            &format!("/api/v1/accounts/{}/removal", eve.account_id),
            &json!({}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .get(&ann, &profile(&eve))
        .await
        .error(ErrorCode::NotFound);
    assert!(!reached(&server, &ann).await.contains(&eve.account_id));
}

/// The rows that a list of contacts reads, for the candidates and for the decision on each
/// account of the page, follow what the caller may reach, not the accounts that it may not
/// see (ADR 0307). Ten thousand other accounts sort before the
/// people of the test, hold a server role without the administrator permission and are
/// members of a workspace of their own.
#[tokio::test]
async fn the_contacts_of_a_member_read_the_candidates_and_not_the_hidden_accounts() {
    use crate::plan_support::{analyze, explained, index_names, visited_rows};
    use junto_server::routes::contacts::{CONTACTS_OF_ADMINISTRATOR, CONTACTS_OF_MEMBER};
    use junto_server::store::{CONTACT_STANDING, FELLOW_MEMBERS};
    let People {
        server,
        owner,
        ann,
        bea,
        eve,
        ..
    } = people().await;
    let hidden: Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({ "name": "Hidden" }))
        .await
        .ok(StatusCode::CREATED);
    let role = server
        .grant_server(&owner, &owner, &[ServerPermission::CreateInvites])
        .await;
    sqlx::query(
        "insert into accounts(id,name,status,created_at)
         select lpad(to_hex(i),32,'0')::uuid,'Hidden','active',now()
         from generate_series(1,10000) i",
    )
    .execute(&mut server.test_data().await)
    .await
    .unwrap();
    sqlx::query(
        "insert into account_server_roles(account_id,role_id)
         select id,$1 from accounts where name='Hidden'",
    )
    .bind(role)
    .execute(&server.db)
    .await
    .unwrap();
    sqlx::query(
        "insert into workspace_members(workspace_id,account_id,joined_at)
         select $1,id,now() from accounts where name='Hidden'",
    )
    .bind(hidden.id)
    .execute(&server.db)
    .await
    .unwrap();
    analyze(
        &server.db,
        &[
            "accounts",
            "workspace_members",
            "account_server_roles",
            "server_roles",
        ],
    )
    .await;
    // The bit of the administrator permission in the stored set (ADR 0301).
    let administrator: i64 = 1;
    for settings in [
        "",
        "set local enable_nestloop = off; set local enable_hashjoin = off",
        "set local enable_nestloop = off; set local enable_mergejoin = off",
    ] {
        let mut transaction = server.db.begin().await.unwrap();
        if !settings.is_empty() {
            sqlx::raw_sql(settings)
                .execute(&mut *transaction)
                .await
                .unwrap();
        }
        let plan: serde_json::Value = sqlx::query_scalar(explained(CONTACTS_OF_MEMBER))
            .bind(ann.account_id)
            .bind("")
            .bind(None::<String>)
            .bind(None::<uuid::Uuid>)
            .bind(administrator)
            .bind(51_i64)
            .fetch_one(&mut *transaction)
            .await
            .unwrap();
        assert!(
            visited_rows(&plan[0]["Plan"]) < 100,
            "a member with {settings:?}: {plan}"
        );
        // The decision for each account of the page reads that account, its roles and the
        // workspaces of the caller.
        let page = [owner.account_id, bea.account_id, eve.account_id];
        let plan: serde_json::Value = sqlx::query_scalar(explained(CONTACT_STANDING))
            .bind(&page[..])
            .fetch_one(&mut *transaction)
            .await
            .unwrap();
        assert!(
            visited_rows(&plan[0]["Plan"]) < 100,
            "the standing with {settings:?}: {plan}"
        );
        let plan: serde_json::Value = sqlx::query_scalar(explained(FELLOW_MEMBERS))
            .bind(ann.account_id)
            .bind(&page[..])
            .fetch_one(&mut *transaction)
            .await
            .unwrap();
        assert!(
            visited_rows(&plan[0]["Plan"]) < 100,
            "the fellow members with {settings:?}: {plan}"
        );
        // An administrator reaches every account; a page reads its rows in the order of
        // the index.
        let plan: serde_json::Value = sqlx::query_scalar(explained(CONTACTS_OF_ADMINISTRATOR))
            .bind(eve.account_id)
            .bind("")
            .bind(None::<String>)
            .bind(None::<uuid::Uuid>)
            .bind(51_i64)
            .fetch_one(&mut *transaction)
            .await
            .unwrap();
        assert!(
            index_names(&plan)
                .iter()
                .any(|name| name == "accounts_contact_order"),
            "an administrator with {settings:?}: {plan}"
        );
        assert!(
            visited_rows(&plan[0]["Plan"]) < 200,
            "an administrator with {settings:?}: {plan}"
        );
    }
    // The answers are those of the rule: none of the hidden accounts for Ann.
    assert_eq!(reached(&server, &ann).await, ids(&[&owner, &bea, &eve]));
}
