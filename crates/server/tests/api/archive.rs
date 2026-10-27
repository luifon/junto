//! Archived channels (ADR 0410): who archives and restores, what an archived channel still
//! allows, the archive period, the notice and the automatic deletion.

use junto_protocol::WorkspacePermission;
use junto_protocol::audit::{AuditEvent, AuditPage};
use junto_protocol::call::ChannelCall;
use junto_protocol::error::ErrorCode;
use junto_protocol::events::{Event, Level};
use junto_protocol::ids::{ChannelId, WorkspaceId};
use junto_protocol::message::{Message, MessagePage, Upload};
use junto_protocol::recording::RecordingDetail;
use junto_protocol::structure::{ArchivePeriod, ArchivedChannel, Channel, ChannelList};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use time::Duration;

use crate::calls_support::Scene;
use crate::messages_support::{send, text};
use crate::recordings_support::Studio;
use crate::support::{Member, Reply, TestServer};

pub async fn archive(server: &TestServer, member: &Member, channel_id: ChannelId) -> Reply {
    server
        .put_empty(member, &format!("/api/v1/channels/{channel_id}/archive"))
        .await
}

async fn restore(server: &TestServer, member: &Member, channel_id: ChannelId) -> Reply {
    server
        .delete(member, &format!("/api/v1/channels/{channel_id}/archive"))
        .await
}

async fn archived(
    server: &TestServer,
    member: &Member,
    workspace_id: WorkspaceId,
) -> Vec<ArchivedChannel> {
    server
        .get(
            member,
            &format!("/api/v1/workspaces/{workspace_id}/archived-channels"),
        )
        .await
        .ok(StatusCode::OK)
}

async fn listed(server: &TestServer, member: &Member, workspace_id: WorkspaceId) -> Vec<ChannelId> {
    let list: ChannelList = server
        .get(
            member,
            &format!("/api/v1/workspaces/{workspace_id}/channels"),
        )
        .await
        .ok(StatusCode::OK);
    list.channels
        .into_iter()
        .map(|channel| channel.id)
        .collect()
}

async fn maintain(server: &TestServer) {
    junto_server::maintenance::run_once(&server.state)
        .await
        .unwrap();
}

async fn audit_events(
    server: &TestServer,
    reader: &Member,
    workspace_id: WorkspaceId,
) -> Vec<(Option<junto_protocol::ids::AccountId>, AuditEvent)> {
    let page: AuditPage = server
        .get(
            reader,
            &format!("/api/v1/workspaces/{workspace_id}/audit-log?limit=200"),
        )
        .await
        .ok(StatusCode::OK);
    page.entries
        .into_iter()
        .map(|entry| (entry.actor_id, entry.event))
        .collect()
}

async fn system_actions(
    server: &TestServer,
    reader: &Member,
    channel_id: ChannelId,
) -> Vec<String> {
    let page: MessagePage = server
        .get(
            reader,
            &format!("/api/v1/channels/{channel_id}/messages?limit=200"),
        )
        .await
        .ok(StatusCode::OK);
    page.messages
        .into_iter()
        .filter_map(|message| message.system.map(|system| system.action))
        .collect()
}

async fn stored_times(
    server: &TestServer,
    channel_id: ChannelId,
) -> (
    Option<time::OffsetDateTime>,
    Option<time::OffsetDateTime>,
    Option<time::OffsetDateTime>,
) {
    sqlx::query_as("select archived_at, deletion_notice_at, deleted_at from channels where id = $1")
        .bind(channel_id)
        .fetch_one(&server.db)
        .await
        .unwrap()
}

/// One text that changes when any row of a table of content or structure changes.
async fn content_state(server: &TestServer) -> String {
    let mut state = String::new();
    for table in [
        "channels",
        "channel_overrides",
        "messages",
        "message_reactions",
        "message_pins",
        "attachments",
        "thread_forks",
        "calls",
        "call_members",
        "recordings",
        "transcripts",
        "transcript_corrections",
        "transcript_speaker_names",
        "audit_entries",
        "events",
    ] {
        let digest: String = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "select coalesce(md5(string_agg(row::text, '|' order by row::text)), 'empty')
             from {table} as row"
        )))
        .fetch_one(&server.db)
        .await
        .unwrap();
        state.push_str(&format!("{table}={digest};"));
    }
    state
}

