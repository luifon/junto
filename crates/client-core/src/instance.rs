//! One desktop process per data directory and activation requests from later starts.

use std::fs::File;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex, PoisonError, mpsc as stopping};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::paths::DataDir;
use crate::storage::StorageError;

const ENDPOINT: &str = "desktop-instance.json";
const MAX_REQUEST: usize = 16 * 1024;
const WAIT: Duration = Duration::from_secs(1);

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error("the running Junto window could not be reached: {0}")]
    Io(#[from] io::Error),
}

#[derive(Serialize, Deserialize)]
struct Endpoint {
    port: u16,
    token: [u8; 32],
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Activation {
    pub link: Option<String>,
    pub activation_token: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Request {
    token: [u8; 32],
    activation: Activation,
}

pub enum Start {
    Primary(Instance, mpsc::Receiver<Activation>),
    Forwarded,
}

#[derive(Default)]
struct Accepted {
    socket: Option<TcpStream>,
    stopping: bool,
}

pub struct Instance {
    _lock: File,
    #[cfg(test)]
    address: SocketAddr,
    stopping: stopping::Sender<()>,
    worker: Option<JoinHandle<()>>,
    accepted: Arc<Mutex<Accepted>>,
}

impl Instance {
    /// Called before opening a client. A later start forwards its URL or an activation.
    pub fn start(directory: &DataDir, activation: Activation) -> Result<Start, Error> {
        if activation
            .link
            .as_ref()
            .is_some_and(|link| link.len() > 4096)
            || activation
                .activation_token
                .as_ref()
                .is_some_and(|token| token.len() > 4096)
        {
            return Err(io::Error::other("the application link is too long").into());
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(lock) = directory.files().try_lock_file("desktop-instance.lock")? {
                return Self::primary(directory, lock);
            }
            match forward(directory, &activation, deadline) {
                Ok(()) => return Ok(Start::Forwarded),
                Err(Error::Io(_)) if Instant::now() < deadline => {
                    thread::sleep(
                        Duration::from_millis(20)
                            .min(deadline.saturating_duration_since(Instant::now())),
                    );
                }
                Err(error) => return Err(error),
            }
        }
    }

    fn primary(directory: &DataDir, lock: File) -> Result<Start, Error> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let mut token = [0; 32];
        getrandom::fill(&mut token).map_err(|_| io::Error::other("no random bytes"))?;
        let endpoint = Endpoint {
            port: address.port(),
            token,
        };
        {
            let _writing = directory.lock()?;
            directory.files().write(
                ENDPOINT,
                &serde_json::to_vec(&endpoint).map_err(io::Error::other)?,
            )?;
        }
        let (send, receive) = mpsc::channel(32);
        let (stopping, cancelled) = stopping::channel();
        let accepted = Arc::new(Mutex::new(Accepted::default()));
        let active = accepted.clone();
        let worker = thread::Builder::new()
            .name("desktop-instance".into())
            .spawn(move || {
                loop {
                    match cancelled.try_recv() {
                        Ok(()) | Err(stopping::TryRecvError::Disconnected) => break,
                        Err(stopping::TryRecvError::Empty) => {}
                    }
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            {
                                let mut active =
                                    active.lock().unwrap_or_else(PoisonError::into_inner);
                                if active.stopping {
                                    break;
                                }
                                let Ok(socket) = stream.try_clone() else {
                                    continue;
                                };
                                active.socket = Some(socket);
                            }
                            let _ = receive_request(&mut stream, &token, &send);
                            active.lock().unwrap_or_else(PoisonError::into_inner).socket = None;
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            if cancelled.recv_timeout(Duration::from_millis(100))
                                != Err(stopping::RecvTimeoutError::Timeout)
                            {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
            })?;
        Ok(Start::Primary(
            Self {
                _lock: lock,
                #[cfg(test)]
                address,
                stopping,
                worker: Some(worker),
                accepted,
            },
            receive,
        ))
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        {
            let mut accepted = self.accepted.lock().unwrap_or_else(PoisonError::into_inner);
            accepted.stopping = true;
            if let Some(socket) = &accepted.socket {
                let _ = socket.shutdown(Shutdown::Both);
            }
        }
        let _ = self.stopping.send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn receive_request(
    stream: &mut TcpStream,
    token: &[u8; 32],
    send: &mpsc::Sender<Activation>,
) -> io::Result<()> {
    let deadline = Instant::now() + WAIT;
    let mut length = [0; 4];
    read_until(stream, &mut length, deadline)?;
    let length = u32::from_be_bytes(length) as usize;
    if length > MAX_REQUEST {
        return Err(io::Error::other("the activation is too large"));
    }
    let mut bytes = vec![0; length];
    read_until(stream, &mut bytes, deadline)?;
    let request: Request = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    if &request.token != token {
        return Err(io::Error::other("the activation is not authenticated"));
    }
    send.try_send(request.activation)
        .map_err(io::Error::other)?;
    write_until(stream, b"ok", deadline)
}

fn remaining(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|left| !left.is_zero())
        .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "the activation deadline expired"))
}

fn read_until(stream: &mut TcpStream, mut bytes: &mut [u8], deadline: Instant) -> io::Result<()> {
    while !bytes.is_empty() {
        stream.set_read_timeout(Some(remaining(deadline)?))?;
        match stream.read(bytes) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(count) => bytes = &mut bytes[count..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn write_until(stream: &mut TcpStream, mut bytes: &[u8], deadline: Instant) -> io::Result<()> {
    while !bytes.is_empty() {
        stream.set_write_timeout(Some(remaining(deadline)?))?;
        match stream.write(bytes) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(count) => bytes = &bytes[count..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn forward(directory: &DataDir, activation: &Activation, deadline: Instant) -> Result<(), Error> {
    let mut buffer = [0; 1024];
    let count = directory
        .files()
        .read_private(ENDPOINT, &mut buffer)?
        .ok_or_else(|| io::Error::other("the running application is still starting"))?;
    let endpoint: Endpoint = serde_json::from_slice(&buffer[..count]).map_err(io::Error::other)?;
    let deadline = deadline.min(Instant::now() + WAIT);
    let mut stream = TcpStream::connect_timeout(
        &SocketAddr::from((Ipv4Addr::LOCALHOST, endpoint.port)),
        remaining(deadline)?,
    )?;
    let bytes = serde_json::to_vec(&Request {
        token: endpoint.token,
        activation: activation.clone(),
    })
    .map_err(io::Error::other)?;
    write_until(&mut stream, &(bytes.len() as u32).to_be_bytes(), deadline)?;
    write_until(&mut stream, &bytes, deadline)?;
    let mut answer = [0; 2];
    read_until(&mut stream, &mut answer, deadline)?;
    if &answer != b"ok" {
        return Err(io::Error::other("the activation was refused").into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
