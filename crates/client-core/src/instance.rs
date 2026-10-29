//! One desktop process per data directory and activation requests from later starts.

use std::fs::File;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc as stopping;
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

pub struct Instance {
    _lock: File,
    #[cfg(test)]
    address: SocketAddr,
    stopping: stopping::Sender<()>,
    worker: Option<JoinHandle<()>>,
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
        let Some(lock) = directory.files().try_lock_file("desktop-instance.lock")? else {
            forward(directory, activation)?;
            return Ok(Start::Forwarded);
        };
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
                            let _ = receive_request(&mut stream, &token, &send);
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
            },
            receive,
        ))
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
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
    stream.set_read_timeout(Some(WAIT))?;
    stream.set_write_timeout(Some(WAIT))?;
    let mut length = [0; 4];
    stream.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length) as usize;
    if length > MAX_REQUEST {
        return Err(io::Error::other("the activation is too large"));
    }
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes)?;
    let request: Request = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    if &request.token != token {
        return Err(io::Error::other("the activation is not authenticated"));
    }
    send.try_send(request.activation)
        .map_err(io::Error::other)?;
    stream.write_all(b"ok")
}

fn forward(directory: &DataDir, activation: Activation) -> Result<(), Error> {
    let deadline = Instant::now() + Duration::from_secs(3);
    let (mut stream, endpoint) = loop {
        let mut buffer = [0; 1024];
        let attempt = directory
            .files()
            .read_private(ENDPOINT, &mut buffer)?
            .ok_or_else(|| io::Error::other("the running application is still starting"))
            .and_then(|count| {
                serde_json::from_slice::<Endpoint>(&buffer[..count]).map_err(io::Error::other)
            })
            .and_then(|endpoint| {
                TcpStream::connect_timeout(
                    &SocketAddr::from((Ipv4Addr::LOCALHOST, endpoint.port)),
                    WAIT,
                )
                .map(|stream| (stream, endpoint))
            });
        match attempt {
            Ok(connected) => break connected,
            Err(error) if Instant::now() >= deadline => return Err(error.into()),
            Err(_) => thread::sleep(Duration::from_millis(20)),
        }
    };
    stream.set_read_timeout(Some(WAIT))?;
    stream.set_write_timeout(Some(WAIT))?;
    let bytes = serde_json::to_vec(&Request {
        token: endpoint.token,
        activation,
    })
    .map_err(io::Error::other)?;
    stream.write_all(&(bytes.len() as u32).to_be_bytes())?;
    stream.write_all(&bytes)?;
    let mut answer = [0; 2];
    stream.read_exact(&mut answer)?;
    if &answer != b"ok" {
        return Err(io::Error::other("the activation was refused").into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