#[tokio::test]
async fn rule_a01_archive_and_restore_need_manage_channel() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let plain = server.join(&owner, "Plain").await;
    let manager = server.join(&owner, "Manager").await;
    let role = server
        .grant_workspace(
            &owner,
            &manager,
            workspace.id,
            &[WorkspacePermission::ManageChannel],
        )
        .await;
    let channel = server.channel(&owner, workspace.id, "plans").await;
    let hidden = server.channel(&owner, workspace.id, "hidden").await;
    server
        .make_private(&owner, workspace.id, hidden.id, role)
        .await;

    archive(&server, &plain, channel.id)
        .await
        .error(ErrorCode::Forbidden);
    archive(&server, &plain, hidden.id)
        .await
        .error(ErrorCode::NotFound);
    assert_eq!(stored_times(&server, channel.id).await.0, None);
    assert_eq!(stored_times(&server, hidden.id).await.0, None);

    let archived: ArchivedChannel = archive(&server, &manager, channel.id)
        .await
        .ok(StatusCode::OK);
    assert_eq!(archived.channel.id, channel.id);
    assert_eq!(archived.archived_at, server.clock.now());
    archive(&server, &manager, hidden.id)
        .await
        .ok::<ArchivedChannel>(StatusCode::OK);

    restore(&server, &plain, channel.id)
        .await
        .error(ErrorCode::Forbidden);
    restore(&server, &plain, hidden.id)
        .await
        .error(ErrorCode::NotFound);
    assert!(stored_times(&server, channel.id).await.0.is_some());
    assert!(stored_times(&server, hidden.id).await.0.is_some());
    restore(&server, &manager, channel.id)
        .await
        .ok::<Channel>(StatusCode::OK);
    assert_eq!(stored_times(&server, channel.id).await.0, None);

    let unknown = ChannelId::generate();
    archive(&server, &owner, unknown)
        .await
        .error(ErrorCode::NotFound);
    server
        .delete(&owner, &format!("/api/v1/channels/{}", hidden.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    archive(&server, &owner, hidden.id)
        .await
        .error(ErrorCode::NotFound);
    restore(&server, &owner, hidden.id)
        .await
        .error(ErrorCode::NotFound);
}

#[tokio::test]
async fn rule_a02_an_archived_channel_leaves_the_list() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let member = server.join(&owner, "Member").await;
    let channel = server.channel(&owner, workspace.id, "plans").await;
    let kept = server.channel(&owner, workspace.id, "kept").await;
    assert!(
        listed(&server, &member, workspace.id)
            .await
            .contains(&channel.id)
    );
    assert!(archived(&server, &member, workspace.id).await.is_empty());

    archive(&server, &owner, channel.id)
        .await
        .ok::<ArchivedChannel>(StatusCode::OK);
    for reader in [&owner, &member] {
        let list = listed(&server, reader, workspace.id).await;
        assert!(!list.contains(&channel.id));
        assert!(list.contains(&kept.id));
        let snapshot = server.snapshot(reader).await;
        let state = &snapshot.workspaces[0];
        assert!(state.channels.iter().all(|listed| listed.id != channel.id));
        assert!(state.channels.iter().any(|listed| listed.id == kept.id));
        assert!(snapshot.messages.iter().all(|state| !matches!(
            state.place,
            junto_protocol::message::MessagePlace::Channel { channel_id, .. }
                if channel_id == channel.id
        )));
        let found = archived(&server, reader, workspace.id).await;
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].channel, channel);
        assert_eq!(found[0].archived_at, server.clock.now());
        assert_eq!(
            found[0].deletes_at,
            Some(server.clock.now() + Duration::days(30))
        );
    }
    let outsider_workspace: junto_protocol::structure::Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({ "name": "Other" }))
        .await
        .ok(StatusCode::CREATED);
    let outsider = server
        .join_into(&owner, "Outsider", &[outsider_workspace.id])
        .await;
    server
        .get(
            &outsider,
            &format!("/api/v1/workspaces/{}/archived-channels", workspace.id),
        )
        .await
        .error(ErrorCode::NotFound);
}

#[tokio::test]
async fn rule_a03_the_archived_list_follows_the_view_permission() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let plain = server.join(&owner, "Plain").await;
    let insider = server.join(&owner, "Insider").await;
    let role = server
        .grant_workspace(&owner, &insider, workspace.id, &[])
        .await;
    let open = server.channel(&owner, workspace.id, "open").await;
    let private = server.channel(&owner, workspace.id, "private").await;
    server
        .make_private(&owner, workspace.id, private.id, role)
        .await;
    for channel in [&open, &private] {
        archive(&server, &owner, channel.id)
            .await
            .ok::<ArchivedChannel>(StatusCode::OK);
    }
    let ids = |found: Vec<ArchivedChannel>| -> Vec<ChannelId> {
        let mut ids: Vec<ChannelId> = found.into_iter().map(|found| found.channel.id).collect();
        ids.sort();
        ids
    };
    let mut both = vec![open.id, private.id];
    both.sort();
    assert_eq!(
        ids(archived(&server, &plain, workspace.id).await),
        [open.id]
    );
    assert_eq!(ids(archived(&server, &insider, workspace.id).await), both);
    assert_eq!(ids(archived(&server, &owner, workspace.id).await), both);
}

