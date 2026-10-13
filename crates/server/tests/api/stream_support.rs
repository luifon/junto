//! A client of the event stream for the tests. It keeps every text frame that it receives.

use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use junto_protocol::WorkspacePermission;
use junto_protocol::error::{ErrorBody, ErrorCode};
use junto_protocol::events::{
    ClientMessage, CloseCode, Cursor, Event, Level, STREAM_PATH, ServerMessage,
};
use junto_protocol::ids::{ChannelId, StreamEpoch, WorkspaceId, WorkspaceRoleId};
use junto_protocol::roles::WorkspaceRole;
use junto_protocol::snapshot::Snapshot;
use junto_protocol::structure::{Category, Channel};
use reqwest::StatusCode;
use serde_json::json;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpSocket, TcpStream};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{self, Message};

use crate::support::{Member, TestServer};

const PATIENCE: Duration = Duration::from_secs(20);

/// The connection of a stream to the server. It can discard what the client writes: the
/// client then reads on, and the server hears nothing more from it.
struct Wire {
    tcp: TcpStream,
    discards: bool,
}

impl Wire {
    fn of(tcp: TcpStream) -> Self {
        Self {
            tcp,
            discards: false,
        }
    }
}

impl AsyncRead for Wire {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.tcp).poll_read(context, buffer)
    }
}

impl AsyncWrite for Wire {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if self.discards {
            return Poll::Ready(Ok(bytes.len()));
        }
        Pin::new(&mut self.tcp).poll_write(context, bytes)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.tcp).poll_flush(context)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.tcp).poll_shutdown(context)
    }
}

pub struct Stream {
    socket: WebSocketStream<Wire>,
    pub epoch: StreamEpoch,
    /// Every text frame that the server sent, in order.
    pub raw: Vec<String>,
}

/// What a stream yields next.
#[derive(Debug, PartialEq)]
#[allow(
    clippy::large_enum_variant,
    reason = "Test frames mirror the inline protocol reader and are consumed one at a time."
)]
pub enum Frame {
    Message(ServerMessage),
    Closed(Option<u16>),
}

fn request(server: &TestServer, token: Option<&str>) -> tungstenite::handshake::client::Request {
    let url = format!("ws://{}{STREAM_PATH}", server.address);
    let mut request = url.into_client_request().unwrap();
    if let Some(token) = token {
        request
            .headers_mut()
            .insert("authorization", format!("Bearer {token}").parse().unwrap());
    }
    request
}

impl TestServer {
    /// Opens the event stream with `token`, or returns the error of the upgrade request.
    pub async fn open_stream(
        &self,
        token: Option<&str>,
    ) -> Result<Stream, (StatusCode, ErrorCode)> {
        let tcp = TcpStream::connect(self.address).await.unwrap();
        match tokio_tungstenite::client_async(request(self, token), Wire::of(tcp)).await {
            Ok((socket, _)) => Ok(Stream::greeted(socket).await),
            Err(tungstenite::Error::Http(response)) => {
                let body: ErrorBody =
                    serde_json::from_slice(response.body().as_deref().unwrap_or_default()).unwrap();
                Err((response.status(), body.error.code))
            }
            Err(error) => panic!("the stream did not open: {error}"),
        }
    }

    pub async fn stream(&self, member: &Member) -> Stream {
        self.open_stream(Some(&member.token))
            .await
            .unwrap_or_else(|error| panic!("the stream did not open: {error:?}"))
    }

    /// Opens the stream on a socket that holds at most about `receive_buffer` unread bytes.
    pub async fn narrow_stream(&self, member: &Member, receive_buffer: u32) -> Stream {
        let socket = TcpSocket::new_v4().unwrap();
        socket.set_recv_buffer_size(receive_buffer).unwrap();
        let tcp = socket.connect(self.address).await.unwrap();
        let (socket, _) =
            tokio_tungstenite::client_async(request(self, Some(&member.token)), Wire::of(tcp))
                .await
                .unwrap();
        Stream::greeted(socket).await
    }

    /// A stream of `member` that is subscribed at `level` after the newest event.
    pub async fn subscribed(&self, member: &Member, level: Level) -> Stream {
        let mut stream = self.stream(member).await;
        stream.subscribe(level, None).await;
        stream
    }

