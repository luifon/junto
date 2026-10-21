//! The glossary of a workspace (ADR 0603).

use junto_protocol::WorkspacePermission as W;
use junto_protocol::audit::AuditEvent;
use junto_protocol::error::ErrorCode;
use junto_protocol::events::Event;
use junto_protocol::structure::Workspace;
use junto_protocol::transcript::Glossary;
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::support::TestServer;

#[tokio::test]
async fn a_person_with_the_permission_replaces_the_glossary_and_every_member_reads_it() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let editor = server.join(&owner, "Editor").await;
    let member = server.join(&owner, "Member").await;
    let workspace = server.default_workspace(&owner).await;
    let path = format!("/api/v1/workspaces/{}/glossary", workspace.id);
    server
        .grant_workspace(&owner, &editor, workspace.id, &[W::ManageGlossary])
        .await;

    let empty: Glossary = server.get(&member, &path).await.ok(StatusCode::OK);
    assert_eq!(
        empty,
        Glossary {
            workspace_id: workspace.id,
            terms: Vec::new()
        }
    );
    let terms = json!({ "terms": ["Ysolde Varkonen", " Quorvex ", "Lumitrax"] });
    server
        .put(&member, &path, &terms)
        .await
        .error(ErrorCode::Forbidden);
    assert_eq!(server.count("glossary_terms").await, 0);

    let mut stream = server.full_stream(&member).await;
    let set: Glossary = server.put(&editor, &path, &terms).await.ok(StatusCode::OK);
    assert_eq!(set.terms, ["Ysolde Varkonen", "Quorvex", "Lumitrax"]);
    assert_eq!(
        server
            .get(&member, &path)
            .await
            .ok::<Glossary>(StatusCode::OK),
        set
    );
    let events: Vec<Event> = server
        .traffic(&member, &mut stream)
        .await
        .into_iter()
        .map(|(_, event)| event)
        .collect();
    assert_eq!(
        events,
        [Event::GlossaryReplaced {
            workspace_id: workspace.id
        }]
    );
    let entries = server.audit_actions(&owner, workspace.id).await;
    let last = entries.last().unwrap();
    assert_eq!(last.event, AuditEvent::GlossaryReplaced { terms: 3 });
    assert_eq!(last.actor_id, Some(editor.account_id));

    // The whole list is replaced, in its new order of priority.
    let reordered: Glossary = server
        .put(
            &editor,
            &path,
            &json!({ "terms": ["Lumitrax", "Ysolde Varkonen"] }),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(reordered.terms, ["Lumitrax", "Ysolde Varkonen"]);
    assert_eq!(server.count("glossary_terms").await, 2);
    let cleared: Glossary = server
        .put(&editor, &path, &json!({ "terms": [] }))
        .await
        .ok(StatusCode::OK);
    assert!(cleared.terms.is_empty());
    assert_eq!(server.count("glossary_terms").await, 0);
}

#[tokio::test]
async fn a_glossary_that_the_job_of_the_worker_would_not_accept_is_refused() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let path = format!("/api/v1/workspaces/{}/glossary", workspace.id);
    let many: Vec<String> = (0..201).map(|number| format!("term {number}")).collect();
    let long = "a".repeat(201);
    let refused: Vec<Value> = vec![
        json!({ "terms": many }),
        json!({ "terms": ["Quorvex", "Quorvex"] }),
        json!({ "terms": ["Quorvex", " Quorvex "] }),
        json!({ "terms": ["fine", ""] }),
        json!({ "terms": ["fine", "   "] }),
        json!({ "terms": ["fine", long] }),
        json!({ "terms": ["two\nlines"] }),
        json!({ "terms": ["<|notimestamps|>"] }),
        json!({ "terms": "Quorvex" }),
        json!({}),
    ];
    let (events, entries) = (
        server.count("events").await,
        server.count("audit_entries").await,
    );
    for body in &refused {
        server
            .put(&owner, &path, body)
            .await
            .error(ErrorCode::InvalidRequest);
    }
    assert_eq!(server.count("glossary_terms").await, 0);
    assert_eq!(server.count("events").await, events);
    assert_eq!(server.count("audit_entries").await, entries);

    let full: Vec<String> = (0..200).map(|number| format!("term {number}")).collect();
    let set: Glossary = server
        .put(&owner, &path, &json!({ "terms": full }))
        .await
        .ok(StatusCode::OK);
    assert_eq!(set.terms.len(), 200);
    assert_eq!(set.terms[199], "term 199");
}

#[tokio::test]
async fn the_glossary_of_a_workspace_is_hidden_from_a_person_outside_it() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let member = server.join(&owner, "Member").await;
    let closed: Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({ "name": "Closed" }))
        .await
        .ok(StatusCode::CREATED);
    let path = format!("/api/v1/workspaces/{}/glossary", closed.id);
    server
        .put(&owner, &path, &json!({ "terms": ["Quorvex"] }))
        .await
        .ok::<Glossary>(StatusCode::OK);
    let absent = format!("/api/v1/workspaces/{}/glossary", uuid::Uuid::now_v7());

    assert_eq!(
        server.get(&member, &path).await.observable(),
        server.get(&member, &absent).await.observable()
    );
    let terms = json!({ "terms": ["Other"] });
    assert_eq!(
        server.put(&member, &path, &terms).await.observable(),
        server.put(&member, &absent, &terms).await.observable()
    );
    server.get(&member, &path).await.error(ErrorCode::NotFound);
    let kept: Glossary = server.get(&owner, &path).await.ok(StatusCode::OK);
    assert_eq!(kept.terms, ["Quorvex"]);
    // The glossary of one workspace is not the glossary of another.
    let default = server.default_workspace(&owner).await;
    let other: Glossary = server
        .get(
            &member,
            &format!("/api/v1/workspaces/{}/glossary", default.id),
        )
        .await
        .ok(StatusCode::OK);
    assert!(other.terms.is_empty());
}

#[tokio::test]
async fn a_change_made_from_an_older_list_is_refused_and_changes_nothing() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let path = format!("/api/v1/workspaces/{}/glossary", workspace.id);
    server
        .put(&owner, &path, &json!({ "terms": ["Quorvex", "Lumitrax"] }))
        .await
        .ok::<Glossary>(StatusCode::OK);

    // A change from the list as it is now replaces it.
    let read = json!(["Quorvex", "Lumitrax"]);
    let added: Glossary = server
        .put(
            &owner,
            &path,
            &json!({ "terms": ["Quorvex", "Lumitrax", "Ysolde Varkonen"], "replaces": read }),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(added.terms, ["Quorvex", "Lumitrax", "Ysolde Varkonen"]);

    // A second change from the same older list would remove the term that was added: it
    // is refused, and nothing is stored, recorded or told.
    let (events, entries) = (
        server.count("events").await,
        server.count("audit_entries").await,
    );
    for replaces in [
        json!(["Quorvex", "Lumitrax"]),
        json!(["Lumitrax", "Quorvex", "Ysolde Varkonen"]),
        json!([]),
    ] {
        server
            .put(
                &owner,
                &path,
                &json!({ "terms": ["Lumitrax"], "replaces": replaces }),
            )
            .await
            .error(ErrorCode::Conflict);
    }
    assert_eq!(server.count("events").await, events);
    assert_eq!(server.count("audit_entries").await, entries);
    let kept: Glossary = server.get(&owner, &path).await.ok(StatusCode::OK);
    assert_eq!(kept.terms, ["Quorvex", "Lumitrax", "Ysolde Varkonen"]);
}
