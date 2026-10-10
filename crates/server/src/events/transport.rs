//! The WebSocket upgrade of the event stream and the socket below the WebSocket frames.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::task::{Context, Poll, ready};
use std::time::Duration;

use axum::extract::Request;
use axum::http::header::{
    CONNECTION, SEC_WEBSOCKET_ACCEPT, SEC_WEBSOCKET_KEY, SEC_WEBSOCKET_VERSION, UPGRADE,
};
use axum::http::{HeaderMap, HeaderName, StatusCode, Version};
use axum::response::{IntoResponse, Response};
use hyper::upgrade::{OnUpgrade, Upgraded};
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::time::Instant;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::{Role, WebSocketConfig};

use crate::config::StreamLimits;

const WRITE_BUFFER_BYTES: usize = 64 * 1024;
const LARGEST_WRITE_BUFFER_BYTES: usize = 1024 * 1024;
const LONGEST_HEADER: usize = 14;
const CONTINUATION: u8 = 0;

pub type Socket = WebSocketStream<Guarded<TokioIo<Upgraded>>>;

/// A WebSocket upgrade request that the server can accept.
pub struct Upgrade {
    on_upgrade: OnUpgrade,
    accept: String,
}

fn names(headers: &HeaderMap, name: HeaderName, token: &str) -> bool {
    headers
        .get_all(name)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|part| part.trim().eq_ignore_ascii_case(token))
}

impl Upgrade {
    /// `None` when the request is not a WebSocket upgrade request of version 13.
    pub fn of(request: &mut Request) -> Option<Self> {
        let headers = request.headers();
        if request.version() != Version::HTTP_11
            || !names(headers, CONNECTION, "upgrade")
            || !names(headers, UPGRADE, "websocket")
            || !names(headers, SEC_WEBSOCKET_VERSION, "13")
        {
            return None;
        }
        let accept = derive_accept_key(headers.get(SEC_WEBSOCKET_KEY)?.as_bytes());
        let on_upgrade = request.extensions_mut().remove::<OnUpgrade>()?;
        Some(Self { on_upgrade, accept })
    }

    pub fn response(&self) -> Response {
        (
            StatusCode::SWITCHING_PROTOCOLS,
            [
                (CONNECTION, "upgrade"),
                (UPGRADE, "websocket"),
                (SEC_WEBSOCKET_ACCEPT, self.accept.as_str()),
            ],
        )
            .into_response()
    }

    /// Waits until the HTTP connection has become the socket of the WebSocket.
    pub async fn socket(self, limits: &StreamLimits) -> Result<Socket, hyper::Error> {
        let upgraded = self.on_upgrade.await?;
        Ok(server_socket(TokioIo::new(upgraded), limits).await)
    }
}

/// The server side of a WebSocket on `stream`, with the limits of ADR 0116.
pub async fn server_socket<S>(stream: S, limits: &StreamLimits) -> WebSocketStream<Guarded<S>>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let config = WebSocketConfig::default()
        .max_message_size(Some(limits.client_message_bytes))
        .max_frame_size(Some(limits.client_message_bytes))
        .write_buffer_size(WRITE_BUFFER_BYTES)
        .max_write_buffer_size(LARGEST_WRITE_BUFFER_BYTES);
    let guarded = Guarded::new(stream, limits);
    WebSocketStream::from_raw_socket(guarded, Role::Server, Some(config)).await
}

/// Why the guard of a socket refused the frames of the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Violation {
    /// A frame was a part of a fragmented message.
    Fragmented,
    /// More frames arrived in the window than the limit.
    TooManyFrames,
}

/// What the guard of a socket found, for the connection that reads the socket.
#[derive(Default)]
pub struct Inbound(AtomicU8);

impl Inbound {
    pub fn violation(&self) -> Option<Violation> {
        match self.0.load(Ordering::SeqCst) {
            1 => Some(Violation::Fragmented),
            2 => Some(Violation::TooManyFrames),
            _ => None,
        }
    }

    fn record(&self, violation: Violation) {
        let stored = match violation {
            Violation::Fragmented => 1,
            Violation::TooManyFrames => 2,
        };
        self.0.store(stored, Ordering::SeqCst);
    }
}

