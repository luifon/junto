use std::{
    io,
    sync::Mutex,
    time::{Duration, Instant},
};

use rustix::event::{PollFd, PollFlags, poll};
use x11rb::reexports::x11rb_protocol::{
    parse_display::{ConnectAddress, parse_display},
    xauth::get_auth,
};
use x11rb::{
    connection::Connection as _,
    protocol::screensaver::ConnectionExt as _,
    rust_connection::{DefaultStream, PollMode, RustConnection, Stream},
    utils::RawFdContainer,
};

const DEADLINE: Duration = Duration::from_secs(1);

pub(super) struct Reader {
    runtime: Option<tokio::runtime::Runtime>,
    bus: Option<zbus::Connection>,
    x11: Option<(RustConnection<TimedStream>, u32)>,
}

impl Reader {
    pub(super) fn new() -> Self {
        Self {
            runtime: tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .ok(),
            bus: None,
            x11: None,
        }
    }
    pub(super) fn read(&mut self) -> Option<bool> {
        if std::env::var_os("WAYLAND_DISPLAY").is_none() && std::env::var_os("DISPLAY").is_some() {
            if self.x11.is_none() {
                self.x11 = connect(self.runtime.as_ref()?);
            }
            if let Some((connection, root)) = &self.x11 {
                *connection
                    .stream()
                    .deadline
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Instant::now() + DEADLINE;
                if let Ok(cookie) = connection.screensaver_query_info(*root)
                    && let Ok(reply) = cookie.reply()
                {
                    return Some(
                        Duration::from_millis(u64::from(reply.ms_since_user_input))
                            >= super::IDLE_AFTER,
                    );
                }
            }
            self.x11 = None;
        }
        self.gnome()
    }
    fn gnome(&mut self) -> Option<bool> {
        let runtime = self.runtime.as_ref()?;
        let cached = self.bus.clone();
        let answer = runtime.block_on(async {
            tokio::time::timeout(DEADLINE, async move {
                let connection = match cached {
                    Some(connection) => connection,
                    None => zbus::Connection::session().await?,
                };
                let proxy = zbus::Proxy::new(
                    &connection,
                    "org.gnome.Mutter.IdleMonitor",
                    "/org/gnome/Mutter/IdleMonitor/Core",
                    "org.gnome.Mutter.IdleMonitor",
                )
                .await?;
                let milliseconds: u64 = proxy.call("GetIdletime", &()).await?;
                drop(proxy);
                Ok::<_, zbus::Error>((connection, milliseconds))
            })
            .await
        });
        match answer {
            Ok(Ok((connection, milliseconds))) => {
                self.bus = Some(connection);
                (milliseconds != u64::MAX)
                    .then(|| Duration::from_millis(milliseconds) >= super::IDLE_AFTER)
            }
            _ => {
                self.bus = None;
                None
            }
        }
    }
}

fn abstract_socket(path: &str) -> Option<std::os::unix::net::UnixStream> {
    use rustix::net::{
        AddressFamily, SocketAddrUnix, SocketFlags, SocketType, connect, socket_with,
        sockopt::socket_error,
    };
    let address = SocketAddrUnix::new_abstract_name(path.as_bytes()).ok()?;
    let socket = socket_with(
        AddressFamily::UNIX,
        SocketType::STREAM,
        SocketFlags::NONBLOCK | SocketFlags::CLOEXEC,
        None,
    )
    .ok()?;
    if let Err(error) = connect(&socket, &address) {
        if error != rustix::io::Errno::INPROGRESS {
            return None;
        }
        let mut descriptors = [PollFd::new(&socket, PollFlags::OUT)];
        let deadline = Instant::now() + DEADLINE;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return None;
            }
            let timeout = rustix::event::Timespec::try_from(remaining).ok()?;
            match poll(&mut descriptors, Some(&timeout)) {
                Err(rustix::io::Errno::INTR) => continue,
                Ok(1) => break,
                _ => return None,
            }
        }
        socket_error(&socket).ok()?.ok()?;
    }
    Some(socket.into())
}

fn connect(runtime: &tokio::runtime::Runtime) -> Option<(RustConnection<TimedStream>, u32)> {
    let display = parse_display(None).ok()?;
    for address in display.connect_instruction() {
        let connected = match address {
            ConnectAddress::Hostname(host, port) => {
                let stream = runtime
                    .block_on(async {
                        tokio::time::timeout(DEADLINE, tokio::net::TcpStream::connect((host, port)))
                            .await
                    })
                    .ok()
                    .and_then(Result::ok)
                    .and_then(|stream| stream.into_std().ok());
                stream.and_then(|stream| DefaultStream::from_tcp_stream(stream).ok())
            }
            ConnectAddress::Socket(path) => {
                let stream = abstract_socket(&path).or_else(|| {
                    runtime
                        .block_on(async {
                            tokio::time::timeout(DEADLINE, tokio::net::UnixStream::connect(&path))
                                .await
                        })
                        .ok()
                        .and_then(Result::ok)
                        .and_then(|stream| stream.into_std().ok())
                });
                stream.and_then(|stream| DefaultStream::from_unix_stream(stream).ok())
            }
            _ => None,
        };
        let Some((stream, (family, address))) = connected else {
            continue;
        };
        let (name, data) = get_auth(family, &address, display.display)
            .ok()
            .flatten()
            .unwrap_or_default();
        let stream = TimedStream {
            stream,
            deadline: Mutex::new(Instant::now() + DEADLINE),
        };
        if let Ok(connection) = RustConnection::connect_to_stream_with_auth_info(
            stream,
            usize::from(display.screen),
            name,
            data,
        ) {
            let root = connection
                .setup()
                .roots
                .get(usize::from(display.screen))?
                .root;
            return Some((connection, root));
        }
    }
    None
}

struct TimedStream {
    stream: DefaultStream,
    deadline: Mutex<Instant>,
}
impl Stream for TimedStream {
    fn poll(&self, mode: PollMode) -> io::Result<()> {
        let mut flags = PollFlags::empty();
        if mode.readable() {
            flags |= PollFlags::IN;
        }
        if mode.writable() {
            flags |= PollFlags::OUT;
        }
        loop {
            let remaining = self
                .deadline
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(io::ErrorKind::TimedOut.into());
            }
            let timeout = rustix::event::Timespec::try_from(remaining).map_err(io::Error::other)?;
            let mut descriptors = [PollFd::new(&self.stream, flags)];
            match poll(&mut descriptors, Some(&timeout)) {
                Ok(0) => return Err(io::ErrorKind::TimedOut.into()),
                Ok(_) => return Ok(()),
                Err(rustix::io::Errno::INTR) => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    fn read(&self, bytes: &mut [u8], descriptors: &mut Vec<RawFdContainer>) -> io::Result<usize> {
        self.stream.read(bytes, descriptors)
    }
    fn write(&self, bytes: &[u8], descriptors: &mut Vec<RawFdContainer>) -> io::Result<usize> {
        self.stream.write(bytes, descriptors)
    }
}
