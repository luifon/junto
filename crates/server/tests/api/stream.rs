//! The order of the events, the replay after a reconnect and the snapshot (ADR 0010,
//! ADR 0011, ADR 0207).

use std::time::Duration;

use junto_protocol::error::ErrorCode;
use junto_protocol::events::{Cursor, Event, Level, ResyncReason, ServerMessage};
use junto_protocol::ids::{StreamEpoch, WorkspaceId};
use junto_protocol::roles::WorkspaceRole;
use junto_protocol::snapshot::{Snapshot, WorkspaceState};
use junto_protocol::structure::{Category, Channel, Workspace};
use junto_server::maintenance;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use crate::stream_support::Frame;
use crate::support::{Member, TestServer, drive_until, join_all};

fn category_name(event: &Event) -> Option<String> {
    match event {
        Event::CategoryCreated { category } => Some(category.name.clone()),
        _ => None,
    }
}

#[tokio::test]
async fn the_stream_needs_a_session_and_an_upgrade_request() {
    let server = TestServer::start().await;
    let owner = server.owner().await;

    for token in [
        None,
        Some("jst_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"),
    ] {
        let refused = server.open_stream(token).await.err().unwrap();
        assert_eq!(
            refused,
            (StatusCode::UNAUTHORIZED, ErrorCode::Unauthenticated)
        );
    }
    server
        .get(&owner, "/api/v1/events")
        .await
        .error(ErrorCode::InvalidRequest);
    assert_eq!(server.state.streams.connections(), 0);
}

