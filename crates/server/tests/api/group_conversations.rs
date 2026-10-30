use crate::messages_support::text;
use crate::support::{Member, TestServer};
use junto_protocol::error::ErrorCode;
use junto_protocol::ids::ConversationId;
use junto_protocol::message::{Conversation, Message, MessagePage};
use reqwest::StatusCode;
use serde_json::{Value, json};

const GROUPS: &str = "/api/v1/group-conversations";

async fn create(
    server: &TestServer,
    caller: &Member,
    others: &[&Member],
    name: &str,
) -> (ConversationId, Value) {
    let accounts: Vec<_> = others.iter().map(|person| person.account_id).collect();
    let group: Value = server
        .post(caller, GROUPS, &json!({"name":name,"account_ids":accounts}))
        .await
        .ok(StatusCode::CREATED);
    let id = serde_json::from_value(group["id"].clone()).unwrap();
    (id, group)
}

#[tokio::test]
async fn a_named_group_is_private_to_its_members_including_from_the_administrator() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let writer = server.join(&owner, "Writer").await;
    let reader = server.join(&owner, "Reader").await;
    let third = server.join(&owner, "Third").await;
    let outsider = server.join(&owner, "Outsider").await;
    let (id, group) = create(&server, &writer, &[&reader, &third], "Project").await;
    assert_eq!(group["name"], "Project");
    let route = format!("/api/v1/conversations/{id}/messages");
    let sent: Message = server
        .post(&writer, &route, &text("Private project message"))
        .await
        .ok(StatusCode::CREATED);
    for member in [&writer, &reader, &third] {
        let groups: Vec<Value> = server.get(member, GROUPS).await.ok(StatusCode::OK);
        assert!(groups.iter().any(|value| value["id"] == json!(id)));
        let history: MessagePage = server.get(member, &route).await.ok(StatusCode::OK);
        assert_eq!(history.messages.len(), 1);
        assert_eq!(history.messages[0].id, sent.id);
    }
    for other in [&owner, &outsider] {
        let groups: Vec<Value> = server.get(other, GROUPS).await.ok(StatusCode::OK);
        assert!(groups.iter().all(|value| value["id"] != json!(id)));
        server.get(other, &route).await.error(ErrorCode::NotFound);
        server
            .post(other, &route, &text("Not a participant"))
            .await
            .error(ErrorCode::NotFound);
        server
            .get(other, &format!("/api/v1/messages/{}", sent.id))
            .await
            .error(ErrorCode::NotFound);
    }
}

