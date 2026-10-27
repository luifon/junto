use junto_protocol::error::ErrorCode;
use junto_protocol::roles::{ServerRole, WorkspaceRole};
use junto_protocol::snapshot::Snapshot;
use junto_protocol::{ServerPermission as S, WorkspacePermission as W};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::support::{Member, TestServer};

async fn server_role(server: &TestServer, owner: &Member, permissions: &[S]) -> ServerRole {
    server
        .post(
            owner,
            "/api/v1/server-roles",
            &json!({"name":"role", "permissions":permissions}),
        )
        .await
        .ok(StatusCode::CREATED)
}

#[tokio::test]
async fn ranked_moderation() {
    for removal in [false, true] {
        let server = TestServer::start().await;
        let owner = server.owner().await;
        let high = server.join(&owner, "High").await;
        let equal = server.join(&owner, "Equal").await;
        let low = server.join(&owner, "Low").await;
        let high_role = server
            .grant_server(&owner, &high, &[S::Administrator])
            .await;
        server
            .put_empty(
                &owner,
                &format!(
                    "/api/v1/accounts/{}/server-roles/{high_role}",
                    equal.account_id
                ),
            )
            .await
            .empty(StatusCode::NO_CONTENT);
        server.grant_server(&owner, &low, &[S::Administrator]).await;
        let action = if removal { "removal" } else { "ban" };
        for (actor, target) in [
            (&low, &high),
            (&high, &equal),
            (&high, &owner),
            (&high, &high),
        ] {
            let route = format!("/api/v1/accounts/{}/{action}", target.account_id);
            let reply = if removal {
                server.post(actor, &route, &Value::Null).await
            } else {
                server.put_empty(actor, &route).await
            };
            if target.account_id == owner.account_id {
                reply.error(ErrorCode::OwnerProtected);
            } else if target.account_id == actor.account_id {
                reply.error(ErrorCode::OwnAccountTarget);
            } else {
                reply.error(ErrorCode::Forbidden);
            }
        }
        for (actor, target) in [(&high, &low), (&owner, &high)] {
            let route = format!("/api/v1/accounts/{}/{action}", target.account_id);
            let reply = if removal {
                server.post(actor, &route, &Value::Null).await
            } else {
                server.put_empty(actor, &route).await
            };
            reply.empty(StatusCode::NO_CONTENT);
        }
    }
}

