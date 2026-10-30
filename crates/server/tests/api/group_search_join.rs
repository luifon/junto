use crate::discovery_support::*;

#[tokio::test]
async fn group_search_filters_and_file_pages_follow_current_membership() {
    let files = Files::open().await;
    let server = &files.server;
    let writer = server.join(&files.owner, "Writer").await;
    let reader = server.join(&files.owner, "Reader").await;
    let third = server.join(&files.owner, "Third").await;
    let group: junto_protocol::message::GroupConversation = server
        .post(
            &writer,
            "/api/v1/group-conversations",
            &json!({"name":"Planning","account_ids":[reader.account_id,third.account_id]}),
        )
        .await
        .ok(StatusCode::CREATED);
    let route = format!("/api/v1/conversations/{}/messages", group.id);
    let ordinary: Message = server
        .post(&writer, &route, &text("quartz plan"))
        .await
        .ok(StatusCode::CREATED);
    let linked: Message = server
        .post(&writer, &route, &text("quartz https://example.test/plan"))
        .await
        .ok(StatusCode::CREATED);
    let mentioned: Message = server.post(&writer, &route, &json!({
        "document":{"blocks":[{"kind":"paragraph","content":[
            {"kind":"text","text":"quartz "},{"kind":"account","account_id":reader.account_id}
        ]}]}
    })).await.ok(StatusCode::CREATED);
    let mut attached = Vec::new();
    for bytes in [
        junto_server::image::samples::png(4, 4),
        b"plain group file".to_vec(),
    ] {
        let uploaded = upload(server, &writer, json!({"conversation_id":group.id}), &bytes).await;
        let message: Message = server
            .post(
                &writer,
                &route,
                &json!({"document":{"blocks":[]},"attachments":[uploaded.id]}),
            )
            .await
            .ok(StatusCode::CREATED);
        attached.push(message);
    }
    let all = [
        ordinary.id,
        linked.id,
        mentioned.id,
        attached[0].id,
        attached[1].id,
    ];
    for (filter, expected) in [
        ("has_file", vec![attached[0].id, attached[1].id]),
        ("has_picture", vec![attached[0].id]),
        ("has_link", vec![linked.id]),
        ("mentions_me", vec![mentioned.id]),
        ("direct", all.to_vec()),
    ] {
        let query = format!("/api/v1/search?{filter}=true&conversation_id={}", group.id);
        let page: SearchPage = server.get(&reader, &query).await.ok(StatusCode::OK);
        assert_eq!(page.results.len(), expected.len(), "{filter}");
        for hit in &page.results {
            assert!(expected.contains(&hit.message_id.unwrap()), "{filter}");
            assert_eq!(hit.place_id, group.id.as_uuid());
            assert_eq!(hit.workspace_id, None);
            assert!(hit.position.is_some());
            assert_eq!(hit.thread_root, None);
        }
        let hidden: SearchPage = server
            .get(&files.owner, &format!("/api/v1/search?{filter}=true"))
            .await
            .ok(StatusCode::OK);
        assert!(
            hidden
                .results
                .iter()
                .all(|hit| !all.contains(&hit.message_id.unwrap()))
        );
    }
    let combined = format!(
        "/api/v1/search?has_link=true&direct=true&author_id={}&conversation_id={}&from=2000-01-01T00:00:00Z&until=2100-01-01T00:00:00Z",
        writer.account_id, group.id
    );
    let page: SearchPage = server.get(&reader, &combined).await.ok(StatusCode::OK);
    assert_eq!(page.results.len(), 1);
    assert_eq!(page.results[0].message_id, Some(linked.id));
    let old: SearchPage = server.get(&reader, &format!("/api/v1/search?direct=true&conversation_id={}&from=2000-01-01T00:00:00Z&until=2000-01-02T00:00:00Z", group.id)).await.ok(StatusCode::OK);
    assert!(old.results.is_empty());
    let file_query = format!(
        "/api/v1/search?files=true&has_file=true&conversation_id={}&limit=1",
        group.id
    );
    let first: SearchPage = server.get(&reader, &file_query).await.ok(StatusCode::OK);
    assert_eq!(first.results.len(), 1);
    assert_eq!(first.results[0].attachments.len(), 1);
    let later = format!("{file_query}&cursor={}", first.next.as_ref().unwrap());
    let second: SearchPage = server.get(&reader, &later).await.ok(StatusCode::OK);
    assert_eq!(second.results.len(), 1);
    assert_eq!(second.results[0].attachments.len(), 1);
    assert!(second.next.is_none());
    for hit in first.results.iter().chain(&second.results) {
        assert_eq!(hit.place_id, group.id.as_uuid());
        assert_eq!(hit.workspace_id, None);
        assert!(
            attached
                .iter()
                .any(|message| Some(message.id) == hit.message_id)
        );
    }
    assert_ne!(first.results[0].message_id, second.results[0].message_id);
    assert!(
        (first.results[0].created_at, first.results[0].message_id)
            > (second.results[0].created_at, second.results[0].message_id)
    );
    server
        .get(&files.owner, &file_query)
        .await
        .error(ErrorCode::NotFound);
    server
        .delete(
            &reader,
            &format!("/api/v1/group-conversations/{}/membership", group.id),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    for filter in [
        "has_file",
        "has_picture",
        "has_link",
        "mentions_me",
        "direct",
    ] {
        let page: SearchPage = server
            .get(&reader, &format!("/api/v1/search?{filter}=true"))
            .await
            .ok(StatusCode::OK);
        assert!(
            page.results
                .iter()
                .all(|hit| !all.contains(&hit.message_id.unwrap()))
        );
    }
    server
        .get(&reader, &file_query)
        .await
        .error(ErrorCode::NotFound);
    server.get(&reader, &later).await.error(ErrorCode::NotFound);
}
