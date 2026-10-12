//! Direct-call rings and their outcomes during this client session.

use junto_client_core::protocol::call::{CallInvitation, CallPlace, InvitationOutcome};
use junto_client_core::protocol::ids::{AccountId, CallInvitationId, ServerId};
use std::{collections::HashMap, time::Instant};
mod lifetime;
use time::OffsetDateTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RingOutcome {
    Waiting,
    Accepted,
    Declined,
    Missed,
    Cancelled,
    Ended,
    EndedElsewhere,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ring {
    pub invitation: CallInvitation,
    pub incoming: bool,
    pub other: AccountId,
    pub outcome: RingOutcome,
    deadline: Instant,
}

#[derive(Default)]
pub struct DirectCalls {
    pub rings: HashMap<(ServerId, CallInvitationId), Ring>,
    active: Option<(junto_media::calls::CallTarget, lifetime::Lifetime)>,
    pub finished: HashMap<(ServerId, AccountId), RingOutcome>,
    early: HashMap<(ServerId, CallInvitationId), (InvitationOutcome, Instant)>,
    voice_invitations: HashMap<(ServerId, CallInvitationId), Instant>,
}
impl DirectCalls {
    pub(super) fn filter_view(&self, mut view: super::ShellView) -> super::ShellView {
        view.invitations.retain(|invited| {
            self.rings
                .get(&(invited.target.server_id, invited.id))
                .is_none_or(|ring| ring.outcome == RingOutcome::Waiting)
        });
        view
    }

    pub fn reconcile_server(&mut self, server: ServerId, invitations: &[CallInvitation]) {
        for ((held, id), ring) in &mut self.rings {
            if *held == server
                && ring.outcome == RingOutcome::Waiting
                && !invitations.iter().any(|invitation| invitation.id == *id)
            {
                ring.outcome = RingOutcome::EndedElsewhere;
            }
        }
    }

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
        now: Instant,
    ) {
        if matches!(invitation.place, CallPlace::Channel { .. }) {
            self.voice_invitations.insert((server, invitation.id), now);
            return;
        }
        if invitation.place != CallPlace::direct(invitation.account_id, invitation.invited_by)
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
        let outcome = self
            .early
            .remove(&key)
            .map(|(outcome, _)| RingOutcome::from(outcome))
            .unwrap_or(RingOutcome::Waiting);
        let duration = (invitation.expires_at - invitation.created_at)
            .unsigned_abs()
            .min(std::time::Duration::from_secs(60));
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
                deadline: now + duration,
            },
        );
    }
    pub fn resolve(&mut self, server: ServerId, id: CallInvitationId, outcome: InvitationOutcome) {
        let key = (server, id);
        if self.voice_invitations.remove(&key).is_some() {
            return;
        }
        if let Some(ring) = self.rings.get_mut(&key) {
            // An authoritative answer also corrects a local expiry estimate.
            if matches!(
                ring.outcome,
                RingOutcome::Waiting | RingOutcome::Missed | RingOutcome::EndedElsewhere
            ) {
                ring.outcome = outcome.into();
            }
        } else {
            if self.early.len() >= 1024
                && let Some(key) = self
                    .early
                    .iter()
                    .min_by_key(|(_, (_, at))| *at)
                    .map(|(key, _)| *key)
            {
                self.early.remove(&key);
            }
            self.early.entry(key).or_insert((outcome, Instant::now()));
        }
    }
    pub fn expire(&mut self, now: Instant) {
        let retention = std::time::Duration::from_secs(65);
        self.early
            .retain(|_, (_, at)| now.saturating_duration_since(*at) < retention);
        self.voice_invitations
            .retain(|_, at| now.saturating_duration_since(*at) < retention);
        for ring in self.rings.values_mut() {
            if ring.outcome == RingOutcome::Waiting && now >= ring.deadline {
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
        self.voice_invitations.retain(|(id, _), _| *id != server);
        self.finished.retain(|(id, _), _| *id != server);
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
    #[cfg(feature = "test-support")]
    pub fn is_ringing(&self) -> bool {
        self.ringer.is_ringing()
    }

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
        self.direct.see(server, own, &invitation, Instant::now());
        if let Some(ring) = self.direct.rings.get_mut(&(server, invitation.id)) {
            ring.outcome = outcome;
        }
        cx.notify();
    }
    fn reconcile_direct_lifetime(&mut self, cx: &mut gpui::Context<Self>) {
        let calls = self.calls.view();
        let Some(call) = calls
            .call
            .as_ref()
            .filter(|call| matches!(call.target.place, CallPlace::Direct { .. }))
        else {
            self.direct.active = None;
            return;
        };
        let target = call.target;
        let Some(own) = self
            .client
            .server(target.server_id)
            .and_then(|server| server.account())
        else {
            return;
        };
        let CallPlace::Direct {
            low_account_id,
            high_account_id,
        } = target.place
        else {
            return;
        };
        let other = if own == low_account_id {
            high_account_id
        } else {
            low_account_id
        };
        if self
            .direct
            .active
            .as_ref()
            .is_none_or(|(held, _)| *held != target)
        {
            self.direct.active = Some((target, lifetime::Lifetime::new(Instant::now())));
            self.direct.finished.remove(&(target.server_id, other));
        }
        let present = self
            .client
            .server(target.server_id)
            .and_then(|server| server.live().view().model)
            .and_then(|model| {
                model
                    .call_at(&target.place)
                    .map(|call| call.participant(other).is_some())
            });
        let outcome = self
            .direct
            .active
            .as_mut()
            .and_then(|(_, lifetime)| lifetime.observe(present, Instant::now()));
        if let Some(outcome) = outcome {
            self.direct
                .finished
                .insert((target.server_id, other), outcome);
            self.leave_call(cx);
        }
    }
    pub(super) fn reconcile_direct(&mut self, cx: &mut gpui::Context<Self>) {
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
                self.direct
                    .reconcile_server(server.id(), &model.invitations);
                for invitation in &model.invitations {
                    self.direct
                        .see(server.id(), own, invitation, Instant::now());
                }
            }
        }
        self.direct.expire(Instant::now());
        self.reconcile_direct_lifetime(cx);
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