/// Follows the frame headers in the bytes of a client, without their payloads.
#[derive(Debug, Default)]
struct Frames {
    header: [u8; LONGEST_HEADER],
    held: usize,
    payload_left: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FrameStart {
    fin: bool,
    opcode: u8,
}

impl Frames {
    /// Takes the next bytes of the stream and reports the frames that start in them.
    fn follow(&mut self, mut bytes: &[u8], mut started: impl FnMut(FrameStart)) {
        while !bytes.is_empty() {
            if self.payload_left > 0 {
                let skipped = usize::try_from(self.payload_left)
                    .unwrap_or(usize::MAX)
                    .min(bytes.len());
                self.payload_left -= skipped as u64;
                bytes = &bytes[skipped..];
                continue;
            }
            let needed = match self.held {
                0 | 1 => 2,
                _ => header_length(self.header[1]),
            };
            let taken = (needed - self.held).min(bytes.len());
            self.header[self.held..self.held + taken].copy_from_slice(&bytes[..taken]);
            self.held += taken;
            bytes = &bytes[taken..];
            if self.held >= 2 && self.held == header_length(self.header[1]) {
                started(FrameStart {
                    fin: self.header[0] & 0x80 != 0,
                    opcode: self.header[0] & 0x0f,
                });
                self.payload_left = payload_length(&self.header);
                self.held = 0;
            }
        }
    }
}

fn header_length(second: u8) -> usize {
    let extended = match second & 0x7f {
        126 => 2,
        127 => 8,
        _ => 0,
    };
    let mask = if second & 0x80 != 0 { 4 } else { 0 };
    2 + extended + mask
}

fn payload_length(header: &[u8; LONGEST_HEADER]) -> u64 {
    match header[1] & 0x7f {
        126 => u64::from(u16::from_be_bytes([header[2], header[3]])),
        127 => {
            let mut length = [0; 8];
            length.copy_from_slice(&header[2..10]);
            u64::from_be_bytes(length)
        }
        short => u64::from(short),
    }
}

/// A socket that reads the frame headers of the client before the WebSocket layer joins the
/// frames to messages. It refuses a fragmented message and more frames per window than the
/// limit, so the work that a client can cause is bounded per frame (ADR 0116).
pub struct Guarded<S> {
    inner: S,
    inbound: Arc<Inbound>,
    frames: Frames,
    most_frames: u32,
    window: Duration,
    window_start: Instant,
    in_window: u32,
}

impl<S> Guarded<S> {
    pub fn new(inner: S, limits: &StreamLimits) -> Self {
        Self {
            inner,
            inbound: Arc::new(Inbound::default()),
            frames: Frames::default(),
            most_frames: limits.client_frames.attempts,
            window: limits.client_frames.window.unsigned_abs(),
            window_start: Instant::now(),
            in_window: 0,
        }
    }

    pub fn inbound(&self) -> Arc<Inbound> {
        self.inbound.clone()
    }

