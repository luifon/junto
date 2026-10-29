use crate::messages_support::{channel, message, send};
use crate::support::TestServer;
use junto_protocol::error::ErrorCode;
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[tokio::test]
async fn reaction_people_are_paginated_and_checked_under_current_message_access() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let first = server.join(&owner, "Wren").await;
    let second = server.join(&owner, "Mara").await;
    let third = server.join(&owner, "Sora").await;
    let place = channel(&server, &owner, "text").await;
    let sent = send(&server, &owner, &place, "confirm this plan").await;
    for person in [&first, &second, &third] {
        server
            .put(
                person,
                &format!("{}/reaction", message(sent.id)),
                &json!({"emoji":"👍"}),
            )
            .await
            .empty(StatusCode::NO_CONTENT);
    }
    let route = format!("{}/reactions?emoji=%F0%9F%91%8D", message(sent.id));
    let mut cursor = None;
    let mut names = BTreeSet::new();
    let mut ids = BTreeSet::new();
    for _ in 0..4 {
        let path = match cursor {
            Some(ref after) => format!("{route}&limit=1&after={after}"),
            None => format!("{route}&limit=1"),
        };
        let page: Value = server.get(&first, &path).await.ok(StatusCode::OK);
        let people = page["people"].as_array().unwrap();
        assert!(people.len() <= 1);
        for person in people {
            assert!(ids.insert(person["id"].as_str().unwrap().to_owned()));
            names.insert(person["name"].as_str().unwrap().to_owned());
        }
        cursor = page["next_after"].as_str().map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(
        names,
        ["Mara", "Sora", "Wren"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    );
    assert!(cursor.is_none());
    let ordered: Vec<_> = ids.iter().collect();
    let back: Value = server
        .get(&first, &format!("{route}&limit=1&before={}", ordered[2]))
        .await
        .ok(StatusCode::OK);
    assert_eq!(back["people"][0]["id"], ordered[1].as_str());

    for bad in [
        format!("{route}&limit=0"),
        format!("{route}&limit=201"),
        format!("{route}&after=invalid"),
    ] {
        server
            .get(&first, &bad)
            .await
            .error(ErrorCode::InvalidRequest);
    }
    let everyone = server.everyone_role(&owner, place.workspace_id).await;
    server
        .put(
            &owner,
            &format!("/api/v1/channels/{}/rules/roles/{}", place.id, everyone.id),
            &json!({"allow":[],"deny":["read_message_history"]}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    server.get(&first, &route).await.error(ErrorCode::Forbidden);
}

#[tokio::test]
async fn reaction_people_respect_private_conversations_threads_and_deleted_names() {
    use junto_protocol::message::{Conversation, Message};
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let author = server.join(&owner, "Writer").await;
    let reader = server.join(&owner, "Reader").await;
    let conversation: Conversation = server
        .post(
            &author,
            "/api/v1/conversations",
            &json!({"account_id":reader.account_id}),
        )
        .await
        .ok(StatusCode::OK);
    let direct: Message = server
        .post(
            &author,
            &format!("/api/v1/conversations/{}/messages", conversation.id),
            &crate::messages_support::text("private confirmation"),
        )
        .await
        .ok(StatusCode::CREATED);
    server
        .put(
            &reader,
            &format!("{}/reaction", message(direct.id)),
            &json!({"emoji":"👍"}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let direct_route = format!("{}/reactions?emoji=%F0%9F%91%8D", message(direct.id));
    server
        .get(&owner, &direct_route)
        .await
        .error(ErrorCode::NotFound);
    let page: Value = server.get(&author, &direct_route).await.ok(StatusCode::OK);
    assert_eq!(page["people"][0]["name"], "Reader");
    let voice = channel(&server, &owner, "voice").await;
    let root = send(&server, &owner, &voice, "discussion").await;
    let reply: Message = server
        .post(
            &author,
            &format!("{}/thread", message(root.id)),
            &crate::messages_support::text("reply confirmation"),
        )
        .await
        .ok(StatusCode::CREATED);
    server
        .put(
            &reader,
            &format!("{}/reaction", message(reply.id)),
            &json!({"emoji":"👍"}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let route = format!("{}/reactions?emoji=%F0%9F%91%8D", message(reply.id));
    let page: Value = server.get(&author, &route).await.ok(StatusCode::OK);
    assert_eq!(page["people"][0]["id"], reader.account_id.to_string());
    server
        .delete(&reader, "/api/v1/account")
        .await
        .empty(StatusCode::NO_CONTENT);
    let page: Value = server.get(&author, &route).await.ok(StatusCode::OK);
    assert_eq!(page["people"][0]["id"], reader.account_id.to_string());
    assert!(page["people"][0]["name"].is_null());
    assert_eq!(page["people"][0]["departed"], true);
}