#[tokio::test]
async fn a_connection_starts_with_hello_and_delivers_after_a_subscription() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;

    let mut stream = server.stream(&owner).await;
    let hello: Value = serde_json::from_str(&stream.raw[0]).unwrap();
    assert_eq!(hello["type"], "hello");
    assert_eq!(hello["server_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(hello["api_versions"], json!([1]));
    assert_eq!(hello["heartbeat_seconds"], 25);
    assert_eq!(hello["idle_seconds"], 60);

    server.category(&owner, workspace.id, "before", 0).await;
    assert_eq!(stream.within(Duration::from_millis(300)).await, None);

    let head = server.log().await.last().unwrap().0;
    let cursor = stream.subscribe(Level::Full, None).await;
    assert_eq!(cursor, stream.cursor(u64::try_from(head).unwrap()));
    assert_eq!(server.snapshot(&owner).await.cursor, cursor);

    server.category(&owner, workspace.id, "first", 1).await;
    server.category(&owner, workspace.id, "second", 2).await;
    let (first_id, first) = stream.event().await;
    let (second_id, second) = stream.event().await;
    assert_eq!(category_name(&first).as_deref(), Some("first"));
    assert_eq!(category_name(&second).as_deref(), Some("second"));
    assert!(cursor.position < first_id && first_id < second_id);

    let frame: Value = serde_json::from_str(stream.raw.last().unwrap()).unwrap();
    assert_eq!(frame["type"], "event");
    assert_eq!(frame["id"], second_id);
    assert_eq!(frame["event"]["type"], "category_created");
    assert_eq!(frame["event"]["category"]["name"], "second");
    assert!(frame["at"].as_str().unwrap().ends_with('Z'));
}

/// Sends a request again while the server answers `busy` (ADR 0009).
async fn insist(server: &TestServer, member: &Member, path: &str, body: &Value) {
    loop {
        let reply = server
            .send(Method::POST, path, Some(&member.token), Some(body), None)
            .await;
        if reply.status == StatusCode::CREATED {
            return;
        }
        reply.error(ErrorCode::Busy);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_client_sees_every_event_of_concurrent_writers_in_the_order_of_the_positions() {
    const WRITERS: usize = 8;
    const CHANGES: usize = 25;
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let mut workspaces = Vec::new();
    for index in 0..WRITERS {
        let workspace: Workspace = server
            .post(
                &owner,
                "/api/v1/workspaces",
                &json!({ "name": format!("Team {index}") }),
            )
            .await
            .ok(StatusCode::CREATED);
        workspaces.push(workspace.id);
    }

    let mut live = server.stream(&owner).await;
    let start = live.subscribe(Level::Full, None).await;

    let writers = workspaces
        .iter()
        .enumerate()
        .map(|(writer, workspace_id)| {
            let (server, owner) = (&server, &owner);
            async move {
                let path = format!("/api/v1/workspaces/{workspace_id}/categories");
                for change in 0..CHANGES {
                    let body = json!({ "name": format!("{writer}-{change}"), "position": change });
                    insist(server, owner, &path, &body).await;
                }
            }
        })
        .collect();
    join_all(writers).await;

    let stored: Vec<(i64, String)> = sqlx::query_as(
        "select position, payload -> 'category' ->> 'name' from events
         where kind = 'category_created' and position > $1
         order by position",
    )
    .bind(i64::try_from(start.position).unwrap())
    .fetch_all(&server.db)
    .await
    .unwrap();
    assert_eq!(stored.len(), WRITERS * CHANGES);

    let mut replayed = server.stream(&owner).await;
    replayed.subscribe(Level::Full, Some(start)).await;
    for stream in [&mut live, &mut replayed] {
        let mut received = Vec::new();
        let mut last_id = start.position;
        while received.len() < stored.len() {
            let (id, event) = stream.event().await;
            assert!(id > last_id, "{id} came after {last_id}");
            last_id = id;
            if let Some(name) = category_name(&event) {
                received.push((i64::try_from(id).unwrap(), name));
            }
        }
        assert_eq!(received, stored);
        for writer in 0..WRITERS {
            let own: Vec<&str> = received
                .iter()
                .map(|(_, name)| name.as_str())
                .filter(|name| name.starts_with(&format!("{writer}-")))
                .collect();
            let expected: Vec<String> = (0..CHANGES)
                .map(|change| format!("{writer}-{change}"))
                .collect();
            assert_eq!(own, expected);
        }
    }
}

#[tokio::test]
async fn a_transaction_that_commits_later_gets_the_later_position() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let mut stream = server.subscribed(&owner, Level::Full).await;
    let path = format!("/api/v1/workspaces/{}/categories", workspace.id);

    server.gate().arm(0);
    let held_body = json!({ "name": "started first", "position": 1 });
    let held = server.post(&owner, &path, &held_body);
    tokio::pin!(held);
    drive_until(&mut held, server.gate().arrived()).await;
    let unassigned: Vec<Option<i64>> =
        sqlx::query_scalar("select position from events where position is null")
            .fetch_all(&server.db)
            .await
            .unwrap();
    assert!(unassigned.is_empty(), "an uncommitted event is visible");

    server
        .category(&owner, workspace.id, "committed first", 2)
        .await;
    let (early_id, early) = stream.event().await;
    assert_eq!(category_name(&early).as_deref(), Some("committed first"));

    server.gate().release();
    held.await.ok::<Category>(StatusCode::CREATED);
    let (late_id, late) = stream.event().await;
    assert_eq!(category_name(&late).as_deref(), Some("started first"));
    assert!(late_id > early_id);
}

#[tokio::test]
async fn an_event_exists_only_when_its_change_committed() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let path = format!("/api/v1/workspaces/{}/categories", workspace.id);
    let before = server.log().await;

    server.gate().lose_next(8);
    server
        .post(&owner, &path, &json!({ "name": "lost" }))
        .await
        .error(ErrorCode::Busy);
    server
        .post(&owner, &path, &json!({ "name": "" }))
        .await
        .error(ErrorCode::InvalidRequest);
    assert_eq!(server.log().await, before);
    let categories: i64 = sqlx::query_scalar("select count(*) from categories")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(categories, 0);

    server.category(&owner, workspace.id, "kept", 0).await;
    let after = server.log().await;
    assert_eq!(after.len(), before.len() + 1);
    assert_eq!(after.last().unwrap().1, "category_created");
    assert!(after.last().unwrap().0 > before.last().unwrap().0);
}

#[tokio::test]
async fn the_database_assigns_positions_at_the_commit_and_refuses_changes_of_events() {
    let server = TestServer::start().await;
    let insert = "insert into events (id, recorded_at, class, kind, audience, payload)
         values ($1, now(), 'general', 'workspace_updated',
                 '{\"to\":\"workspace\"}', '{\"type\":\"workspace_updated\"}')";

    let mut transaction = server.db.begin().await.unwrap();
    let id = uuid::Uuid::now_v7();
    sqlx::query(insert)
        .bind(id)
        .execute(&mut *transaction)
        .await
        .unwrap();
    let inside: Option<i64> = sqlx::query_scalar("select position from events where id = $1")
        .bind(id)
        .fetch_one(&mut *transaction)
        .await
        .unwrap();
    assert_eq!(inside, None);
    transaction.commit().await.unwrap();
    let first: i64 = sqlx::query_scalar("select position from events where id = $1")
        .bind(id)
        .fetch_one(&server.db)
        .await
        .unwrap();

    let next = uuid::Uuid::now_v7();
    sqlx::query(insert)
        .bind(next)
        .execute(&server.db)
        .await
        .unwrap();
    let second: i64 = sqlx::query_scalar("select position from events where id = $1")
        .bind(next)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert!(second > first);

    for statement in [
        "update events set position = position + 100",
        "update events set payload = '{\"type\":\"workspace_updated\",\"changed\":true}'",
        "update events set audience = '{\"to\":\"account\"}'",
        "insert into events (id, recorded_at, class, kind, audience, payload)
         values (gen_random_uuid(), now(), 'general', 'a', '{\"to\":\"workspace\"}', '{\"type\":\"b\"}')",
        "insert into events (id, recorded_at, class, kind, audience, payload)
         values (gen_random_uuid(), now(), 'loud', 'a', '{\"to\":\"workspace\"}', '{\"type\":\"a\"}')",
        "insert into events (id, recorded_at, class, kind, audience, payload)
         values (gen_random_uuid(), now(), 'general', 'a', '{\"to\":\"account\"}', '{\"type\":\"a\"}')",
        "insert into events
             (id, recorded_at, class, kind, audience, audience_account_id, payload)
         values (gen_random_uuid(), now(), 'control', 'a',
                 '{\"to\":\"account\",\"account_id\":\"0198f1c2-7d3a-7b1e-9c55-0a1b2c3d4e5f\"}',
                 '0198f1c2-7d3a-7b1e-9c55-000000000000', '{\"type\":\"a\"}')",
        "update event_stream set trimmed_through = -1",
        "insert into event_stream (singleton, epoch) values (false, gen_random_uuid())",
    ] {
        assert!(
            sqlx::query(statement).execute(&server.db).await.is_err(),
            "{statement}"
        );
    }
}

async fn categories(
    server: &TestServer,
    owner: &Member,
    workspace_id: WorkspaceId,
    names: &[&str],
) {
    for (position, name) in names.iter().enumerate() {
        server
            .category(owner, workspace_id, name, i32::try_from(position).unwrap())
            .await;
    }
}

async fn names(stream: &mut crate::stream_support::Stream, count: usize) -> Vec<(u64, String)> {
    let mut received = Vec::new();
    while received.len() < count {
        let (id, event) = stream.event().await;
        if let Some(name) = category_name(&event) {
            received.push((id, name));
        }
    }
    received
}

#[tokio::test]
async fn a_reconnect_replays_the_events_after_the_cursor_and_repeats_are_recognised() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;

    let mut first = server.subscribed(&owner, Level::Full).await;
    categories(&server, &owner, workspace.id, &["a", "b", "c"]).await;
    let seen = names(&mut first, 3).await;
    drop(first);
    categories(&server, &owner, workspace.id, &["d", "e"]).await;

    let mut second = server.stream(&owner).await;
    let applied = seen[1].0;
    let cursor = second.cursor(applied);
    assert_eq!(second.subscribe(Level::Full, Some(cursor)).await, cursor);
    let replayed = names(&mut second, 3).await;
    assert_eq!(replayed[0], seen[2]);
    assert_eq!(
        replayed
            .iter()
            .map(|(_, name)| name.as_str())
            .collect::<Vec<_>>(),
        ["c", "d", "e"]
    );

    let again = second.subscribe(Level::Full, Some(cursor)).await;
    assert_eq!(again, cursor);
    assert_eq!(names(&mut second, 3).await, replayed);
    let repeats = replayed.iter().filter(|(id, _)| *id <= seen[2].0).count();
    assert_eq!(repeats, 1);
}