    fn check(&mut self, bytes: &[u8]) -> Option<Violation> {
        let mut starts = Vec::new();
        self.frames.follow(bytes, |start| starts.push(start));
        for start in starts {
            if !start.fin || start.opcode == CONTINUATION {
                return Some(Violation::Fragmented);
            }
            if self.window_start.elapsed() > self.window {
                self.window_start = Instant::now();
                self.in_window = 0;
            }
            self.in_window += 1;
            if self.in_window > self.most_frames {
                return Some(Violation::TooManyFrames);
            }
        }
        None
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Guarded<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.inbound.violation().is_some() {
            return Poll::Ready(Err(refused()));
        }
        let before = buffer.filled().len();
        ready!(Pin::new(&mut self.inner).poll_read(context, buffer))?;
        let arrived = buffer.filled()[before..].to_vec();
        match self.check(&arrived) {
            None => Poll::Ready(Ok(())),
            Some(violation) => {
                self.inbound.record(violation);
                Poll::Ready(Err(refused()))
            }
        }
    }
}

fn refused() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "the frames of the client are outside the limits",
    )
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Guarded<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(context, buffer)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(context)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(context)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(fin: bool, opcode: u8, payload: usize) -> Vec<u8> {
        let mut bytes = vec![(if fin { 0x80 } else { 0 }) | opcode];
        match payload {
            0..=125 => bytes.push(0x80 | payload as u8),
            126..=65_535 => {
                bytes.push(0x80 | 126);
                bytes.extend((payload as u16).to_be_bytes());
            }
            _ => {
                bytes.push(0x80 | 127);
                bytes.extend((payload as u64).to_be_bytes());
            }
        }
        bytes.extend([1, 2, 3, 4]);
        bytes.extend(std::iter::repeat_n(0x55, payload));
        bytes
    }

    fn starts(chunks: &[&[u8]]) -> Vec<FrameStart> {
        let mut frames = Frames::default();
        let mut found = Vec::new();
        for chunk in chunks {
            frames.follow(chunk, |start| found.push(start));
        }
        found
    }

    #[test]
    fn frame_headers_are_found_in_any_split_of_the_bytes() {
        let mut stream = Vec::new();
        let expected = [
            (true, 1, 0),
            (true, 1, 125),
            (false, 1, 126),
            (false, 0, 70_000),
            (true, 0, 3),
            (true, 9, 0),
            (true, 10, 5),
            (true, 8, 2),
        ];
        for (fin, opcode, payload) in expected {
            stream.extend(frame(fin, opcode, payload));
        }
        let expected: Vec<FrameStart> = expected
            .iter()
            .map(|(fin, opcode, _)| FrameStart {
                fin: *fin,
                opcode: *opcode,
            })
            .collect();

        assert_eq!(starts(&[&stream]), expected);
        for size in [1, 2, 3, 5, 7, 13, 14, 15, 1024] {
            let chunks: Vec<&[u8]> = stream.chunks(size).collect();
            assert_eq!(starts(&chunks), expected, "chunks of {size}");
        }
    }

    #[test]
    fn a_frame_without_a_mask_has_a_shorter_header() {
        let unmasked = [0x81, 0x02, b'h', b'i', 0x89, 0x00];
        assert_eq!(
            starts(&[&unmasked]),
            [
                FrameStart {
                    fin: true,
                    opcode: 1
                },
                FrameStart {
                    fin: true,
                    opcode: 9
                }
            ]
        );
    }

    fn guard(most_frames: u32, window: time::Duration) -> Guarded<()> {
        let mut limits = StreamLimits::default();
        limits.client_frames.attempts = most_frames;
        limits.client_frames.window = window;
        Guarded::new((), &limits)
    }

    #[tokio::test]
    async fn a_part_of_a_fragmented_message_is_refused() {
        let guard = || guard(30, time::Duration::minutes(1));
        assert_eq!(guard().check(&frame(true, 1, 10)), None);
        assert_eq!(
            guard().check(&frame(false, 1, 10)),
            Some(Violation::Fragmented)
        );
        assert_eq!(
            guard().check(&frame(true, 0, 0)),
            Some(Violation::Fragmented)
        );
        assert_eq!(
            guard().check(&frame(false, 0, 0)),
            Some(Violation::Fragmented)
        );
    }

    #[tokio::test]
    async fn frames_are_counted_per_window_whatever_their_kind() {
        let mut limited = guard(3, time::Duration::minutes(1));
        assert_eq!(limited.check(&frame(true, 9, 0)), None);
        assert_eq!(limited.check(&frame(true, 10, 0)), None);
        assert_eq!(limited.check(&frame(true, 1, 4)), None);
        assert_eq!(
            limited.check(&frame(true, 9, 0)),
            Some(Violation::TooManyFrames)
        );

        let mut together = guard(3, time::Duration::minutes(1));
        let mut four = Vec::new();
        for _ in 0..4 {
            four.extend(frame(true, 9, 0));
        }
        assert_eq!(together.check(&four), Some(Violation::TooManyFrames));

        let mut new_window_each_time = guard(1, time::Duration::ZERO);
        for _ in 0..5 {
            tokio::time::sleep(Duration::from_millis(2)).await;
            assert_eq!(new_window_each_time.check(&frame(true, 9, 0)), None);
        }
    }
}
