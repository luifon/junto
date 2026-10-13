//! A private connection and one short-lived ScreenCast session per share.

use std::collections::HashMap;
use std::os::fd::OwnedFd;
use std::time::Duration;

use futures_util::StreamExt;
use serde::Serialize;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Type, Value};
use zbus::{Connection, Proxy};

use crate::share::{CaptureError, ShareKind};

const SERVICE: &str = "org.freedesktop.portal.Desktop";
const OBJECT: &str = "/org/freedesktop/portal/desktop";
const SCREENCAST: &str = "org.freedesktop.portal.ScreenCast";
const SESSION: &str = "org.freedesktop.portal.Session";
const REQUEST: &str = "org.freedesktop.portal.Request";
const WAIT: Duration = Duration::from_secs(15);

type Results = HashMap<String, OwnedValue>;

pub(super) fn error(error: impl std::fmt::Display) -> CaptureError {
    CaptureError::Failed(error.to_string())
}

fn token() -> String {
    format!("junto{}", uuid::Uuid::new_v4().simple())
}

pub(super) struct Portal {
    connection: Connection,
    session: Option<OwnedObjectPath>,
    pending: std::sync::Mutex<Option<OwnedObjectPath>>,
    /// The longest wait for the answer to a request that shows no dialog.
    limit: Duration,
}

pub(super) struct Capabilities {
    pub kinds: Vec<ShareKind>,
    version: u32,
    cursors: u32,
}

pub(super) struct Video {
    pub fd: OwnedFd,
    pub node: u32,
    pub serial: Option<u64>,
}

impl Portal {
    pub async fn connect() -> Result<Self, CaptureError> {
        let connection = zbus::connection::Builder::session()
            .map_err(error)?
            .method_timeout(WAIT)
            .build()
            .await
            .map_err(error)?;
        Ok(Self {
            connection,
            session: None,
            pending: std::sync::Mutex::new(None),
            limit: WAIT,
        })
    }