#[tokio::test]
async fn rule_a04_an_archived_channel_is_read() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let member = server.join(&owner, "Member").await;
    let channel = server.channel(&owner, workspace.id, "plans").await;
    let root = send(&server, &owner, &channel, "granite decision").await;
    let reply: Message = server
        .post(
            &member,
            &format!("/api/v1/messages/{}/thread", root.id),
            &text("granite reply"),
        )
        .await
        .ok(StatusCode::CREATED);
    server
        .put_empty(&owner, &format!("/api/v1/messages/{}/pin", root.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    archive(&server, &owner, channel.id)
        .await
        .ok::<ArchivedChannel>(StatusCode::OK);

    let history: MessagePage = server
        .get(
            &member,
            &format!("/api/v1/channels/{}/messages", channel.id),
        )
        .await
        .ok(StatusCode::OK);
    assert!(history.messages.iter().any(|message| message.id == root.id));
    server
        .get(&member, &format!("/api/v1/messages/{}", root.id))
        .await
        .ok::<Message>(StatusCode::OK);
    let thread: junto_protocol::message::ThreadPage = server
        .get(&member, &format!("/api/v1/messages/{}/thread", root.id))
        .await
        .ok(StatusCode::OK);
    assert_eq!(thread.replies.messages[0].id, reply.id);
    let pins: Vec<Message> = server
        .get(&member, &format!("/api/v1/channels/{}/pins", channel.id))
        .await
        .ok(StatusCode::OK);
    assert_eq!(pins.len(), 1);
    let found: junto_protocol::discovery::SearchPage = server
        .get(&member, "/api/v1/search?q=granite")
        .await
        .ok(StatusCode::OK);
    assert_eq!(found.results.len(), 2);
    server
        .get(
            &member,
            &format!("/api/v1/channels/{}/messages/read", channel.id),
        )
        .await
        .ok::<junto_protocol::message::MessageState>(StatusCode::OK);
    let marked = server
        .put(
            &member,
            &format!("/api/v1/channels/{}/messages/read", channel.id),
            &json!({ "position": root.position }),
        )
        .await;
    assert!(marked.status.is_success(), "{}", marked.text());
}

struct Change {
    name: &'static str,
    method: Method,
    path: String,
    body: Value,
}

fn change(name: &'static str, method: Method, path: String, body: Value) -> Change {
    Change {
        name,
        method,
        path,
        body,
    }
}

async fn all_refused(server: &TestServer, member: &Member, changes: &[Change]) {
    for change in changes {
        let before = content_state(server).await;
        let body = (!change.body.is_null()).then_some(&change.body);
        let reply = server
            .send(
                change.method.clone(),
                &change.path,
                Some(&member.token),
                body,
                None,
            )
            .await;
        assert_eq!(
            reply.code(),
            ErrorCode::ChannelArchived,
            "{}: {} {}",
            change.name,
            reply.status,
            reply.text()
        );
        assert!(reply.text().contains("archived"), "{}", change.name);
        assert_eq!(
            content_state(server).await,
            before,
            "{} changed the database",
            change.name
        );
    }
}

async fn all_accepted(server: &TestServer, member: &Member, changes: &[Change]) {
    for change in changes {
        let body = (!change.body.is_null()).then_some(&change.body);
        let reply = server
            .send(
                change.method.clone(),
                &change.path,
                Some(&member.token),
                body,
                None,
            )
            .await;
        assert!(
            reply.status.is_success(),
            "{}: {} {}",
            change.name,
            reply.status,
            reply.text()
        );
    }
}

fn text_changes(channel: &Channel, own: &Message, pinned: &Message, root: &Message) -> Vec<Change> {
    let channel_id = channel.id;
    vec![
        change(
            "send",
            Method::POST,
            format!("/api/v1/channels/{channel_id}/messages"),
            text("late"),
        ),
        change(
            "edit",
            Method::PUT,
            format!("/api/v1/messages/{}", own.id),
            text("changed"),
        ),
        change(
            "add reaction",
            Method::PUT,
            format!("/api/v1/messages/{}/reaction", own.id),
            json!({ "emoji": "👍" }),
        ),
        change(
            "remove reaction",
            Method::DELETE,
            format!("/api/v1/messages/{}/reaction/🎉", pinned.id),
            Value::Null,
        ),
        change(
            "pin",
            Method::PUT,
            format!("/api/v1/messages/{}/pin", own.id),
            Value::Null,
        ),
        change(
            "unpin",
            Method::DELETE,
            format!("/api/v1/messages/{}/pin", pinned.id),
            Value::Null,
        ),
        change(
            "thread reply",
            Method::POST,
            format!("/api/v1/messages/{}/thread", root.id),
            text("late reply"),
        ),
        change(
            "fork",
            Method::POST,
            format!("/api/v1/messages/{}/fork", root.id),
            json!({ "name": "forked", "kind": "text" }),
        ),
        change(
            "upload",
            Method::POST,
            "/api/v1/attachments".to_owned(),
            json!({ "channel_id": channel_id, "name": "a.txt", "bytes": 3,
                    "sha256": junto_server::messages::uploads::hash(b"abc") }),
        ),
        change(
            "update channel",
            Method::PATCH,
            format!("/api/v1/channels/{channel_id}"),
            json!({ "name": "renamed" }),
        ),
        change(
            "delete message",
            Method::DELETE,
            format!("/api/v1/messages/{}", own.id),
            Value::Null,
        ),
    ]
}

#[tokio::test]
async fn rule_a05_an_archived_channel_takes_no_change() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let channel = server.channel(&owner, workspace.id, "plans").await;
    let own = send(&server, &owner, &channel, "first").await;
    let pinned = send(&server, &owner, &channel, "second").await;
    let root = send(&server, &owner, &channel, "third").await;
    server
        .put_empty(&owner, &format!("/api/v1/messages/{}/pin", pinned.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .put(
            &owner,
            &format!("/api/v1/messages/{}/reaction", pinned.id),
            &json!({ "emoji": "🎉" }),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .post(
            &owner,
            &format!("/api/v1/messages/{}/thread", root.id),
            &text("reply"),
        )
        .await
        .ok::<Message>(StatusCode::CREATED);

    archive(&server, &owner, channel.id)
        .await
        .ok::<ArchivedChannel>(StatusCode::OK);
    let changes = text_changes(&channel, &own, &pinned, &root);
    all_refused(&server, &owner, &changes).await;

    // The same requests change the channel when it is restored: the refusals above are
    // the refusals of the archive and of nothing else.
    restore(&server, &owner, channel.id)
        .await
        .ok::<Channel>(StatusCode::OK);
    let accepted: Vec<Change> = changes
        .into_iter()
        .filter(|change| change.name != "upload")
        .collect();
    all_accepted(&server, &owner, &accepted).await;
}

#[tokio::test]
async fn rule_a06_an_archived_voice_channel_has_no_call() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, owner, ann) = (&scene.server, &scene.owner, &scene.ann);
    let voice = &scene.voice;
    let recording = studio.finished().await;
    let call: ChannelCall = server.channel_call(ann, voice.id).await;
    assert!(call.in_use, "{call:?}");

    archive(server, owner, voice.id)
        .await
        .ok::<ArchivedChannel>(StatusCode::OK);
    server.check().await;
    let call: ChannelCall = server.channel_call(ann, voice.id).await;
    assert!(!call.in_use && call.call.is_none(), "{call:?}");
    assert_eq!(server.count("call_members").await, 0);
    assert_eq!(server.count("calls").await, 0);
    assert!(server.own_call(ann).await.call.is_none());

    let recording_id = recording.recording.id;
    let changes = vec![
        change(
            "join",
            Method::POST,
            format!("/api/v1/channels/{}/call/join", voice.id),
            Value::Null,
        ),
        change(
            "start recording",
            Method::PUT,
            format!("/api/v1/channels/{}/call/recording", voice.id),
            json!({}),
        ),
        change(
            "call settings",
            Method::PATCH,
            format!("/api/v1/channels/{}/call/settings", voice.id),
            json!({ "user_limit": 4 }),
        ),
        change(
            "lock",
            Method::PUT,
            format!("/api/v1/channels/{}/call/lock", voice.id),
            Value::Null,
        ),
        change(
            "remove recording",
            Method::DELETE,
            format!("/api/v1/recordings/{recording_id}"),
            Value::Null,
        ),
        change(
            "correct line",
            Method::PUT,
            format!("/api/v1/recordings/{recording_id}/transcript/lines/0"),
            json!({ "text": "corrected" }),
        ),
        change(
            "rename speaker",
            Method::PUT,
            format!("/api/v1/recordings/{recording_id}/transcript/speakers/1"),
            json!({ "name": "Noor" }),
        ),
        change(
            "request transcript",
            Method::POST,
            format!("/api/v1/recordings/{recording_id}/transcript"),
            Value::Null,
        ),
    ];
    all_refused(server, owner, &changes).await;

    // The recording of an archived channel is read and downloaded.
    let detail: RecordingDetail = server.recording(ann, recording_id).await;
    let segment = detail
        .segments
        .iter()
        .find(|segment| segment.file.is_some())
        .unwrap();
    let file = server
        .get(
            ann,
            &format!(
                "/api/v1/recordings/{recording_id}/segments/{}/file",
                segment.id
            ),
        )
        .await;
    assert_eq!(file.status, StatusCode::OK);
    assert!(!file.bytes().is_empty());

    restore(server, owner, voice.id)
        .await
        .ok::<Channel>(StatusCode::OK);
    server.enter_call(ann, voice.id).await;
    server
        .delete(owner, &format!("/api/v1/recordings/{recording_id}"))
        .await
        .empty(StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn rule_a07_a_restored_channel_takes_changes() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let member = server.join(&owner, "Member").await;
    let channel = server.channel(&owner, workspace.id, "plans").await;
    archive(&server, &owner, channel.id)
        .await
        .ok::<ArchivedChannel>(StatusCode::OK);
    server
        .post(
            &member,
            &format!("/api/v1/channels/{}/messages", channel.id),
            &text("refused"),
        )
        .await
        .error(ErrorCode::ChannelArchived);

    let restored: Channel = restore(&server, &owner, channel.id)
        .await
        .ok(StatusCode::OK);
    assert_eq!(restored, channel);
    assert!(
        listed(&server, &member, workspace.id)
            .await
            .contains(&channel.id)
    );
    assert!(archived(&server, &member, workspace.id).await.is_empty());
    send(&server, &member, &channel, "accepted").await;
    let snapshot = server.snapshot(&member).await;
    assert!(
        snapshot.workspaces[0]
            .channels
            .iter()
            .any(|listed| listed.id == channel.id)
    );
}

#[tokio::test]
async fn rule_a08_archive_and_restore_are_idempotent() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let channel = server.channel(&owner, workspace.id, "plans").await;
    let count = |action: &'static str| {
        let server = &server;
        let owner = &owner;
        async move {
            audit_events(server, owner, workspace.id)
                .await
                .iter()
                .filter(|(_, event)| event.action() == action)
                .count()
        }
    };

    restore(&server, &owner, channel.id)
        .await
        .ok::<Channel>(StatusCode::OK);
    assert_eq!(count("channel_restored").await, 0);

    let first: ArchivedChannel = archive(&server, &owner, channel.id)
        .await
        .ok(StatusCode::OK);
    server.clock.advance(Duration::hours(1));
    let before = content_state(&server).await;
    let second: ArchivedChannel = archive(&server, &owner, channel.id)
        .await
        .ok(StatusCode::OK);
    assert_eq!(second, first);
    assert_eq!(content_state(&server).await, before);
    assert_eq!(count("channel_archived").await, 1);

    restore(&server, &owner, channel.id)
        .await
        .ok::<Channel>(StatusCode::OK);
    let before = content_state(&server).await;
    restore(&server, &owner, channel.id)
        .await
        .ok::<Channel>(StatusCode::OK);
    assert_eq!(content_state(&server).await, before);
    assert_eq!(count("channel_restored").await, 1);
}

