//! Exports (ADR 0411): who asks for one, what its file holds, how it is built, downloaded
//! and removed.

use std::sync::Arc;

use junto_protocol::WorkspacePermission;
use junto_protocol::audit::{AuditEvent, AuditPage};
use junto_protocol::error::ErrorCode;
use junto_protocol::events::{Event, Level};
use junto_protocol::export::{
    Export, ExportFailure, ExportManifest, ExportRemoval, ExportState, ExportedRecording,
};
use junto_protocol::ids::{ChannelId, ExportId, MessageId, WorkspaceId, WorkspaceRoleId};
use junto_protocol::message::{Conversation, Message, MessagePage, MessagePlace, Upload};
use junto_protocol::structure::Channel;
use junto_server::exports::build::{Job, Progress};
use junto_server::exports::zip;
use junto_server::storage::FakeDisk;
use reqwest::StatusCode;
use serde_json::{Value, json};
use time::Duration;

use crate::messages_support::{send, text};
use crate::recordings_support::{FakeWorker, Said, Studio, transcript_of};
use crate::support::{FREE_DISK, Member, Reply, Scratch, TestServer};

/// A server with a directory for attachments and one for exports, each on a disk whose
/// free space the test states, an owner, a member who holds `export_channel` through a
/// role, a member without a role, and one text channel.
struct Desk {
    attachments: Scratch,
    exports: Scratch,
    disk: Arc<FakeDisk>,
    server: TestServer,
    owner: Member,
    exporter: Member,
    plain: Member,
    workspace_id: WorkspaceId,
    role_id: WorkspaceRoleId,
    channel: Channel,
}

/// Gives a server its two directories and the disks of the tests.
fn with_directories(
    attachments: &Scratch,
    exports: &Scratch,
) -> impl FnOnce(&mut junto_server::config::Config) {
    let (attachments, exports) = (attachments.0.clone(), exports.0.clone());
    move |config| {
        config.attachments_directory = Some(attachments);
        config.exports_directory = Some(exports);
    }
}

fn use_fake_disks(server: &TestServer) -> Arc<FakeDisk> {
    let disk = Arc::new(FakeDisk::new(FREE_DISK));
    server.state.exports.space().use_disk(disk.clone());
    server
        .state
        .attachment_space
        .use_disk(Arc::new(FakeDisk::new(FREE_DISK)));
    disk
}

impl Desk {
    async fn open() -> Self {
        let (attachments, exports) = (Scratch::new(), Scratch::new());
        let server = TestServer::start_with(with_directories(&attachments, &exports)).await;
        Self::on(server, attachments, exports).await
    }

    async fn on(server: TestServer, attachments: Scratch, exports: Scratch) -> Self {
        let disk = use_fake_disks(&server);
        let owner = server.owner().await;
        let workspace_id = server.default_workspace(&owner).await.id;
        let exporter = server.join(&owner, "Exporter").await;
        let plain = server.join(&owner, "Plain").await;
        let role_id = server
            .grant_workspace(
                &owner,
                &exporter,
                workspace_id,
                &[WorkspacePermission::ExportChannel],
            )
            .await;
        let channel = server.channel(&owner, workspace_id, "plans").await;
        Self {
            attachments,
            exports,
            disk,
            server,
            owner,
            exporter,
            plain,
            workspace_id,
            role_id,
            channel,
        }
    }

    /// Takes the export permission from the exporter, or gives it back.
    async fn permit(&self, permitted: bool) {
        let path = format!(
            "/api/v1/workspaces/{}/members/{}/roles/{}",
            self.workspace_id, self.exporter.account_id, self.role_id
        );
        let reply = if permitted {
            self.server.put_empty(&self.owner, &path).await
        } else {
            self.server.delete(&self.owner, &path).await
        };
        reply.empty(StatusCode::NO_CONTENT);
    }

    /// The names in the directory of an export.
    fn files_of(&self, id: ExportId) -> Vec<String> {
        files_in(&self.exports, id)
    }
}

fn files_in(exports: &Scratch, id: ExportId) -> Vec<String> {
    let Ok(read) = std::fs::read_dir(exports.0.join(id.to_string())) else {
        return Vec::new();
    };
    let mut names: Vec<String> = read
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

async fn request(server: &TestServer, member: &Member, channel_id: ChannelId) -> Reply {
    server
        .post(
            member,
            &format!("/api/v1/channels/{channel_id}/exports"),
            &Value::Null,
        )
        .await
}

async fn requested(server: &TestServer, member: &Member, channel_id: ChannelId) -> Export {
    request(server, member, channel_id)
        .await
        .ok(StatusCode::CREATED)
}

async fn build(server: &TestServer) {
    junto_server::exports::run_once(&server.state)
        .await
        .unwrap();
}

async fn export_reply(server: &TestServer, member: &Member, id: ExportId) -> Reply {
    server.get(member, &format!("/api/v1/exports/{id}")).await
}

async fn export_of(server: &TestServer, member: &Member, id: ExportId) -> Export {
    export_reply(server, member, id).await.ok(StatusCode::OK)
}

async fn download(server: &TestServer, member: &Member, id: ExportId) -> Reply {
    server
        .get(member, &format!("/api/v1/exports/{id}/file"))
        .await
}

/// The entries of the file of a ready export, read by the requester.
async fn entries(server: &TestServer, member: &Member, id: ExportId) -> Vec<zip::Entry> {
    let reply = download(server, member, id).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    zip::read(reply.bytes()).unwrap()
}

fn entry<'a>(entries: &'a [zip::Entry], name: &str) -> &'a zip::Entry {
    entries
        .iter()
        .find(|entry| entry.name == name)
        .unwrap_or_else(|| {
            let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
            panic!("no entry {name} among {names:?}")
        })
}

fn messages_of(entries: &[zip::Entry]) -> Vec<Message> {
    serde_json::from_slice(&entry(entries, "messages.json").content).unwrap()
}

fn page_of(entries: &[zip::Entry]) -> String {
    String::from_utf8(entry(entries, "messages.html").content.clone()).unwrap()
}

async fn upload_named(
    server: &TestServer,
    author: &Member,
    place: Value,
    name: &str,
    bytes: &[u8],
) -> Upload {
    let mut body = place;
    body["name"] = json!(name);
    body["bytes"] = json!(bytes.len());
    body["sha256"] = json!(junto_server::messages::uploads::hash(bytes));
    let upload: Upload = server
        .post(author, "/api/v1/attachments", &body)
        .await
        .ok(StatusCode::CREATED);
    for (index, chunk) in bytes.chunks(1024 * 1024).enumerate() {
        server
            .put_bytes(
                author,
                &format!(
                    "/api/v1/attachments/{}/chunks/{}",
                    upload.id,
                    index * 1024 * 1024
                ),
                None,
                chunk.to_vec(),
            )
            .await
            .ok::<Upload>(StatusCode::OK);
    }
    server
        .post(
            author,
            &format!("/api/v1/attachments/{}/complete", upload.id),
            &json!({}),
        )
        .await
        .ok(StatusCode::OK)
}

async fn send_with(
    server: &TestServer,
    author: &Member,
    path: &str,
    words: &str,
    attachments: &[junto_protocol::ids::AttachmentId],
) -> Message {
    let mut body = text(words);
    body["attachments"] = json!(attachments);
    server
        .post(author, path, &body)
        .await
        .ok(StatusCode::CREATED)
}

async fn rows(server: &TestServer, statement: &str) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(statement.to_owned()))
        .fetch_one(&server.db)
        .await
        .unwrap()
}

async fn events_of(
    server: &TestServer,
    member: &Member,
    stream: &mut crate::stream_support::Stream,
) -> Vec<Event> {
    server
        .traffic(member, stream)
        .await
        .into_iter()
        .map(|(_, event)| event)
        .filter(|event| !matches!(event, Event::MessageChanged { .. }))
        .collect()
}

#[tokio::test]
async fn rule_e01_a_request_needs_export_channel() {
    let desk = Desk::open().await;
    let server = &desk.server;
    let hidden = server
        .channel(&desk.owner, desk.workspace_id, "hidden")
        .await;
    let insiders = server
        .grant_workspace(&desk.owner, &desk.owner, desk.workspace_id, &[])
        .await;
    server
        .make_private(&desk.owner, desk.workspace_id, hidden.id, insiders)
        .await;

    request(server, &desk.plain, desk.channel.id)
        .await
        .error(ErrorCode::Forbidden);
    for outside in [&desk.plain, &desk.exporter] {
        request(server, outside, hidden.id)
            .await
            .error(ErrorCode::NotFound);
    }
    request(server, &desk.owner, ChannelId::generate())
        .await
        .error(ErrorCode::NotFound);
    assert_eq!(rows(server, "select count(*) from exports").await, 0);

    let of_exporter = requested(server, &desk.exporter, desk.channel.id).await;
    assert_eq!(of_exporter.state, ExportState::Waiting);
    // The administrator permission holds `export_channel` in every channel.
    requested(server, &desk.owner, hidden.id).await;
    desk.permit(false).await;
    request(server, &desk.exporter, desk.channel.id)
        .await
        .error(ErrorCode::Forbidden);
}

