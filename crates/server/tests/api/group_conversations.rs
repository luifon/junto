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
    for person in [&writer, &reader, &third] {
        let memberships: Value = server
            .get(person, &format!("{GROUPS}/memberships"))
            .await
            .ok(StatusCode::OK);
        assert_eq!(memberships, json!([{"id": id, "name": "Project"}]));
    }
    let outsiders: Value = server
        .get(&owner, &format!("{GROUPS}/memberships"))
        .await
        .ok(StatusCode::OK);
    assert_eq!(outsiders, json!([]));
    server
        .delete(&reader, &format!("{GROUPS}/{id}/membership"))
        .await
        .empty(StatusCode::NO_CONTENT);
    let left: Value = server
        .get(&reader, &format!("{GROUPS}/memberships"))
        .await
        .ok(StatusCode::OK);
    assert_eq!(left, json!([]));
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

#[tokio::test]
async fn concurrent_additions_keep_the_ten_member_limit() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let writer = server.join(&owner, "Writer").await;
    let mut people = Vec::new();
    for n in 0..10 {
        people.push(server.join(&owner, &format!("Reader {n}")).await);
    }
    let others: Vec<_> = people[..8].iter().collect();
    let (id, _) = create(&server, &writer, &others, "Project").await;
    let route = format!("{GROUPS}/{id}/members");
    let left = json!({"account_id":people[8].account_id});
    let right = json!({"account_id":people[9].account_id});
    let (a, b) = tokio::join!(
        server.post(&writer, &route, &left),
        server.post(&writer, &route, &right)
    );
    assert_eq!(
        [&a, &b]
            .iter()
            .filter(|reply| reply.status == StatusCode::OK)
            .count(),
        1,
        "exactly one of two simultaneous additions to nine members succeeds: {}, {}",
        a.text(),
        b.text()
    );
    for reply in [&a, &b] {
        if reply.status != StatusCode::OK {
            reply.error(ErrorCode::InvalidRequest);
        }
    }
    let groups: Vec<Value> = server.get(&writer, GROUPS).await.ok(StatusCode::OK);
    let group = groups
        .iter()
        .find(|group| group["id"] == json!(id))
        .unwrap();
    assert_eq!(group["account_ids"].as_array().unwrap().len(), 10);
}

