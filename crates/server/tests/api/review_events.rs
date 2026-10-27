//! The deletion event of a channel follows the rules that applied to the channel (ADR 0304).

use crate::support::{Member, TestServer};
use junto_protocol::events::{Event, Level};
use junto_protocol::ids::ChannelId;
use junto_protocol::structure::{Category, Channel};
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn deleted_channels(events: &[(u64, Event)]) -> BTreeSet<ChannelId> {
    events
        .iter()
        .filter_map(|(_, event)| match event {
            Event::ChannelDeleted { channel_id, .. } => Some(*channel_id),
            _ => None,
        })
        .collect()
}

async fn put_rule(server: &TestServer, owner: &Member, path: &str, body: Value) {
    server
        .put(owner, path, &body)
        .await
        .ok::<Value>(StatusCode::OK);
}

#[tokio::test]
async fn a_deleted_channel_reaches_exactly_the_people_who_saw_it() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await.id;
    let everyone = server.everyone_role(&owner, workspace).await.id;
    let via_category = server.join(&owner, "Ana").await;
    let via_person = server.join(&owner, "Bruno").await;
    let denied = server.join(&owner, "Carla").await;
    let plain = server.join(&owner, "Davi").await;

    let category: Category = server.category(&owner, workspace, "Clients", 0).await;
    let prefix = format!(
        "/api/v1/workspaces/{workspace}/categories/{}/rules",
        category.id
    );
    put_rule(
        &server,
        &owner,
        &format!("{prefix}/roles/{everyone}"),
        json!({"deny": ["view_channel"]}),
    )
    .await;
    put_rule(
        &server,
        &owner,
        &format!("{prefix}/people/{}", via_category.account_id),
        json!({"allow": ["view_channel"]}),
    )
    .await;
    let synced: Channel = server
        .post(
            &owner,
            &format!("/api/v1/workspaces/{workspace}/channels"),
            &json!({"name": "b-plans", "kind": "text", "category_id": category.id}),
        )
        .await
        .ok(StatusCode::CREATED);

    let hidden_root = server.channel(&owner, workspace, "hidden-root").await;
    put_rule(
        &server,
        &owner,
        &format!("/api/v1/channels/{}/rules/roles/{everyone}", hidden_root.id),
        json!({"deny": ["view_channel"]}),
    )
    .await;
    put_rule(
        &server,
        &owner,
        &format!(
            "/api/v1/channels/{}/rules/people/{}",
            hidden_root.id, via_person.account_id
        ),
        json!({"allow": ["view_channel"]}),
    )
    .await;

    let denied_root = server.channel(&owner, workspace, "denied-root").await;
    put_rule(
        &server,
        &owner,
        &format!(
            "/api/v1/channels/{}/rules/people/{}",
            denied_root.id, denied.account_id
        ),
        json!({"deny": ["view_channel"]}),
    )
    .await;

    let people = [&via_category, &via_person, &denied, &plain];
    let mut live = Vec::new();
    for person in people {
        live.push(server.subscribed(person, Level::Full).await);
    }
    let start = server.snapshot(&plain).await.cursor;
    for channel in [&synced, &hidden_root, &denied_root] {
        server
            .delete(&owner, &format!("/api/v1/channels/{}", channel.id))
            .await
            .empty(StatusCode::NO_CONTENT);
    }
    let expected: [BTreeSet<ChannelId>; 4] = [
        BTreeSet::from([synced.id, denied_root.id]),
        BTreeSet::from([hidden_root.id, denied_root.id]),
        BTreeSet::new(),
        BTreeSet::from([denied_root.id]),
    ];
    for ((person, stream), expected) in people.into_iter().zip(live.iter_mut()).zip(expected) {
        let events = server.traffic(person, stream).await;
        assert_eq!(deleted_channels(&events), expected, "{}", person.account_id);
    }
    let mut replay = server.stream(&plain).await;
    replay.subscribe(Level::Full, Some(start)).await;
    let events = server.traffic(&plain, &mut replay).await;
    assert_eq!(deleted_channels(&events), BTreeSet::from([denied_root.id]));
}
