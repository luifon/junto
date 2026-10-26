//! Direct-call rings and their outcomes during this client session.

use junto_client_core::protocol::call::{CallInvitation, CallPlace, InvitationOutcome, OwnCall};
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
    /// The direct call of this device, by the identity that the server gave the device
    /// for it, and its lifetime.
    active: Option<(String, lifetime::Lifetime)>,
    pub finished: HashMap<(ServerId, AccountId), RingOutcome>,
    early: HashMap<(ServerId, CallInvitationId), (InvitationOutcome, Instant)>,
    voice_invitations: HashMap<(ServerId, CallInvitationId), Instant>,
    /// The account that what is held of each server belongs to.
    owners: HashMap<ServerId, AccountId>,
}
impl DirectCalls {
    pub(super) fn filter_view(&self, mut view: super::ShellView) -> super::ShellView {
        let mut hidden = Vec::new();
        view.invitations.retain(|invited| {
            let shown = self
                .rings
                .get(&(invited.target.server_id, invited.id))
                .is_none_or(|ring| ring.outcome == RingOutcome::Waiting);
            if !shown {
                hidden.push((invited.target.server_id, invited.caller));
            }
            shown
        });
        // The view names the caller of each invitation last, for the read of the profile.
        // An invitation that is not shown names nobody.
        for (server_id, caller) in hidden {
            if let Some(wanted) = view.wanted_people.get_mut(&server_id)
                && let Some(at) = wanted
                    .accounts
                    .iter()
                    .rposition(|account| *account == caller)
            {
                wanted.accounts.remove(at);
            }
        }
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
    /// The outcome of the newest ring that this account made for the call `call`.
    fn outgoing_outcome(
        &self,
        server: ServerId,
        call: junto_client_core::protocol::ids::CallId,
    ) -> Option<RingOutcome> {
        self.rings
            .iter()
            .filter(|((held, _), ring)| {
                *held == server && !ring.incoming && ring.invitation.call_id == call
            })
            .max_by_key(|(_, ring)| ring.invitation.created_at)
            .map(|(_, ring)| ring.outcome)
    }
    /// The outcome of the newest ring that this account made for the call in which this
    /// device has the seat `seat`. `own_call`, the own call that the state of the server
    /// names, identifies that call only when it names the same seat: every new place of the
    /// account in a call has a new seat (ADR 0508), so an own call that the state still
    /// holds from an older call at the same place says nothing of this call.
    pub(super) fn ring_of_seat(
        &self,
        seat: &junto_media::calls::Seat,
        own_call: Option<&OwnCall>,
    ) -> Option<RingOutcome> {
        own_call
            .filter(|own| own.place == seat.target.place && own.identity == seat.identity)
            .and_then(|own| self.outgoing_outcome(seat.target.server_id, own.call_id))
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
        self.owners.remove(&server);
    }

    /// Takes the account that the device has on each saved server now. What is held of a
    /// server belongs to one account: it is dropped when the device has another account
    /// there, or none, or when the server is not saved any more. The outcome of a call of
    /// one account is not shown to the next one.
    pub fn own(&mut self, accounts: &HashMap<ServerId, AccountId>) {
        let held: std::collections::HashSet<ServerId> = (self.rings.keys().map(|(id, _)| *id))
            .chain(self.finished.keys().map(|(id, _)| *id))
            .chain(self.early.keys().map(|(id, _)| *id))
            .chain(self.voice_invitations.keys().map(|(id, _)| *id))
            .chain(self.owners.keys().copied())
            .collect();
        for server in held {
            let now = accounts.get(&server);
            let owner = self.owners.get(&server);
            let same = now.is_some() && owner.is_none_or(|owner| Some(owner) == now);
            if !same {
                self.forget_server(server);
            }
        }
        self.owners.clone_from(accounts);
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

/// The account that a device has on a server, with or without a session.
fn account_of(access: &junto_client_core::flow::access::AccessState) -> Option<AccountId> {
    use junto_client_core::flow::access::AccessState;
    match access {
        AccessState::LoggedIn { account_id, .. }
        | AccessState::LoggedOut { account_id }
        | AccessState::LoggedOutByPerson { account_id } => Some(*account_id),
        AccessState::Banned { account_id }
        | AccessState::Inactive { account_id }
        | AccessState::Deleted { account_id } => *account_id,
        AccessState::NoAccount { .. }
        | AccessState::WaitingForApproval { .. }
        | AccessState::WaitingForLink { .. } => None,
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
    /// Holds the outcome of a direct call with a person, as the end of such a call does.
    #[cfg(feature = "test-support")]
    pub fn finish_direct_fixture(
        &mut self,
        server: ServerId,
        other: AccountId,
        outcome: RingOutcome,
    ) {
        self.direct.finished.insert((server, other), outcome);
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
        // A new call to the same person can follow the last one without a moment in no
        // call: the lifetime belongs to the identity of the call, not to its place.
        if self
            .direct
            .active
            .as_ref()
            .is_none_or(|(held, _)| *held != call.identity)
        {
            self.direct.active = Some((
                call.identity.clone(),
                lifetime::Lifetime::new(Instant::now()),
            ));
            self.direct.finished.remove(&(target.server_id, other));
        }
        let model = self
            .client
            .server(target.server_id)
            .and_then(|server| server.live().view().model);
        let present = model.as_ref().and_then(|model| {
            model
                .call_at(&target.place)
                .map(|call| call.participant(other).is_some())
        });
        // The ring of this call is found by the call that the state of the server names
        // for this device. The server tells that call in the same change as the ring, so
        // it does not depend on when the media connection of the call is made.
        let ring = self.direct.ring_of_seat(
            &call.seat(),
            model.as_ref().and_then(|model| model.own_call.as_ref()),
        );
        let outcome = self
            .direct
            .active
            .as_mut()
            .and_then(|(_, lifetime)| lifetime.observe(present, ring, Instant::now()));
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
        let accounts = (servers.iter())
            .filter_map(|server| Some((server.id(), account_of(&server.access().state)?)))
            .collect();
        self.direct.own(&accounts);
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
            && self
                .known
                .media
                .voice
                .sounds
                .plays(junto_client_core::voice::SoundKind::Ring)
            && self.direct.rings.iter().any(|((id, _), ring)| {
                ring.incoming
                    && ring.outcome == RingOutcome::Waiting
                    && self
                        .client
                        .server(*id)
                        .and_then(|s| s.live().view().model)
                        .is_some_and(|m| !super::notices::quiet_at(&m, now))
            });
        let device = self
            .known
            .media
            .voice
            .output_device
            .clone()
            .map(junto_media::audio::DeviceId);
        self.ringer.set_ringing(ringing, device.as_ref());
    }
}