#[tokio::test]
async fn beyond_the_replay_bound_the_client_is_told_to_load_a_snapshot() {
    let server = TestServer::start_with(|config| {
        config.limits.stream.replay_most_events = 5;
    })
    .await;
    let owner = server.owner().await;
    let other = server.join(&owner, "Other").await;
    let workspace = server.default_workspace(&owner).await;
    let mut stream = server.stream(&owner).await;
    let start = stream.subscribe(Level::Full, None).await;

    for index in 0..10 {
        server
            .patch(
                &other,
                &format!("/api/v1/account/devices/{}", other.device_id),
                &json!({ "name": format!("desk {index}") }),
            )
            .await
            .ok::<Value>(StatusCode::OK);
    }
    categories(&server, &owner, workspace.id, &["a", "b", "c", "d", "e"]).await;
    let mut reconnected = server.stream(&owner).await;
    reconnected.subscribe(Level::Full, Some(start)).await;
    assert_eq!(names(&mut reconnected, 5).await.len(), 5);

    let last = server.category(&owner, workspace.id, "f", 5).await;
    reconnected
        .send(&junto_protocol::events::ClientMessage::Subscribe {
            level: Level::Full,
            cursor: Some(start),
        })
        .await;
    let mut answer = reconnected.next().await;
    if let Frame::Message(ServerMessage::Event {
        event: Event::CategoryCreated { category },
        ..
    }) = &answer
    {
        assert_eq!(category.id, last.id);
        answer = reconnected.next().await;
    }
    assert_eq!(
        answer,
        Frame::Message(ServerMessage::Resync {
            reason: ResyncReason::TooFarBehind
        })
    );
    server.category(&owner, workspace.id, "unseen", 6).await;
    assert_eq!(reconnected.within(Duration::from_millis(300)).await, None);

    let snapshot = server.snapshot(&owner).await;
    assert_eq!(
        reconnected
            .subscribe(Level::Full, Some(snapshot.cursor))
            .await,
        snapshot.cursor
    );
    server.category(&owner, workspace.id, "g", 7).await;
    assert_eq!(names(&mut reconnected, 1).await[0].1, "g");
}