#[tokio::test]
async fn rule_a09_archive_events_reach_the_viewers_of_the_channel() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let plain = server.join(&owner, "Plain").await;
    let insider = server.join(&owner, "Insider").await;
    let role = server
        .grant_workspace(&owner, &insider, workspace.id, &[])
        .await;
    let private = server.channel(&owner, workspace.id, "private").await;
    server
        .make_private(&owner, workspace.id, private.id, role)
        .await;
    let mut plain_stream = server.subscribed(&plain, Level::Full).await;
    let mut insider_stream = server.subscribed(&insider, Level::Full).await;

    let archived: ArchivedChannel = archive(&server, &owner, private.id)
        .await
        .ok(StatusCode::OK);
    restore(&server, &owner, private.id)
        .await
        .ok::<Channel>(StatusCode::OK);

    let seen: Vec<Event> = server
        .traffic(&insider, &mut insider_stream)
        .await
        .into_iter()
        .map(|(_, event)| event)
        .filter(|event| !matches!(event, Event::MessageChanged { .. }))
        .collect();
    assert_eq!(
        seen,
        [
            Event::ChannelArchived { channel: archived },
            Event::ChannelRestored {
                channel: private.clone()
            },
            // The channel is back with its overrides: the accounts that see it load the
            // lists of their permissions again (ADR 0310).
            Event::PermissionsChanged {
                workspace_id: Some(workspace.id)
            },
        ]
    );
    assert!(server.traffic(&plain, &mut plain_stream).await.is_empty());

    let entries = audit_events(&server, &owner, workspace.id).await;
    let of_channel: Vec<&AuditEvent> = entries
        .iter()
        .filter(|(actor, _)| *actor == Some(owner.account_id))
        .map(|(_, event)| event)
        .filter(|event| {
            matches!(
                event,
                AuditEvent::ChannelArchived | AuditEvent::ChannelRestored
            )
        })
        .collect();
    assert_eq!(
        of_channel,
        [&AuditEvent::ChannelRestored, &AuditEvent::ChannelArchived]
    );
    let lines = system_actions(&server, &insider, private.id).await;
    assert!(lines.contains(&"channel_archived".to_owned()), "{lines:?}");
    assert!(lines.contains(&"channel_restored".to_owned()), "{lines:?}");
}

