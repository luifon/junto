//! The approving side of a device link: a logged-in device looks up the code of a new
//! device, shows the public key behind it, and approves that key (ADR 0112).

use std::future::Future;
use std::sync::Arc;

use junto_protocol::account::Device;
use junto_protocol::error::ErrorCode;
use junto_protocol::ids::AccountId;
use junto_protocol::keys::PublicKey;
use junto_protocol::secret::LinkCode;
use time::OffsetDateTime;

use super::machine::{Machine, Outcome};
use super::{FlowError, FlowView};
use crate::api::{ApiError, ServerApi};
use crate::manager::ServerHandle;
use crate::profile::NAME_MAX_CHARS;

/// A link that the user saw and that belongs to one account of this device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingApproval {
    pub code: LinkCode,
    pub public_key: PublicKey,
    pub device_name: String,
    pub expires_at: OffsetDateTime,
    /// The account that the device had when the link was looked up. The approval is sent
    /// for this account and for no other.
    pub account_id: AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApproveLinkState {
    EnterCode,
    /// The interface shows the public key and the device name. The user compares the key
    /// with the one that the new device shows, and then approves.
    Confirm(PendingApproval),
    /// The approval was sent and no answer arrived, and the list of devices could not be
    /// read either. `Check` reads the list again.
    OutcomeUnknown(PendingApproval),
    Approved {
        device: Device,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApproveLinkAction {
    LookUp {
        code: LinkCode,
    },
    /// Approves the public key that the state shows. The server adds the device only when
    /// the key behind the code is still that key.
    Approve,
    /// Finds out whether an approval without an answer took effect.
    Check,
    Reset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ApproveLinkActionKind {
    LookUp,
    Approve,
    Check,
    Reset,
}

impl ApproveLinkAction {
    pub fn kind(&self) -> ApproveLinkActionKind {
        match self {
            Self::LookUp { .. } => ApproveLinkActionKind::LookUp,
            Self::Approve => ApproveLinkActionKind::Approve,
            Self::Check => ApproveLinkActionKind::Check,
            Self::Reset => ApproveLinkActionKind::Reset,
        }
    }
}

pub fn actions(state: &ApproveLinkState) -> &'static [ApproveLinkActionKind] {
    use ApproveLinkActionKind as K;
    match state {
        ApproveLinkState::EnterCode => &[K::LookUp],
        ApproveLinkState::Confirm(_) => &[K::Approve, K::Reset],
        ApproveLinkState::OutcomeUnknown(_) => &[K::Check, K::Reset],
        ApproveLinkState::Approved { .. } => &[],
    }
}

/// How one approval ended.
pub(crate) enum Approval {
    Approved(Box<Device>),
    /// The device was not added.
    Refused(ApiError),
    /// The approval may have taken effect, and the list of devices could not be read.
    Unknown(ApiError),
}

/// True when the server may have done the work although the client has no answer that it
/// can read: no answer at all, or an answer in another form, whatever its status.
fn answer_was_lost(error: &ApiError) -> bool {
    match error {
        ApiError::Unreachable(transport) => transport.request_may_have_arrived(),
        ApiError::OutcomeUnknown | ApiError::InvalidResponse { .. } => true,
        _ => false,
    }
}

/// The device of the account that has `public_key`.
pub(crate) async fn device_with_key(
    api: &ServerApi,
    public_key: PublicKey,
) -> Result<Option<Device>, ApiError> {
    let devices = api.devices().await?;
    Ok(devices
        .into_iter()
        .find(|device| device.public_key == public_key))
}

/// Approves a link and finds the truth when the answer does not tell it: after a lost
/// answer and after a code that the server no longer knows, the list of devices says
/// whether the key was added.
pub(crate) async fn approve(api: &ServerApi, code: LinkCode, public_key: PublicKey) -> Approval {
    let error = match api.approve_device_link(code, public_key).await {
        Ok(device) => return Approval::Approved(Box::new(device)),
        Err(error) => error,
    };
    let spent = error.code() == Some(ErrorCode::LinkCodeInvalid);
    if !spent && !answer_was_lost(&error) {
        return Approval::Refused(error);
    }
    match device_with_key(api, public_key).await {
        Ok(Some(device)) => Approval::Approved(Box::new(device)),
        Ok(None) => Approval::Refused(error),
        Err(_) if spent => Approval::Refused(error),
        Err(_) => Approval::Unknown(error),
    }
}

/// True when the code cannot be approved any more as it was shown, so the flow returns to
/// the entry of a code.
fn needs_a_new_code(error: &ApiError) -> bool {
    matches!(
        error.code(),
        Some(ErrorCode::LinkCodeInvalid | ErrorCode::PublicKeyMismatch)
    ) || matches!(error, ApiError::AccountChanged { .. })
}

#[derive(Clone)]
pub struct ApproveLinkFlow {
    server: ServerHandle,
    machine: Arc<Machine<ApproveLinkState, ApproveLinkActionKind>>,
}

impl ApproveLinkFlow {
    pub(crate) fn new(server: ServerHandle) -> Self {
        Self {
            server,
            machine: Arc::new(Machine::new(ApproveLinkState::EnterCode, actions)),
        }
    }

    pub fn view(&self) -> FlowView<ApproveLinkState, ApproveLinkActionKind> {
        self.machine.view()
    }

    pub fn dispatch(
        &self,
        action: ApproveLinkAction,
    ) -> impl Future<Output = Result<(), FlowError>> + Send + 'static + use<> {
        let flow = self.clone();
        let work = self.server.core().runtime.run(async move {
            let (before, run) = flow.machine.begin(action.kind())?;
            flow.server.core().notify();
            let outcome = flow.perform(before, action).await;
            let result = run.finish(outcome);
            flow.server.core().notify();
            result
        });
        async move { work.await? }
    }

    async fn perform(
        &self,
        before: ApproveLinkState,
        action: ApproveLinkAction,
    ) -> Outcome<ApproveLinkState> {
        match (action, before) {
            (ApproveLinkAction::Reset, _) => Outcome::moved(ApproveLinkState::EnterCode),
            (ApproveLinkAction::LookUp { code }, _) => {
                let Some(account_id) = self.server.account() else {
                    return Outcome::failed(FlowError::NotLoggedIn);
                };
                let api = self.server.api_for(account_id);
                match api.look_up_device_link(code.clone()).await {
                    Ok(pending) => Outcome::moved(ApproveLinkState::Confirm(PendingApproval {
                        code,
                        public_key: pending.public_key,
                        device_name: pending
                            .device_name
                            .chars()
                            .filter(|c| !c.is_control())
                            .take(NAME_MAX_CHARS)
                            .collect(),
                        expires_at: pending.expires_at,
                        account_id,
                    })),
                    Err(error) => Outcome::failed(error),
                }
            }
            (ApproveLinkAction::Approve, ApproveLinkState::Confirm(pending)) => {
                let api = self.server.api_for(pending.account_id);
                match approve(&api, pending.code.clone(), pending.public_key).await {
                    Approval::Approved(device) => {
                        Outcome::moved(ApproveLinkState::Approved { device: *device })
                    }
                    Approval::Refused(error) if needs_a_new_code(&error) => {
                        Outcome::failed_and_moved(ApproveLinkState::EnterCode, error)
                    }
                    Approval::Refused(error) => Outcome::failed(error),
                    Approval::Unknown(error) => {
                        Outcome::failed_and_moved(ApproveLinkState::OutcomeUnknown(pending), error)
                    }
                }
            }
            (ApproveLinkAction::Check, ApproveLinkState::OutcomeUnknown(pending)) => {
                let api = self.server.api_for(pending.account_id);
                match device_with_key(&api, pending.public_key).await {
                    Ok(Some(device)) => Outcome::moved(ApproveLinkState::Approved { device }),
                    Ok(None) => Outcome::moved(ApproveLinkState::Confirm(pending)),
                    Err(error) if needs_a_new_code(&error) => {
                        Outcome::failed_and_moved(ApproveLinkState::EnterCode, error)
                    }
                    Err(error) => Outcome::failed(error),
                }
            }
            (ApproveLinkAction::Approve | ApproveLinkAction::Check, _) => {
                Outcome::failed(FlowError::NotAvailable)
            }
        }
    }
}
