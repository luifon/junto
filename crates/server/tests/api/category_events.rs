use crate::support::TestServer;
use junto_protocol::events::{Event, Level};
use junto_protocol::structure::Category;
use reqwest::StatusCode;
use serde_json::json;

#[tokio::test]
async fn category_events_require_current_visibility_in_delivery_and_replay() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let workspace = server.default_workspace(&owner).await;
    let mut stream = server.subscribed(&reader, Level::Full).await;
    let start = server.snapshot(&reader).await.cursor;
    let category = server.category(&owner, workspace.id, "Plans", 0).await;
    server
        .patch(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/categories/{}",
                workspace.id, category.id
            ),
            &json!({"name":"Private plans"}),
        )
        .await
        .ok::<Category>(StatusCode::OK);
    let hidden = server.traffic(&reader, &mut stream).await;
    assert!(
        hidden.iter().all(|(_, event)| !matches!(
            event,
            Event::CategoryCreated { .. } | Event::CategoryUpdated { .. }
        )),
        "{hidden:?}"
    );
    let mut replay = server.stream(&reader).await;
    replay.subscribe(Level::Full, Some(start)).await;
    let hidden = server.traffic(&reader, &mut replay).await;
    assert!(
        hidden.iter().all(|(_, event)| !matches!(
            event,
            Event::CategoryCreated { .. } | Event::CategoryUpdated { .. }
        )),
        "{hidden:?}"
    );
    sqlx::query("insert into category_person_overrides(category_id,account_id,workspace_id,allow,deny) values($1,$2,$3,4,0)")
        .bind(category.id).bind(reader.account_id).bind(workspace.id).execute(&server.db).await.unwrap();
    server
        .patch(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/categories/{}",
                workspace.id, category.id
            ),
            &json!({"name":"Visible plans"}),
        )
        .await
        .ok::<Category>(StatusCode::OK);
    let visible = server.traffic(&reader, &mut stream).await;
    assert!(visible.iter().any(|(_,event)| matches!(event,Event::CategoryUpdated{category: changed} if changed.id==category.id)));
}

#[tokio::test]
async fn deleting_a_hidden_category_does_not_send_its_identifier() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let workspace = server.default_workspace(&owner).await;
    let category = server.category(&owner, workspace.id, "Plans", 0).await;
    let mut stream = server.subscribed(&reader, Level::Full).await;
    server
        .delete(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/categories/{}",
                workspace.id, category.id
            ),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let events = server.traffic(&reader, &mut stream).await;
    assert!(
        !events
            .iter()
            .any(|(_, event)| matches!(event, Event::CategoryDeleted { .. })),
        "{events:?}"
    );
    assert!(events.iter().any(|(_,event)| matches!(event,Event::PermissionsChanged{workspace_id:Some(id)} if *id==workspace.id)));
}