async fn period(server: &TestServer, member: &Member, workspace_id: WorkspaceId) -> Reply {
    server
        .get(
            member,
            &format!("/api/v1/workspaces/{workspace_id}/archive-period"),
        )
        .await
}

async fn set_period(
    server: &TestServer,
    member: &Member,
    workspace_id: WorkspaceId,
    days: Value,
) -> Reply {
    server
        .put(
            member,
            &format!("/api/v1/workspaces/{workspace_id}/archive-period"),
            &json!({ "days": days }),
        )
        .await
}

#[tokio::test]
async fn rule_a10_the_archive_period_is_a_workspace_setting() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let plain = server.join(&owner, "Plain").await;
    let manager = server.join(&owner, "Manager").await;
    server
        .grant_workspace(&owner, &manager, workspace.id, WorkspacePermission::ALL)
        .await;
    let mut stream = server.subscribed(&plain, Level::Full).await;

    let default: ArchivePeriod = period(&server, &owner, workspace.id)
        .await
        .ok(StatusCode::OK);
    assert_eq!(default.days, Some(30));
    for refused in [&plain, &manager] {
        period(&server, refused, workspace.id)
            .await
            .error(ErrorCode::Forbidden);
        set_period(&server, refused, workspace.id, json!(60))
            .await
            .error(ErrorCode::Forbidden);
    }
    for invalid in [json!(0), json!(6), json!(36_501)] {
        set_period(&server, &owner, workspace.id, invalid)
            .await
            .error(ErrorCode::InvalidRequest);
    }
    let unchanged: ArchivePeriod = period(&server, &owner, workspace.id)
        .await
        .ok(StatusCode::OK);
    assert_eq!(unchanged.days, Some(30));

    for days in [Some(7), Some(36_500), None, Some(14)] {
        let set: ArchivePeriod = set_period(&server, &owner, workspace.id, json!(days))
            .await
            .ok(StatusCode::OK);
        assert_eq!(set.days, days);
        let read: ArchivePeriod = period(&server, &owner, workspace.id)
            .await
            .ok(StatusCode::OK);
        assert_eq!(read.days, days);
    }
    // The same value again is no change.
    set_period(&server, &owner, workspace.id, json!(14))
        .await
        .ok::<ArchivePeriod>(StatusCode::OK);
    let changes: Vec<AuditEvent> = audit_events(&server, &owner, workspace.id)
        .await
        .into_iter()
        .map(|(_, event)| event)
        .filter(|event| matches!(event, AuditEvent::ArchivePeriodChanged { .. }))
        .collect();
    assert_eq!(
        changes,
        [
            AuditEvent::ArchivePeriodChanged { days: Some(14) },
            AuditEvent::ArchivePeriodChanged { days: None },
            AuditEvent::ArchivePeriodChanged { days: Some(36_500) },
            AuditEvent::ArchivePeriodChanged { days: Some(7) },
        ]
    );
    let told = server.traffic(&plain, &mut stream).await;
    assert_eq!(told.len(), 4);
    assert!(told.iter().all(|(_, event)| *event
        == Event::RetentionChanged {
            workspace_id: Some(workspace.id)
        }));
}