    async fn proxy(&self) -> Result<Proxy<'static>, CaptureError> {
        Proxy::new(&self.connection, SERVICE, OBJECT, SCREENCAST)
            .await
            .map_err(error)
    }

    pub async fn capabilities(&self) -> Result<Capabilities, CaptureError> {
        let proxy = self.proxy().await?;
        let version: u32 = proxy.get_property("version").await.map_err(error)?;
        let types: u32 = proxy
            .get_property("AvailableSourceTypes")
            .await
            .map_err(error)?;
        let cursors = if version >= 2 {
            proxy
                .get_property("AvailableCursorModes")
                .await
                .map_err(error)?
        } else {
            1
        };
        let mut kinds = Vec::new();
        if types & 1 != 0 {
            kinds.push(ShareKind::Screen);
        }
        if types & 2 != 0 {
            kinds.push(ShareKind::Window);
        }
        if version >= 2 && cursors & 3 == 0 {
            kinds.clear();
        }
        Ok(Capabilities {
            kinds,
            version,
            cursors,
        })
    }

    fn path(&self, category: &str, token: &str) -> Result<OwnedObjectPath, CaptureError> {
        let unique = self
            .connection
            .unique_name()
            .ok_or_else(|| error("the portal connection has no bus name"))?;
        let sender = unique.as_str().trim_start_matches(':').replace('.', "_");
        format!("/org/freedesktop/portal/desktop/{category}/{sender}/{token}")
            .try_into()
            .map_err(error)
    }

    /// Makes a request and waits for its response. `limit` bounds the wait for a request
    /// that shows no dialog; the request that a person answers has none.
    async fn request<B: Serialize + Type>(
        &self,
        token: &str,
        method: &str,
        body: &B,
        limit: Option<Duration>,
    ) -> Result<Results, CaptureError> {
        let path = self.path("request", token)?;
        *self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(path.clone());
        let request = Proxy::new(&self.connection, SERVICE, path.clone(), REQUEST)
            .await
            .map_err(error)?;
        // Subscribe before calling: a fake or fast portal may respond before the method reply.
        let mut responses = request.receive_signal("Response").await.map_err(error)?;
        let returned: OwnedObjectPath = self
            .proxy()
            .await?
            .call(method, body)
            .await
            .map_err(error)?;
        if returned != path {
            return Err(error("the portal returned a different request handle"));
        }
        // Dropping this future closes the private connection.
        let response = match limit {
            Some(limit) => tokio::time::timeout(limit, responses.next())
                .await
                .map_err(|_| error(format!("the desktop portal did not answer {method}")))?,
            None => responses.next().await,
        }
        .ok_or_else(|| error("the portal request ended without a response"))?;
        let (code, results): (u32, Results) = response.body().deserialize().map_err(error)?;
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        match code {
            0 => Ok(results),
            1 => Err(CaptureError::Cancelled),
            _ => Err(error(format!(
                "the desktop portal did not complete {method} (response {code})"
            ))),
        }
    }

    pub async fn start(
        &mut self,
        kind: ShareKind,
    ) -> Result<(Video, zbus::proxy::SignalStream<'static>), CaptureError> {
        let capabilities = self.capabilities().await?;
        if !capabilities.kinds.contains(&kind) {
            return Err(CaptureError::Unsupported);
        }
        let handle = token();
        let session_token = token();
        let expected = self.path("session", &session_token)?;
        self.session = Some(expected.clone());
        let session = Proxy::new(&self.connection, SERVICE, expected.clone(), SESSION)
            .await
            .map_err(error)?;
        let mut closed = session.receive_signal("Closed").await.map_err(error)?;
        // A portal process that ends sends neither a response nor the end of the session.
        let service = self.proxy().await?;
        let mut owners = service.receive_owner_changed().await.map_err(error)?;
        let gone = || error("the desktop portal ended during the request");
        let options = HashMap::from([
            ("handle_token", Value::from(handle.as_str())),
            ("session_handle_token", Value::from(session_token.as_str())),
        ]);
        let create = (options,);
        let results = tokio::select! {
            result = self.request(&handle, "CreateSession", &create, Some(self.limit)) => result?,
            _ = closed.next() => return Err(CaptureError::SourceGone),
            _ = owners.next() => return Err(gone()),
        };
        let value = results
            .get("session_handle")
            .ok_or_else(|| error("the portal returned no session"))?;
        let returned = if let Ok(path) = <&str>::try_from(value) {
            path.to_owned()
        } else {
            OwnedObjectPath::try_from(value.try_clone().map_err(error)?)
                .map_err(error)?
                .to_string()
        };
        if returned != expected.as_str() {
            return Err(error("the portal returned a different session handle"));
        }
        let handle = token();
        let mut options = HashMap::from([
            ("handle_token", Value::from(handle.as_str())),
            (
                "types",
                Value::from(match kind {
                    ShareKind::Screen => 1_u32,
                    ShareKind::Window => 2_u32,
                }),
            ),
            ("multiple", Value::from(false)),
        ]);
        if capabilities.version >= 2 {
            options.insert(
                "cursor_mode",
                Value::from(if capabilities.cursors & 2 != 0 {
                    2_u32
                } else {
                    1_u32
                }),
            );
        }
        if capabilities.version >= 4 {
            options.insert("persist_mode", Value::from(0_u32));
        }
        let select = (expected.clone(), options);
        tokio::select! {
            result = self.request(&handle, "SelectSources", &select, Some(self.limit)) => { result?; },
            _ = closed.next() => return Err(CaptureError::SourceGone),
            _ = owners.next() => return Err(gone()),
        }
        let handle = token();
        let options = HashMap::from([("handle_token", Value::from(handle.as_str()))]);
        let start = (expected.clone(), "", options);
        // The desktop shows its dialog now, and a person answers it: no limit.
        let mut results = tokio::select! {
            result = self.request(&handle, "Start", &start, None) => result?,
            _ = closed.next() => return Err(CaptureError::SourceGone),
            _ = owners.next() => return Err(gone()),
        };
        let streams = results
            .remove("streams")
            .ok_or_else(|| error("the portal returned no video stream"))?;
        let streams: Vec<(u32, Results)> = streams.try_into().map_err(error)?;
        if streams.len() != 1 {
            return Err(error("the portal did not return exactly one video stream"));
        }
        let (node, properties) = streams
            .into_iter()
            .next()
            .ok_or_else(|| error("the portal returned no video stream"))?;
        let serial = properties
            .get("pipewire-serial")
            .map(u64::try_from)
            .transpose()
            .map_err(error)?;
        if serial.is_none() && (node == 0 || node == u32::MAX) {
            return Err(error("the portal returned an invalid video node"));
        }
        if let Some(actual) = properties.get("source_type") {
            let expected = match kind {
                ShareKind::Screen => 1,
                ShareKind::Window => 2,
            };
            if u32::try_from(actual).map_err(error)? != expected {
                return Err(error("the portal returned a different source kind"));
            }
        }
        let fd: zbus::zvariant::OwnedFd = self
            .proxy()
            .await?
            .call(
                "OpenPipeWireRemote",
                &(expected, HashMap::<&str, Value<'_>>::new()),
            )
            .await
            .map_err(error)?;
        Ok((
            Video {
                fd: fd.into(),
                node,
                serial,
            },
            closed,
        ))
    }
}

impl Drop for Portal {
    fn drop(&mut self) {
        let connection = self.connection.clone();
        let session = self.session.take();
        let pending = self
            .pending
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                for (path, interface) in [(pending, REQUEST), (session, SESSION)] {
                    if let Some(path) = path
                        && let Ok(proxy) = Proxy::new(&connection, SERVICE, path, interface).await
                    {
                        let _ =
                            tokio::time::timeout(WAIT, proxy.call::<_, _, ()>("Close", &())).await;
                    }
                }
                let _ = connection.close().await;
            });
        }
    }
}

#[cfg(test)]
mod tests;