#[tokio::test]
async fn members_rename_add_and_leave_a_group_without_changing_pair_conversations() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let writer = server.join(&owner, "Writer").await;
    let reader = server.join(&owner, "Reader").await;
    let third = server.join(&owner, "Third").await;
    let added = server.join(&owner, "Added").await;
    let pair: Conversation = server
        .post(
            &writer,
            "/api/v1/conversations",
            &json!({"account_id":reader.account_id}),
        )
        .await
        .ok(StatusCode::OK);
    let (id, _) = create(&server, &writer, &[&reader, &third], "Project").await;
    let group_route = format!("{GROUPS}/{id}");
    let route = format!("/api/v1/conversations/{id}/messages");
    let sent: Message = server
        .post(&writer, &route, &text("Earlier project message"))
        .await
        .ok(StatusCode::CREATED);
    let renamed: Value = server
        .patch(&reader, &group_route, &json!({"name":"Planning"}))
        .await
        .ok(StatusCode::OK);
    assert_eq!(renamed["name"], "Planning");
    server
        .post(
            &reader,
            &format!("{group_route}/members"),
            &json!({"account_id":added.account_id}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    let history: MessagePage = server.get(&added, &route).await.ok(StatusCode::OK);
    assert_eq!(history.messages[0].id, sent.id);
    server
        .delete(&reader, &format!("{group_route}/membership"))
        .await
        .empty(StatusCode::NO_CONTENT);
    server.get(&reader, &route).await.error(ErrorCode::NotFound);
    server
        .get(&reader, &format!("/api/v1/messages/{}", sent.id))
        .await
        .error(ErrorCode::NotFound);
    let history: MessagePage = server.get(&third, &route).await.ok(StatusCode::OK);
    assert_eq!(history.messages[0].id, sent.id);
    let reopened: Conversation = server
        .post(
            &reader,
            "/api/v1/conversations",
            &json!({"account_id":writer.account_id}),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(reopened, pair);
}

#[tokio::test]
async fn group_privacy_covers_live_replay_snapshots_search_notices_files_and_exports() {
    use junto_protocol::{
        discovery::{NotificationPage, SearchPage},
        events::{Event, Level},
        message::MessagePlace,
    };
    let attachments = crate::support::Scratch::new();
    let exports = crate::support::Scratch::new();
    let server = TestServer::start_with(|config| {
        config.attachments_directory = Some(attachments.0.clone());
        config.exports_directory = Some(exports.0.clone());
    })
    .await;
    let owner = server.owner().await;
    let writer = server.join(&owner, "Writer").await;
    let reader = server.join(&owner, "Reader").await;
    let third = server.join(&owner, "Third").await;
    let cursor = server.snapshot(&reader).await.cursor;
    let mut admin_stream = server.subscribed(&owner, Level::Full).await;
    let mut reader_stream = server.subscribed(&reader, Level::Background).await;
    let (id, _) = create(&server, &writer, &[&reader, &third], "Planning").await;
    let upload = crate::messages_support::upload(
        &server,
        &writer,
        json!({"conversation_id":id}),
        b"private group file",
    )
    .await;
    let mut body = text("Confidentialgroupword");
    body["attachments"] = json!([upload.id]);
    let sent: Message = server
        .post(
            &writer,
            &format!("/api/v1/conversations/{id}/messages"),
            &body,
        )
        .await
        .ok(StatusCode::CREATED);
    let visible = server.traffic(&reader, &mut reader_stream).await;
    assert!(visible.iter().any(|(_, event)| matches!(event, Event::MessageChanged { message_id, .. } if *message_id == sent.id)));
    let hidden = server.traffic(&owner, &mut admin_stream).await;
    assert!(hidden.iter().all(|(_, event)| !matches!(
        event,
        Event::GroupConversationChanged { .. }
    ) && !matches!(event, Event::MessageChanged { message_id, .. } if *message_id == sent.id)));
    let snapshot = server.snapshot(&reader).await;
    assert!(
        snapshot
            .group_conversations
            .iter()
            .any(|group| group.id == id)
    );
    assert!(snapshot.messages.iter().any(|state| state.place
        == MessagePlace::Group {
            conversation_id: id
        }));
    assert!(server.snapshot(&owner).await.group_conversations.is_empty());
    for person in [&reader, &owner] {
        let search: SearchPage = server
            .get(person, "/api/v1/search?q=Confidentialgroupword")
            .await
            .ok(StatusCode::OK);
        assert_eq!(
            search
                .results
                .iter()
                .any(|hit| hit.message_id == Some(sent.id)),
            person.account_id == reader.account_id
        );
        let notices: NotificationPage = server
            .get(person, "/api/v1/notifications")
            .await
            .ok(StatusCode::OK);
        assert_eq!(
            notices
                .notifications
                .iter()
                .any(|notice| notice.message_id == sent.id),
            person.account_id == reader.account_id
        );
    }
    let file = format!("/api/v1/attachments/{}/message", upload.id);
    server
        .get(&reader, &file)
        .await
        .ok::<Message>(StatusCode::OK);
    server.get(&owner, &file).await.error(ErrorCode::NotFound);
    server
        .post(
            &owner,
            &format!("/api/v1/conversations/{id}/exports"),
            &json!({}),
        )
        .await
        .error(ErrorCode::NotFound);
    let export: Value = server
        .post(
            &reader,
            &format!("/api/v1/conversations/{id}/exports"),
            &json!({}),
        )
        .await
        .ok(StatusCode::CREATED);
    let export_route = format!("/api/v1/exports/{}", export["id"].as_str().unwrap());
    server
        .get(&reader, &export_route)
        .await
        .ok::<Value>(StatusCode::OK);
    server
        .delete(&reader, &format!("{GROUPS}/{id}/membership"))
        .await
        .empty(StatusCode::NO_CONTENT);
    server.get(&reader, &file).await.error(ErrorCode::NotFound);
    server
        .get(&reader, &export_route)
        .await
        .error(ErrorCode::NotFound);
    let mut replay = server.stream(&reader).await;
    replay.subscribe(Level::Full, Some(cursor)).await;
    let replayed = server.traffic(&reader, &mut replay).await;
    assert!(replayed.iter().all(|(_, event)| !matches!(event, Event::MessageChanged { message_id, .. } if *message_id == sent.id)));
    assert!(
        server
            .snapshot(&reader)
            .await
            .group_conversations
            .is_empty()
    );
}

#[tokio::test]
async fn group_calls_use_the_conversation_and_current_membership() {
    use junto_protocol::call::{CallJoin, CallPlace};
    let server = TestServer::start_for_calls().await;
    let owner = server.owner().await;
    let writer = server.join(&owner, "Writer").await;
    let reader = server.join(&owner, "Reader").await;
    let third = server.join(&owner, "Third").await;
    let (id, _) = create(&server, &writer, &[&reader, &third], "Planning").await;
    let path = format!("{GROUPS}/{id}/call/join");
    server
        .post(&owner, &path, &json!({}))
        .await
        .error(ErrorCode::NotFound);
    let first: CallJoin = server
        .post(&writer, &path, &json!({}))
        .await
        .ok(StatusCode::OK);
    let second: CallJoin = server
        .post(&reader, &path, &json!({}))
        .await
        .ok(StatusCode::OK);
    assert_eq!(first.call.id, second.call.id);
    assert_eq!(
        first.call.place,
        CallPlace::Group {
            conversation_id: id
        }
    );
    assert!(first.call.meeting.is_none());
    assert!(first.call.recording.is_none());
    server.connect(first.clone());
    server.connect(second);
    server.check().await;
    assert!(
        server
            .snapshot(&writer)
            .await
            .direct_calls
            .iter()
            .any(|call| call.id == first.call.id)
    );
    assert!(server.snapshot(&owner).await.direct_calls.is_empty());
    server
        .delete(&reader, &format!("{GROUPS}/{id}/membership"))
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .post(&reader, &path, &json!({}))
        .await
        .error(ErrorCode::NotFound);
    server.check().await;
    assert!(server.snapshot(&reader).await.own_call.is_none());
}

#[tokio::test]
async fn a_group_rechecks_contact_eligibility_and_members_can_leave_an_unavailable_group() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let writer = server.join(&owner, "Writer").await;
    let reader = server.join(&owner, "Reader").await;
    let third = server.join(&owner, "Third").await;
    let (id, _) = create(&server, &writer, &[&reader, &third], "Project").await;
    let message_route = format!("/api/v1/conversations/{id}/messages");
    let sent: Message = server
        .post(&writer, &message_route, &text("Private group history"))
        .await
        .ok(StatusCode::CREATED);
    let workspace = server.default_workspace(&owner).await;
    // The reader stays active in another workspace while losing the common workspace.
    let second: junto_protocol::structure::Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({"name":"Other"}))
        .await
        .ok(StatusCode::CREATED);
    server
        .put_empty(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}",
                second.id, reader.account_id
            ),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .post(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}/removal",
                workspace.id, reader.account_id
            ),
            &json!({}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    for person in [&writer, &reader, &third] {
        server
            .get(person, &message_route)
            .await
            .error(ErrorCode::NotFound);
        server
            .post(person, &message_route, &text("Unavailable"))
            .await
            .error(ErrorCode::NotFound);
        server
            .get(person, &format!("/api/v1/messages/{}", sent.id))
            .await
            .error(ErrorCode::NotFound);
        assert!(server.snapshot(person).await.group_conversations.is_empty());
    }
    server
        .delete(&reader, &format!("{GROUPS}/{id}/membership"))
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .get(&writer, &message_route)
        .await
        .ok::<MessagePage>(StatusCode::OK);
    server
        .get(&reader, &message_route)
        .await
        .error(ErrorCode::NotFound);
}