#[tokio::test]
async fn rule_e02_a_request_stores_one_waiting_export() {
    let desk = Desk::open().await;
    let server = &desk.server;
    let mut of_exporter = server.subscribed(&desk.exporter, Level::Background).await;
    let mut of_owner = server.subscribed(&desk.owner, Level::Full).await;

    let export = requested(server, &desk.exporter, desk.channel.id).await;
    assert_eq!(
        export,
        Export {
            id: export.id,
            place: MessagePlace::Channel {
                workspace_id: desk.workspace_id,
                channel_id: desk.channel.id
            },
            state: ExportState::Waiting,
            failure: None,
            requested_at: server.clock.now(),
            finished_at: None,
            expires_at: None,
            bytes: None,
        }
    );
    assert_eq!(
        events_of(server, &desk.exporter, &mut of_exporter).await,
        [Event::ExportUpdated {
            export: export.clone()
        }]
    );
    assert!(
        events_of(server, &desk.owner, &mut of_owner)
            .await
            .is_empty()
    );
    let page: AuditPage = server
        .get(
            &desk.owner,
            &format!("/api/v1/workspaces/{}/audit-log", desk.workspace_id),
        )
        .await
        .ok(StatusCode::OK);
    let entry = &page.entries[0];
    assert_eq!(
        (entry.actor_id, entry.channel_id, &entry.event),
        (
            Some(desk.exporter.account_id),
            Some(desk.channel.id),
            &AuditEvent::ExportRequested {
                export_id: export.id
            }
        )
    );
    let history: MessagePage = server
        .get(
            &desk.plain,
            &format!("/api/v1/channels/{}/messages", desk.channel.id),
        )
        .await
        .ok(StatusCode::OK);
    assert!(history.messages.iter().any(|message| {
        message
            .system
            .as_ref()
            .is_some_and(|system| system.action == "export_requested")
    }));

    // While the export waits, a second request returns it.
    let again: Export = request(server, &desk.exporter, desk.channel.id)
        .await
        .ok(StatusCode::OK);
    assert_eq!(again, export);
    assert_eq!(rows(server, "select count(*) from exports").await, 1);
    assert_eq!(
        rows(
            server,
            "select count(*) from audit_entries where action = 'export_requested'"
        )
        .await,
        1
    );
    // The export of another account for the same channel is another export.
    let of_owner_export = requested(server, &desk.owner, desk.channel.id).await;
    assert_ne!(of_owner_export.id, export.id);
}

#[tokio::test]
async fn rule_e03_an_account_has_a_ceiling_of_exports() {
    let desk = Desk::open().await;
    let server = &desk.server;
    let limits = &server.state.config.limits.exports;
    assert_eq!(
        (limits.most_open_per_account, limits.most_per_account),
        (3, 10)
    );
    let mut channels = vec![desk.channel.clone()];
    for name in ["second", "third", "fourth"] {
        channels.push(server.channel(&desk.owner, desk.workspace_id, name).await);
    }
    for channel in &channels[..3] {
        requested(server, &desk.exporter, channel.id).await;
    }
    request(server, &desk.exporter, channels[3].id)
        .await
        .error(ErrorCode::LimitReached);
    assert_eq!(rows(server, "select count(*) from exports").await, 3);
    // The ceiling counts the exports of the account, not of the server.
    requested(server, &desk.owner, channels[3].id).await;

    build(server).await;
    for round in 0..3 {
        let wanted = if round == 2 { 1 } else { 3 };
        for channel in &channels[..wanted] {
            requested(server, &desk.exporter, channel.id).await;
        }
        build(server).await;
    }
    let listed: Vec<Export> = server
        .get(&desk.exporter, "/api/v1/exports")
        .await
        .ok(StatusCode::OK);
    assert_eq!(listed.len(), 10);
    assert!(
        listed
            .iter()
            .all(|export| export.state == ExportState::Ready)
    );
    request(server, &desk.exporter, channels[0].id)
        .await
        .error(ErrorCode::LimitReached);

    server
        .delete(&desk.exporter, &format!("/api/v1/exports/{}", listed[0].id))
        .await
        .empty(StatusCode::NO_CONTENT);
    requested(server, &desk.exporter, channels[0].id).await;
}