#[tokio::test]
async fn rule_a11_one_notice_is_recorded_seven_days_before() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let member = server.join(&owner, "Member").await;
    let channel = server.channel(&owner, workspace.id, "plans").await;
    let archived_at = server.clock.now();
    archive(&server, &owner, channel.id)
        .await
        .ok::<ArchivedChannel>(StatusCode::OK);
    let mut stream = server.subscribed(&member, Level::Full).await;

    server
        .clock
        .advance(Duration::days(23) - Duration::seconds(1));
    maintain(&server).await;
    assert_eq!(stored_times(&server, channel.id).await.1, None);
    assert!(server.traffic(&member, &mut stream).await.is_empty());

    server.clock.advance(Duration::seconds(1));
    maintain(&server).await;
    let notice_at = server.clock.now();
    assert_eq!(stored_times(&server, channel.id).await.1, Some(notice_at));
    let told: Vec<Event> = server
        .traffic(&member, &mut stream)
        .await
        .into_iter()
        .map(|(_, event)| event)
        .filter(|event| !matches!(event, Event::MessageChanged { .. }))
        .collect();
    let Event::ChannelDeletionDue { channel: due } = &told[0] else {
        panic!("{told:?}");
    };
    assert_eq!(told.len(), 1);
    assert_eq!(due.channel.id, channel.id);
    assert_eq!(due.archived_at, archived_at);
    assert_eq!(due.deletes_at, Some(archived_at + Duration::days(30)));
    let lines = system_actions(&server, &member, channel.id).await;
    assert_eq!(
        lines
            .iter()
            .filter(|action| *action == "channel_deletion_due")
            .count(),
        1
    );
    let listed = archived(&server, &member, workspace.id).await;
    assert_eq!(listed[0].deletes_at, Some(notice_at + Duration::days(7)));

    server.clock.advance(Duration::days(1));
    maintain(&server).await;
    assert!(server.traffic(&member, &mut stream).await.is_empty());
    assert_eq!(stored_times(&server, channel.id).await.1, Some(notice_at));
}