#[tokio::test]
async fn a_group_mute_is_private_and_silences_messages_and_rings() {
    use junto_protocol::{call::CallJoin, discovery::NotificationPage, message::MessageState};
    let server = TestServer::start_for_calls().await;
    let owner = server.owner().await;
    let writer = server.join(&owner, "Writer").await;
    let reader = server.join(&owner, "Reader").await;
    let third = server.join(&owner, "Third").await;
    let (id, _) = create(&server, &writer, &[&reader, &third], "Project").await;
    let settings = format!("/api/v1/notification-settings/{id}");
    server
        .put(&owner, &settings, &json!({"mute":true}))
        .await
        .error(ErrorCode::NotFound);
    server
        .put(&reader, &settings, &json!({"mute":true}))
        .await
        .ok::<Value>(StatusCode::OK);
    server
        .post(
            &writer,
            &format!("/api/v1/conversations/{id}/messages"),
            &text("Muted group message"),
        )
        .await
        .ok::<Message>(StatusCode::CREATED);
    let state: MessageState = server
        .get(
            &reader,
            &format!("/api/v1/conversations/{id}/messages/read"),
        )
        .await
        .ok(StatusCode::OK);
    assert!(state.muted);
    let notices: NotificationPage = server
        .get(&reader, "/api/v1/notifications")
        .await
        .ok(StatusCode::OK);
    assert!(notices.notifications.is_empty());
    server
        .post(&writer, &format!("{GROUPS}/{id}/call/join"), &json!({}))
        .await
        .ok::<CallJoin>(StatusCode::OK);
    assert!(server.snapshot(&reader).await.call_invitations.is_empty());
    assert_eq!(server.snapshot(&third).await.call_invitations.len(), 1);
    server
        .put(&reader, &settings, &json!({"mute":false}))
        .await
        .ok::<Value>(StatusCode::OK);
    let state: MessageState = server
        .get(
            &reader,
            &format!("/api/v1/conversations/{id}/messages/read"),
        )
        .await
        .ok(StatusCode::OK);
    assert!(!state.muted);
}

