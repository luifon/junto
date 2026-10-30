use crate::messages_support::channel;
use crate::support::TestServer;
use junto_protocol::discovery::SearchPage;
use junto_protocol::message::{Document, Message, MessagePage};
use reqwest::StatusCode;
use serde_json::json;

#[tokio::test]
async fn large_forwarded_documents_round_trip_through_postgres_and_bounded_pages() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let channel = channel(&server, &owner, "text").await;
    let path = format!("/api/v1/channels/{}/messages", channel.id);
    let content = format!("needle {}", "世界".repeat(36_000));
    let document = json!({"blocks": [
        {"kind": "forward", "server": "https://source.example.test", "message_id": "0198f1c2-7d3a-7b1e-9c55-0a1b2c3d4e5f"},
        {"kind": "paragraph", "content": [{"kind": "text", "text": content}]}
    ]});
    assert!(serde_json::to_vec(&document).unwrap().len() > 32_768);
    let expected: Document = serde_json::from_value(document.clone()).unwrap();
    let initial: MessagePage = server.get(&owner, &path).await.ok(StatusCode::OK);
    assert!(!initial.has_more);
    let mut ids: Vec<_> = initial.messages.iter().map(|message| message.id).collect();
    for _ in 0..20 {
        let sent: Message = server
            .post(&owner, &path, &json!({"document":document}))
            .await
            .ok(StatusCode::CREATED);
        assert_eq!(sent.document.as_ref(), Some(&expected));
        ids.push(sent.id);
    }
    let stored: i32 =
        sqlx::query_scalar("select octet_length(document::text) from messages where id=$1")
            .bind(*ids.last().unwrap())
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert!(stored > 32_768);
    for ascending in [false, true] {
        let mut seen = Vec::new();
        let mut cursor = None;
        for _ in 0..21 {
            let mut query = format!("{path}?limit=200");
            if let Some(cursor) = cursor {
                query.push_str(&format!(
                    "&{}_message={cursor}",
                    if ascending { "after" } else { "before" }
                ));
            } else if ascending {
                query.push_str("&after=0");
            }
            let page: MessagePage = server.get(&owner, &query).await.ok(StatusCode::OK);
            assert!(serde_json::to_vec(&page).unwrap().len() < 4 * 1024 * 1024);
            assert!(!page.messages.is_empty());
            cursor = Some(if ascending {
                page.messages.last().unwrap().id
            } else {
                page.messages[0].id
            });
            seen.extend(page.messages.iter().map(|message| message.id));
            if !page.has_more {
                break;
            }
        }
        seen.sort();
        let mut expected = ids.clone();
        expected.sort();
        assert_eq!(seen, expected);
    }
    let hits: SearchPage = server
        .get(&owner, "/api/v1/search?q=needle&limit=200")
        .await
        .ok(StatusCode::OK);
    assert_eq!(hits.results.len(), 20);
    assert!(
        hits.results
            .iter()
            .all(|hit| hit.text.chars().count() <= 512)
    );
}