    pub async fn snapshot(&self, member: &Member) -> Snapshot {
        self.get(member, "/api/v1/snapshot")
            .await
            .ok(StatusCode::OK)
    }

    /// Renames the device of `member` to a new name, and returns the events that `stream`
    /// receives before the event of that change. The rename is delivered at every level, so
    /// the result is everything that the stream was sent up to this call.
    pub async fn traffic(&self, member: &Member, stream: &mut Stream) -> Vec<(u64, Event)> {
        let marker = format!("marker-{}", uuid::Uuid::new_v4().simple());
        self.patch(
            member,
            &format!("/api/v1/account/devices/{}", member.device_id),
            &json!({ "name": marker }),
        )
        .await
        .ok::<serde_json::Value>(StatusCode::OK);
        let mut events = Vec::new();
        loop {
            match stream.event().await {
                (_, Event::DeviceRenamed { name, .. }) if name == marker => return events,
                event => events.push(event),
            }
        }
    }
}

impl TestServer {
    pub async fn category(
        &self,
        member: &Member,
        workspace_id: WorkspaceId,
        name: &str,
        position: i32,
    ) -> Category {
        self.post(
            member,
            &format!("/api/v1/workspaces/{workspace_id}/categories"),
            &json!({ "name": name, "position": position }),
        )
        .await
        .ok(StatusCode::CREATED)
    }

    pub async fn channel(&self, member: &Member, workspace_id: WorkspaceId, name: &str) -> Channel {
        self.post(
            member,
            &format!("/api/v1/workspaces/{workspace_id}/channels"),
            &json!({ "name": name, "kind": "text" }),
        )
        .await
        .ok(StatusCode::CREATED)
    }

    pub async fn everyone_role(&self, member: &Member, workspace_id: WorkspaceId) -> WorkspaceRole {
        let roles: Vec<WorkspaceRole> = self
            .get(member, &format!("/api/v1/workspaces/{workspace_id}/roles"))
            .await
            .ok(StatusCode::OK);
        roles.into_iter().find(|role| role.is_everyone).unwrap()
    }

    /// Hides a channel from the members of its workspace who do not hold `role_id`.
    pub async fn make_private(
        &self,
        manager: &Member,
        workspace_id: WorkspaceId,
        channel_id: ChannelId,
        role_id: WorkspaceRoleId,
    ) {
        let everyone = self.everyone_role(manager, workspace_id).await;
        let view = [WorkspacePermission::ViewChannel];
        for (role, allow, deny) in [
            (role_id, &view[..], &[][..]),
            (everyone.id, &[][..], &view[..]),
        ] {
            self.put(
                manager,
                &format!("/api/v1/channels/{channel_id}/overrides/{role}"),
                &json!({ "allow": allow, "deny": deny }),
            )
            .await
            .ok::<serde_json::Value>(StatusCode::OK);
        }
    }

    /// Writes `count` events into the log in one transaction, each a copy of `payload` whose
    /// text at `name_path` is 8000 characters long and ends with the number of the event.
    pub async fn large_events(
        &self,
        audience: serde_json::Value,
        payload: serde_json::Value,
        name_path: &[&str],
        count: usize,
    ) {
        sqlx::query(
            "insert into events (id, recorded_at, class, kind, audience, payload)
             select gen_random_uuid(), now(), 'general', $2 ->> 'type', $1,
                    jsonb_set($2, $3, to_jsonb(lpad(change::text, 8000, '0')))
             from generate_series(0, $4 - 1) as change
             order by change",
        )
        .bind(sqlx::types::Json(audience))
        .bind(sqlx::types::Json(payload))
        .bind(name_path)
        .bind(i32::try_from(count).unwrap())
        .execute(&self.db)
        .await
        .unwrap();
    }

    /// The positions and the types of the events of the log, in the order of the positions.
    pub async fn log(&self) -> Vec<(i64, String)> {
        sqlx::query_as("select position, kind from events order by position")
            .fetch_all(&self.db)
            .await
            .unwrap()
    }
}

impl TestServer {
    /// Changes the limits of the event stream of this server from now on. A test shortens
    /// a time limit with this after its preparation, for the step that it examines.
    pub fn tighten_streams(&self, change: impl FnOnce(&mut junto_server::config::StreamLimits)) {
        let mut limits = (*self.state.streams.limits()).clone();
        change(&mut limits);
        self.state.streams.set_limits(limits);
    }
}