#[tokio::test]
async fn rule_a12_an_archived_channel_is_deleted_after_its_period() {
    let server = TestServer::start().await;
    let mut owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let mut member = server.join(&owner, "Member").await;
    let channel = server.channel(&owner, workspace.id, "plans").await;
    let kept = server.channel(&owner, workspace.id, "kept").await;
    send(&server, &member, &channel, "to be removed").await;
    send(&server, &member, &kept, "stays").await;
    archive(&server, &owner, channel.id)
        .await
        .ok::<ArchivedChannel>(StatusCode::OK);

    server.clock.advance(Duration::days(23));
    maintain(&server).await;
    // A session lasts 30 days.
    server.renew(&mut owner).await;
    server.renew(&mut member).await;
    server
        .clock
        .advance(Duration::days(7) - Duration::seconds(1));
    maintain(&server).await;
    assert_eq!(stored_times(&server, channel.id).await.2, None);
    server
        .get(
            &member,
            &format!("/api/v1/channels/{}/messages", channel.id),
        )
        .await
        .ok::<MessagePage>(StatusCode::OK);

    let mut stream = server.subscribed(&member, Level::Full).await;
    server.clock.advance(Duration::seconds(1));
    maintain(&server).await;
    assert_eq!(
        stored_times(&server, channel.id).await.2,
        Some(server.clock.now())
    );
    server
        .get(
            &member,
            &format!("/api/v1/channels/{}/messages", channel.id),
        )
        .await
        .error(ErrorCode::NotFound);
    assert!(archived(&server, &member, workspace.id).await.is_empty());
    let told = server.traffic(&member, &mut stream).await;
    assert!(
        told.iter().any(|(_, event)| *event
            == Event::ChannelDeleted {
                workspace_id: workspace.id,
                channel_id: channel.id
            }),
        "{told:?}"
    );
    let deletions: Vec<_> = audit_events(&server, &owner, workspace.id)
        .await
        .into_iter()
        .filter(|(_, event)| matches!(event, AuditEvent::ChannelDeleted { .. }))
        .collect();
    assert_eq!(
        deletions,
        [(
            None,
            AuditEvent::ChannelDeleted {
                name: "plans".to_owned()
            }
        )]
    );

    junto_server::purge::run_once(&server.state).await.unwrap();
    let left: i64 = sqlx::query_scalar("select count(*) from messages where place_id = $1")
        .bind(channel.id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(left, 0);
    let stays: i64 = sqlx::query_scalar("select count(*) from messages where place_id = $1")
        .bind(kept.id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert!(stays > 0);
    assert_eq!(stored_times(&server, kept.id).await.2, None);
}

#[tokio::test]
async fn rule_a13_automatic_deletion_skips_what_must_stay() {
    let server = TestServer::start().await;
    let mut owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let permanent = server.channel(&owner, workspace.id, "records").await;
    server
        .patch(
            &owner,
            &format!("/api/v1/channels/{}", permanent.id),
            &json!({ "permanent_messages": true }),
        )
        .await
        .ok::<Channel>(StatusCode::OK);
    send(&server, &owner, &permanent, "a record").await;
    let restored = server.channel(&owner, workspace.id, "restored").await;
    let never: junto_protocol::structure::Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({ "name": "Never" }))
        .await
        .ok(StatusCode::CREATED);
    set_period(&server, &owner, never.id, Value::Null)
        .await
        .ok::<ArchivePeriod>(StatusCode::OK);
    let kept = server.channel(&owner, never.id, "kept").await;
    for channel in [&permanent, &restored, &kept] {
        archive(&server, &owner, channel.id)
            .await
            .ok::<ArchivedChannel>(StatusCode::OK);
    }
    let of = |found: Vec<ArchivedChannel>, id: ChannelId| {
        found
            .into_iter()
            .find(|found| found.channel.id == id)
            .unwrap()
    };
    let in_default = archived(&server, &owner, workspace.id).await;
    assert_eq!(of(in_default.clone(), permanent.id).deletes_at, None);
    assert!(of(in_default, restored.id).deletes_at.is_some());
    let in_never = archived(&server, &owner, never.id).await;
    assert_eq!(of(in_never, kept.id).deletes_at, None);

    server.clock.advance(Duration::days(25));
    maintain(&server).await;
    assert_eq!(stored_times(&server, permanent.id).await.1, None);
    assert_eq!(stored_times(&server, kept.id).await.1, None);
    assert!(stored_times(&server, restored.id).await.1.is_some());
    restore(&server, &owner, restored.id)
        .await
        .ok::<Channel>(StatusCode::OK);
    assert_eq!(stored_times(&server, restored.id).await, (None, None, None));

    server.clock.advance(Duration::days(400));
    maintain(&server).await;
    server.renew(&mut owner).await;
    for channel in [&permanent, &restored, &kept] {
        let (_, notice, deleted) = stored_times(&server, channel.id).await;
        assert_eq!((notice, deleted), (None, None), "{}", channel.name);
    }
    // A person deletes the channel with permanent messages: only the server owner.
    server
        .delete(&owner, &format!("/api/v1/channels/{}", permanent.id))
        .await
        .empty(StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn rule_a14_a_change_of_the_period_gives_a_new_notice() {
    let server = TestServer::start().await;
    let mut owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let channel = server.channel(&owner, workspace.id, "plans").await;
    archive(&server, &owner, channel.id)
        .await
        .ok::<ArchivedChannel>(StatusCode::OK);
    server.clock.advance(Duration::days(25));
    maintain(&server).await;
    assert!(stored_times(&server, channel.id).await.1.is_some());

    // A longer period: the notice of the earlier time no longer counts.
    set_period(&server, &owner, workspace.id, json!(60))
        .await
        .ok::<ArchivePeriod>(StatusCode::OK);
    assert_eq!(stored_times(&server, channel.id).await.1, None);
    server.clock.advance(Duration::days(4));
    maintain(&server).await;
    let (_, notice, deleted) = stored_times(&server, channel.id).await;
    assert_eq!((notice, deleted), (None, None));

    // A period that has ended already: the channel is not deleted at once. It gets its
    // notice now and is deleted 7 days later.
    set_period(&server, &owner, workspace.id, json!(7))
        .await
        .ok::<ArchivePeriod>(StatusCode::OK);
    maintain(&server).await;
    let notice_at = server.clock.now();
    let (_, notice, deleted) = stored_times(&server, channel.id).await;
    assert_eq!((notice, deleted), (Some(notice_at), None));
    let lines = system_actions(&server, &owner, channel.id).await;
    assert_eq!(
        lines
            .iter()
            .filter(|action| *action == "channel_deletion_due")
            .count(),
        2
    );
    server.renew(&mut owner).await;
    server
        .clock
        .advance(Duration::days(7) - Duration::seconds(1));
    maintain(&server).await;
    assert_eq!(stored_times(&server, channel.id).await.2, None);
    server.clock.advance(Duration::seconds(1));
    maintain(&server).await;
    assert!(stored_times(&server, channel.id).await.2.is_some());
}

fn uses_index(plan: &Value, index: &str) -> bool {
    match plan {
        Value::Object(node) => {
            node.get("Index Name").and_then(Value::as_str) == Some(index)
                || node.values().any(|child| uses_index(child, index))
        }
        Value::Array(nodes) => nodes.iter().any(|child| uses_index(child, index)),
        _ => false,
    }
}

fn scans_sequentially(plan: &Value, table: &str) -> bool {
    match plan {
        Value::Object(node) => {
            (node.get("Node Type").and_then(Value::as_str) == Some("Seq Scan")
                && node.get("Relation Name").and_then(Value::as_str) == Some(table))
                || node.values().any(|child| scans_sequentially(child, table))
        }
        Value::Array(nodes) => nodes.iter().any(|child| scans_sequentially(child, table)),
        _ => false,
    }
}

#[tokio::test]
async fn rule_a15_the_statements_of_the_archive_read_through_indexes() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    sqlx::query(
        "insert into channels (id, workspace_id, kind, name, position, created_at, archived_at,
                               deletion_notice_at)
         select gen_random_uuid(), $1, 'text', 'filler', n, $2,
                case when n % 3 > 0 then $2 end, case when n % 3 = 2 then $2 end
         from generate_series(1, 3000) as n",
    )
    .bind(workspace.id)
    .bind(server.clock.now())
    .execute(&mut server.test_data().await)
    .await
    .unwrap();
    sqlx::query("analyze channels")
        .execute(&server.db)
        .await
        .unwrap();
    let mut connection = server.db.acquire().await.unwrap();
    sqlx::query("set enable_seqscan = off")
        .execute(&mut *connection)
        .await
        .unwrap();
    let now = server.clock.now();
    let explain = |statement: &str| format!("explain (format json) {statement}");

    let workspace_plan: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(explain(
        junto_server::archive::NOTICE_WORKSPACE,
    )))
    .bind(uuid::Uuid::nil())
    .bind(now)
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert!(
        uses_index(&workspace_plan, "channels_archive_notice"),
        "{workspace_plan}"
    );
    assert!(
        !scans_sequentially(&workspace_plan, "channels"),
        "{workspace_plan}"
    );

    let batch_plan: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(explain(
        junto_server::archive::NOTICE_BATCH,
    )))
    .bind(workspace.id)
    .bind(now)
    .bind(30_i32)
    .bind(500_i64)
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert!(
        uses_index(&batch_plan, "channels_archive_notice"),
        "{batch_plan}"
    );
    assert!(!scans_sequentially(&batch_plan, "channels"), "{batch_plan}");

    let deletion_plan: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(explain(
        junto_server::archive::DELETION_NEXT,
    )))
    .bind(now)
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert!(
        uses_index(&deletion_plan, "channels_archive_deletion"),
        "{deletion_plan}"
    );
    assert!(
        !scans_sequentially(&deletion_plan, "channels"),
        "{deletion_plan}"
    );
}

