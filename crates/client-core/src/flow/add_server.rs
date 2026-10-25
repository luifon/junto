//! Adding a server by its address: the client asks the address for the public server
//! information and shows the name and the logo before any account exists (ADR 0101).

use std::future::Future;
use std::sync::Arc;

use junto_protocol::error::ErrorCode;
use junto_protocol::ids::ServerId;
use url::Url;

use super::machine::{Machine, Outcome};
use super::{FlowError, FlowView};
use crate::address::ServerAddress;
use crate::api::{ApiClient, server_info};
use crate::book::{ConnectionMode, ServerEntry};
use crate::manager::Inner;
use crate::version::{ApiVersion, Incompatibility};

/// What the server at an address says about itself, cut to what the client shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerPreview {
    pub address: ServerAddress,
    pub server_id: ServerId,
    pub name: String,
    pub logo_url: Option<Url>,
    /// The API version that the client would use, or why the server cannot be used now.
    /// A server that cannot be used can still be saved.
    pub version: Result<ApiVersion, Incompatibility>,
    entry: ServerEntry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddServerState {
    EnterAddress,
    Preview(Box<ServerPreview>),
    /// This device holds a key of the server identifier that the address reports, and the
    /// key was created for another address. The client does not use that key here: an
    /// address that copies the identifier of another server would get its signatures.
    /// The server can be added only after the user discards the stored key.
    KeyOfOtherAddress {
        preview: Box<ServerPreview>,
        key_address: ServerAddress,
    },
    Saved {
        server_id: ServerId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddServerAction {
    /// Asks the address for the server information.
    Fetch { address: String },
    /// Saves the previewed server. The connection mode is chosen here (ADR 0107).
    Save { connection_mode: ConnectionMode },
    /// Deletes the key that was created for the other address. The device loses the
    /// account that the key belongs to, and can then add the server as a new device.
    DiscardKey,
    /// Returns to the entry of an address.
    Reset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AddServerActionKind {
    Fetch,
    Save,
    DiscardKey,
    Reset,
}

impl AddServerAction {
    pub fn kind(&self) -> AddServerActionKind {
        match self {
            Self::Fetch { .. } => AddServerActionKind::Fetch,
            Self::Save { .. } => AddServerActionKind::Save,
            Self::DiscardKey => AddServerActionKind::DiscardKey,
            Self::Reset => AddServerActionKind::Reset,
        }
    }
}

pub fn actions(state: &AddServerState) -> &'static [AddServerActionKind] {
    use AddServerActionKind as K;
    match state {
        AddServerState::EnterAddress => &[K::Fetch],
        AddServerState::Preview(_) => &[K::Save, K::Fetch, K::Reset],
        AddServerState::KeyOfOtherAddress { .. } => &[K::DiscardKey, K::Reset],
        AddServerState::Saved { .. } => &[],
    }
}

#[derive(Clone)]
pub struct AddServerFlow {
    client: Arc<Inner>,
    machine: Arc<Machine<AddServerState, AddServerActionKind>>,
}

impl AddServerFlow {
    pub(crate) fn new(client: Arc<Inner>) -> Self {
        Self {
            client,
            machine: Arc::new(Machine::new(AddServerState::EnterAddress, actions)),
        }
    }

    pub fn view(&self) -> FlowView<AddServerState, AddServerActionKind> {
        self.machine.view()
    }

    pub fn dispatch(
        &self,
        action: AddServerAction,
    ) -> impl Future<Output = Result<(), FlowError>> + Send + 'static + use<> {
        let flow = self.clone();
        let work = self.client.core.runtime.run(async move {
            let (before, run) = flow.machine.begin(action.kind())?;
            flow.client.core.notify();
            let outcome = flow.perform(before, action).await;
            let result = run.finish(outcome);
            flow.client.core.notify();
            result
        });
        async move { work.await? }
    }

    /// The address that a stored key of this server was created for, when it is not
    /// `address`.
    fn key_of_other_address(
        &self,
        server_id: ServerId,
        address: &ServerAddress,
    ) -> Result<Option<ServerAddress>, FlowError> {
        let stored = self.client.core.keys.origin(server_id)?;
        Ok(stored.filter(|origin| origin != address))
    }

    async fn perform(
        &self,
        before: AddServerState,
        action: AddServerAction,
    ) -> Outcome<AddServerState> {
        let core = &self.client.core;
        match (action, before) {
            (AddServerAction::Reset, _) => Outcome::moved(AddServerState::EnterAddress),
            (AddServerAction::Fetch { address }, _) => match self.fetch(&address).await {
                Ok(preview) => {
                    match self.key_of_other_address(preview.server_id, &preview.address) {
                        Ok(None) => Outcome::moved(AddServerState::Preview(preview)),
                        Ok(Some(key_address)) => Outcome::failed_and_moved(
                            AddServerState::KeyOfOtherAddress {
                                preview,
                                key_address: key_address.clone(),
                            },
                            FlowError::KeyOfOtherAddress {
                                stored: key_address,
                            },
                        ),
                        Err(error) => Outcome::failed(error),
                    }
                }
                Err(error) => Outcome::failed(error),
            },
            (
                AddServerAction::DiscardKey,
                AddServerState::KeyOfOtherAddress {
                    preview,
                    key_address,
                },
            ) => {
                if let Err(error) = self.withdraw_with_stored_key(&preview, &key_address).await {
                    return Outcome::failed(error);
                }
                let discarded = core.book.lock().map_err(FlowError::from).and_then(|_lock| {
                    let saved = core.book.entry(preview.server_id)?;
                    if let Some(saved) = saved {
                        return Err(FlowError::AlreadySaved {
                            server_id: saved.server_id,
                        });
                    }
                    Ok(core.keys.remove(preview.server_id)?)
                });
                match discarded {
                    Ok(()) => Outcome::moved(AddServerState::Preview(preview)),
                    Err(error) => Outcome::failed(error),
                }
            }
            (AddServerAction::Save { connection_mode }, AddServerState::Preview(preview)) => {
                let mut entry = preview.entry.clone();
                entry.connection_mode = connection_mode;
                let checked = core.book.add_after(entry, || {
                    match self.key_of_other_address(preview.server_id, &preview.address)? {
                        None => Ok(()),
                        Some(stored) => Err(FlowError::KeyOfOtherAddress { stored }),
                    }
                });
                match checked {
                    Ok(entry) => Outcome::moved(AddServerState::Saved {
                        server_id: self.client.adopt(entry),
                    }),
                    Err(FlowError::KeyOfOtherAddress { stored }) => Outcome::failed_and_moved(
                        AddServerState::KeyOfOtherAddress {
                            preview,
                            key_address: stored.clone(),
                        },
                        FlowError::KeyOfOtherAddress { stored },
                    ),
                    Err(error) => Outcome::failed(error),
                }
            }
            (AddServerAction::Save { .. } | AddServerAction::DiscardKey, _) => {
                Outcome::failed(FlowError::NotAvailable)
            }
        }
    }

    /// Withdraws a join request that the stored key of the previewed server can still have
    /// waiting, before the key is discarded: a removal that the server did not confirm keeps
    /// the key of a waiting request (ADR 0712). The device signs at the previewed address; the
    /// signature names that origin and the withdrawal, so it withdraws at most a request and
    /// a server whose public origins do not include that address refuses it (ADR 0711). A key
    /// that belongs to an account is discarded as before.
    async fn withdraw_with_stored_key(
        &self,
        preview: &ServerPreview,
        key_address: &ServerAddress,
    ) -> Result<(), FlowError> {
        let core = &self.client.core;
        let Some(key) = core.keys.load(preview.server_id, key_address)? else {
            return Ok(());
        };
        let version = preview.version.map_err(FlowError::Incompatible)?;
        let api = ApiClient::new(
            core.transport.clone(),
            preview.address.clone(),
            preview.server_id,
            version,
            core.policy,
        );
        match api.withdraw_join_request(&key).await {
            Ok(()) => Ok(()),
            Err(error) if error.code() == Some(ErrorCode::KeyAlreadyRegistered) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    async fn fetch(&self, address: &str) -> Result<Box<ServerPreview>, FlowError> {
        let core = &self.client.core;
        let address = ServerAddress::parse(address)
            .map_err(|error| FlowError::InvalidInput(error.to_string()))?;
        let info = server_info(core.transport.as_ref(), &address, &core.policy).await?;
        for saved in core.book.entries()? {
            if saved.server_id == info.server_id || saved.address == address {
                return Err(FlowError::AlreadySaved {
                    server_id: saved.server_id,
                });
            }
        }
        let entry = ServerEntry::new(address.clone(), &info, ConnectionMode::default());
        Ok(Box::new(ServerPreview {
            server_id: entry.server_id,
            name: entry.name.clone(),
            logo_url: entry
                .logo_path
                .as_deref()
                .and_then(|path| address.logo_url(path)),
            version: core.versions.negotiate(&entry.api_versions),
            address,
            entry,
        }))
    }
}