#[tokio::test]
async fn a_client_without_group_capability_keeps_the_original_message_and_call_kinds() {
    use junto_protocol::{
        call::CallJoin,
        events::{Event, Level},
        message::GroupConversation,
    };
    let server = TestServer::start_for_calls().await;
    let owner = server.owner().await;
    let writer = server.join(&owner, "Writer").await;
    let reader = server.join(&owner, "Reader").await;
    let third = server.join(&owner, "Third").await;
    let mut stream = server.legacy_stream(&reader).await;
    stream.subscribe(Level::Full, None).await;
    let group: GroupConversation = server
        .post(
            &writer,
            GROUPS,
            &json!({"name":"Project","account_ids":[reader.account_id,third.account_id]}),
        )
        .await
        .ok(StatusCode::CREATED);
    let sent: Message = server
        .post(
            &writer,
            &format!("/api/v1/conversations/{}/messages", group.id),
            &text("Privategroupcompatword"),
        )
        .await
        .ok(StatusCode::CREATED);
    let join: CallJoin = server
        .post(
            &writer,
            &format!("{GROUPS}/{}/call/join", group.id),
            &json!({}),
        )
        .await
        .ok(StatusCode::OK);
    server.connect(join);
    server.check().await;
    let reader_join: CallJoin = server
        .post(
            &reader,
            &format!("{GROUPS}/{}/call/join", group.id),
            &json!({}),
        )
        .await
        .ok(StatusCode::OK);
    server.connect(reader_join);
    server.check().await;
    let legacy = reqwest::Client::new();
    let snapshot: Value = legacy
        .get(format!("{}/api/v1/snapshot", server.base))
        .bearer_auth(&reader.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(snapshot["own_call"].is_null());
    assert_eq!(snapshot["group_conversations"], json!([]));
    assert_eq!(snapshot["direct_calls"], json!([]));
    assert_eq!(snapshot["call_invitations"], json!([]));
    assert!(
        snapshot["messages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|state| state["place"]["kind"] != "group")
    );
    let response = legacy
        .get(format!("{}/api/v1/messages/{}", server.base, sent.id))
        .bearer_auth(&reader.token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let traffic = server.traffic(&reader, &mut stream).await;
    assert!(traffic.iter().all(|(_, event)| !matches!(
        event,
        Event::GroupConversationChanged { .. }
            | Event::MessageChanged {
                place: junto_protocol::message::MessagePlace::Group { .. },
                ..
            }
            | Event::CallStarted {
                place: junto_protocol::call::CallPlace::Group { .. },
                ..
            }
            | Event::CallInvited { .. }
    )));
    let pair: Conversation = server
        .post(
            &reader,
            "/api/v1/conversations",
            &json!({"account_id":writer.account_id}),
        )
        .await
        .ok(StatusCode::OK);
    let response = legacy
        .get(format!(
            "{}/api/v1/conversations/{}/messages",
            server.base, pair.id
        ))
        .bearer_auth(&reader.token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn a_group_mentions_only_current_members() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let writer = server.join(&owner, "Writer").await;
    let reader = server.join(&owner, "Reader").await;
    let third = server.join(&owner, "Third").await;
    let (id, _) = create(&server, &writer, &[&reader, &third], "Project").await;
    let route = format!("/api/v1/conversations/{id}/messages");
    let mention = |account| json!({"document":{"blocks":[{"kind":"paragraph","content":[{"kind":"account","account_id":account}]}]}});
    server
        .post(&writer, &route, &mention(reader.account_id))
        .await
        .ok::<Message>(StatusCode::CREATED);
    server
        .post(&writer, &route, &mention(owner.account_id))
        .await
        .error(ErrorCode::InvalidRequest);
    server
        .delete(&reader, &format!("{GROUPS}/{id}/membership"))
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .post(&writer, &route, &mention(reader.account_id))
        .await
        .error(ErrorCode::InvalidRequest);
}

async fn separate_group_administrator()
-> (TestServer, Member, Member, Member, Member, ConversationId) {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let writer = server.join(&owner, "Writer").await;
    let reader = server.join(&owner, "Reader").await;
    let closed: junto_protocol::structure::Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({"name":"Other"}))
        .await
        .ok(StatusCode::CREATED);
    let administrator = server
        .join_into(&owner, "Administrator", &[closed.id])
        .await;
    server
        .grant_server(
            &owner,
            &administrator,
            &[junto_protocol::ServerPermission::Administrator],
        )
        .await;
    let (id, _) = create(&server, &writer, &[&reader, &administrator], "Project").await;
    (server, owner, writer, reader, administrator, id)
}

#[tokio::test]
async fn a_former_group_reader_does_not_replay_a_private_departure() {
    use junto_protocol::events::{Event, Level};
    let (server, _owner, _writer, reader, administrator, id) = separate_group_administrator().await;
    let cursor = server.snapshot(&reader).await.cursor;
    server
        .delete(&administrator, "/api/v1/account")
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .delete(&reader, &format!("{GROUPS}/{id}/membership"))
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .get(
            &reader,
            &format!("/api/v1/accounts/{}/profile", administrator.account_id),
        )
        .await
        .error(ErrorCode::NotFound);
    let mut replay = server.stream(&reader).await;
    replay.subscribe(Level::Full, Some(cursor)).await;
    let events = server.traffic(&reader, &mut replay).await;
    assert!(events.iter().all(|(_,event)| !matches!(event,Event::AccountDepartureChanged { account_id,.. } if *account_id == administrator.account_id)));
}

#[tokio::test]
async fn a_current_group_reader_keeps_the_attribution_of_a_former_author() {
    use junto_protocol::{
        account::{AccountProfile, Departure},
        events::{Event, Level},
    };
    let (server, _owner, _writer, reader, administrator, id) = separate_group_administrator().await;
    server
        .post(
            &administrator,
            &format!("/api/v1/conversations/{id}/messages"),
            &text("Earlier project decision"),
        )
        .await
        .ok::<Message>(StatusCode::CREATED);
    server
        .delete(&administrator, &format!("{GROUPS}/{id}/membership"))
        .await
        .empty(StatusCode::NO_CONTENT);
    let mut stream = server.subscribed(&reader, Level::Full).await;
    server
        .delete(&administrator, "/api/v1/account")
        .await
        .empty(StatusCode::NO_CONTENT);
    let profile: AccountProfile = server
        .get(
            &reader,
            &format!("/api/v1/accounts/{}/profile", administrator.account_id),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(profile.departure, Some(Departure::Deleted));
    let events = server.traffic(&reader, &mut stream).await;
    assert!(events.iter().any(|(_,event)| matches!(event,Event::AccountDepartureChanged { account_id,departure:Some(Departure::Deleted) } if *account_id == administrator.account_id)));
}