#[tokio::test]
async fn a_cursor_that_the_log_does_not_know_needs_a_snapshot() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let mut stream = server.stream(&owner).await;
    let head = server.snapshot(&owner).await.cursor;

    for (cursor, reason) in [
        (
            Cursor {
                epoch: StreamEpoch::generate(),
                position: 0,
            },
            ResyncReason::UnknownCursor,
        ),
        (
            Cursor {
                epoch: head.epoch,
                position: head.position + 1,
            },
            ResyncReason::UnknownCursor,
        ),
        (
            Cursor {
                epoch: head.epoch,
                position: u64::MAX,
            },
            ResyncReason::UnknownCursor,
        ),
    ] {
        stream
            .send(&junto_protocol::events::ClientMessage::Subscribe {
                level: Level::Full,
                cursor: Some(cursor),
            })
            .await;
        assert_eq!(
            stream.next().await,
            Frame::Message(ServerMessage::Resync { reason })
        );
    }
    assert_eq!(stream.subscribe(Level::Full, Some(head)).await, head);
}

#[tokio::test]
async fn the_log_keeps_a_bounded_number_of_positions_and_older_cursors_need_a_snapshot() {
    let server = TestServer::start_with(|config| {
        config.limits.stream.events_kept = 3;
        config.limits.maintenance_batch = 2;
    })
    .await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let mut held = server.stream(&owner).await;
    let start = held.subscribe(Level::Full, None).await;
    server.state.streams.hold();

    categories(
        &server,
        &owner,
        workspace.id,
        &["a", "b", "c", "d", "e", "f"],
    )
    .await;
    let before = server.log().await;
    maintenance::run_once(&server.state).await.unwrap();
    let kept = server.log().await;
    let head = before.last().unwrap().0;
    assert_eq!(kept, before[before.len() - 3..]);
    let floor: i64 = sqlx::query_scalar("select trimmed_through from event_stream")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(floor, head - 3);

    let mut stream = server.stream(&owner).await;
    stream
        .send(&junto_protocol::events::ClientMessage::Subscribe {
            level: Level::Full,
            cursor: Some(start),
        })
        .await;
    assert_eq!(
        stream.next().await,
        Frame::Message(ServerMessage::Resync {
            reason: ResyncReason::TooFarBehind
        })
    );
    let at_floor = stream.cursor(u64::try_from(floor).unwrap());
    stream.subscribe(Level::Full, Some(at_floor)).await;
    server.state.streams.release();
    assert_eq!(
        names(&mut stream, 3)
            .await
            .iter()
            .map(|(_, name)| name.as_str())
            .collect::<Vec<_>>(),
        ["d", "e", "f"]
    );

    assert_eq!(
        held.next().await,
        Frame::Message(ServerMessage::Resync {
            reason: ResyncReason::TooFarBehind
        })
    );
    assert_eq!(
        server.snapshot(&owner).await.cursor.position,
        u64::try_from(head).unwrap()
    );

    maintenance::run_once(&server.state).await.unwrap();
    assert_eq!(server.log().await, kept);
}