impl Stream {
    async fn greeted(socket: WebSocketStream<Wire>) -> Self {
        let mut stream = Self {
            socket,
            epoch: StreamEpoch::generate(),
            raw: Vec::new(),
        };
        match stream.next().await {
            Frame::Message(ServerMessage::Hello { epoch, .. }) => stream.epoch = epoch,
            other => panic!("the stream did not start with hello: {other:?}"),
        }
        stream
    }

    pub async fn send(&mut self, message: &ClientMessage) {
        self.send_text(&serde_json::to_string(message).unwrap())
            .await;
    }

    pub async fn send_text(&mut self, text: &str) {
        self.socket
            .send(Message::Text(text.to_owned().into()))
            .await
            .unwrap();
    }

    pub async fn send_frame(&mut self, frame: Message) -> Result<(), tungstenite::Error> {
        self.socket.send(frame).await
    }

    /// From now on the server hears nothing from this client: no answer to a ping and no
    /// answer to a close frame. The client still reads what the server sends.
    pub fn answer_nothing(&mut self) {
        self.socket.get_mut().discards = true;
    }

    /// The next message or the end of the connection. Pings and pongs are passed over.
    pub async fn next(&mut self) -> Frame {
        tokio::time::timeout(PATIENCE, self.read())
            .await
            .unwrap_or_else(|_| panic!("the stream sent nothing; so far: {:?}", self.raw))
    }

    async fn read(&mut self) -> Frame {
        loop {
            match self.socket.next().await {
                Some(Ok(Message::Text(text))) => {
                    self.raw.push(text.to_string());
                    let message = serde_json::from_str(&text)
                        .unwrap_or_else(|error| panic!("{error}: {text}"));
                    return Frame::Message(message);
                }
                Some(Ok(Message::Close(frame))) => {
                    return Frame::Closed(frame.map(|frame| u16::from(frame.code)));
                }
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                Some(Ok(other)) => panic!("unexpected frame: {other:?}"),
                Some(Err(_)) | None => return Frame::Closed(None),
            }
        }
    }

    /// The next message within `wait`, or `None` when the server sends none.
    pub async fn within(&mut self, wait: Duration) -> Option<Frame> {
        tokio::time::timeout(wait, self.read()).await.ok()
    }

    /// Sends a subscribe and returns the cursor that the server started from.
    pub async fn subscribe(&mut self, level: Level, cursor: Option<Cursor>) -> Cursor {
        self.send(&ClientMessage::Subscribe { level, cursor }).await;
        match self.next().await {
            Frame::Message(ServerMessage::Subscribed {
                level: started,
                cursor,
            }) => {
                assert_eq!(started, level);
                cursor
            }
            other => panic!("the subscription did not start: {other:?}"),
        }
    }

    /// The next event. Checkpoints are passed over.
    pub async fn event(&mut self) -> (u64, Event) {
        loop {
            match self.next().await {
                Frame::Message(ServerMessage::Event { id, event, .. }) => return (id, event),
                Frame::Message(ServerMessage::Checkpoint { .. }) => {}
                other => panic!("expected an event: {other:?}"),
            }
        }
    }

    /// The next `count` events. A test reads the events about a channel with this before
    /// it deletes the channel: an event that is delivered after the deletion reaches nobody.
    pub async fn events(&mut self, count: usize) -> Vec<(u64, Event)> {
        let mut events = Vec::new();
        for _ in 0..count {
            events.push(self.event().await);
        }
        events
    }

    /// Reads until the connection ends. Returns the messages before the end and the code of
    /// the close frame.
    pub async fn until_closed(&mut self) -> (Vec<ServerMessage>, Option<CloseCode>) {
        let mut messages = Vec::new();
        loop {
            match self.next().await {
                Frame::Message(message) => messages.push(message),
                Frame::Closed(code) => {
                    return (messages, code.and_then(CloseCode::from_code));
                }
            }
        }
    }

    pub fn cursor(&self, position: u64) -> Cursor {
        Cursor {
            epoch: self.epoch,
            position,
        }
    }
}