#[tokio::test]
async fn rule_a16_an_archived_channel_is_administered() {
    let scene = Scene::start().await;
    let (server, owner, ann) = (&scene.server, &scene.owner, &scene.ann);
    let channel = server.channel(owner, scene.workspace_id, "plans").await;
    send(server, ann, &channel, "kept until the deletion").await;
    archive(server, owner, channel.id)
        .await
        .ok::<ArchivedChannel>(StatusCode::OK);

    let everyone = server.everyone_role(owner, scene.workspace_id).await;
    server
        .put(
            owner,
            &format!("/api/v1/channels/{}/overrides/{}", channel.id, everyone.id),
            &json!({ "allow": [], "deny": ["view_channel"] }),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    server
        .get(ann, &format!("/api/v1/channels/{}/messages", channel.id))
        .await
        .error(ErrorCode::NotFound);
    assert!(archived(server, ann, scene.workspace_id).await.is_empty());
    assert_eq!(archived(server, owner, scene.workspace_id).await.len(), 1);

    server
        .delete(owner, &format!("/api/v1/channels/{}", channel.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    assert!(archived(server, owner, scene.workspace_id).await.is_empty());
    assert!(stored_times(server, channel.id).await.2.is_some());
}

#[tokio::test]
async fn rule_a17_an_upload_of_an_archived_channel_takes_no_bytes() {
    let files = crate::messages_support::Files::open().await;
    let (server, owner) = (&files.server, &files.owner);
    let empty = files.start(b"abc").await;
    let filled = files.start(b"xyz").await;
    let chunk =
        |id: junto_protocol::ids::AttachmentId| format!("/api/v1/attachments/{id}/chunks/0");
    let complete =
        |id: junto_protocol::ids::AttachmentId| format!("/api/v1/attachments/{id}/complete");
    server
        .put_bytes(owner, &chunk(filled.id), None, b"xyz".to_vec())
        .await
        .ok::<Upload>(StatusCode::OK);

    archive(server, owner, files.channel.id)
        .await
        .ok::<ArchivedChannel>(StatusCode::OK);
    let before = content_state(server).await;
    let refused = [
        server
            .put_bytes(owner, &chunk(empty.id), None, b"abc".to_vec())
            .await,
        server.post(owner, &complete(filled.id), &json!({})).await,
    ];
    for reply in refused {
        assert_eq!(reply.code(), ErrorCode::ChannelArchived, "{}", reply.text());
        assert!(reply.text().contains("archived"), "{}", reply.text());
    }
    assert_eq!(content_state(server).await, before);
    let stored: (i64, i64) = sqlx::query_as(
        "select (select count(*) from attachment_chunks where attachment_id = $1),
                (select count(*) from attachments where id = $2 and complete)",
    )
    .bind(empty.id)
    .bind(filled.id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(stored, (0, 0));
    // The owner of an upload still reads it and gives it up.
    let upload = format!("/api/v1/attachments/{}/upload", empty.id);
    server
        .get(owner, &upload)
        .await
        .ok::<Upload>(StatusCode::OK);
    server
        .delete(owner, &upload)
        .await
        .empty(StatusCode::NO_CONTENT);

    restore(server, owner, files.channel.id)
        .await
        .ok::<Channel>(StatusCode::OK);
    let done: Upload = server
        .post(owner, &complete(filled.id), &json!({}))
        .await
        .ok(StatusCode::OK);
    assert_eq!(done.id, filled.id);
}