pub fn sorted(mut snapshot: Snapshot) -> Snapshot {
    snapshot.messages.sort_by_key(|m| match m.place {
        junto_protocol::message::MessagePlace::Channel { channel_id, .. } => channel_id.as_uuid(),
        junto_protocol::message::MessagePlace::Direct {
            conversation_id, ..
        } => conversation_id.as_uuid(),
    });
    snapshot.workspaces.sort_by_key(|state| state.workspace.id);
    for state in &mut snapshot.workspaces {
        state
            .categories
            .sort_by_key(|category| (category.position, category.id));
        state
            .channels
            .sort_by_key(|channel| (channel.position, channel.id));
        state.roles.sort_by_key(|role| (!role.is_everyone, role.id));
    }
    snapshot
}

fn upsert<T, K: PartialEq>(items: &mut Vec<T>, item: T, key: impl Fn(&T) -> K) {
    match items
        .iter_mut()
        .find(|existing| key(existing) == key(&item))
    {
        Some(existing) => *existing = item,
        None => items.push(item),
    }
}

/// Applies an event to a snapshot as a client does.
pub async fn apply(server: &TestServer, member: &Member, snapshot: &mut Snapshot, event: Event) {
    if let Event::MessageChanged { place, .. } | Event::MessageRead { place, .. } = &event {
        let path = match place {
            junto_protocol::message::MessagePlace::Channel { channel_id, .. } => {
                format!("/api/v1/channels/{channel_id}/messages/read")
            }
            junto_protocol::message::MessagePlace::Direct {
                conversation_id, ..
            } => format!("/api/v1/conversations/{conversation_id}/messages/read"),
        };
        let reply = server.get(member, &path).await;
        if reply.status == StatusCode::NOT_FOUND {
            snapshot.messages.retain(|m| m.place != *place);
        } else {
            let current: junto_protocol::message::MessageState = reply.ok(StatusCode::OK);
            upsert(&mut snapshot.messages, current, |m| m.place);
        }
    }

    let state = |snapshot: &mut Snapshot, workspace_id: WorkspaceId| -> Option<usize> {
        snapshot
            .workspaces
            .iter()
            .position(|state| state.workspace.id == workspace_id)
    };
    match event {
        Event::WorkspaceCreated { workspace } => {
            if state(snapshot, workspace.id).is_none() {
                snapshot.workspaces.push(WorkspaceState {
                    workspace,
                    own_role_ids: Vec::new(),
                    roles: Vec::new(),
                    categories: Vec::new(),
                    channels: Vec::new(),
                    calls: Vec::new(),
                    permissions: Vec::new(),
                    channel_permissions: Vec::new(),
                    notification_default: None,
                });
            }
        }
        Event::WorkspaceUpdated { workspace } => {
            if let Some(index) = state(snapshot, workspace.id) {
                snapshot.workspaces[index].workspace = workspace;
            }
        }
        // The client loads a snapshot (ADR 0208). The lists of the permissions of the
        // account are part of it (ADR 0310).
        Event::PermissionsChanged {
            workspace_id: Some(workspace_id),
        } => {
            let loaded = server.snapshot(member).await;
            let current = loaded
                .workspaces
                .iter()
                .find(|state| state.workspace.id == workspace_id);
            if let (Some(index), Some(current)) = (state(snapshot, workspace_id), current) {
                snapshot.workspaces[index].permissions = current.permissions.clone();
                snapshot.workspaces[index].channel_permissions =
                    current.channel_permissions.clone();
            }
        }
        Event::CategoryCreated { category } | Event::CategoryUpdated { category } => {
            if let Some(index) = state(snapshot, category.workspace_id) {
                upsert(
                    &mut snapshot.workspaces[index].categories,
                    category,
                    |c: &Category| c.id,
                );
            }
        }
        Event::ChannelCreated { channel } | Event::ChannelUpdated { channel } => {
            if let Some(index) = state(snapshot, channel.workspace_id) {
                upsert(
                    &mut snapshot.workspaces[index].channels,
                    channel,
                    |c: &Channel| c.id,
                );
            }
        }
        Event::ChannelDeleted {
            workspace_id,
            channel_id,
        } => {
            snapshot.messages.retain(|m| !matches!(m.place, junto_protocol::message::MessagePlace::Channel {channel_id:id,..} if id==channel_id));
            if let Some(index) = state(snapshot, workspace_id) {
                snapshot.workspaces[index]
                    .channels
                    .retain(|channel| channel.id != channel_id);
            }
        }
        Event::WorkspaceRolePositionsShifted {
            workspace_id,
            positions,
        } => {
            if let Some(index) = state(snapshot, workspace_id) {
                for changed in positions {
                    if let Some(role) = snapshot.workspaces[index]
                        .roles
                        .iter_mut()
                        .find(|r| r.id == changed.role_id)
                    {
                        role.position = changed.position;
                    }
                }
            }
        }
        Event::WorkspaceRoleCreated { role } | Event::WorkspaceRoleUpdated { role } => {
            if let Some(index) = state(snapshot, role.workspace_id) {
                upsert(
                    &mut snapshot.workspaces[index].roles,
                    role,
                    |r: &WorkspaceRole| r.id,
                );
            }
        }
        _ => {}
    }
}