#[tokio::test]
async fn rule_e04_an_export_reserves_its_space() {
    let desk = Desk::open().await;
    let server = &desk.server;
    let reserve = server.state.storage_reserve_bytes();
    let content = vec![7u8; 300_000];
    let upload = upload_named(
        server,
        &desk.owner,
        json!({ "channel_id": desk.channel.id }),
        "data.bin",
        &content,
    )
    .await;
    let path = format!("/api/v1/channels/{}/messages", desk.channel.id);
    send_with(server, &desk.owner, &path, "with a file", &[upload.id]).await;
    let reservations = "select count(*) from storage_reservations where kind = 'export'";

    // At the reserve, and with a disk that cannot be read, no export starts.
    desk.disk.set_free(reserve);
    request(server, &desk.exporter, desk.channel.id)
        .await
        .error(ErrorCode::StorageLow);
    desk.disk.set_unreadable();
    request(server, &desk.exporter, desk.channel.id)
        .await
        .error(ErrorCode::StorageLow);
    assert_eq!(rows(server, "select count(*) from exports").await, 0);

    // Above the reserve the request is taken. The build needs the bytes of the file on
    // top of the reserve; without them the export fails and leaves nothing.
    desk.disk.set_free(reserve + 100_000);
    let short = requested(server, &desk.exporter, desk.channel.id).await;
    build(server).await;
    let failed = export_of(server, &desk.exporter, short.id).await;
    assert_eq!(
        (failed.state, failed.failure, failed.bytes),
        (ExportState::Failed, Some(ExportFailure::StorageLow), None)
    );
    assert!(failed.expires_at.is_some());
    assert_eq!(rows(server, reservations).await, 0);
    assert!(desk.files_of(short.id).is_empty());

    // With the space, the bytes are reserved while the file is written, and not after.
    desk.disk.set_free(FREE_DISK);
    let export = requested(server, &desk.exporter, desk.channel.id).await;
    let mut job = Job::claim(&server.state).await.unwrap().unwrap();
    assert_eq!(job.export_id(), export.id);
    while rows(server, reservations).await == 0 {
        assert_eq!(job.step(&server.state).await.unwrap(), Progress::More);
    }
    let reserved: i64 =
        sqlx::query_scalar("select bytes from storage_reservations where subject_id = $1")
            .bind(export.id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    while job.step(&server.state).await.unwrap() == Progress::More {
        assert_eq!(rows(server, reservations).await, 1);
    }
    assert_eq!(rows(server, reservations).await, 0);
    let ready = export_of(server, &desk.exporter, export.id).await;
    let bytes = i64::try_from(ready.bytes.unwrap()).unwrap();
    assert!(bytes > 300_000, "{bytes}");
    assert!(reserved >= bytes, "{reserved} {bytes}");
    assert_eq!(desk.files_of(export.id), ["1.zip"]);
}

#[tokio::test]
async fn rule_e05_the_file_holds_messages_attachments_recordings_and_transcripts() {
    let (attachments, exports) = (Scratch::new(), Scratch::new());
    let worker = FakeWorker::new();
    let settings = worker.settings();
    let directories = with_directories(&attachments, &exports);
    let server = TestServer::start_for_calls_with(|config| {
        config.recorder.as_mut().unwrap().transcriber = Some(settings);
        directories(config);
    })
    .await;
    use_fake_disks(&server);
    let studio = Studio::on(server).await;
    let scene = &studio.scene;
    let (server, owner, ann) = (&scene.server, &scene.owner, &scene.ann);
    let voice = &scene.voice;
    let path = format!("/api/v1/channels/{}/messages", voice.id);

    let content: Vec<u8> = (0..2_500_000u32).map(|n| (n % 251) as u8).collect();
    let upload = upload_named(
        server,
        ann,
        json!({ "channel_id": voice.id }),
        "minutes.bin",
        &content,
    )
    .await;
    let root = send_with(server, ann, &path, "the first message", &[upload.id]).await;
    let reply: Message = server
        .post(
            owner,
            &format!("/api/v1/messages/{}/thread", root.id),
            &text("a reply in the thread"),
        )
        .await
        .ok(StatusCode::CREATED);
    server
        .put(
            owner,
            &format!("/api/v1/messages/{}/reaction", root.id),
            &json!({ "emoji": "👍" }),
        )
        .await
        .empty(StatusCode::NO_CONTENT);

    let length = Duration::seconds(i64::from(junto_server::recorder::samples::SECONDS));
    let done = studio.finished_after(length).await;
    let said = [
        Said(0, 500, 1500, "Good morning everyone.", None),
        Said(1, 700, 1900, "Can you hear me?", None),
    ];
    worker.give(&transcript_of(&done, (&[], &[]), &said));
    assert!(server.transcribe().await);
    let recording_id = done.recording.id;

    let export = requested(server, owner, voice.id).await;
    build(server).await;
    let ready = export_of(server, owner, export.id).await;
    assert_eq!(ready.state, ExportState::Ready, "{ready:?}");
    assert_eq!(ready.finished_at, Some(server.clock.now()));
    assert_eq!(
        ready.expires_at,
        Some(server.clock.now() + Duration::days(7))
    );

    let reply_of = download(server, owner, export.id).await;
    assert_eq!(reply_of.status, StatusCode::OK);
    assert_eq!(reply_of.header("content-type"), Some("application/zip"));
    assert_eq!(
        reply_of.header("content-disposition"),
        Some(format!("attachment; filename=\"junto-export-{}.zip\"", export.id).as_str())
    );
    assert_eq!(reply_of.bytes().len() as u64, ready.bytes.unwrap());
    let found = zip::read(reply_of.bytes()).unwrap();

    let read = messages_of(&found);
    let authored: Vec<&Message> = read.iter().filter(|m| m.author_id.is_some()).collect();
    assert_eq!(
        authored.iter().map(|m| m.id).collect::<Vec<_>>(),
        [root.id, reply.id]
    );
    assert_eq!(authored[0].attachments[0].id, upload.id);
    assert_eq!(authored[0].reactions[0].count, 1);
    assert_eq!(authored[1].thread_root, Some(root.id));
    assert!(read.iter().any(|message| message.system.is_some()));
    let page = page_of(&found);
    assert!(page.starts_with("<!doctype html>"));
    assert!(page.contains("the first message"));
    assert!(page.contains("a reply in the thread"));
    assert!(page.contains(&format!("attachments/{}/minutes.bin", upload.id)));
    assert!(page.ends_with("</html>\n"));

    let file = entry(&found, &format!("attachments/{}/minutes.bin", upload.id));
    assert_eq!(file.content, content);

    let description: ExportedRecording = serde_json::from_slice(
        &entry(&found, &format!("recordings/{recording_id}/recording.json")).content,
    )
    .unwrap();
    assert_eq!(description.recording.recording.id, recording_id);
    assert!(description.transcript);
    assert_eq!(description.files.len(), 3);
    for file in &description.files {
        let name = format!("recordings/{recording_id}/{}", file.name);
        let on_disk = std::fs::read(
            server
                .recordings_dir()
                .join(format!("{recording_id}/{}", file.name)),
        )
        .unwrap();
        assert_eq!(entry(&found, &name).content, on_disk, "{name}");
    }
    let transcript = String::from_utf8(
        entry(&found, &format!("recordings/{recording_id}/transcript.txt"))
            .content
            .clone(),
    )
    .unwrap();
    assert!(
        transcript.contains("Ann: Good morning everyone."),
        "{transcript}"
    );
    assert!(transcript.contains("Ben: Can you hear me?"), "{transcript}");

    let manifest: ExportManifest =
        serde_json::from_slice(&entry(&found, "export.json").content).unwrap();
    assert_eq!(
        manifest,
        ExportManifest {
            format: 1,
            export_id: export.id,
            place: export.place,
            channel_name: Some("lounge".to_owned()),
            requested_by: owner.account_id,
            requested_at: export.requested_at,
            messages: read.len() as u64,
            attachments: 1,
            recordings: 1,
        }
    );
    assert_eq!(found.len(), 4 + 5);

    // A range of the file is served like the range of a recording.
    let ranged = reqwest::Client::new()
        .get(format!("{}/api/v1/exports/{}/file", server.base, export.id))
        .bearer_auth(&owner.token)
        .header("Range", "bytes=0-3");
    let part = TestServer::reply(ranged).await;
    assert_eq!(part.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(part.bytes(), b"PK\x03\x04");

    // The removal of the recording removes the export that holds it.
    server.leave_call(ann).await;
    server.leave_call(&scene.ben).await;
    server.check().await;
    server
        .delete(owner, &format!("/api/v1/recordings/{recording_id}"))
        .await
        .empty(StatusCode::NO_CONTENT);
    export_reply(server, owner, export.id)
        .await
        .error(ErrorCode::NotFound);
    drop((attachments, exports));
}

#[tokio::test]
async fn rule_e06_all_content_in_the_html_is_escaped() {
    let desk = Desk::open().await;
    let server = &desk.server;
    let hostile = "<script>alert('x')</script>\"&";
    let named = server.join(&desk.owner, "<b>Bold</b>").await;
    let channel: Channel = server
        .post(
            &desk.owner,
            &format!("/api/v1/workspaces/{}/channels", desk.workspace_id),
            &json!({ "name": "<i>plans</i>", "kind": "text" }),
        )
        .await
        .ok(StatusCode::CREATED);
    let upload = upload_named(
        server,
        &named,
        json!({ "channel_id": channel.id }),
        "<img src=x onerror=alert(1)>.txt",
        b"file",
    )
    .await;
    let path = format!("/api/v1/channels/{}/messages", channel.id);
    send_with(server, &named, &path, hostile, &[upload.id]).await;
    server
        .post(
            &named,
            &path,
            &json!({"document":{"blocks":[
                {"kind":"code","text": hostile, "language": null},
                {"kind":"paragraph","content":[
                    {"kind":"link","text": hostile, "url":"https://example.test/?a=<b>&c=\"d\""},
                    {"kind":"account","account_id": named.account_id},
                    {"kind":"code","text": hostile}]}]}}),
        )
        .await
        .ok::<Message>(StatusCode::CREATED);

    let export = requested(server, &desk.owner, channel.id).await;
    build(server).await;
    let found = entries(server, &desk.owner, export.id).await;
    let page = page_of(&found);
    for raw in [
        "<script",
        "<b>",
        "<i>",
        "<img",
        "alert('x')",
        "onerror=alert(1)>",
    ] {
        assert!(!page.contains(raw), "{raw}: {page}");
    }
    assert!(page.contains("&lt;script&gt;alert(&#39;x&#39;)&lt;/script&gt;&quot;&amp;"));
    assert!(page.contains("<title>&lt;i&gt;plans&lt;/i&gt;</title>"));
    assert!(page.contains("@&lt;b&gt;Bold&lt;/b&gt;"));
    assert!(page.contains("href=\"https://example.test/?a=&lt;b&gt;&amp;c=&quot;d&quot;\""));
    // The JSON form holds the text as it was written.
    let read = messages_of(&found);
    assert!(serde_json::to_string(&read).unwrap().contains("<script>"));
}

#[tokio::test]
async fn rule_e07_an_export_holds_only_what_its_requester_reads() {
    let (attachments, exports) = (Scratch::new(), Scratch::new());
    let server = TestServer::start_for_calls_with(with_directories(&attachments, &exports)).await;
    use_fake_disks(&server);
    let scene = crate::calls_support::Scene::on(server).await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let voice = &scene.voice;
    let export_and_record = [
        WorkspacePermission::ExportChannel,
        WorkspacePermission::RecordCalls,
    ];
    server
        .grant_workspace(owner, ann, scene.workspace_id, &export_and_record)
        .await;
    let outside = server.join(owner, "Outside").await;
    server
        .grant_workspace(
            owner,
            &outside,
            scene.workspace_id,
            &[WorkspacePermission::ExportChannel],
        )
        .await;
    // The channel hides who is inside: the lines of its call and its recording are for
    // the people inside while the call runs.
    server
        .call_settings(owner, voice.id, json!({ "hides_occupants": true }))
        .await;
    let ann_seat = server.enter_call(ann, voice.id).await;
    let ben_seat = server.enter_call(ben, voice.id).await;
    server.speak(&ann_seat);
    server.speak(&ben_seat);
    let started = server.record(ann, voice.id).await;
    server.tend().await;
    server
        .stop_recording(ann, voice.id)
        .await
        .empty(StatusCode::NO_CONTENT);
    for _ in 0..3 {
        server.tend().await;
    }
    assert_eq!(server.process().await, 1);
    let private: Vec<MessageId> =
        sqlx::query_scalar("select id from messages where place_id = $1 and readers is not null")
            .bind(voice.id)
            .fetch_all(&server.db)
            .await
            .unwrap();
    assert!(!private.is_empty());

    let path = format!("/api/v1/channels/{}/messages", voice.id);
    let kept = send_with(server, ann, &path, "kept", &[]).await;
    let deleted = send_with(server, ann, &path, "deleted before the request", &[]).await;
    // A message answers it, so its row stays as a marker when it is deleted.
    let mut answer = text("an answer");
    answer["reply_to"] = json!(deleted.id);
    server
        .post(ann, &path, &answer)
        .await
        .ok::<Message>(StatusCode::CREATED);
    server
        .delete(ann, &format!("/api/v1/messages/{}", deleted.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    let markers: i64 =
        sqlx::query_scalar("select count(*) from messages where id = $1 and deleted")
            .bind(deleted.id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(markers, 1);

    let of_outside = requested(server, &outside, voice.id).await;
    let of_owner = requested(server, owner, voice.id).await;
    let of_ann = requested(server, ann, voice.id).await;
    // What is written after the request is not part of the export.
    let later = send_with(server, ann, &path, "after the request", &[]).await;
    build(server).await;

    let recording_entry = format!("recordings/{}/recording.json", started.id);
    for (member, export) in [(&outside, &of_outside), (owner, &of_owner)] {
        let found = entries(server, member, export.id).await;
        let ids: Vec<MessageId> = messages_of(&found).iter().map(|m| m.id).collect();
        assert!(ids.contains(&kept.id));
        assert!(!ids.contains(&deleted.id) && !ids.contains(&later.id));
        assert!(private.iter().all(|id| !ids.contains(id)), "{ids:?}");
        assert!(
            found
                .iter()
                .all(|entry| !entry.name.starts_with("recordings/"))
        );
        let page = page_of(&found);
        assert!(!page.contains("after the request"));
        assert!(!page.contains("deleted before the request"));
    }
    let found = entries(server, ann, of_ann.id).await;
    let ids: Vec<MessageId> = messages_of(&found).iter().map(|m| m.id).collect();
    assert!(private.iter().all(|id| ids.contains(id)), "{ids:?}");
    assert!(!ids.contains(&later.id) && !ids.contains(&deleted.id));
    assert!(found.iter().any(|entry| entry.name == recording_entry));
    drop((attachments, exports));
}

#[tokio::test]
async fn rule_e08_a_lost_access_fails_the_build() {
    let desk = Desk::open().await;
    let server = &desk.server;
    send(server, &desk.owner, &desk.channel, "content").await;

    // Lost before the build.
    let early = requested(server, &desk.exporter, desk.channel.id).await;
    desk.permit(false).await;
    build(server).await;
    let failed = export_of(server, &desk.exporter, early.id).await;
    assert_eq!(
        (failed.state, failed.failure),
        (ExportState::Failed, Some(ExportFailure::AccessEnded))
    );
    assert!(desk.files_of(early.id).is_empty());
    download(server, &desk.exporter, early.id)
        .await
        .error(ErrorCode::NotFound);

    // Lost while the file is written: the next page fails the export, and the partial
    // file is deleted.
    desk.permit(true).await;
    let late = requested(server, &desk.exporter, desk.channel.id).await;
    let mut stream = server.subscribed(&desk.exporter, Level::Background).await;
    let mut job = Job::claim(&server.state).await.unwrap().unwrap();
    let reservations = "select count(*) from storage_reservations where kind = 'export'";
    while rows(server, reservations).await == 0 {
        assert_eq!(job.step(&server.state).await.unwrap(), Progress::More);
    }
    assert_eq!(job.step(&server.state).await.unwrap(), Progress::More);
    assert!(
        desk.files_of(late.id).contains(&"1.zip.partial".to_owned()),
        "{:?}",
        desk.files_of(late.id)
    );
    desk.permit(false).await;
    let mut steps = 0;
    while job.step(&server.state).await.unwrap() == Progress::More {
        steps += 1;
        assert!(steps < 3, "the build went on without the permission");
    }
    let failed = export_of(server, &desk.exporter, late.id).await;
    assert_eq!(
        (failed.state, failed.failure, failed.bytes),
        (ExportState::Failed, Some(ExportFailure::AccessEnded), None)
    );
    assert!(
        desk.files_of(late.id).is_empty(),
        "{:?}",
        desk.files_of(late.id)
    );
    assert_eq!(rows(server, reservations).await, 0);
    let told = events_of(server, &desk.exporter, &mut stream).await;
    assert_eq!(told.last(), Some(&Event::ExportUpdated { export: failed }));

    // Lost after the last page and before the export is ready: the finished file is
    // deleted. A first build of the same content counts the steps.
    desk.permit(true).await;
    let counted = requested(server, &desk.exporter, desk.channel.id).await;
    let mut job = Job::claim(&server.state).await.unwrap().unwrap();
    let mut pages = 0;
    while job.step(&server.state).await.unwrap() == Progress::More {
        pages += 1;
    }
    assert_eq!(
        export_of(server, &desk.exporter, counted.id).await.state,
        ExportState::Ready
    );
    let last = requested(server, &desk.exporter, desk.channel.id).await;
    let mut job = Job::claim(&server.state).await.unwrap().unwrap();
    assert_eq!(job.export_id(), last.id);
    for _ in 0..pages {
        assert_eq!(job.step(&server.state).await.unwrap(), Progress::More);
    }
    assert_eq!(
        desk.files_of(last.id),
        ["1.directory.partial", "1.html.partial", "1.zip.partial"]
    );
    desk.permit(false).await;
    assert_eq!(job.step(&server.state).await.unwrap(), Progress::Finished);
    let failed = export_of(server, &desk.exporter, last.id).await;
    assert_eq!(
        (failed.state, failed.failure, failed.bytes),
        (ExportState::Failed, Some(ExportFailure::AccessEnded), None)
    );
    assert!(
        desk.files_of(last.id).is_empty(),
        "{:?}",
        desk.files_of(last.id)
    );
    assert_eq!(rows(server, reservations).await, 0);

    // A member who no longer sees the channel fails in the same way.
    desk.permit(true).await;
    let hidden = requested(server, &desk.exporter, desk.channel.id).await;
    let everyone = server.everyone_role(&desk.owner, desk.workspace_id).await;
    server
        .put(
            &desk.owner,
            &format!(
                "/api/v1/channels/{}/rules/roles/{}",
                desk.channel.id, everyone.id
            ),
            &json!({ "allow": [], "deny": ["view_channel"] }),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    build(server).await;
    let state: (String, Option<String>) =
        sqlx::query_as("select state, failure from exports where id = $1")
            .bind(hidden.id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(
        state,
        ("failed".to_owned(), Some("access_ended".to_owned()))
    );
}

#[tokio::test]
async fn rule_e09_only_the_requester_with_the_permission_downloads() {
    let desk = Desk::open().await;
    let server = &desk.server;
    send(server, &desk.owner, &desk.channel, "content").await;
    let second = server.join(&desk.owner, "Second").await;
    server
        .grant_workspace(
            &desk.owner,
            &second,
            desk.workspace_id,
            &[WorkspacePermission::ExportChannel],
        )
        .await;
    let export = requested(server, &desk.exporter, desk.channel.id).await;
    // An export that is not ready has no file.
    download(server, &desk.exporter, export.id)
        .await
        .error(ErrorCode::NotFound);
    build(server).await;

    let whole = download(server, &desk.exporter, export.id).await;
    assert_eq!(whole.status, StatusCode::OK);
    assert!(zip::read(whole.bytes()).is_ok());
    // Another account does not read the export, whatever it holds: not its state, not
    // its file.
    for other in [&second, &desk.owner, &desk.plain] {
        download(server, other, export.id)
            .await
            .error(ErrorCode::NotFound);
        export_reply(server, other, export.id)
            .await
            .error(ErrorCode::NotFound);
        server
            .delete(other, &format!("/api/v1/exports/{}", export.id))
            .await
            .error(ErrorCode::NotFound);
    }
    download(server, &desk.exporter, ExportId::generate())
        .await
        .error(ErrorCode::NotFound);

    // The permission is decided at each download.
    desk.permit(false).await;
    download(server, &desk.exporter, export.id)
        .await
        .error(ErrorCode::NotFound);
    assert_eq!(
        export_of(server, &desk.exporter, export.id).await.state,
        ExportState::Ready
    );
    desk.permit(true).await;
    assert_eq!(
        download(server, &desk.exporter, export.id).await.status,
        StatusCode::OK
    );

    // An export past its lifetime has no file, also before maintenance removes it.
    server.clock.advance(Duration::days(7));
    download(server, &desk.exporter, export.id)
        .await
        .error(ErrorCode::NotFound);
}

#[tokio::test]
async fn rule_e10_an_export_is_removed_seven_days_after_it_finished() {
    let desk = Desk::open().await;
    let server = &desk.server;
    let export = requested(server, &desk.exporter, desk.channel.id).await;
    build(server).await;
    let failing = requested(server, &desk.exporter, desk.channel.id).await;
    desk.permit(false).await;
    build(server).await;
    desk.permit(true).await;
    assert_eq!(desk.files_of(export.id), ["1.zip"]);
    let mut stream = server.subscribed(&desk.exporter, Level::Background).await;

    server
        .clock
        .advance(Duration::days(7) - Duration::seconds(1));
    junto_server::maintenance::run_once(&server.state)
        .await
        .unwrap();
    build(server).await;
    assert_eq!(rows(server, "select count(*) from exports").await, 2);
    assert_eq!(desk.files_of(export.id), ["1.zip"]);
    assert_eq!(
        download(server, &desk.exporter, export.id).await.status,
        StatusCode::OK
    );

    // At the expiry the file is not sent any more, also before maintenance has run.
    server.clock.advance(Duration::seconds(1));
    download(server, &desk.exporter, export.id)
        .await
        .error(ErrorCode::NotFound);
    assert_eq!(
        export_of(server, &desk.exporter, export.id).await.state,
        ExportState::Ready
    );
    junto_server::maintenance::run_once(&server.state)
        .await
        .unwrap();
    for id in [export.id, failing.id] {
        export_reply(server, &desk.exporter, id)
            .await
            .error(ErrorCode::NotFound);
    }
    let mut told = events_of(server, &desk.exporter, &mut stream).await;
    told.sort_by_key(|event| format!("{event:?}"));
    let mut expected = vec![
        Event::ExportRemoved {
            export_id: export.id,
            reason: ExportRemoval::Expired,
        },
        Event::ExportRemoved {
            export_id: failing.id,
            reason: ExportRemoval::Expired,
        },
    ];
    expected.sort_by_key(|event| format!("{event:?}"));
    assert_eq!(told, expected);
    // The task of the exports deletes the file and then the row.
    build(server).await;
    assert_eq!(rows(server, "select count(*) from exports").await, 0);
    assert!(!desk.exports.0.join(export.id.to_string()).exists());
}

#[tokio::test]
async fn rule_e11_a_deletion_inside_an_export_removes_it() {
    let desk = Desk::open().await;
    let server = &desk.server;
    let path = format!("/api/v1/channels/{}/messages", desk.channel.id);
    let inside = send(server, &desk.owner, &desk.channel, "inside").await;
    let other_channel = server
        .channel(&desk.owner, desk.workspace_id, "other")
        .await;
    let elsewhere = send(server, &desk.owner, &other_channel, "elsewhere").await;
    let upload = upload_named(
        server,
        &desk.owner,
        json!({ "channel_id": desk.channel.id }),
        "old.bin",
        b"old file",
    )
    .await;
    send_with(server, &desk.owner, &path, "with a file", &[upload.id]).await;
    let message = |id: MessageId| format!("/api/v1/messages/{id}");

    let export = requested(server, &desk.exporter, desk.channel.id).await;
    build(server).await;
    let mut stream = server.subscribed(&desk.exporter, Level::Background).await;

    // A message that is newer than the export, and a message of another channel, are not
    // in it: their deletion leaves the export.
    let later = send(server, &desk.owner, &desk.channel, "later").await;
    for id in [later.id, elsewhere.id] {
        server
            .delete(&desk.owner, &message(id))
            .await
            .empty(StatusCode::NO_CONTENT);
    }
    assert_eq!(
        export_of(server, &desk.exporter, export.id).await.state,
        ExportState::Ready
    );
    assert!(
        events_of(server, &desk.exporter, &mut stream)
            .await
            .is_empty()
    );

    server
        .delete(&desk.owner, &message(inside.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    export_reply(server, &desk.exporter, export.id)
        .await
        .error(ErrorCode::NotFound);
    download(server, &desk.exporter, export.id)
        .await
        .error(ErrorCode::NotFound);
    assert_eq!(
        events_of(server, &desk.exporter, &mut stream).await,
        [Event::ExportRemoved {
            export_id: export.id,
            reason: ExportRemoval::ContentDeleted,
        }]
    );
    build(server).await;
    assert!(!desk.exports.0.join(export.id.to_string()).exists());

    // An export that is written when the deletion happens stops and leaves no file.
    let inside = send(server, &desk.owner, &desk.channel, "inside again").await;
    let running = requested(server, &desk.exporter, desk.channel.id).await;
    let mut job = Job::claim(&server.state).await.unwrap().unwrap();
    let reservations = "select count(*) from storage_reservations where kind = 'export'";
    while rows(server, reservations).await == 0 {
        assert_eq!(job.step(&server.state).await.unwrap(), Progress::More);
    }
    assert_eq!(job.step(&server.state).await.unwrap(), Progress::More);
    server
        .delete(&desk.owner, &message(inside.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(rows(server, reservations).await, 0);
    assert_eq!(job.step(&server.state).await.unwrap(), Progress::Finished);
    export_reply(server, &desk.exporter, running.id)
        .await
        .error(ErrorCode::NotFound);
    build(server).await;
    assert!(!desk.exports.0.join(running.id.to_string()).exists());
    assert_eq!(rows(server, "select count(*) from exports").await, 0);

    // File retention deletes the files of a message: the export that holds them goes.
    let with_file = requested(server, &desk.exporter, desk.channel.id).await;
    build(server).await;
    assert_eq!(
        export_of(server, &desk.exporter, with_file.id).await.state,
        ExportState::Ready
    );
    server.clock.advance(Duration::days(2));
    server
        .put(
            &desk.owner,
            &format!("/api/v1/workspaces/{}/file-retention", desk.workspace_id),
            &json!({ "days": 1 }),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    junto_server::maintenance::run_once(&server.state)
        .await
        .unwrap();
    export_reply(server, &desk.exporter, with_file.id)
        .await
        .error(ErrorCode::NotFound);
    drop(desk.attachments);
}

/// The person who deletes a message reads the same positions whether the deletion removes
/// the export of another account or none (ADR 0010, ADR 0011).
#[tokio::test]
async fn rule_e11_a_deletion_shows_its_author_nothing_of_the_exports_it_removes() {
    let mut seen = Vec::new();
    for exported in [false, true] {
        let desk = Desk::open().await;
        let server = &desk.server;
        let inside = send(server, &desk.owner, &desk.channel, "inside").await;
        let export = if exported {
            let export = requested(server, &desk.exporter, desk.channel.id).await;
            build(server).await;
            Some(export)
        } else {
            None
        };
        let before = server.snapshot(&desk.owner).await.cursor;
        let mut stream = server.stream(&desk.owner).await;
        stream.subscribe(Level::Full, Some(before)).await;
        server
            .delete(&desk.owner, &format!("/api/v1/messages/{}", inside.id))
            .await
            .empty(StatusCode::NO_CONTENT);
        let after = server.snapshot(&desk.owner).await.cursor;
        let received: Vec<u64> = server
            .traffic(&desk.owner, &mut stream)
            .await
            .into_iter()
            .map(|(id, _)| id - before.position)
            .collect();
        assert!(!received.is_empty());
        seen.push((received, after.position - before.position));
        if let Some(export) = export {
            export_reply(server, &desk.exporter, export.id)
                .await
                .error(ErrorCode::NotFound);
        }
    }
    assert_eq!(seen[0], seen[1]);
}

#[tokio::test]
async fn rule_e12_the_requester_removes_an_export() {
    let desk = Desk::open().await;
    let server = &desk.server;
    let export = requested(server, &desk.exporter, desk.channel.id).await;
    build(server).await;
    let mut stream = server.subscribed(&desk.exporter, Level::Background).await;
    let path = format!("/api/v1/exports/{}", export.id);

    server
        .delete(&desk.owner, &path)
        .await
        .error(ErrorCode::NotFound);
    assert_eq!(
        export_of(server, &desk.exporter, export.id).await.state,
        ExportState::Ready
    );
    server
        .delete(&desk.exporter, &path)
        .await
        .empty(StatusCode::NO_CONTENT);
    export_reply(server, &desk.exporter, export.id)
        .await
        .error(ErrorCode::NotFound);
    download(server, &desk.exporter, export.id)
        .await
        .error(ErrorCode::NotFound);
    server
        .delete(&desk.exporter, &path)
        .await
        .error(ErrorCode::NotFound);
    assert_eq!(
        events_of(server, &desk.exporter, &mut stream).await,
        [Event::ExportRemoved {
            export_id: export.id,
            reason: ExportRemoval::Removed,
        }]
    );
    assert_eq!(desk.files_of(export.id), ["1.zip"]);
    build(server).await;
    assert!(!desk.exports.0.join(export.id.to_string()).exists());
    assert_eq!(rows(server, "select count(*) from exports").await, 0);
}

#[tokio::test]
async fn rule_e13_the_list_holds_own_exports() {
    let desk = Desk::open().await;
    let server = &desk.server;
    let second = server
        .channel(&desk.owner, desk.workspace_id, "second")
        .await;
    let first = requested(server, &desk.exporter, desk.channel.id).await;
    let newer = requested(server, &desk.exporter, second.id).await;
    let of_owner = requested(server, &desk.owner, desk.channel.id).await;
    let list = |member: &Member| {
        let member_token = member.token.clone();
        async move {
            let reply = server
                .send::<Value>(
                    reqwest::Method::GET,
                    "/api/v1/exports",
                    Some(&member_token),
                    None,
                    None,
                )
                .await;
            let listed: Vec<Export> = reply.ok(StatusCode::OK);
            listed
                .into_iter()
                .map(|export| export.id)
                .collect::<Vec<_>>()
        }
    };
    assert_eq!(list(&desk.exporter).await, [newer.id, first.id]);
    assert_eq!(list(&desk.owner).await, [of_owner.id]);
    assert!(list(&desk.plain).await.is_empty());
}

#[tokio::test]
async fn rule_e14_builds_run_one_at_a_time_and_are_taken_again() {
    let desk = Desk::open().await;
    let server = &desk.server;
    let limits = server.state.config.limits.exports.clone();
    let second = server
        .channel(&desk.owner, desk.workspace_id, "second")
        .await;
    let first = requested(server, &desk.exporter, desk.channel.id).await;
    let other = requested(server, &desk.exporter, second.id).await;
    let state = |id: ExportId| async move {
        let row: (String, Option<String>, i32) =
            sqlx::query_as("select state, failure, attempts from exports where id = $1")
                .bind(id)
                .fetch_one(&server.db)
                .await
                .unwrap();
        row
    };

    // The oldest export is taken, and no second one while its process writes.
    let job = Job::claim(&server.state).await.unwrap().unwrap();
    assert_eq!(job.export_id(), first.id);
    assert_eq!(state(first.id).await, ("running".to_owned(), None, 1));
    assert!(Job::claim(&server.state).await.unwrap().is_none());
    assert_eq!(state(other.id).await, ("waiting".to_owned(), None, 0));

    // The process ends without an end of the build. After the lease, the export is taken
    // again, and at its fourth turn it fails.
    drop(job);
    for attempt in 2..=limits.attempts {
        server.clock.advance(limits.lease - Duration::seconds(1));
        assert!(Job::claim(&server.state).await.unwrap().is_none());
        server.clock.advance(Duration::seconds(1));
        let job = Job::claim(&server.state).await.unwrap().unwrap();
        assert_eq!(job.export_id(), first.id);
        let attempts = i32::try_from(attempt).unwrap();
        assert_eq!(
            state(first.id).await,
            ("running".to_owned(), None, attempts)
        );
    }
    server.clock.advance(limits.lease);
    let job = Job::claim(&server.state).await.unwrap().unwrap();
    assert_eq!(job.export_id(), other.id);
    assert_eq!(
        state(first.id).await,
        ("failed".to_owned(), Some("interrupted".to_owned()), 3)
    );
    let failed = export_of(server, &desk.exporter, first.id).await;
    assert_eq!(failed.failure, Some(ExportFailure::Interrupted));
    drop(job);
    server.clock.advance(limits.lease);
    build(server).await;
    assert_eq!(
        export_of(server, &desk.exporter, other.id).await.state,
        ExportState::Ready
    );
    assert_eq!(desk.files_of(other.id), ["2.zip"]);
}

#[tokio::test]
async fn rule_e15_export_events_reach_the_requester_only() {
    let desk = Desk::open().await;
    let server = &desk.server;
    let second = server.join(&desk.owner, "Second").await;
    server
        .grant_workspace(
            &desk.owner,
            &second,
            desk.workspace_id,
            &[WorkspacePermission::ExportChannel],
        )
        .await;
    let mut of_exporter = server.subscribed(&desk.exporter, Level::Background).await;
    let mut of_second = server.subscribed(&second, Level::Full).await;
    let mut of_owner = server.subscribed(&desk.owner, Level::Full).await;

    let export = requested(server, &desk.exporter, desk.channel.id).await;
    build(server).await;
    let ready = export_of(server, &desk.exporter, export.id).await;
    let states: Vec<ExportState> = events_of(server, &desk.exporter, &mut of_exporter)
        .await
        .into_iter()
        .map(|event| match event {
            Event::ExportUpdated { export: told } => {
                assert_eq!(told.id, export.id);
                told.state
            }
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        states,
        [
            ExportState::Waiting,
            ExportState::Running,
            ExportState::Ready
        ]
    );
    assert_eq!(ready.state, ExportState::Ready);
    for (member, stream) in [(&second, &mut of_second), (&desk.owner, &mut of_owner)] {
        assert!(events_of(server, member, stream).await.is_empty());
    }
}

/// ADR 0411, ADR 0518: the export of a direct conversation holds the lines of its calls as
/// its history shows them, and its page states each line with its duration.
#[tokio::test]
async fn rule_e16_the_export_of_a_direct_conversation_holds_the_lines_of_its_calls() {
    let (attachments, exports) = (Scratch::new(), Scratch::new());
    let server = TestServer::start_for_calls_with(with_directories(&attachments, &exports)).await;
    use_fake_disks(&server);
    let scene = crate::calls_support::Scene::on(server).await;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    server.enter_direct(ann, ben).await;
    server.enter_direct(ben, ann).await;
    server.clock.advance(Duration::seconds(125));
    server.leave_call(ben).await;
    server.leave_call(ann).await;
    let conversations: Vec<Conversation> = server
        .get(ann, "/api/v1/conversations")
        .await
        .ok(StatusCode::OK);
    let conversation = conversations.into_iter().next().expect("a conversation");
    let path = format!("/api/v1/conversations/{}/messages", conversation.id);
    send_with(server, &scene.ben, &path, "after the call", &[]).await;
    server.clock.advance(Duration::seconds(1));
    server.enter_direct(ben, ann).await;
    let lifetime = server.state.config.limits.calls.invitation_lifetime;
    server.clock.advance(lifetime + Duration::seconds(1));
    server.check().await;
    server.leave_call(ben).await;
    let history: MessagePage = server.get(ann, &path).await.ok(StatusCode::OK);
    let actions: Vec<Option<&str>> = history
        .messages
        .iter()
        .map(|message| message.system.as_ref().map(|line| line.action.as_str()))
        .collect();
    assert_eq!(
        actions,
        [
            Some("direct_call_started"),
            Some("direct_call_ended"),
            None,
            Some("direct_call_started"),
            Some("direct_call_not_answered"),
        ]
    );

    let export: Export = server
        .post(
            ann,
            &format!("/api/v1/conversations/{}/exports", conversation.id),
            &Value::Null,
        )
        .await
        .ok(StatusCode::CREATED);
    build(server).await;
    let found = entries(server, ann, export.id).await;
    assert_eq!(messages_of(&found), history.messages);
    let page = page_of(&found);
    for line in [
        "<p class=\"system\">direct call started by Ann</p>",
        "<p class=\"system\">direct call ended by Ann, 125 seconds</p>",
        "<p class=\"system\">direct call started by Ben</p>",
        "<p class=\"system\">direct call not answered by Ben</p>",
    ] {
        assert!(page.contains(line), "{line} is not in {page}");
    }
}

#[tokio::test]
async fn rule_e16_a_participant_exports_a_direct_conversation() {
    let (attachments, exports) = (Scratch::new(), Scratch::new());
    let server = TestServer::start_with(with_directories(&attachments, &exports)).await;
    use_fake_disks(&server);
    let owner = server.owner().await;
    let ann = server.join(&owner, "Ann").await;
    let ben = server.join(&owner, "Ben").await;
    let third = server.join(&owner, "Third").await;
    let conversation: Conversation = server
        .post(
            &ann,
            "/api/v1/conversations",
            &json!({ "account_id": ben.account_id }),
        )
        .await
        .ok(StatusCode::OK);
    let path = format!("/api/v1/conversations/{}/messages", conversation.id);
    let upload = upload_named(
        &server,
        &ann,
        json!({ "conversation_id": conversation.id }),
        "note.txt",
        b"between two people",
    )
    .await;
    let first = send_with(&server, &ann, &path, "hello Ben", &[upload.id]).await;
    let second = send_with(&server, &ben, &path, "hello Ann", &[]).await;
    let exports_path = format!("/api/v1/conversations/{}/exports", conversation.id);

    // Nobody except the two participants asks for it, the administrators included.
    for outsider in [&third, &owner] {
        server
            .post(outsider, &exports_path, &Value::Null)
            .await
            .error(ErrorCode::NotFound);
    }
    let export: Export = server
        .post(&ann, &exports_path, &Value::Null)
        .await
        .ok(StatusCode::CREATED);
    assert_eq!(
        export.place,
        MessagePlace::Direct {
            conversation_id: conversation.id,
            low_account_id: conversation.low_account_id,
            high_account_id: conversation.high_account_id,
        }
    );
    // No audit entry tells that the conversation exists.
    assert_eq!(
        rows(
            &server,
            "select count(*) from audit_entries where action = 'export_requested'"
        )
        .await,
        0
    );
    build(&server).await;
    let found = entries(&server, &ann, export.id).await;
    let ids: Vec<MessageId> = messages_of(&found).iter().map(|m| m.id).collect();
    assert_eq!(ids, [first.id, second.id]);
    assert_eq!(
        entry(&found, &format!("attachments/{}/note.txt", upload.id)).content,
        b"between two people"
    );
    let manifest: ExportManifest =
        serde_json::from_slice(&entry(&found, "export.json").content).unwrap();
    assert_eq!((manifest.channel_name, manifest.recordings), (None, 0));
    assert_eq!(found.len(), 4);
    for other in [&ben, &owner, &third] {
        download(&server, other, export.id)
            .await
            .error(ErrorCode::NotFound);
    }

    // Ben is removed from the server: Ann no longer reaches the conversation, and its
    // export is not downloaded.
    server
        .post(
            &owner,
            &format!("/api/v1/accounts/{}/removal", ben.account_id),
            &Value::Null,
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    download(&server, &ann, export.id)
        .await
        .error(ErrorCode::NotFound);
    server
        .post(&ann, &exports_path, &Value::Null)
        .await
        .error(ErrorCode::NotFound);
    drop((attachments, exports));
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

fn sequential_scans(plan: &Value, found: &mut Vec<String>) {
    match plan {
        Value::Object(node) => {
            if node.get("Node Type").and_then(Value::as_str) == Some("Seq Scan") {
                let table = node.get("Relation Name").and_then(Value::as_str);
                found.push(table.unwrap_or("?").to_owned());
            }
            node.values()
                .for_each(|child| sequential_scans(child, found));
        }
        Value::Array(nodes) => nodes
            .iter()
            .for_each(|child| sequential_scans(child, found)),
        _ => {}
    }
}

#[tokio::test]
async fn rule_e17_the_statements_of_exports_read_through_indexes() {
    let desk = Desk::open().await;
    let server = &desk.server;
    send(server, &desk.owner, &desk.channel, "one").await;
    sqlx::query(
        "with made as (
             insert into channels (id, workspace_id, kind, name, position, created_at)
             select gen_random_uuid(), $1, 'text', 'filler', n, $2
             from generate_series(1, 200) as n
             returning id),
         places as (
             insert into message_places (id, channel_id) select id, id from made
             returning id),
         people as (
             insert into accounts (id, name, status, created_at)
             select gen_random_uuid(), 'filler', 'active', $2 from generate_series(1, 10)
             returning id)
         insert into exports (id, account_id, place_id, state, requested_at)
         select gen_random_uuid(), people.id, places.id, 'waiting', $2
         from places cross join people",
    )
    .bind(desk.workspace_id)
    .bind(server.clock.now())
    .execute(&mut server.test_data().await)
    .await
    .unwrap();
    sqlx::query("analyze exports")
        .execute(&server.db)
        .await
        .unwrap();
    let mut connection = server.db.acquire().await.unwrap();
    sqlx::query("set enable_seqscan = off")
        .execute(&mut *connection)
        .await
        .unwrap();
    let now = server.clock.now();
    let nil = uuid::Uuid::nil();
    let place = desk.channel.id.as_uuid();
    let explain = |statement: &str| format!("explain (format json) {statement}");
    use junto_server::exports::build::{
        ATTACHMENT_PAGE_QUERY, BUSY_QUERY, MESSAGE_PAGE_QUERY, NEXT_QUERY, RECORDING_PAGE_QUERY,
    };
    use junto_server::exports::{EXPIRED, OF_ITEM, REMOVALS};

    let mut plans: Vec<(&str, &str, Value)> = Vec::new();
    let plan: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(explain(MESSAGE_PAGE_QUERY)))
        .bind(place)
        .bind(desk.owner.account_id)
        .bind(-1_i64)
        .bind(nil)
        .bind(nil)
        .bind(200_i64)
        .fetch_one(&mut *connection)
        .await
        .unwrap();
    plans.push(("message page", "messages_history", plan));
    for (name, statement, index) in [
        (
            "attachment page",
            ATTACHMENT_PAGE_QUERY,
            "attachments_live_of_place",
        ),
        (
            "recording page",
            RECORDING_PAGE_QUERY,
            "recordings_of_channel",
        ),
    ] {
        let plan: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(explain(statement)))
            .bind(place)
            .bind(nil)
            .bind(nil)
            .bind(50_i64)
            .fetch_one(&mut *connection)
            .await
            .unwrap();
        plans.push((name, index, plan));
    }
    for (name, statement, index) in [
        ("next export", NEXT_QUERY, "exports_open"),
        ("busy", BUSY_QUERY, "exports_open"),
    ] {
        let plan: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(explain(statement)))
            .bind(now)
            .fetch_one(&mut *connection)
            .await
            .unwrap();
        plans.push((name, index, plan));
    }
    for (name, statement, index) in [
        ("expired", EXPIRED, "exports_expiry"),
        ("removals", REMOVALS, "exports_removal"),
    ] {
        let plan: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(explain(statement)))
            .bind(now)
            .bind(500_i64)
            .fetch_one(&mut *connection)
            .await
            .unwrap();
        plans.push((name, index, plan));
    }
    let plan: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(explain(OF_ITEM)))
        .bind(place)
        .bind(nil)
        .bind(now)
        .fetch_one(&mut *connection)
        .await
        .unwrap();
    plans.push(("exports of an item", "exports_of_place", plan));
    for (statement, index) in [
        (
            "select id from exports where account_id = $1 and removed_at is null order by id desc",
            "exports_of_account",
        ),
        (
            "select count(*) from exports where account_id = $1 and removed_at is null",
            "exports_of_account",
        ),
    ] {
        let plan: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(explain(statement)))
            .bind(desk.owner.account_id)
            .fetch_one(&mut *connection)
            .await
            .unwrap();
        plans.push(("exports of an account", index, plan));
    }
    for (name, index, plan) in plans {
        assert!(uses_index(&plan, index), "{name}: {plan}");
        let mut scans = Vec::new();
        sequential_scans(&plan, &mut scans);
        assert!(scans.is_empty(), "{name}: {scans:?} {plan}");
    }
}

#[tokio::test]
async fn rule_e18_names_in_the_file_are_safe() {
    let desk = Desk::open().await;
    let server = &desk.server;
    let path = format!("/api/v1/channels/{}/messages", desk.channel.id);
    let mut uploads = Vec::new();
    for name in ["..", ".", "a:b*c?.txt", ".hidden", "trailing. ", "ok.txt"] {
        let upload = upload_named(
            server,
            &desk.owner,
            json!({ "channel_id": desk.channel.id }),
            name,
            name.as_bytes(),
        )
        .await;
        uploads.push((name, upload.id));
    }
    let ids: Vec<_> = uploads.iter().map(|(_, id)| *id).collect();
    send_with(server, &desk.owner, &path, "files", &ids).await;
    let export = requested(server, &desk.owner, desk.channel.id).await;
    build(server).await;
    let found = entries(server, &desk.owner, export.id).await;
    let expected = ["file", "file", "a_b_c_.txt", "hidden", "trailing", "ok.txt"];
    for ((name, id), safe) in uploads.iter().zip(expected) {
        let stored = entry(&found, &format!("attachments/{id}/{safe}"));
        assert_eq!(stored.content, name.as_bytes(), "{name}");
    }
    for stored in &found {
        assert!(zip::is_entry_name(&stored.name), "{}", stored.name);
        assert!(!stored.name.contains(".."), "{}", stored.name);
    }
    let page = page_of(&found);
    for ((_, id), safe) in uploads.iter().zip(expected) {
        assert!(
            page.contains(&format!("href=\"attachments/{id}/{safe}\"")),
            "{safe}"
        );
    }
}

#[tokio::test]
async fn rule_e19_exports_end_with_their_account_and_their_channel() {
    let desk = Desk::open().await;
    let server = &desk.server;
    let other = server
        .channel(&desk.owner, desk.workspace_id, "other")
        .await;
    let of_exporter = requested(server, &desk.exporter, desk.channel.id).await;
    let of_owner = requested(server, &desk.owner, desk.channel.id).await;
    let elsewhere = requested(server, &desk.owner, other.id).await;
    build(server).await;
    let waiting = requested(server, &desk.exporter, other.id).await;

    // A ban ends the exports of the account, and the others stay.
    server
        .put_empty(
            &desk.owner,
            &format!("/api/v1/accounts/{}/ban", desk.exporter.account_id),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let removed: Vec<ExportId> =
        sqlx::query_scalar("select id from exports where removed_at is not null order by id")
            .fetch_all(&server.db)
            .await
            .unwrap();
    assert_eq!(removed, [of_exporter.id, waiting.id]);
    assert_eq!(
        rows(
            server,
            "select count(*) from storage_reservations where kind = 'export'"
        )
        .await,
        0
    );
    build(server).await;
    assert!(!desk.exports.0.join(of_exporter.id.to_string()).exists());
    assert_eq!(rows(server, "select count(*) from exports").await, 2);

    // The removal of an account ends its exports in the same way.
    server
        .grant_workspace(
            &desk.owner,
            &desk.plain,
            desk.workspace_id,
            &[WorkspacePermission::ExportChannel],
        )
        .await;
    let of_plain = requested(server, &desk.plain, other.id).await;
    build(server).await;
    assert_eq!(desk.files_of(of_plain.id), ["1.zip"]);
    server
        .post(
            &desk.owner,
            &format!("/api/v1/accounts/{}/removal", desk.plain.account_id),
            &json!({}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let removed: Vec<ExportId> =
        sqlx::query_scalar("select id from exports where removed_at is not null order by id")
            .fetch_all(&server.db)
            .await
            .unwrap();
    assert_eq!(removed, [of_plain.id]);
    build(server).await;
    assert!(!desk.exports.0.join(of_plain.id.to_string()).exists());
    assert_eq!(rows(server, "select count(*) from exports").await, 2);

    // The deletion of a channel removes its exports with its content, and no other.
    let mut stream = server.subscribed(&desk.owner, Level::Background).await;
    server
        .delete(
            &desk.owner,
            &format!("/api/v1/channels/{}", desk.channel.id),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    junto_server::purge::run_once(&server.state).await.unwrap();
    export_reply(server, &desk.owner, of_owner.id)
        .await
        .error(ErrorCode::NotFound);
    assert_eq!(
        export_of(server, &desk.owner, elsewhere.id).await.state,
        ExportState::Ready
    );
    let told = events_of(server, &desk.owner, &mut stream).await;
    assert!(
        told.contains(&Event::ExportRemoved {
            export_id: of_owner.id,
            reason: ExportRemoval::PlaceDeleted,
        }),
        "{told:?}"
    );
    build(server).await;
    assert!(!desk.exports.0.join(of_owner.id.to_string()).exists());
    assert_eq!(desk.files_of(elsewhere.id), ["1.zip"]);
}

#[tokio::test]
async fn rule_e20_a_server_without_the_directory_refuses() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let channel = server.channel(&owner, workspace.id, "plans").await;
    request(&server, &owner, channel.id)
        .await
        .error(ErrorCode::Unavailable);
    assert_eq!(rows(&server, "select count(*) from exports").await, 0);
    server
        .get(&owner, "/api/v1/exports")
        .await
        .ok::<Vec<Export>>(StatusCode::OK);
    build(&server).await;
}

#[tokio::test]
async fn rule_e21_a_file_with_a_recording_of_a_running_hidden_call_is_for_the_people_inside() {
    let (attachments, exports) = (Scratch::new(), Scratch::new());
    let server = TestServer::start_for_calls_with(with_directories(&attachments, &exports)).await;
    use_fake_disks(&server);
    let scene = crate::calls_support::Scene::on(server).await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let voice = &scene.voice;
    let export_and_record = [
        WorkspacePermission::ExportChannel,
        WorkspacePermission::RecordCalls,
    ];
    server
        .grant_workspace(owner, ann, scene.workspace_id, &export_and_record)
        .await;
    server
        .call_settings(owner, voice.id, json!({ "hides_occupants": true }))
        .await;
    let ann_seat = server.enter_call(ann, voice.id).await;
    let ben_seat = server.enter_call(ben, voice.id).await;
    server.speak(&ann_seat);
    server.speak(&ben_seat);
    let started = server.record(ann, voice.id).await;
    server.tend().await;
    server
        .stop_recording(ann, voice.id)
        .await
        .empty(StatusCode::NO_CONTENT);
    for _ in 0..3 {
        server.tend().await;
    }
    assert_eq!(server.process().await, 1);

    // The call still runs. Ann is inside, so her export holds the recording.
    let export = requested(server, ann, voice.id).await;
    build(server).await;
    let recording_entry = format!("recordings/{}/recording.json", started.id);
    let found = entries(server, ann, export.id).await;
    assert!(found.iter().any(|entry| entry.name == recording_entry));

    // A second export is counted while Ann is inside and written after she left.
    let second = requested(server, ann, voice.id).await;
    let mut job = Job::claim(&server.state).await.unwrap().unwrap();
    assert_eq!(job.export_id(), second.id);
    let reservations = "select count(*) from storage_reservations where kind = 'export'";
    while rows(server, reservations).await == 0 {
        assert_eq!(job.step(&server.state).await.unwrap(), Progress::More);
    }

    // Ann leaves and the call goes on: she reads neither the recording nor the file that
    // holds it. The export itself stays.
    server.leave_call(ann).await;
    while job.step(&server.state).await.unwrap() == Progress::More {}
    // The file that was written after she left holds no recording, so she downloads it.
    let found = entries(server, ann, second.id).await;
    assert!(
        found
            .iter()
            .all(|entry| !entry.name.starts_with("recordings/"))
    );
    server
        .recording_reply(ann, started.id)
        .await
        .error(ErrorCode::NotFound);
    download(server, ann, export.id)
        .await
        .error(ErrorCode::NotFound);
    assert_eq!(
        export_of(server, ann, export.id).await.state,
        ExportState::Ready
    );

    // The call ends: she reads the recording and the file again.
    server.leave_call(ben).await;
    server.check().await;
    assert_eq!(server.count("calls").await, 0);
    server
        .recording_reply(ann, started.id)
        .await
        .ok::<junto_protocol::recording::RecordingDetail>(StatusCode::OK);
    let found = entries(server, ann, export.id).await;
    assert!(found.iter().any(|entry| entry.name == recording_entry));
    drop((attachments, exports));
}

/// Reads a download to its end. The bytes that arrived, and whether the response ended
/// as a whole.
async fn drained(mut response: reqwest::Response, mut received: u64) -> (u64, bool) {
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => received += chunk.len() as u64,
            Ok(None) => return (received, true),
            Err(_) => return (received, false),
        }
    }
}

#[tokio::test]
async fn rule_e22_a_running_download_ends_when_the_access_ends() {
    let desk = Desk::open().await;
    let server = &desk.server;
    send(server, &desk.owner, &desk.channel, "content").await;
    let export = requested(server, &desk.exporter, desk.channel.id).await;
    build(server).await;
    // A file that is larger than what the connection holds unsent, so that the server
    // still sends when the test changes the access.
    let length: u64 = 256 * 1024 * 1024;
    let file = desk.exports.0.join(export.id.to_string()).join("1.zip");
    std::fs::OpenOptions::new()
        .write(true)
        .open(&file)
        .unwrap()
        .set_len(length)
        .unwrap();
    let interval = server.state.config.limits.recordings.download_check;
    let client = reqwest::Client::new();
    let begin = || async {
        let mut response = client
            .get(format!("{}/api/v1/exports/{}/file", server.base, export.id))
            .bearer_auth(&desk.exporter.token)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let first = response.chunk().await.unwrap().unwrap();
        (response, first.len() as u64)
    };

    // While the access stays, the download is complete, also after the time of a check.
    let (response, first) = begin().await;
    server.clock.advance(interval + Duration::seconds(1));
    assert_eq!(drained(response, first).await, (length, true));

    // The access ends while the file is sent: the rest of the file does not arrive.
    let (response, first) = begin().await;
    desk.permit(false).await;
    server.clock.advance(interval + Duration::seconds(1));
    let (received, whole) = drained(response, first).await;
    assert!(received < length && !whole, "{received} {whole}");

    // The same when the export is removed while its file is sent.
    desk.permit(true).await;
    let (response, first) = begin().await;
    server
        .delete(&desk.exporter, &format!("/api/v1/exports/{}", export.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    server.clock.advance(interval + Duration::seconds(1));
    let (received, whole) = drained(response, first).await;
    assert!(received < length && !whole, "{received} {whole}");
}

/// ADR 0608: the interval of the download check is also measured on a clock that a change
/// of the time of the machine does not move. While the clock of the server stands still, a
/// running download is decided again after the interval: it ends when the access ended.
#[tokio::test]
async fn rule_e22_a_download_is_decided_again_while_the_clock_of_the_server_stands_still() {
    let (attachments, exports) = (Scratch::new(), Scratch::new());
    let interval = Duration::milliseconds(200);
    let directories = with_directories(&attachments, &exports);
    let server = TestServer::start_with(|config| {
        directories(config);
        config.limits.recordings.download_check = interval;
    })
    .await;
    let desk = Desk::on(server, attachments, exports).await;
    let server = &desk.server;
    send(server, &desk.owner, &desk.channel, "content").await;
    let export = requested(server, &desk.exporter, desk.channel.id).await;
    build(server).await;
    let length: u64 = 256 * 1024 * 1024;
    let file = desk.exports.0.join(export.id.to_string()).join("1.zip");
    std::fs::OpenOptions::new()
        .write(true)
        .open(&file)
        .unwrap()
        .set_len(length)
        .unwrap();
    let client = reqwest::Client::new();
    let begin = || async {
        let mut response = client
            .get(format!("{}/api/v1/exports/{}/file", server.base, export.id))
            .bearer_auth(&desk.exporter.token)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let first = response.chunk().await.unwrap().unwrap();
        (response, first.len() as u64)
    };
    let pass = || tokio::time::sleep(interval.unsigned_abs());
    let before = server.clock.now();

    let (response, first) = begin().await;
    pass().await;
    assert_eq!(drained(response, first).await, (length, true));

    let (response, first) = begin().await;
    desk.permit(false).await;
    pass().await;
    let (received, whole) = drained(response, first).await;
    assert!(received < length && !whole, "{received} {whole}");
    assert_eq!(server.clock.now(), before);
}

#[tokio::test]
async fn rule_e23_content_that_grew_after_the_count_is_reserved_before_it_is_written() {
    let desk = Desk::open().await;
    let server = &desk.server;
    let reserve = server.state.storage_reserve_bytes();
    let message = send(server, &desk.owner, &desk.channel, "short").await;
    let reserved = "select coalesce(sum(bytes), 0)::bigint from storage_reservations
                    where kind = 'export'";
    let owner = &desk.owner;
    let path = format!("/api/v1/messages/{}", message.id);
    let grow = async |words: String| {
        server
            .put(owner, &path, &text(&words))
            .await
            .ok::<Message>(StatusCode::OK);
    };
    let counted = |export: Export| async move {
        let mut job = Job::claim(&server.state).await.unwrap().unwrap();
        assert_eq!(job.export_id(), export.id);
        while rows(server, reserved).await == 0 {
            assert_eq!(job.step(&server.state).await.unwrap(), Progress::More);
        }
        job
    };

    // The message grows between the count and the writing: the reservation is raised
    // before the larger page is written, and the file holds the larger text.
    let export = requested(server, &desk.exporter, desk.channel.id).await;
    let mut job = counted(export.clone()).await;
    let first = rows(server, reserved).await;
    grow("longer ".repeat(200)).await;
    let mut most = first;
    while job.step(&server.state).await.unwrap() == Progress::More {
        most = most.max(rows(server, reserved).await);
    }
    assert!(most > first, "{most} {first}");
    let ready = export_of(server, &desk.exporter, export.id).await;
    assert_eq!(ready.state, ExportState::Ready);
    assert!(i64::try_from(ready.bytes.unwrap()).unwrap() <= most);
    let found = entries(server, &desk.exporter, export.id).await;
    assert!(page_of(&found).contains("longer longer"));

    // Without the space for what was added, the export fails and leaves no file.
    let export = requested(server, &desk.exporter, desk.channel.id).await;
    let mut job = counted(export.clone()).await;
    grow("longest ".repeat(400)).await;
    desk.disk.set_free(reserve + 100);
    while job.step(&server.state).await.unwrap() == Progress::More {}
    let failed = export_of(server, &desk.exporter, export.id).await;
    assert_eq!(
        (failed.state, failed.failure),
        (ExportState::Failed, Some(ExportFailure::StorageLow))
    );
    assert!(desk.files_of(export.id).is_empty());
    assert_eq!(rows(server, reserved).await, 0);
}

/// The readable page of the messages is written twice: into its own file with each page,
/// and into the archive after the last page. The space of the second copy is part of what
/// a raised reservation needs.
#[tokio::test]
async fn rule_e24_a_raised_reservation_needs_the_space_of_what_is_written_later() {
    let desk = Desk::open().await;
    let server = &desk.server;
    let reserve = server.state.storage_reserve_bytes();
    let page = i64::from(server.state.config.limits.exports.page);
    let first = send(server, &desk.owner, &desk.channel, &"wide ".repeat(200)).await;
    let last = send(server, &desk.owner, &desk.channel, "short").await;
    // One full page of messages before the last message, which is on a later page. The
    // positions after the two messages stay free for the lines that the requests write.
    sqlx::query("update messages set position = position + 100000 where id = $1")
        .bind(last.id)
        .execute(&server.db)
        .await
        .unwrap();
    sqlx::query(
        "insert into messages (id, place_id, position, author_id, created_at, document)
         select ('00000000-0000-7000-8000-' || lpad(to_hex(n), 12, '0'))::uuid, place_id,
                position + 1000 + n, author_id, created_at, document
         from messages cross join generate_series(1, $2) as n where id = $1",
    )
    .bind(first.id)
    .bind(page)
    .execute(&mut server.test_data().await)
    .await
    .unwrap();

    let reserved = "select coalesce(sum(bytes), 0)::bigint from storage_reservations
                    where kind = 'export'";
    let export = requested(server, &desk.exporter, desk.channel.id).await;
    let mut job = Job::claim(&server.state).await.unwrap().unwrap();
    while rows(server, reserved).await == 0 {
        assert_eq!(job.step(&server.state).await.unwrap(), Progress::More);
    }
    server
        .put(
            &desk.owner,
            &format!("/api/v1/messages/{}", last.id),
            &text(&"longer ".repeat(200)),
        )
        .await
        .ok::<Message>(StatusCode::OK);
    // The disk has the space for the margin of the count and for the page that grew, and
    // not for the second copy of the first page of messages, which is 200 000 bytes and
    // more.
    desk.disk
        .set_free(reserve + junto_server::exports::build::MARGIN + 100_000);
    while job.step(&server.state).await.unwrap() == Progress::More {}
    let failed = export_of(server, &desk.exporter, export.id).await;
    assert_eq!(
        (failed.state, failed.failure),
        (ExportState::Failed, Some(ExportFailure::StorageLow))
    );
    assert!(desk.files_of(export.id).is_empty());

    // With the space for the second copy the same export is written.
    desk.disk.set_free(FREE_DISK);
    let export = requested(server, &desk.exporter, desk.channel.id).await;
    let mut job = Job::claim(&server.state).await.unwrap().unwrap();
    while rows(server, reserved).await == 0 {
        assert_eq!(job.step(&server.state).await.unwrap(), Progress::More);
    }
    server
        .put(
            &desk.owner,
            &format!("/api/v1/messages/{}", last.id),
            &text(&"longest ".repeat(400)),
        )
        .await
        .ok::<Message>(StatusCode::OK);
    desk.disk
        .set_free(reserve + junto_server::exports::build::MARGIN + 2_000_000);
    while job.step(&server.state).await.unwrap() == Progress::More {}
    let ready = export_of(server, &desk.exporter, export.id).await;
    assert_eq!(ready.state, ExportState::Ready);
    let found = entries(server, &desk.exporter, export.id).await;
    assert!(messages_of(&found).len() >= usize::try_from(page).unwrap() + 2);
    assert!(page_of(&found).contains("longest longest"));
}
