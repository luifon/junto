//! Direct-call rings and their outcomes during this client session.

use junto_client_core::protocol::call::{CallInvitation, CallPlace, InvitationOutcome};
use junto_client_core::protocol::ids::{AccountId, CallInvitationId, ServerId};
use std::collections::HashMap;
use time::OffsetDateTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RingOutcome {
    Waiting,
    Accepted,
    Declined,
    Missed,
    Cancelled,
    Ended,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ring {
    pub invitation: CallInvitation,
    pub incoming: bool,
    pub other: AccountId,
    pub outcome: RingOutcome,
}

#[derive(Default)]
pub struct DirectCalls {
    pub rings: HashMap<(ServerId, CallInvitationId), Ring>,
    early: HashMap<(ServerId, CallInvitationId), InvitationOutcome>,
}
impl DirectCalls {
    pub fn ended(&mut self, server: ServerId, call: junto_client_core::protocol::ids::CallId) {
        for ((held, _), ring) in &mut self.rings {
            if *held == server
                && ring.invitation.call_id == call
                && ring.outcome == RingOutcome::Accepted
            {
                ring.outcome = RingOutcome::Ended;
            }
        }
    }
    pub fn see(
        &mut self,
        server: ServerId,
        own: AccountId,
        invitation: &CallInvitation,
        now: OffsetDateTime,
    ) {
        if !matches!(invitation.place, CallPlace::Direct { .. })
            || invitation.place != CallPlace::direct(invitation.account_id, invitation.invited_by)
            || invitation.account_id == invitation.invited_by
            || (own != invitation.account_id && own != invitation.invited_by)
        {
            return;
        }
        let key = (server, invitation.id);
        if self.rings.contains_key(&key) {
            return;
        }
        if self.rings.len() >= 1024
            && let Some(oldest) = self
                .rings
                .iter()
                .min_by_key(|(_, ring)| ring.invitation.created_at)
                .map(|(key, _)| *key)
        {
            self.rings.remove(&oldest);
        }
        let incoming = own == invitation.account_id;
        let outcome = self.early.remove(&key).map(RingOutcome::from).unwrap_or(
            if now >= invitation.expires_at {
                RingOutcome::Missed
            } else {
                RingOutcome::Waiting
            },
        );
        self.rings.insert(
            key,
            Ring {
                invitation: invitation.clone(),
                incoming,
                other: if incoming {
                    invitation.invited_by
                } else {
                    invitation.account_id
                },
                outcome,
            },
        );
    }
    pub fn resolve(&mut self, server: ServerId, id: CallInvitationId, outcome: InvitationOutcome) {
        let key = (server, id);
        if let Some(ring) = self.rings.get_mut(&key) {
            // An authoritative answer also corrects a local expiry estimate.
            if matches!(ring.outcome, RingOutcome::Waiting | RingOutcome::Missed) {
                ring.outcome = outcome.into();
            }
        } else {
            if self.early.len() >= 1024
                && let Some(key) = self.early.keys().next().copied()
            {
                self.early.remove(&key);
            }
            self.early.entry(key).or_insert(outcome);
        }
    }
    pub fn expire(&mut self, now: OffsetDateTime) {
        for ring in self.rings.values_mut() {
            if ring.outcome == RingOutcome::Waiting && now >= ring.invitation.expires_at {
                ring.outcome = RingOutcome::Missed;
            }
        }
    }
    pub fn ringing(&self) -> bool {
        self.rings
            .values()
            .any(|ring| ring.incoming && ring.outcome == RingOutcome::Waiting)
    }
    pub fn forget_server(&mut self, server: ServerId) {
        self.rings.retain(|(id, _), _| *id != server);
        self.early.retain(|(id, _), _| *id != server);
    }
}

impl From<InvitationOutcome> for RingOutcome {
    fn from(value: InvitationOutcome) -> Self {
        match value {
            InvitationOutcome::Accepted => Self::Accepted,
            InvitationOutcome::Declined => Self::Declined,
            InvitationOutcome::Expired => Self::Missed,
            InvitationOutcome::Cancelled => Self::Cancelled,
        }
    }
}

#[cfg(test)]
mod tests;

impl super::AppState {
    pub fn start_direct_call(
        &mut self,
        server: ServerId,
        other: AccountId,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(own) = self.client.server(server).and_then(|s| s.account()) else {
            return;
        };
        let joining = self.calls.join(junto_media::calls::CallTarget {
            server_id: server,
            place: CallPlace::direct(own, other),
        });
        cx.spawn(async move |_, _| {
            let _ = joining.await;
        })
        .detach();
    }
    pub fn direct_calls(&self) -> &DirectCalls {
        &self.direct
    }
    pub fn direct_connected(&self, server: ServerId, other: AccountId) -> bool {
        self.snapshots()
            .iter()
            .find(|snapshot| snapshot.id == server)
            .and_then(|snapshot| snapshot.model.as_ref())
            .is_some_and(|model| {
                model
                    .call_at(&CallPlace::direct(model.account.id, other))
                    .is_some_and(|call| call.participant(other).is_some())
            })
    }
    #[cfg(feature = "test-support")]
    pub fn show_direct_fixture(
        &mut self,
        server: ServerId,
        own: AccountId,
        invitation: CallInvitation,
        outcome: RingOutcome,
        cx: &mut gpui::Context<Self>,
    ) {
        self.direct
            .see(server, own, &invitation, invitation.created_at);
        if let Some(ring) = self.direct.rings.get_mut(&(server, invitation.id)) {
            ring.outcome = outcome;
        }
        cx.notify();
    }
    pub(super) fn reconcile_direct(&mut self) {
        #[cfg(feature = "test-support")]
        if self.fixture.is_some() {
            return;
        }
        let now = OffsetDateTime::now_utc();
        let servers = self.client.servers();
        self.direct.rings.retain(|(id, _), ring| {
            servers.iter().any(|server| {
                server.id() == *id
                    && server.account().is_some_and(|own| {
                        own == if ring.incoming {
                            ring.invitation.account_id
                        } else {
                            ring.invitation.invited_by
                        }
                    })
            })
        });
        for server in &servers {
            if let Some(model) = server.live().view().model
                && let Some(own) = server.account()
            {
                for invitation in &model.invitations {
                    self.direct.see(server.id(), own, invitation, now);
                }
            }
        }
        self.direct.expire(now);
        let ringing = !self.calls.view().deafened
            && self.direct.rings.iter().any(|((id, _), ring)| {
                ring.incoming
                    && ring.outcome == RingOutcome::Waiting
                    && self
                        .client
                        .server(*id)
                        .and_then(|s| s.live().view().model)
                        .is_some_and(|m| !m.is_quiet(now))
            });
        let device = self
            .known
            .media
            .voice
            .output_device
            .clone()
            .map(junto_media::audio::DeviceId);
        if let Err(error) = self.ringer.set_ringing(ringing, device.as_ref()) {
            tracing::debug!(%error, "incoming call sound could not start");
        }
    }
}