/// A series of changes of the structure of one workspace.
async fn change_structure(
    server: &TestServer,
    owner: &Member,
    workspace_id: WorkspaceId,
    tag: usize,
) {
    let post = |path: String, body: Value| async move {
        loop {
            let reply = server
                .send(Method::POST, &path, Some(&owner.token), Some(&body), None)
                .await;
            if reply.status == StatusCode::CREATED {
                return reply;
            }
            reply.error(ErrorCode::Busy);
        }
    };
    let change = |method: Method, path: String, body: Value| async move {
        loop {
            let body = (!body.is_null()).then_some(&body);
            let reply = server
                .send(method.clone(), &path, Some(&owner.token), body, None)
                .await;
            if reply.status.is_success() {
                return;
            }
            reply.error(ErrorCode::Busy);
        }
    };
    let base = format!("/api/v1/workspaces/{workspace_id}");
    for round in 0..4 {
        let name = format!("{tag}-{round}");
        let position = i32::try_from(tag * 10 + round).unwrap();
        let category: Category = post(
            format!("{base}/categories"),
            json!({ "name": name, "position": position }),
        )
        .await
        .ok(StatusCode::CREATED);
        let channel: Channel = post(
            format!("{base}/channels"),
            json!({ "name": name, "kind": "text", "position": position }),
        )
        .await
        .ok(StatusCode::CREATED);
        let doomed: Channel = post(
            format!("{base}/channels"),
            json!({ "name": format!("{name}-doomed"), "kind": "voice", "position": position }),
        )
        .await
        .ok(StatusCode::CREATED);
        let role: WorkspaceRole = post(format!("{base}/roles"), json!({ "name": name }))
            .await
            .ok(StatusCode::CREATED);
        change(
            Method::PATCH,
            format!("/api/v1/channels/{}", channel.id),
            json!({ "name": format!("{name}-renamed"), "category_id": category.id }),
        )
        .await;
        change(
            Method::PATCH,
            format!("{base}/categories/{}", category.id),
            json!({ "name": format!("{name}-renamed") }),
        )
        .await;
        change(
            Method::PATCH,
            format!("{base}/roles/{}", role.id),
            json!({ "permissions": ["manage_messages"] }),
        )
        .await;
        change(
            Method::DELETE,
            format!("/api/v1/channels/{}", doomed.id),
            Value::Null,
        )
        .await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_snapshot_and_the_events_after_its_cursor_give_the_live_state() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let member = server.join(&owner, "Member").await;
    let workspace = server.default_workspace(&owner).await;
    let empty = server.snapshot(&member).await;

    let writers: Vec<_> = (0..4)
        .map(|tag| change_structure(&server, &owner, workspace.id, tag))
        .collect();
    let during = async {
        let mut taken = Vec::new();
        for _ in 0..6 {
            tokio::time::sleep(Duration::from_millis(40)).await;
            taken.push(server.snapshot(&member).await);
        }
        taken
    };
    let (_, mut snapshots) = tokio::join!(join_all(writers), during);
    snapshots.insert(0, empty);
    server
        .patch(
            &owner,
            &format!("/api/v1/workspaces/{}", workspace.id),
            &json!({ "name": "Renamed" }),
        )
        .await
        .ok::<Workspace>(StatusCode::OK);

    let live = sorted(server.snapshot(&member).await);
    assert_eq!(live.workspaces[0].workspace.name, "Renamed");
    assert_eq!(live.workspaces[0].channels.len(), 2 + 16);
    assert!(
        snapshots
            .windows(2)
            .any(|pair| pair[0].cursor != pair[1].cursor),
        "no snapshot was taken during the changes"
    );

    for mut snapshot in snapshots {
        let mut stream = server.stream(&member).await;
        stream.subscribe(Level::Full, Some(snapshot.cursor)).await;
        for (id, event) in server.traffic(&member, &mut stream).await {
            assert!(id > snapshot.cursor.position);
            apply(&server, &member, &mut snapshot, event).await;
        }
        snapshot.cursor = live.cursor;
        assert_eq!(sorted(snapshot), live);
    }
}

#[tokio::test]
async fn a_snapshot_holds_only_what_the_caller_may_see() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let insider = server.join(&owner, "Insider").await;
    let outsider = server.join(&owner, "Outsider").await;
    let workspace = server.default_workspace(&owner).await;
    let closed: Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({ "name": "Closed" }))
        .await
        .ok(StatusCode::CREATED);
    let role_id = server
        .grant_workspace(&owner, &insider, workspace.id, &[])
        .await;
    let secret = server.channel(&owner, workspace.id, "secret-plans").await;
    server
        .make_private(&owner, workspace.id, secret.id, role_id)
        .await;

    let of = |snapshot: &Snapshot| -> WorkspaceState { snapshot.workspaces[0].clone() };
    let inside = server.snapshot(&insider).await;
    let outside = server.snapshot(&outsider).await;
    assert_eq!(inside.workspaces.len(), 1);
    assert_eq!(inside.account.id, insider.account_id);
    assert_eq!(of(&inside).own_role_ids, [role_id]);
    assert!(
        of(&inside)
            .channels
            .iter()
            .any(|channel| channel.id == secret.id)
    );
    assert_eq!(of(&inside).roles.len(), 2);
    assert!(of(&inside).roles[0].is_everyone);

    assert!(of(&outside).own_role_ids.is_empty());
    assert_eq!(of(&outside).channels.len(), 2);
    let text = server.get(&outsider, "/api/v1/snapshot").await.text();
    assert!(!text.contains(&secret.id.to_string()));
    assert!(!text.contains("secret-plans"));
    assert!(!text.contains(&closed.id.to_string()));

    let all = server.snapshot(&owner).await;
    assert_eq!(all.workspaces.len(), 2);
    assert!(all.account.is_owner);
    assert_eq!(inside.cursor, all.cursor);
}