#[tokio::test]
async fn group_and_visibility_migrations_preserve_populated_pairs_history_and_calls() {
    use junto_protocol::call::CallJoin;
    let server = TestServer::start_for_calls().await;
    let owner = server.owner().await;
    let writer = server.join(&owner, "Writer").await;
    let reader = server.join(&owner, "Reader").await;
    let third = server.join(&owner, "Third").await;
    let pair: Conversation = server
        .post(
            &writer,
            "/api/v1/conversations",
            &json!({"account_id":reader.account_id}),
        )
        .await
        .ok(StatusCode::OK);
    let history = format!("/api/v1/conversations/{}/messages", pair.id);
    let written: Message = server
        .post(&writer, &history, &text("Existing pair history"))
        .await
        .ok(StatusCode::CREATED);
    let joined: CallJoin = server
        .post(
            &writer,
            &format!("/api/v1/direct-calls/{}/join", reader.account_id),
            &json!({}),
        )
        .await
        .ok(StatusCode::OK);
    let call = joined.call.id;
    server.connect(joined);
    server.check().await;
    let mut transaction = server.db.begin().await.unwrap();
    sqlx::raw_sql("drop table hidden_conversations,conversation_members,conversation_former_members; alter table calls drop constraint calls_one_place; alter table calls drop column conversation_id; alter table calls add constraint calls_pair_place check ((channel_id is null) <> (direct_low is null)); alter table conversations drop constraint conversation_kind; alter table conversations drop column name; alter table conversations alter column low_account_id set not null; alter table conversations alter column high_account_id set not null;")
        .execute(&mut *transaction).await.unwrap();
    sqlx::raw_sql(include_str!(
        "../../migrations/0051_group_conversations.sql"
    ))
    .execute(&mut *transaction)
    .await
    .unwrap();
    sqlx::raw_sql(include_str!(
        "../../migrations/0053_hidden_conversations.sql"
    ))
    .execute(&mut *transaction)
    .await
    .unwrap();
    transaction.commit().await.unwrap();
    let reopened: Conversation = server
        .post(
            &reader,
            "/api/v1/conversations",
            &json!({"account_id":writer.account_id}),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(reopened, pair);
    let page: MessagePage = server.get(&reader, &history).await.ok(StatusCode::OK);
    assert!(page.messages.iter().any(|message| message.id == written.id));
    let snapshot = server.snapshot(&writer).await;
    assert_eq!(snapshot.own_call.unwrap().call_id, call);
    let (group, _) = create(&server, &writer, &[&reader, &third], "New group").await;
    server
        .put(
            &reader,
            &format!("/api/v1/conversations/{group}/visibility"),
            &json!({"hidden":true}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(
        server.snapshot(&reader).await.hidden_conversations,
        vec![group]
    );
}

#[tokio::test]
async fn group_call_arrival_reads_an_indexed_transition_beside_large_unrelated_history() {
    use junto_protocol::call::CallJoin;
    let server = TestServer::start_for_calls().await;
    let owner = server.owner().await;
    let writer = server.join(&owner, "Writer").await;
    let reader = server.join(&owner, "Reader").await;
    let third = server.join(&owner, "Third").await;
    let channel = crate::messages_support::channel(&server, &owner, "text").await;
    let root = crate::messages_support::send(&server, &owner, &channel, "Unrelated history").await;
    sqlx::query("insert into messages(id,place_id,position,author_id,created_at,document) select gen_random_uuid(),place_id,position+n,author_id,created_at,document from messages cross join generate_series(1,19999)n where id=$1")
        .bind(root.id).execute(&mut server.test_data().await).await.unwrap();
    let (id, _) = create(&server, &writer, &[&reader, &third], "Project").await;
    let route = format!("{GROUPS}/{id}/call/join");
    let joined: CallJoin = server
        .post(&writer, &route, &json!({}))
        .await
        .ok(StatusCode::OK);
    let call_id = joined.call.id;
    server.connect(joined);
    server.check().await;
    sqlx::query("analyze messages")
        .execute(&server.db)
        .await
        .unwrap();
    let query = junto_server::messages::groups::CALL_ARRIVAL_QUERY;
    let transition: Option<String> = sqlx::query_scalar(query)
        .bind(call_id)
        .bind(writer.account_id)
        .fetch_optional(&server.db)
        .await
        .unwrap();
    assert!(
        transition.is_some(),
        "the fixture must contain the participant's transition"
    );
    let plan: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "explain (analyze,format json) {query}"
    )))
    .bind(call_id)
    .bind(writer.account_id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    let visited = crate::messages_queries::visited_rows(&plan[0]["Plan"]);
    eprintln!("group arrival plan visited rows: {visited}; {plan}");
    assert!(
        visited < 64,
        "one participant transition must not scan unrelated history: {plan}"
    );
}

#[tokio::test]
async fn a_finished_group_export_keeps_former_member_names_and_is_revoked_after_leaving() {
    use junto_protocol::{
        export::{Export, ExportState},
        ids::CallId,
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
    let former = server.join(&owner, "Former").await;
    let (id, _) = create(&server, &writer, &[&reader, &former], "Project").await;
    server.post(&writer, &format!("/api/v1/conversations/{id}/messages"),
        &json!({"document":{"blocks":[{"kind":"paragraph","content":[{"kind":"account","account_id":former.account_id}]}]}}))
        .await.ok::<Message>(StatusCode::CREATED);
    let mut tx = server.db.begin().await.unwrap();
    junto_server::messages::groups::call_line(
        &mut tx,
        server.state.now(),
        id,
        CallId::generate(),
        "person_joined",
        Some(former.account_id),
        None,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    server
        .delete(&former, &format!("{GROUPS}/{id}/membership"))
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .delete(&former, "/api/v1/account")
        .await
        .empty(StatusCode::NO_CONTENT);
    let export: Export = server
        .post(
            &writer,
            &format!("/api/v1/conversations/{id}/exports"),
            &json!({}),
        )
        .await
        .ok(StatusCode::CREATED);
    junto_server::exports::run_once(&server.state)
        .await
        .unwrap();
    let route = format!("/api/v1/exports/{}", export.id);
    let finished: Export = server.get(&writer, &route).await.ok(StatusCode::OK);
    assert_eq!(finished.state, ExportState::Ready);
    let file = format!("{route}/file");
    let reply = server.get(&writer, &file).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let entries = junto_server::exports::zip::read(reply.bytes()).unwrap();
    let page = String::from_utf8(
        entries
            .iter()
            .find(|entry| entry.name == "messages.html")
            .unwrap()
            .content
            .clone(),
    )
    .unwrap();
    assert!(
        page.contains("@Former"),
        "mention attribution must survive departure and deletion: {page}"
    );
    assert!(
        page.contains("person joined by Former"),
        "call attribution must survive departure and deletion: {page}"
    );
    server
        .delete(&writer, &format!("{GROUPS}/{id}/membership"))
        .await
        .empty(StatusCode::NO_CONTENT);
    server.get(&writer, &file).await.error(ErrorCode::NotFound);
}

async fn nonactive_member_keeps_the_remaining_group_readable(banned: bool) {
    use junto_protocol::call::CallJoin;
    let server = TestServer::start_for_calls().await;
    let owner = server.owner().await;
    let writer = server.join(&owner, "Writer").await;
    let reader = server.join(&owner, "Reader").await;
    let excluded = server.join(&owner, "Excluded").await;
    let (id, _) = create(&server, &writer, &[&reader, &excluded], "Project").await;
    let history = format!("/api/v1/conversations/{id}/messages");
    let written: Message = server
        .post(&excluded, &history, &text("Earlier decision"))
        .await
        .ok(StatusCode::CREATED);
    let call = format!("{GROUPS}/{id}/call/join");
    for person in [&writer, &excluded] {
        let joined: CallJoin = server
            .post(person, &call, &json!({}))
            .await
            .ok(StatusCode::OK);
        server.connect(joined);
    }
    server.check().await;
    if banned {
        server
            .put_empty(
                &owner,
                &format!("/api/v1/accounts/{}/ban", excluded.account_id),
            )
            .await
            .empty(StatusCode::NO_CONTENT);
    } else {
        server
            .post(
                &owner,
                &format!("/api/v1/accounts/{}/removal", excluded.account_id),
                &json!({}),
            )
            .await
            .empty(StatusCode::NO_CONTENT);
    }
    server
        .get(&excluded, &history)
        .await
        .error(ErrorCode::Unauthenticated);
    server
        .post(&excluded, &call, &json!({}))
        .await
        .error(ErrorCode::Unauthenticated);
    server.check().await;
    for person in [&writer, &reader] {
        let page: MessagePage = server.get(person, &history).await.ok(StatusCode::OK);
        assert!(page.messages.iter().any(|message| message.id == written.id
            && message.author_name.as_deref() == Some("Excluded")));
    }
    let snapshot = server.snapshot(&writer).await;
    assert!(
        snapshot.own_call.is_some(),
        "the remaining member keeps the call seat"
    );
    assert!(snapshot.direct_calls.iter().all(|room| {
        room.participants
            .iter()
            .all(|person| person.account_id != excluded.account_id)
    }));
    let joined: CallJoin = server
        .post(&reader, &call, &json!({}))
        .await
        .ok(StatusCode::OK);
    server.connect(joined);
    server.check().await;
    let seats: i64 = sqlx::query_scalar("select count(*) from call_members where account_id=$1")
        .bind(excluded.account_id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(seats, 0, "the inactive caller's seat ended");
}

#[tokio::test]
async fn banning_a_member_keeps_history_and_calls_for_the_remaining_members() {
    nonactive_member_keeps_the_remaining_group_readable(true).await;
}

#[tokio::test]
async fn removing_a_member_keeps_history_and_calls_for_the_remaining_members() {
    nonactive_member_keeps_the_remaining_group_readable(false).await;
}

#[tokio::test]
async fn snapshot_group_authorization_has_a_constant_statement_bound_for_many_groups() {
    let server = TestServer::start_measured().await;
    let evictions = server.statement_evictions().await;
    let owner = server.owner().await;
    let writer = server.join(&owner, "Writer").await;
    let reader = server.join(&owner, "Reader").await;
    let third = server.join(&owner, "Third").await;
    create(&server, &writer, &[&reader, &third], "Project").await;
    let (mut held, role) = crate::messages_queries::measure_requests(&server).await;
    let observer = &mut **held.first_mut().unwrap();
    server.snapshot(&writer).await;
    let before = crate::messages_queries::statements(observer, role).await;
    server.snapshot(&writer).await;
    let small = crate::messages_queries::since(
        crate::messages_queries::statements(observer, role).await,
        before,
    );
    sqlx::query("with made as (insert into conversations(id,name) select gen_random_uuid(),'Project '||n::text from generate_series(1,200)n returning id) insert into conversation_members(conversation_id,account_id) select id,unnest($1::uuid[]) from made")
        .bind(vec![writer.account_id,reader.account_id,third.account_id]).execute(&mut server.test_data().await).await.unwrap();
    server.snapshot(&writer).await;
    let before = crate::messages_queries::statements(observer, role).await;
    let snapshot = server.snapshot(&writer).await;
    let large = crate::messages_queries::since(
        crate::messages_queries::statements(observer, role).await,
        before,
    );
    assert_eq!(snapshot.group_conversations.len(), 201);
    let small: i64 = small.values().sum();
    let large_total: i64 = large.values().sum();
    eprintln!("group snapshot statements: one={small}, many={large_total}");
    assert!(
        large_total <= small + 5,
        "201 groups must add no per-group authorization queries: one={small}, many={large_total}, statements={large:?}"
    );
    server.require_no_statement_eviction(evictions).await;
}

#[tokio::test]
async fn a_group_collection_limit_applies_to_creation_and_addition_and_leave_releases_a_place() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let writer = server.join(&owner, "Writer").await;
    let reader = server.join(&owner, "Reader").await;
    let third = server.join(&owner, "Third").await;
    sqlx::query("with made as (insert into conversations(id,name) select gen_random_uuid(),'Project '||n::text from generate_series(1,256)n returning id) insert into conversation_members(conversation_id,account_id) select id,unnest($1::uuid[]) from made")
        .bind(vec![writer.account_id,reader.account_id,third.account_id]).execute(&mut server.test_data().await).await.unwrap();
    server
        .post(
            &writer,
            GROUPS,
            &json!({"name":"One too many","account_ids":[reader.account_id,third.account_id]}),
        )
        .await
        .error(ErrorCode::InvalidRequest);
    let fourth = server.join(&owner, "Fourth").await;
    let fifth = server.join(&owner, "Fifth").await;
    let sixth = server.join(&owner, "Sixth").await;
    let (id, _) = create(&server, &fourth, &[&fifth, &sixth], "Other project").await;
    let route = format!("{GROUPS}/{id}/members");
    server
        .post(&fourth, &route, &json!({"account_id":writer.account_id}))
        .await
        .error(ErrorCode::InvalidRequest);
    let leave:ConversationId=sqlx::query_scalar("select conversation_id from conversation_members where account_id=$1 order by conversation_id limit 1")
        .bind(writer.account_id).fetch_one(&server.db).await.unwrap();
    server
        .delete(&writer, &format!("{GROUPS}/{leave}/membership"))
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .post(&fourth, &route, &json!({"account_id":writer.account_id}))
        .await
        .ok::<Value>(StatusCode::OK);
}

#[tokio::test]
async fn group_file_and_export_transfers_end_after_leaving() {
    use junto_protocol::export::Export;
    const LENGTH: usize = 24 * 1024 * 1024;
    let attachments = crate::support::Scratch::new();
    let exports = crate::support::Scratch::new();
    let server = TestServer::start_tuned(
        |config| {
            config.attachments_directory = Some(attachments.0.clone());
            config.exports_directory = Some(exports.0.clone());
        },
        Some(16 * 1024),
    )
    .await;
    server.state.attachment_space.use_disk(std::sync::Arc::new(
        junto_server::storage::FakeDisk::new(crate::support::FREE_DISK),
    ));
    let owner = server.owner().await;
    let writer = server.join(&owner, "Writer").await;
    let reader = server.join(&owner, "Reader").await;
    let third = server.join(&owner, "Third").await;
    let (id, _) = create(&server, &writer, &[&reader, &third], "Project").await;
    let bytes = vec![7u8; LENGTH];
    let attachment =
        crate::messages_support::upload(&server, &writer, json!({"conversation_id":id}), &bytes)
            .await;
    server
        .post(
            &writer,
            &format!("/api/v1/conversations/{id}/messages"),
            &json!({"document":{"blocks":[]},"attachments":[attachment.id]}),
        )
        .await
        .ok::<Message>(StatusCode::CREATED);
    let file = format!("/api/v1/attachments/{}/file", attachment.id);
    let complete = server.get(&reader, &file).await;
    assert_eq!(complete.status, StatusCode::OK);
    assert_eq!(complete.bytes(), bytes);
    let export: Export = server
        .post(
            &reader,
            &format!("/api/v1/conversations/{id}/exports"),
            &json!({}),
        )
        .await
        .ok(StatusCode::CREATED);
    junto_server::exports::run_once(&server.state)
        .await
        .unwrap();
    let archive = format!("/api/v1/exports/{}/file", export.id);
    let archive_length = 256 * 1024 * 1024u64;
    std::fs::OpenOptions::new()
        .write(true)
        .open(exports.0.join(export.id.to_string()).join("1.zip"))
        .unwrap()
        .set_len(archive_length)
        .unwrap();
    let client = reqwest::Client::new();
    let mut running = Vec::new();
    for (path, length) in [(&file, LENGTH as u64), (&archive, archive_length)] {
        let mut response = client
            .get(format!("{}{path}", server.base))
            .header(junto_protocol::message::GROUPS_HEADER, "1")
            .bearer_auth(&reader.token)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let first = response.chunk().await.unwrap().unwrap().len() as u64;
        running.push((response, first, length));
    }
    server
        .delete(&reader, &format!("{GROUPS}/{id}/membership"))
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .clock
        .advance(server.state.config.limits.recordings.download_check + time::Duration::seconds(1));
    for (mut response, mut received, length) in running {
        let whole = loop {
            match response.chunk().await {
                Ok(Some(chunk)) => received += chunk.len() as u64,
                Ok(None) => break true,
                Err(_) => break false,
            }
        };
        assert!(
            received < length && !whole,
            "a departed reader must not finish the transfer: {received}/{length}, complete={whole}"
        );
    }
    for path in [&file, &archive] {
        server.get(&reader, path).await.error(ErrorCode::NotFound);
    }
    let complete = server.get(&third, &file).await;
    assert_eq!(complete.status, StatusCode::OK);
    assert_eq!(complete.bytes(), bytes);
}

#[tokio::test]
async fn an_inactive_member_returns_only_after_contact_is_restored() {
    use junto_protocol::{auth::SessionCreated, structure::Workspace};
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let writer = server.join(&owner, "Writer").await;
    let reader = server.join(&owner, "Reader").await;
    let former = server.join(&owner, "Former").await;
    let (id, _) = create(&server, &writer, &[&reader, &former], "Project").await;
    let history = format!("/api/v1/conversations/{id}/messages");
    let written: Message = server
        .post(&former, &history, &text("Earlier decision"))
        .await
        .ok(StatusCode::CREATED);
    server
        .post(
            &former,
            &format!("/api/v1/workspaces/{}/departure", workspace.id),
            &Value::Null,
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let page: MessagePage = server.get(&reader, &history).await.ok(StatusCode::OK);
    assert!(page.messages.iter().any(|message| message.id == written.id));
    server
        .get(&former, &history)
        .await
        .error(ErrorCode::Unauthenticated);
    let other: Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({"name":"Other"}))
        .await
        .ok(StatusCode::CREATED);
    let invite = server
        .invite(&owner, json!({"workspace_ids":[other.id]}))
        .await;
    let returned: SessionCreated = server
        .join_reply(&former.key, invite.code.expose(), "Another name")
        .await
        .ok(StatusCode::CREATED);
    assert_eq!(returned.account_id, former.account_id);
    let former = crate::support::member(former.key, returned);
    for person in [&writer, &reader, &former] {
        server
            .get(person, &history)
            .await
            .error(ErrorCode::NotFound);
    }
    server
        .put_empty(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}",
                workspace.id, former.account_id
            ),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    for person in [&writer, &reader, &former] {
        let page: MessagePage = server.get(person, &history).await.ok(StatusCode::OK);
        assert!(
            page.messages.iter().any(|message| message.id == written.id
                && message.author_name.as_deref() == Some("Former"))
        );
    }
}