#[tokio::test]
async fn ranked_assignments_and_role_changes() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let manager = server.join(&owner, "Manager").await;
    let top = server_role(&server, &owner, &[S::Administrator]).await;
    let own = server
        .grant_server(&owner, &manager, &[S::Administrator])
        .await;
    let low = server_role(&server, &owner, &[]).await;
    for role in [top.id, own] {
        let route = format!("/api/v1/server-roles/{role}");
        server
            .patch(&manager, &route, &json!({"name":"changed"}))
            .await
            .error(ErrorCode::Forbidden);
        server
            .delete(&manager, &route)
            .await
            .error(ErrorCode::Forbidden);
        let assignment = format!(
            "/api/v1/accounts/{}/server-roles/{role}",
            manager.account_id
        );
        server
            .put_empty(&manager, &assignment)
            .await
            .error(ErrorCode::Forbidden);
        server
            .delete(&manager, &assignment)
            .await
            .error(ErrorCode::Forbidden);
    }
    for target in [&manager, &owner] {
        let route = format!(
            "/api/v1/accounts/{}/server-roles/{}",
            target.account_id, low.id
        );
        server
            .put_empty(&manager, &route)
            .await
            .empty(StatusCode::NO_CONTENT);
        server
            .delete(&manager, &route)
            .await
            .empty(StatusCode::NO_CONTENT);
    }
    let route = format!("/api/v1/server-roles/{}", low.id);
    server
        .patch(&manager, &route, &json!({"name":"changed"}))
        .await
        .ok::<ServerRole>(StatusCode::OK);
    server
        .delete(&manager, &route)
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .delete(&owner, &format!("/api/v1/server-roles/{own}"))
        .await
        .empty(StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn ranked_workspace_roles_and_two_levels() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let manager = server.join(&owner, "Manager").await;
    let workspace = server.default_workspace(&owner).await.id;
    let top = server.grant_workspace(&owner, &owner, workspace, &[]).await;
    let own = server
        .grant_workspace(&owner, &manager, workspace, &[W::ManageRoles])
        .await;
    let low = server.grant_workspace(&owner, &owner, workspace, &[]).await;
    for role in [top, own] {
        let route = format!("/api/v1/workspaces/{workspace}/roles/{role}");
        server
            .patch(&manager, &route, &json!({"name":"changed"}))
            .await
            .error(ErrorCode::Forbidden);
        server
            .delete(&manager, &route)
            .await
            .error(ErrorCode::Forbidden);
        let assignment = format!(
            "/api/v1/workspaces/{workspace}/members/{}/roles/{role}",
            manager.account_id
        );
        server
            .put_empty(&manager, &assignment)
            .await
            .error(ErrorCode::Forbidden);
        server
            .delete(&manager, &assignment)
            .await
            .error(ErrorCode::Forbidden);
    }
    for target in [&owner, &manager] {
        let assignment = format!(
            "/api/v1/workspaces/{workspace}/members/{}/roles/{low}",
            target.account_id
        );
        server
            .put_empty(&manager, &assignment)
            .await
            .empty(StatusCode::NO_CONTENT);
        server
            .delete(&manager, &assignment)
            .await
            .empty(StatusCode::NO_CONTENT);
    }
    let route = format!("/api/v1/workspaces/{workspace}/roles/{low}");
    server
        .patch(&manager, &route, &json!({"name":"changed"}))
        .await
        .ok::<WorkspaceRole>(StatusCode::OK);
    server
        .delete(&manager, &route)
        .await
        .empty(StatusCode::NO_CONTENT);
    // A non-administrator server role does not supply a workspace position.
    server
        .grant_server(&owner, &manager, &[S::CreateInvites])
        .await;
    server
        .patch(
            &manager,
            &format!("/api/v1/workspaces/{workspace}/roles/{top}"),
            &json!({"name":"changed"}),
        )
        .await
        .error(ErrorCode::Forbidden);
    server
        .delete(
            &owner,
            &format!("/api/v1/workspaces/{workspace}/roles/{own}"),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .patch(
            &manager,
            &format!("/api/v1/workspaces/{workspace}/roles/{top}"),
            &json!({"name":"again"}),
        )
        .await
        .error(ErrorCode::Forbidden);
}

#[tokio::test]
async fn ranked_reorder_atomicity_and_fresh_permissions() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let manager = server.join(&owner, "Manager").await;
    let top = server_role(&server, &owner, &[]).await;
    let own = server
        .grant_server(&owner, &manager, &[S::Administrator])
        .await;
    let low = server_role(&server, &owner, &[]).await;
    let route = "/api/v1/server-roles/positions";
    let before: Value = server
        .get(&owner, "/api/v1/server-roles")
        .await
        .ok(StatusCode::OK);
    for body in [
        json!({"positions":[{"role_id":own,"position":10}]}),
        json!({"positions":[{"role_id":low.id,"position":2}]}),
        json!({"positions":[{"role_id":top.id,"position":10}]}),
    ] {
        server
            .patch(&manager, route, &body)
            .await
            .error(ErrorCode::Forbidden);
        assert_eq!(
            before,
            server
                .get(&owner, "/api/v1/server-roles")
                .await
                .ok::<Value>(StatusCode::OK)
        );
    }
    for body in [
        json!({"positions":[{"role_id":low.id,"position":3}]}),
        json!({"positions":[{"role_id":low.id,"position":0}]}),
        json!({"positions":[{"role_id":low.id,"position":5},{"role_id":low.id,"position":6}]}),
        json!({"positions":[{"role_id":low.id,"position":5},{"role_id":own,"position":5}]}),
    ] {
        server
            .patch(&owner, route, &body)
            .await
            .error(ErrorCode::InvalidRequest);
        assert_eq!(
            before,
            server
                .get(&owner, "/api/v1/server-roles")
                .await
                .ok::<Value>(StatusCode::OK)
        );
    }
    server
        .patch(
            &owner,
            route,
            &json!({"positions":[{"role_id":low.id,"position":3},{"role_id":top.id,"position":1}]}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .patch(
            &manager,
            &format!("/api/v1/server-roles/{}", low.id),
            &json!({"name":"no"}),
        )
        .await
        .error(ErrorCode::Forbidden);
    server
        .patch(
            &manager,
            &format!("/api/v1/server-roles/{}", top.id),
            &json!({"name":"yes"}),
        )
        .await
        .ok::<ServerRole>(StatusCode::OK);
}

#[tokio::test]
async fn reorder_events_audit_snapshot_and_storage() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await.id;
    let a = server.grant_workspace(&owner, &owner, workspace, &[]).await;
    let b = server.grant_workspace(&owner, &owner, workspace, &[]).await;
    let route = format!("/api/v1/workspaces/{workspace}/roles/positions");
    let body = json!({"positions":[{"role_id":a,"position":1},{"role_id":b,"position":2}]});
    server
        .patch(&owner, &route, &body)
        .await
        .empty(StatusCode::NO_CONTENT);
    let updates: Vec<sqlx::types::Json<junto_protocol::events::Event>> = sqlx::query_scalar(
        "select payload from events where kind = 'workspace_role_updated' order by position desc limit 2",
    ).fetch_all(&server.db).await.unwrap();
    for (id, position) in [(a, 1), (b, 2)] {
        assert!(updates.iter().any(|event| matches!(&event.0, junto_protocol::events::Event::WorkspaceRoleUpdated { role } if role.id == id && role.position == position)));
    }
    let audit: junto_protocol::audit::AuditPage = server
        .get(&owner, &format!("/api/v1/workspaces/{workspace}/audit-log"))
        .await
        .ok(StatusCode::OK);
    for (role_id, position) in [(a, 1), (b, 2)] {
        assert!(audit.entries.iter().any(|entry| entry.event
            == junto_protocol::audit::AuditEvent::WorkspaceRolePositionChanged {
                role_id,
                position
            }));
    }
    let snapshot: Snapshot = server
        .get(&owner, "/api/v1/snapshot")
        .await
        .ok(StatusCode::OK);
    let roles = &snapshot
        .workspaces
        .iter()
        .find(|w| w.workspace.id == workspace)
        .unwrap()
        .roles;
    assert_eq!(roles.iter().find(|r| r.id == a).unwrap().position, 1);
    assert_eq!(roles.iter().find(|r| r.id == b).unwrap().position, 2);
    let everyone = roles.iter().find(|r| r.is_everyone).unwrap();
    assert_eq!(everyone.position, 0);
    server
        .patch(
            &owner,
            &route,
            &json!({"positions":[{"role_id":everyone.id,"position":3}]}),
        )
        .await
        .error(ErrorCode::InvalidRequest);
    let events: i64 = sqlx::query_scalar("select count(*) from events")
        .fetch_one(&server.db)
        .await
        .unwrap();
    let audits: i64 = sqlx::query_scalar("select count(*) from audit_entries")
        .fetch_one(&server.db)
        .await
        .unwrap();
    server
        .patch(&owner, &route, &body)
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(
        events,
        sqlx::query_scalar::<_, i64>("select count(*) from events")
            .fetch_one(&server.db)
            .await
            .unwrap()
    );
    assert_eq!(
        audits,
        sqlx::query_scalar::<_, i64>("select count(*) from audit_entries")
            .fetch_one(&server.db)
            .await
            .unwrap()
    );
    assert!(
        sqlx::query("update workspace_roles set position = 1 where id = $1")
            .bind(b)
            .execute(&server.db)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn role_order_visibility() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let outsider = server.join(&owner, "Outsider").await;
    let workspace: junto_protocol::structure::Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({"name":"Hidden"}))
        .await
        .ok(StatusCode::CREATED);
    let role = server
        .grant_workspace(&owner, &owner, workspace.id, &[])
        .await;
    let known = format!("/api/v1/workspaces/{}/roles/{role}", workspace.id);
    let absent = format!(
        "/api/v1/workspaces/{}/roles/{}",
        workspace.id,
        junto_protocol::ids::WorkspaceRoleId::generate()
    );
    let a = server.patch(&outsider, &known, &json!({"name":"no"})).await;
    let b = server
        .patch(&outsider, &absent, &json!({"name":"no"}))
        .await;
    assert_eq!(a.observable(), b.observable());
}

#[tokio::test]
async fn hierarchy_exempt_calls() {
    let scene = crate::calls_support::Scene::start().await;
    let server = &scene.server;
    let high = &scene.ann;
    let low = &scene.ben;
    server
        .grant_server(&scene.owner, high, &[S::Administrator])
        .await;
    let low_role = server
        .grant_server(&scene.owner, low, &[S::Administrator])
        .await;
    let equal = server.join(&scene.owner, "Equal").await;
    server
        .put_empty(
            &scene.owner,
            &format!(
                "/api/v1/accounts/{}/server-roles/{low_role}",
                equal.account_id
            ),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    for (actor, target) in [
        (low, high),
        (high, low),
        (low, &equal),
        (&scene.owner, high),
        (low, &scene.owner),
    ] {
        server.enter_call(target, scene.voice.id).await;
        let base = format!(
            "/api/v1/channels/{}/call/participants/{}",
            scene.voice.id, target.account_id
        );
        for block in ["microphone-block", "sound-block"] {
            server
                .put_empty(actor, &format!("{base}/{block}"))
                .await
                .empty(StatusCode::NO_CONTENT);
            server
                .delete(actor, &format!("{base}/{block}"))
                .await
                .empty(StatusCode::NO_CONTENT);
        }
        server
            .put_empty(target, "/api/v1/call/hand")
            .await
            .empty(StatusCode::NO_CONTENT);
        server
            .delete(actor, &format!("{base}/hand"))
            .await
            .empty(StatusCode::NO_CONTENT);
        server
            .delete(actor, &base)
            .await
            .empty(StatusCode::NO_CONTENT);
        server.check().await;
    }
}

#[tokio::test]
async fn role_order_setup() {
    use junto_protocol::structure_file::{StructureApplied, StructureExport, StructureFile};
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await.id;
    server.grant_workspace(&owner, &owner, workspace, &[]).await;
    server.grant_workspace(&owner, &owner, workspace, &[]).await;
    let before: StructureExport = server
        .get(&owner, "/api/v1/structure")
        .await
        .ok(StatusCode::OK);
    let mut wanted: StructureFile = before.structure.clone();
    let roles = &mut wanted
        .workspaces
        .iter_mut()
        .find(|w| w.default)
        .unwrap()
        .roles;
    for role in roles.iter_mut().filter(|r| !r.everyone) {
        role.position = Some(3 - role.position.unwrap());
    }
    let plan = junto_setup::plan::plan(
        &wanted,
        &before.structure,
        &mut junto_protocol::ids::StructureId::generate,
    )
    .unwrap();
    assert_eq!(plan.len(), 1);
    let changes: Vec<_> = plan.into_iter().map(|p| p.change).collect();
    server
        .post(
            &owner,
            "/api/v1/structure/changes",
            &json!({"revision":before.revision,"changes":changes}),
        )
        .await
        .ok::<StructureApplied>(StatusCode::OK);
    let after: StructureExport = server
        .get(&owner, "/api/v1/structure")
        .await
        .ok(StatusCode::OK);
    assert!(
        junto_setup::plan::plan(
            &wanted,
            &after.structure,
            &mut junto_protocol::ids::StructureId::generate
        )
        .unwrap()
        .is_empty()
    );
}

#[tokio::test]
async fn role_order_migration() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await.id;
    let administrator = server_role(&server, &owner, &[S::Administrator]).await;
    let moderator = server_role(&server, &owner, &[S::BanMembers]).await;
    let ordinary = server_role(&server, &owner, &[S::CreateInvites]).await;
    let manager = server
        .grant_workspace(&owner, &owner, workspace, &[W::ManageRoles])
        .await;
    let member = server.grant_workspace(&owner, &owner, workspace, &[]).await;
    sqlx::query("update server_roles set created_at = now() - interval '3 hours' where id = $1")
        .bind(administrator.id)
        .execute(&server.db)
        .await
        .unwrap();
    sqlx::query("update server_roles set created_at = now() - interval '2 hours' where id = $1")
        .bind(moderator.id)
        .execute(&server.db)
        .await
        .unwrap();
    sqlx::query("update workspace_roles set created_at = now() - interval '2 hours' where id = $1")
        .bind(manager)
        .execute(&server.db)
        .await
        .unwrap();
    sqlx::query("update server_roles set created_at = now() where id = $1")
        .bind(ordinary.id)
        .execute(&server.db)
        .await
        .unwrap();
    sqlx::query("update workspace_roles set created_at = now() where id = $1")
        .bind(member)
        .execute(&server.db)
        .await
        .unwrap();
    let epoch: uuid::Uuid = sqlx::query_scalar("select epoch from event_stream")
        .fetch_one(&server.db)
        .await
        .unwrap();
    let mut tx = server.db.begin().await.unwrap();
    sqlx::raw_sql("alter table server_roles drop column position; alter table workspace_roles drop column position; drop index if exists member_roles_account_workspace;").execute(&mut *tx).await.unwrap();
    sqlx::raw_sql(include_str!("../../migrations/0014_role_order.sql"))
        .execute(&mut *tx)
        .await
        .unwrap();
    let server_positions: Vec<(junto_protocol::ids::ServerRoleId, i32)> =
        sqlx::query_as("select id, position from server_roles order by position")
            .fetch_all(&mut *tx)
            .await
            .unwrap();
    assert_eq!(
        server_positions,
        [(ordinary.id, 1), (administrator.id, 2), (moderator.id, 3)]
    );
    let workspace_positions: Vec<(junto_protocol::ids::WorkspaceRoleId, i32)> = sqlx::query_as(
        "select id, position from workspace_roles where workspace_id = $1 order by position",
    )
    .bind(workspace)
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert_eq!(workspace_positions[0].1, 0);
    assert_eq!(workspace_positions[1..], [(member, 1), (manager, 2)]);
    let migrated_epoch: uuid::Uuid = sqlx::query_scalar("select epoch from event_stream")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_ne!(epoch, migrated_epoch);
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn role_order_token_uses_current_creator_rank() {
    use junto_protocol::access_token::TokenAction;
    use reqwest::Method;
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let manager = server.join(&owner, "Manager").await;
    let own = server
        .grant_server(&owner, &manager, &[S::Administrator])
        .await;
    let role = server_role(&server, &owner, &[]).await;
    let token = server
        .access_token(&manager, &[TokenAction::ManageStructure])
        .await;
    let route = format!("/api/v1/server-roles/{}", role.id);
    server
        .send(
            Method::PATCH,
            &route,
            Some(token.token.expose()),
            Some(&json!({"name":"allowed"})),
            None,
        )
        .await
        .ok::<ServerRole>(StatusCode::OK);
    server
        .patch(
            &owner,
            "/api/v1/server-roles/positions",
            &json!({"positions":[{"role_id":own,"position":1},{"role_id":role.id,"position":2}]}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .send(
            Method::PATCH,
            &route,
            Some(token.token.expose()),
            Some(&json!({"name":"refused"})),
            None,
        )
        .await
        .error(ErrorCode::Forbidden);
    let owner_token = server
        .access_token(&owner, &[TokenAction::ManageStructure])
        .await;
    server
        .send(
            Method::PATCH,
            &route,
            Some(owner_token.token.expose()),
            Some(&json!({"name":"owner token"})),
            None,
        )
        .await
        .ok::<ServerRole>(StatusCode::OK);
}

#[tokio::test]
async fn ranked_reorder_checks_each_old_and_new_position() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let manager = server.join(&owner, "Manager").await;
    let own = server
        .grant_server(&owner, &manager, &[S::Administrator])
        .await;
    let top = server_role(&server, &owner, &[]).await;
    let low = server_role(&server, &owner, &[]).await;
    let route = "/api/v1/server-roles/positions";
    server
        .patch(
            &owner,
            route,
            &json!({"positions":[{"role_id":own,"position":10},{"role_id":top.id,"position":20}]}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .patch(
            &manager,
            route,
            &json!({"positions":[{"role_id":top.id,"position":5}]}),
        )
        .await
        .error(ErrorCode::Forbidden);
    server
        .patch(
            &manager,
            route,
            &json!({"positions":[{"role_id":low.id,"position":11}]}),
        )
        .await
        .error(ErrorCode::Forbidden);
    server
        .patch(
            &manager,
            route,
            &json!({"positions":[{"role_id":low.id,"position":5}]}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn role_positions_creation_requires_a_rank_above_everyone() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let member = server.join(&owner, "Member").await;
    let workspace = server.default_workspace(&owner).await.id;
    let roles: Vec<WorkspaceRole> = server
        .get(&owner, &format!("/api/v1/workspaces/{workspace}/roles"))
        .await
        .ok(StatusCode::OK);
    let everyone = roles.iter().find(|r| r.is_everyone).unwrap();
    server
        .patch(
            &owner,
            &format!("/api/v1/workspaces/{workspace}/roles/{}", everyone.id),
            &json!({"permissions":["view_channel","manage_roles"]}),
        )
        .await
        .ok::<WorkspaceRole>(StatusCode::OK);
    server
        .post(
            &member,
            &format!("/api/v1/workspaces/{workspace}/roles"),
            &json!({"name":"cannot raise myself"}),
        )
        .await
        .error(ErrorCode::Forbidden);
    server
        .grant_workspace(&owner, &member, workspace, &[])
        .await;
    let created: WorkspaceRole = server
        .post(
            &member,
            &format!("/api/v1/workspaces/{workspace}/roles"),
            &json!({"name":"lower"}),
        )
        .await
        .ok(StatusCode::CREATED);
    assert_eq!(created.position, 1);
}

#[tokio::test]
async fn hierarchy_exempt_moves() {
    use junto_protocol::call::{CallJoin, CallMove};
    let scene = crate::calls_support::Scene::start().await;
    let server = &scene.server;
    server
        .grant_server(&scene.owner, &scene.ann, &[S::Administrator])
        .await;
    let low = server
        .grant_server(&scene.owner, &scene.ben, &[S::Administrator])
        .await;
    let equal = server.join(&scene.owner, "Equal").await;
    server
        .put_empty(
            &scene.owner,
            &format!("/api/v1/accounts/{}/server-roles/{low}", equal.account_id),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let destination = server
        .voice(&scene.owner, scene.workspace_id, "destination")
        .await
        .id;
    for (actor, target) in [
        (&scene.ben, &scene.ann),
        (&scene.ann, &scene.ben),
        (&scene.ben, &equal),
        (&scene.owner, &scene.ann),
        (&scene.ben, &scene.owner),
    ] {
        server.enter_call(target, scene.voice.id).await;
        let route = format!(
            "/api/v1/channels/{}/call/participants/{}/move",
            scene.voice.id, target.account_id
        );
        let moved: CallMove = server
            .post(actor, &route, &json!({"to_channel_id":destination}))
            .await
            .ok(StatusCode::CREATED);
        let join: CallJoin = server
            .post(
                target,
                &format!("/api/v1/call/moves/{}/join", moved.id),
                &Value::Null,
            )
            .await
            .ok(StatusCode::OK);
        let call_id = join.call.id;
        server.connect(join);
        server.check().await;
        assert_eq!(
            server.own_call(target).await.call.unwrap().own.call_id,
            call_id
        );
        server.leave_call(target).await;
        server.check().await;
    }
}

#[tokio::test]
async fn hierarchy_exempt_message_deletion() {
    use crate::messages_support::{channel, message, send};
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let high = server.join(&owner, "High").await;
    let low = server.join(&owner, "Low").await;
    let equal = server.join(&owner, "Equal").await;
    server
        .grant_server(&owner, &high, &[S::Administrator])
        .await;
    let role = server.grant_server(&owner, &low, &[S::Administrator]).await;
    server
        .put_empty(
            &owner,
            &format!("/api/v1/accounts/{}/server-roles/{role}", equal.account_id),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let place = channel(&server, &owner, "text").await;
    for (actor, target) in [
        (&low, &high),
        (&high, &low),
        (&low, &equal),
        (&owner, &high),
        (&low, &owner),
    ] {
        let posted = send(&server, target, &place, "remove this").await;
        server
            .delete(actor, &message(posted.id))
            .await
            .empty(StatusCode::NO_CONTENT);
    }
}

#[tokio::test]
#[ignore = "requires cargo build -p junto-setup"]
async fn role_order_setup_binary() {
    use crate::support::Scratch;
    use junto_protocol::access_token::TokenAction;
    use junto_protocol::structure_file::StructureFile;
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let administrator = server.join(&owner, "Administrator").await;
    server
        .grant_server(&owner, &administrator, &[S::Administrator])
        .await;
    server_role(&server, &owner, &[]).await;
    server_role(&server, &owner, &[]).await;
    let token = server
        .access_token(
            &administrator,
            &[TokenAction::ReadStructure, TokenAction::ManageStructure],
        )
        .await
        .token
        .expose()
        .to_owned();
    let binary = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("junto-setup");
    assert!(binary.exists(), "build junto-setup first");
    let run = |arguments: Vec<String>| {
        let binary = binary.clone();
        let token = token.clone();
        let address = server.base.clone();
        tokio::task::spawn_blocking(move || {
            let output = std::process::Command::new(binary)
                .args(arguments)
                .env("JUNTO_SERVER", address)
                .env("JUNTO_ACCESS_TOKEN", token)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap()
        })
    };
    let exported = run(vec!["export".to_owned()]).await.unwrap();
    let mut file: StructureFile = serde_json::from_str(&exported).unwrap();
    let scratch = Scratch::new();
    let file_path = scratch.0.join("roles.json");
    std::fs::write(&file_path, serde_json::to_vec_pretty(&file).unwrap()).unwrap();
    server_role(&server, &owner, &[]).await;
    let stable: Value = server
        .get(&owner, "/api/v1/structure")
        .await
        .ok(StatusCode::OK);
    run(vec![
        "apply".to_owned(),
        file_path.to_string_lossy().into_owned(),
    ])
    .await
    .unwrap();
    assert_eq!(
        stable,
        server
            .get(&owner, "/api/v1/structure")
            .await
            .ok::<Value>(StatusCode::OK)
    );
    for role in &mut file.server_roles {
        if !role
            .permissions
            .as_ref()
            .is_some_and(|p| p.contains(&S::Administrator))
        {
            role.position = Some(3 - role.position.unwrap());
        }
    }
    std::fs::write(&file_path, serde_json::to_vec_pretty(&file).unwrap()).unwrap();

    run(vec![
        "apply".to_owned(),
        file_path.to_string_lossy().into_owned(),
    ])
    .await
    .unwrap();
    let before: Value = server
        .get(&owner, "/api/v1/structure")
        .await
        .ok(StatusCode::OK);
    let events = server.count("events").await;
    run(vec![
        "apply".to_owned(),
        file_path.to_string_lossy().into_owned(),
    ])
    .await
    .unwrap();
    assert_eq!(
        before,
        server
            .get(&owner, "/api/v1/structure")
            .await
            .ok::<Value>(StatusCode::OK)
    );
    assert_eq!(events, server.count("events").await);
    for positioned in [false, true] {
        for role in &mut file.server_roles {
            role.position = role.position.map(|p| p * 10);
        }
        file.server_roles
            .push(junto_protocol::structure_file::ServerRoleEntry {
                id: junto_protocol::ids::StructureId::generate(),
                name: format!("new {positioned}"),
                permissions: Some(vec![]),
                position: positioned.then_some(1),
            });
        std::fs::write(&file_path, serde_json::to_vec(&file).unwrap()).unwrap();
        run(vec![
            "apply".to_owned(),
            file_path.to_string_lossy().into_owned(),
        ])
        .await
        .unwrap();
        let before: Value = server
            .get(&owner, "/api/v1/structure")
            .await
            .ok(StatusCode::OK);
        run(vec![
            "apply".to_owned(),
            file_path.to_string_lossy().into_owned(),
        ])
        .await
        .unwrap();
        assert_eq!(
            before,
            server
                .get(&owner, "/api/v1/structure")
                .await
                .ok::<Value>(StatusCode::OK)
        );
    }
}

#[tokio::test]
async fn role_order_bounds_and_server_constraint() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    sqlx::query("insert into server_roles (id, name, permissions, created_at, position) select gen_random_uuid(), 'role', 0, now(), n from generate_series(1, 251) n").execute(&mut server.test_data().await).await.unwrap();
    server
        .post(
            &owner,
            "/api/v1/server-roles",
            &json!({"name":"over ceiling"}),
        )
        .await
        .error(ErrorCode::LimitReached);
    let roles: Vec<ServerRole> = server
        .get(&owner, "/api/v1/server-roles")
        .await
        .ok(StatusCode::OK);
    let positions: Vec<_> = roles
        .iter()
        .map(|r| json!({"role_id":r.id,"position":r.position + 1000}))
        .collect();
    server
        .patch(
            &owner,
            "/api/v1/server-roles/positions",
            &json!({"positions":positions}),
        )
        .await
        .error(ErrorCode::InvalidRequest);
    server
        .patch(
            &owner,
            "/api/v1/server-roles/positions",
            &json!({"positions":[positions[0]]}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    assert!(
        sqlx::query("update server_roles set position = 2 where id = $1")
            .bind(roles[0].id)
            .execute(&server.db)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn role_order_requires_manage_permissions() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let plain = server.join(&owner, "Plain").await;
    let workspace = server.default_workspace(&owner).await.id;
    server
        .patch(
            &plain,
            "/api/v1/server-roles/positions",
            &json!({"positions":[]}),
        )
        .await
        .error(ErrorCode::Forbidden);
    server
        .patch(
            &plain,
            &format!("/api/v1/workspaces/{workspace}/roles/positions"),
            &json!({"positions":[]}),
        )
        .await
        .error(ErrorCode::Forbidden);
}
