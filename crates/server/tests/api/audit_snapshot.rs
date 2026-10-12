//! Proofs of the audit of the whole server: what a device that loads a snapshot must learn.
//! Each test fails while its defect exists.

use junto_protocol::call::EntryRequest;
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::calls_review2::{invite, move_to};
use crate::calls_support::Scene;

/// ADR 0207: a snapshot followed by the events after its cursor gives the state of the
/// server. A move that waits, an invitation that the account made and its own request to
/// enter were told by events only, so a device that had to load a snapshot never learned
/// of them. The snapshot carries each while the account may be at its places.
#[tokio::test]
async fn audit_the_snapshot_carries_the_moves_invitations_and_requests_that_wait() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let lounge = scene.voice.id;
    let garden = server.voice(owner, scene.workspace_id, "garden").await.id;
    let carl = server.join(owner, "Carl").await;
    server.enter_call(ann, lounge).await;
    server.enter_call(owner, lounge).await;

    let pending = move_to(server, owner, lounge, ann.account_id, garden).await;
    let invitation = invite(server, ann, lounge, ben).await;
    server
        .put_empty(ann, &format!("/api/v1/channels/{lounge}/call/lock"))
        .await
        .empty(StatusCode::NO_CONTENT);
    let request: EntryRequest = server
        .post(
            &carl,
            &format!("/api/v1/channels/{lounge}/call/entry-requests"),
            &Value::Null,
        )
        .await
        .ok(StatusCode::CREATED);

    let of_ann = server.snapshot(ann).await;
    assert_eq!(of_ann.call_moves, std::slice::from_ref(&pending));
    assert_eq!(
        of_ann.sent_call_invitations,
        std::slice::from_ref(&invitation)
    );
    assert!(of_ann.call_invitations.is_empty() && of_ann.entry_requests.is_empty());
    let of_owner = server.snapshot(owner).await;
    assert_eq!(of_owner.call_moves, [pending]);
    assert!(of_owner.sent_call_invitations.is_empty());
    let of_ben = server.snapshot(ben).await;
    assert_eq!(of_ben.call_invitations, [invitation]);
    assert!(of_ben.sent_call_invitations.is_empty() && of_ben.call_moves.is_empty());
    let of_carl = server.snapshot(&carl).await;
    assert_eq!(of_carl.entry_requests, [request]);
    assert!(of_carl.call_moves.is_empty() && of_carl.call_invitations.is_empty());

    // An account that can no longer see a place of a waiting thing gets nothing of it.
    let everyone = server.everyone_role(owner, scene.workspace_id).await;
    server
        .put(
            owner,
            &format!("/api/v1/channels/{garden}/overrides/{}", everyone.id),
            &json!({ "allow": [], "deny": ["view_channel"] }),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    assert!(server.snapshot(ann).await.call_moves.is_empty());
    server
        .put(
            owner,
            &format!("/api/v1/channels/{lounge}/overrides/{}", everyone.id),
            &json!({ "allow": [], "deny": ["view_channel"] }),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    assert!(server.snapshot(&carl).await.entry_requests.is_empty());
    assert!(server.snapshot(ben).await.call_invitations.is_empty());
    assert!(server.snapshot(ann).await.sent_call_invitations.is_empty());
}

/// ADR 0207: a snapshot followed by the events after its cursor gives the state of the
/// server. The snapshot carries the own requests to enter, and the creation of a request
/// was told to the people inside only, so a second device of the account that asked never
/// learned of the request.
#[tokio::test]
async fn audit_the_account_that_asks_to_enter_is_told_of_its_request_on_every_device() {
    let scene = Scene::start().await;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    let lounge = scene.voice.id;
    server.enter_call(ann, lounge).await;
    server
        .put_empty(ann, &format!("/api/v1/channels/{lounge}/call/lock"))
        .await
        .empty(StatusCode::NO_CONTENT);
    let before = server.snapshot(ben).await;
    assert!(before.entry_requests.is_empty());
    let mut to_ben = server.full_stream(ben).await;

    let request: EntryRequest = server
        .post(
            ben,
            &format!("/api/v1/channels/{lounge}/call/entry-requests"),
            &Value::Null,
        )
        .await
        .ok(StatusCode::CREATED);
    let told = server.call_events(ben, &mut to_ben).await;
    assert!(
        told.iter().any(|event| matches!(
            event,
            junto_protocol::events::Event::CallEntryRequested { request: own } if *own == request
        )),
        "the account that asked got {:?}",
        crate::calls_support::names(&told)
    );
    assert_eq!(server.snapshot(ben).await.entry_requests, [request]);
}
