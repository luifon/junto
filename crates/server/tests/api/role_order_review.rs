use crate::support::{Member, TestServer};
use junto_protocol::error::ErrorCode;
use junto_protocol::roles::{ServerRole, WorkspaceRole};
use junto_protocol::structure_file::{StructureExport, StructureFile};
use junto_protocol::{ServerPermission as S, WorkspacePermission as W};
use reqwest::StatusCode;
use serde_json::json;

async fn role(server: &TestServer, owner: &Member, permissions: &[S]) -> ServerRole {
    server
        .post(
            owner,
            "/api/v1/server-roles",
            &json!({"name":"role","permissions":permissions}),
        )
        .await
        .ok(StatusCode::CREATED)
}

#[tokio::test]
async fn review_h1_small_server_roles_do_not_bypass_workspace_order() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let manager = server.join(&owner, "Manager").await;
    let workspace = server.default_workspace(&owner).await.id;
    let top = server.grant_workspace(&owner, &owner, workspace, &[]).await;
    let own = server
        .grant_workspace(&owner, &manager, workspace, &[W::ManageRoles])
        .await;
    server
        .grant_server(&owner, &manager, &[S::CreateInvites])
        .await;
    let path = format!("/api/v1/workspaces/{workspace}/roles/{top}");
    server
        .patch(&manager, &path, &json!({"permissions":[]}))
        .await
        .error(ErrorCode::Forbidden);
    server
        .delete(&manager, &path)
        .await
        .error(ErrorCode::Forbidden);
    let assignment = format!(
        "/api/v1/workspaces/{workspace}/members/{}/roles/{top}",
        owner.account_id
    );
    server
        .put_empty(&manager, &assignment)
        .await
        .error(ErrorCode::Forbidden);
    server
        .delete(&manager, &assignment)
        .await
        .error(ErrorCode::Forbidden);
    server
        .patch(
            &manager,
            &format!("/api/v1/workspaces/{workspace}/roles/positions"),
            &json!({"positions":[{"role_id":top,"position":1},{"role_id":own,"position":2}]}),
        )
        .await
        .error(ErrorCode::Forbidden);
    let administrator = server.join(&owner, "Administrator").await;
    server
        .grant_server(&owner, &administrator, &[S::Administrator])
        .await;
    server
        .patch(&administrator, &path, &json!({"permissions":[]}))
        .await
        .ok::<WorkspaceRole>(StatusCode::OK);
    server
        .delete(&administrator, &assignment)
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .put_empty(&administrator, &assignment)
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .patch(
            &administrator,
            &format!("/api/v1/workspaces/{workspace}/roles/positions"),
            &json!({"positions":[{"role_id":top,"position":3}]}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .delete(&administrator, &path)
        .await
        .empty(StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn review_h2_unban_checks_every_restored_level() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let high = server.join(&owner, "High").await;
    let equal = server.join(&owner, "Equal").await;
    let low = server.join(&owner, "Low").await;
    let plain = server.join(&owner, "Plain").await;
    server
        .grant_server(&owner, &high, &[S::Administrator])
        .await;
    let low_role = server.grant_server(&owner, &low, &[S::BanMembers]).await;
    server
        .put_empty(
            &owner,
            &format!(
                "/api/v1/accounts/{}/server-roles/{low_role}",
                equal.account_id
            ),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    for target in [&high, &equal, &plain] {
        let path = format!("/api/v1/accounts/{}/ban", target.account_id);
        server
            .put_empty(&owner, &path)
            .await
            .empty(StatusCode::NO_CONTENT);
        if target.account_id == plain.account_id {
            server
                .delete(&low, &path)
                .await
                .empty(StatusCode::NO_CONTENT);
        } else {
            server.delete(&low, &path).await.error(ErrorCode::Forbidden);
            server
                .delete(&owner, &path)
                .await
                .empty(StatusCode::NO_CONTENT);
        }
    }
    let workspace = server.default_workspace(&owner).await.id;
    server
        .grant_workspace(&owner, &plain, workspace, &[W::ManageRoles])
        .await;
    let path = format!("/api/v1/accounts/{}/ban", plain.account_id);
    server
        .put_empty(&owner, &path)
        .await
        .empty(StatusCode::NO_CONTENT);
    server.delete(&low, &path).await.error(ErrorCode::Forbidden);
    server
        .grant_workspace(&owner, &low, workspace, &[W::ManageRoles])
        .await;
    server.delete(&low, &path).await.error(ErrorCode::Forbidden);
    server.grant_server(&owner, &low, &[S::Administrator]).await;
    server
        .delete(&low, &path)
        .await
        .empty(StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn review_m1_creation_has_constant_event_cost() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await.id;
    for workspace_scope in [false, true] {
        if workspace_scope {
            sqlx::query("insert into workspace_roles (id,workspace_id,name,permissions,created_at,position) select gen_random_uuid(),$1,'role',0,now(),n from generate_series(1,100) n").bind(workspace).execute(&mut server.test_data().await).await.unwrap();
        } else {
            sqlx::query("insert into server_roles (id,name,permissions,created_at,position) select gen_random_uuid(),'role',0,now(),n from generate_series(1,100) n").execute(&mut server.test_data().await).await.unwrap();
        }
        let table = if workspace_scope {
            "workspace_roles"
        } else {
            "server_roles"
        };
        let query = format!("select id from {table} where position > 0 order by position");
        let before: Vec<uuid::Uuid> = sqlx::query_scalar(sqlx::AssertSqlSafe(query.clone()))
            .fetch_all(&server.db)
            .await
            .unwrap();
        let audits = server.count("audit_entries").await;
        let events = server.count("events").await;
        let path = if workspace_scope {
            format!("/api/v1/workspaces/{workspace}/roles")
        } else {
            "/api/v1/server-roles".to_owned()
        };
        server
            .post(&owner, &path, &json!({"name":"new"}))
            .await
            .ok::<serde_json::Value>(StatusCode::CREATED);
        assert_eq!(server.count("audit_entries").await - audits, 1);
        assert_eq!(server.count("events").await - events, 2);
        let after: Vec<uuid::Uuid> = sqlx::query_scalar(sqlx::AssertSqlSafe(query))
            .fetch_all(&server.db)
            .await
            .unwrap();
        assert_eq!(&after[1..], before);
        let kinds: Vec<String> =
            sqlx::query_scalar("select kind from events order by position desc limit 2")
                .fetch_all(&server.db)
                .await
                .unwrap();
        assert!(!kinds.iter().any(|kind| kind == "permissions_changed"));
        let shift: serde_json::Value = sqlx::query_scalar(
            "select payload from events order by position desc offset 1 limit 1",
        )
        .fetch_one(&server.db)
        .await
        .unwrap();
        let positions = shift["positions"].as_array().unwrap();
        assert_eq!(positions.len(), 100);
        for (index, id) in before.iter().enumerate() {
            assert!(
                positions
                    .iter()
                    .any(|p| p["role_id"] == id.to_string() && p["position"] == json!(index + 2))
            );
        }
    }
}

#[tokio::test]
async fn review_m3_moderators_read_and_receive_role_positions() {
    use junto_protocol::events::Level;
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let moderator = server.join(&owner, "Moderator").await;
    server
        .grant_server(&owner, &moderator, &[S::BanMembers])
        .await;
    let target = role(&server, &owner, &[]).await;
    let roles: Vec<ServerRole> = server
        .get(&moderator, "/api/v1/server-roles")
        .await
        .ok(StatusCode::OK);
    assert!(roles.iter().any(|r| r.id == target.id && r.position == 1));
    let mut stream = server.subscribed(&moderator, Level::Full).await;
    server
        .patch(
            &owner,
            "/api/v1/server-roles/positions",
            &json!({"positions":[{"role_id":target.id,"position":10}]}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let received = server.traffic(&moderator, &mut stream).await;
    assert!(received.iter().any(|(_,event)|matches!(event,junto_protocol::events::Event::ServerRoleUpdated{role} if role.id==target.id && role.position==10)));
}

#[tokio::test]
async fn review_h3_setup_preserves_relative_order() {
    use crate::setup_tool::tool;
    use crate::support::Scratch;
    use junto_protocol::access_token::TokenAction;
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let administrator = server.join(&owner, "Administrator").await;
    server
        .grant_server(&owner, &administrator, &[S::Administrator])
        .await;
    role(&server, &owner, &[]).await;
    role(&server, &owner, &[]).await;
    let token = server
        .access_token(
            &administrator,
            &[TokenAction::ReadStructure, TokenAction::ManageStructure],
        )
        .await
        .token
        .expose()
        .to_owned();
    let exported = tool(&server, &token, &["export"]).await.succeeded().out;
    let mut file: StructureFile = serde_json::from_str(&exported).unwrap();
    let scratch = Scratch::new();
    let file_path = scratch.0.join("roles.json");
    let filename = file_path.to_str().unwrap();
    std::fs::write(&file_path, &exported).unwrap();
    role(&server, &owner, &[]).await;
    let before: StructureExport = server
        .get(&owner, "/api/v1/structure")
        .await
        .ok(StatusCode::OK);
    tool(&server, &token, &["apply", filename])
        .await
        .succeeded();
    let after: StructureExport = server
        .get(&owner, "/api/v1/structure")
        .await
        .ok(StatusCode::OK);
    assert_eq!(before, after);
    for positioned in [false, true] {
        let mut value = serde_json::to_value(&file).unwrap();
        for entry in value["server_roles"].as_array_mut().unwrap() {
            entry["position"] = json!(entry["position"].as_i64().unwrap() * 10);
        }
        let mut added = json!({"id":junto_protocol::ids::StructureId::generate(),"name":format!("added {positioned}")});
        if positioned {
            added["position"] = json!(1);
        }
        value["server_roles"].as_array_mut().unwrap().push(added);
        std::fs::write(&file_path, serde_json::to_vec(&value).unwrap()).unwrap();
        tool(&server, &token, &["apply", filename])
            .await
            .succeeded();
        let before: StructureExport = server
            .get(&owner, "/api/v1/structure")
            .await
            .ok(StatusCode::OK);
        tool(&server, &token, &["apply", filename])
            .await
            .succeeded();
        assert_eq!(
            before,
            server
                .get(&owner, "/api/v1/structure")
                .await
                .ok::<StructureExport>(StatusCode::OK)
        );
        file = before.structure;
    }
    for entry in &mut file.server_roles {
        entry.position = None;
    }
    std::fs::write(&file_path, serde_json::to_vec(&file).unwrap()).unwrap();
    let before: StructureExport = server
        .get(&owner, "/api/v1/structure")
        .await
        .ok(StatusCode::OK);
    tool(&server, &token, &["apply", filename])
        .await
        .succeeded();
    assert_eq!(
        before,
        server
            .get(&owner, "/api/v1/structure")
            .await
            .ok::<StructureExport>(StatusCode::OK)
    );
}

#[tokio::test]
async fn review_l2_deferred_unique_commit_is_retried() {
    use junto_server::{error::ApiResult, state::AppState, transact};
    async fn attempt(state: &AppState, attempts: &mut u32) -> ApiResult<()> {
        transact!(state, |connection| {
            *attempts += 1;
            if *attempts == 1 {
                sqlx::query("insert into commit_unique values (1),(1)")
                    .execute(&mut *connection)
                    .await?;
            }
            Ok(())
        })
    }
    let server = TestServer::start().await;
    sqlx::query("create table commit_unique (value integer unique deferrable initially deferred)")
        .execute(&server.db)
        .await
        .unwrap();
    let mut attempts = 0;
    attempt(&server.state, &mut attempts).await.unwrap();
    assert_eq!(attempts, 2);
}

#[tokio::test]
async fn review_l7_extreme_reorder_cannot_exhaust_creation() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let role = role(&server, &owner, &[]).await;
    for position in [-1, 0] {
        server
            .patch(
                &owner,
                "/api/v1/server-roles/positions",
                &json!({"positions":[{"role_id":role.id,"position":position}]}),
            )
            .await
            .error(ErrorCode::InvalidRequest);
    }
    server
        .post(
            &owner,
            "/api/v1/server-roles",
            &json!({"name":"still creates"}),
        )
        .await
        .ok::<ServerRole>(StatusCode::CREATED);
    server
        .patch(
            &owner,
            "/api/v1/server-roles/positions",
            &json!({"positions":[{"role_id":role.id,"position":i32::MAX}]}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let admin = server.administrator(&owner, "Manager").await;
    server
        .post(
            &admin,
            "/api/v1/server-roles",
            &json!({"name":"Still creates at the limit"}),
        )
        .await
        .ok::<ServerRole>(StatusCode::CREATED);
}

#[tokio::test]
async fn review_l5_export_uses_format_three() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let exported: StructureExport = server
        .get(&owner, "/api/v1/structure")
        .await
        .ok(StatusCode::OK);
    assert_eq!(exported.structure.format, 3);
}

#[tokio::test]
async fn review_m4_concurrent_reorders_and_creations_are_serializable() {
    use crate::support::drive_until;
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let first_role = role(&server, &owner, &[]).await;
    let second_role = role(&server, &owner, &[]).await;
    let first_body = json!({"positions":[{"role_id":first_role.id,"position":10}]});
    let second_body = json!({"positions":[{"role_id":second_role.id,"position":10}]});
    server.gate().arm(0);
    let first = server.patch(&owner, "/api/v1/server-roles/positions", &first_body);
    tokio::pin!(first);
    drive_until(&mut first, server.gate().arrived()).await;
    let second = server.patch(&owner, "/api/v1/server-roles/positions", &second_body);
    tokio::pin!(second);
    drive_until(&mut second, server.until_blocked()).await;
    server.gate().release();
    first.await.empty(StatusCode::NO_CONTENT);
    second.await.error(ErrorCode::InvalidRequest);
    let workspace: junto_protocol::structure::Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({"name":"Empty"}))
        .await
        .ok(StatusCode::CREATED);
    let path = format!("/api/v1/workspaces/{}/roles", workspace.id);
    let a = json!({"name":"first"});
    let b = json!({"name":"second"});
    server.gate().arm(0);
    let first = server.post(&owner, &path, &a);
    tokio::pin!(first);
    drive_until(&mut first, server.gate().arrived()).await;
    let second = server.post(&owner, &path, &b);
    tokio::pin!(second);
    drive_until(&mut second, server.until_blocked()).await;
    server.gate().release();
    first.await.ok::<WorkspaceRole>(StatusCode::CREATED);
    second.await.ok::<WorkspaceRole>(StatusCode::CREATED);
    let roles: Vec<WorkspaceRole> = server.get(&owner, &path).await.ok(StatusCode::OK);
    assert_eq!(
        roles.iter().map(|r| r.position).collect::<Vec<_>>(),
        [0, 1, 2]
    );
}

#[tokio::test]
async fn review_m4_reorder_overlapping_ban_rechecks_positions() {
    use crate::support::drive_until;
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let moderator = server.join(&owner, "Moderator").await;
    let target = server.join(&owner, "Target").await;
    server
        .grant_server(&owner, &moderator, &[S::BanMembers])
        .await;
    let target_role = server.grant_server(&owner, &target, &[]).await;
    server.gate().arm(0);
    let body = json!({"positions":[{"role_id":target_role,"position":10}]});
    let reorder = server.patch(&owner, "/api/v1/server-roles/positions", &body);
    tokio::pin!(reorder);
    drive_until(&mut reorder, server.gate().arrived()).await;
    let path = format!("/api/v1/accounts/{}/ban", target.account_id);
    server
        .put_empty(&moderator, &path)
        .await
        .empty(StatusCode::NO_CONTENT);
    server.gate().release();
    reorder.await.empty(StatusCode::NO_CONTENT);
    server
        .delete(&owner, &path)
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .put_empty(&moderator, &path)
        .await
        .error(ErrorCode::Forbidden);
}

#[tokio::test]
async fn review_m4_workspace_reorder_checks_old_new_and_equal_positions() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let manager = server.join(&owner, "Manager").await;
    let workspace = server.default_workspace(&owner).await.id;
    let top = server.grant_workspace(&owner, &owner, workspace, &[]).await;
    let own = server
        .grant_workspace(&owner, &manager, workspace, &[W::ManageRoles])
        .await;
    let low = server.grant_workspace(&owner, &owner, workspace, &[]).await;
    let path = format!("/api/v1/workspaces/{workspace}/roles/positions");
    for (role, position) in [(top, 1), (own, 4), (low, 2), (low, 4)] {
        server
            .patch(
                &manager,
                &path,
                &json!({"positions":[{"role_id":role,"position":position}]}),
            )
            .await
            .error(ErrorCode::Forbidden);
    }
    server
        .patch(
            &owner,
            &path,
            &json!({"positions":[{"role_id":own,"position":10}]}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .patch(
            &manager,
            &path,
            &json!({"positions":[{"role_id":low,"position":5}]}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn review_m3_member_management_token_reads_positions() {
    use junto_protocol::access_token::TokenAction;
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let owner_token = server
        .access_token(&owner, &[TokenAction::ManageMembers])
        .await;
    server
        .send::<serde_json::Value>(
            reqwest::Method::GET,
            "/api/v1/structure",
            Some(owner_token.token.expose()),
            None,
            None,
        )
        .await
        .error(ErrorCode::Forbidden);
    let moderator = server.join(&owner, "Moderator").await;
    let held = server
        .grant_server(&owner, &moderator, &[S::Administrator, S::BanMembers])
        .await;
    let token = server
        .access_token(&moderator, &[TokenAction::ManageMembers])
        .await;
    server
        .patch(
            &owner,
            &format!("/api/v1/server-roles/{held}"),
            &json!({"permissions":["ban_members"]}),
        )
        .await
        .ok::<ServerRole>(StatusCode::OK);
    server
        .send::<serde_json::Value>(
            reqwest::Method::GET,
            "/api/v1/server-roles",
            Some(token.token.expose()),
            None,
            None,
        )
        .await
        .ok::<Vec<ServerRole>>(StatusCode::OK);
    server
        .send::<serde_json::Value>(
            reqwest::Method::GET,
            "/api/v1/structure",
            Some(token.token.expose()),
            None,
            None,
        )
        .await
        .error(ErrorCode::Forbidden);
}

#[tokio::test]
async fn review_m4_moderation_tokens_and_unranked_targets() {
    use junto_protocol::access_token::TokenAction;
    use reqwest::Method;
    for removal in [false, true] {
        let server = TestServer::start().await;
        let owner = server.owner().await;
        let high = server.join(&owner, "High").await;
        let low = server.join(&owner, "Low").await;
        let plain = server.join(&owner, "Plain").await;
        server
            .grant_server(&owner, &high, &[S::Administrator])
            .await;
        let held = server
            .grant_server(&owner, &low, &[S::Administrator, S::BanMembers])
            .await;
        let low_token = server
            .access_token(&low, &[TokenAction::ManageMembers])
            .await;
        server
            .patch(
                &owner,
                &format!("/api/v1/server-roles/{held}"),
                &json!({"permissions":["ban_members"]}),
            )
            .await
            .ok::<ServerRole>(StatusCode::OK);
        let owner_token = server
            .access_token(&owner, &[TokenAction::ManageMembers])
            .await;
        let action = if removal { "removal" } else { "ban" };
        let method = if removal { Method::POST } else { Method::PUT };
        let path = format!("/api/v1/accounts/{}/{action}", high.account_id);
        server
            .send::<serde_json::Value>(
                method.clone(),
                &path,
                Some(low_token.token.expose()),
                None,
                None,
            )
            .await
            .error(ErrorCode::Forbidden);
        server
            .send::<serde_json::Value>(
                method.clone(),
                &path,
                Some(owner_token.token.expose()),
                None,
                None,
            )
            .await
            .empty(StatusCode::NO_CONTENT);
        let path = format!("/api/v1/accounts/{}/{action}", plain.account_id);
        server
            .send::<serde_json::Value>(method, &path, Some(low_token.token.expose()), None, None)
            .await
            .empty(StatusCode::NO_CONTENT);
    }
}

#[tokio::test]
async fn review_m4_bulk_refusals_roll_back_every_prior_change() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let administrator = server.join(&owner, "Administrator").await;
    let top = role(&server, &owner, &[]).await;
    let own = server
        .grant_server(&owner, &administrator, &[S::Administrator])
        .await;
    let before: StructureExport = server
        .get(&owner, "/api/v1/structure")
        .await
        .ok(StatusCode::OK);
    for id in [top.id, own] {
        for bad in [
            json!({"change":"update_server_role","id":id,"name":"bad"}),
            json!({"change":"delete_server_role","id":id}),
            json!({"change":"reorder_server_roles","positions":[{"id":id,"position":10}]}),
        ] {
            let audits = server.count("audit_entries").await;
            let events = server.count("events").await;
            server.post(&administrator,"/api/v1/structure/changes",&json!({"revision":before.revision,"changes":[{"change":"create_server_role","id":junto_protocol::ids::StructureId::generate(),"name":"rolled back","permissions":[]},bad]})).await.error(ErrorCode::Forbidden);
            assert_eq!(
                before,
                server
                    .get(&owner, "/api/v1/structure")
                    .await
                    .ok::<StructureExport>(StatusCode::OK)
            );
            assert_eq!(events, server.count("events").await);
            assert_eq!(audits, server.count("audit_entries").await);
        }
    }
}

#[tokio::test]
async fn review_m4_tokens_reorder_at_both_levels() {
    use junto_protocol::access_token::TokenAction;
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let administrator = server.join(&owner, "Administrator").await;
    let own = server
        .grant_server(&owner, &administrator, &[S::Administrator])
        .await;
    let low = role(&server, &owner, &[]).await;
    let workspace = server.default_workspace(&owner).await.id;
    let workspace_role = server.grant_workspace(&owner, &owner, workspace, &[]).await;
    let token = server
        .access_token(&administrator, &[TokenAction::ManageStructure])
        .await;
    server
        .send::<serde_json::Value>(
            reqwest::Method::PATCH,
            "/api/v1/server-roles/positions",
            Some(token.token.expose()),
            Some(&json!({"positions":[{"role_id":own,"position":10}]})),
            None,
        )
        .await
        .error(ErrorCode::Forbidden);
    server
        .patch(
            &owner,
            "/api/v1/server-roles/positions",
            &json!({"positions":[{"role_id":own,"position":10}]}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    for (path, id) in [
        (
            "/api/v1/server-roles/positions".to_owned(),
            low.id.to_string(),
        ),
        (
            format!("/api/v1/workspaces/{workspace}/roles/positions"),
            workspace_role.to_string(),
        ),
    ] {
        server
            .send::<serde_json::Value>(
                reqwest::Method::PATCH,
                &path,
                Some(token.token.expose()),
                Some(&json!({"positions":[{"role_id":id,"position":5}]})),
                None,
            )
            .await
            .empty(StatusCode::NO_CONTENT);
    }
}

#[tokio::test]
async fn review_m4_everyone_alone_cannot_edit_itself() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let manager = server.join(&owner, "Manager").await;
    let workspace = server.default_workspace(&owner).await.id;
    let roles: Vec<WorkspaceRole> = server
        .get(&owner, &format!("/api/v1/workspaces/{workspace}/roles"))
        .await
        .ok(StatusCode::OK);
    let everyone = roles.iter().find(|r| r.is_everyone).unwrap();
    let path = format!("/api/v1/workspaces/{workspace}/roles/{}", everyone.id);
    server
        .patch(
            &owner,
            &path,
            &json!({"permissions":["manage_roles","view_channel"]}),
        )
        .await
        .ok::<WorkspaceRole>(StatusCode::OK);
    server
        .patch(&manager, &path, &json!({"permissions":[]}))
        .await
        .error(ErrorCode::Forbidden);
}

#[tokio::test]
async fn review_q1_administrator_may_revoke_an_owner_token() {
    use junto_protocol::access_token::TokenAction;
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let administrator = server.join(&owner, "Administrator").await;
    server
        .grant_server(&owner, &administrator, &[S::Administrator])
        .await;
    let token = server
        .access_token(&owner, &[TokenAction::ReadStructure])
        .await;
    server
        .delete(
            &administrator,
            &format!("/api/v1/access-tokens/{}", token.access_token.id),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .send::<serde_json::Value>(
            reqwest::Method::GET,
            "/api/v1/structure",
            Some(token.token.expose()),
            None,
            None,
        )
        .await
        .error(ErrorCode::Unauthenticated);
}

#[tokio::test]
async fn review_m4_reorder_notifies_holders_without_leaking_workspaces() {
    use junto_protocol::events::{Event, Level};
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let outsider = server.join(&owner, "Outsider").await;
    let workspace: junto_protocol::structure::Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({"name":"Hidden"}))
        .await
        .ok(StatusCode::CREATED);
    let held = server
        .grant_workspace(&owner, &owner, workspace.id, &[])
        .await;
    let mut inside = server.subscribed(&owner, Level::Full).await;
    let mut outside = server.subscribed(&outsider, Level::Full).await;
    server
        .patch(
            &owner,
            &format!("/api/v1/workspaces/{}/roles/positions", workspace.id),
            &json!({"positions":[{"role_id":held,"position":10}]}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let events = server.traffic(&owner, &mut inside).await;
    assert!(events.iter().any(|(_,event)|matches!(event,Event::PermissionsChanged{workspace_id:Some(id),..} if *id==workspace.id)));
    assert!(server.traffic(&outsider, &mut outside).await.is_empty());
    assert!(!outside.raw.join("\n").contains(&held.to_string()));
}

#[tokio::test]
async fn review_l6_unknown_unassignment_is_not_found() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await.id;
    for path in [
        format!(
            "/api/v1/accounts/{}/server-roles/{}",
            owner.account_id,
            junto_protocol::ids::ServerRoleId::generate()
        ),
        format!(
            "/api/v1/workspaces/{workspace}/members/{}/roles/{}",
            owner.account_id,
            junto_protocol::ids::WorkspaceRoleId::generate()
        ),
    ] {
        server
            .delete(&owner, &path)
            .await
            .error(ErrorCode::NotFound);
    }
}

#[tokio::test]
async fn review_m1_bulk_creation_at_the_real_change_limit() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let before: StructureExport = server
        .get(&owner, "/api/v1/structure")
        .await
        .ok(StatusCode::OK);
    let changes:Vec<_>=(0..junto_protocol::structure_file::MOST_CHANGES).map(|n|json!({"change":"create_server_role","id":junto_protocol::ids::StructureId::generate(),"name":format!("role {n}"),"permissions":[]})).collect();
    let audits = server.count("audit_entries").await;
    let events = server.count("events").await;
    server
        .post(
            &owner,
            "/api/v1/structure/changes",
            &json!({"revision":before.revision,"changes":changes}),
        )
        .await
        .ok::<junto_protocol::structure_file::StructureApplied>(StatusCode::OK);
    assert_eq!(server.count("audit_entries").await - audits, 200);
    assert_eq!(server.count("events").await - events, 470);
    let roles: Vec<ServerRole> = server
        .get(&owner, "/api/v1/server-roles")
        .await
        .ok(StatusCode::OK);
    assert_eq!(roles.len(), 200);
    assert_eq!(
        roles.iter().map(|r| r.position).collect::<Vec<_>>(),
        (1..=200).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn review_h2_workspace_administrators_compare_only_admin_roles() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let high = server.join(&owner, "High").await;
    let low = server.join(&owner, "Low").await;
    let high_role = server
        .grant_server(&owner, &high, &[S::Administrator])
        .await;
    let low_role = server.grant_server(&owner, &low, &[S::Administrator]).await;
    let unrelated = server.grant_server(&owner, &low, &[S::CreateInvites]).await;
    server.patch(&owner,"/api/v1/server-roles/positions",&json!({"positions":[{"role_id":high_role,"position":20},{"role_id":low_role,"position":10},{"role_id":unrelated,"position":100}]})).await.empty(StatusCode::NO_CONTENT);
    let workspace = server.default_workspace(&owner).await.id;
    server.grant_workspace(&owner, &high, workspace, &[]).await;
    let path = format!("/api/v1/accounts/{}/ban", high.account_id);
    server
        .put_empty(&owner, &path)
        .await
        .empty(StatusCode::NO_CONTENT);
    server.delete(&low, &path).await.error(ErrorCode::Forbidden);
    server
        .delete(&owner, &path)
        .await
        .empty(StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn review_h2_unban_checks_workspaces_beyond_the_first_page() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let actor = server.join(&owner, "Moderator").await;
    let target = server.join(&owner, "Target").await;
    server.grant_server(&owner, &actor, &[S::BanMembers]).await;
    let workspaces:Vec<uuid::Uuid>=sqlx::query_scalar("insert into workspaces (id,name,created_at) select gen_random_uuid(),'restored',now() from generate_series(1,65) returning id").fetch_all(&mut server.test_data().await).await.unwrap();
    for account in [actor.account_id, target.account_id] {
        sqlx::query("insert into workspace_members (workspace_id,account_id,joined_at) select unnest($1::uuid[]),$2,now()").bind(&workspaces).bind(account).execute(&server.db).await.unwrap();
    }
    sqlx::query("insert into workspace_roles (id,workspace_id,name,permissions,is_everyone,created_at,position) select gen_random_uuid(),unnest($1::uuid[]),'everyone',0,true,now(),0").bind(&workspaces).execute(&server.db).await.unwrap();
    for (account, position) in [(target.account_id, 1), (actor.account_id, 2)] {
        sqlx::query("with roles as (insert into workspace_roles (id,workspace_id,name,permissions,created_at,position) select gen_random_uuid(),unnest($1::uuid[]),'held',0,now(),$2 returning id,workspace_id) insert into member_roles (workspace_id,account_id,role_id) select workspace_id,$3,id from roles").bind(&workspaces).bind(position).bind(account).execute(&server.db).await.unwrap();
    }
    sqlx::query("update workspace_roles set position=3 where position=1 and workspace_id=$1")
        .bind(workspaces.iter().max().unwrap())
        .execute(&server.db)
        .await
        .unwrap();
    let path = format!("/api/v1/accounts/{}/ban", target.account_id);
    server
        .put_empty(&owner, &path)
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .delete(&actor, &path)
        .await
        .error(ErrorCode::Forbidden);
    server
        .delete(&owner, &path)
        .await
        .empty(StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn review_m4_two_server_creations_in_an_empty_scope() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let a = json!({"name":"first"});
    let b = json!({"name":"second"});
    let (first, second) = tokio::join!(
        server.post(&owner, "/api/v1/server-roles", &a),
        server.post(&owner, "/api/v1/server-roles", &b),
    );
    first.ok::<ServerRole>(StatusCode::CREATED);
    second.ok::<ServerRole>(StatusCode::CREATED);
    let roles: Vec<ServerRole> = server
        .get(&owner, "/api/v1/server-roles")
        .await
        .ok(StatusCode::OK);
    assert_eq!(roles.iter().map(|r| r.position).collect::<Vec<_>>(), [1, 2]);
}
