use super::*;
use junto_client_core::Owner;

#[derive(Clone)]
pub(crate) struct PinnedPerson {
    pub owner: Owner,
    pub seat: Seat,
    pub account: AccountId,
    pub show_person: bool,
}

impl AppState {
    fn pin_context_current(&self, pin: &PinnedPerson, participant_required: bool) -> bool {
        let Some(server) = self
            .client
            .server(pin.owner.entry.server_id)
            .filter(|server| server.owner() == Some(pin.owner))
        else {
            return false;
        };
        if pin.seat.target.server_id != pin.owner.entry.server_id
            || self
                .media_calls()
                .call
                .is_none_or(|call| call.seat() != pin.seat)
        {
            return false;
        }
        match server.live().view().model {
            Some(model) => model
                .call_at(&pin.seat.target.place)
                .is_some_and(|call| call.participant(pin.account).is_some()),
            None => !participant_required,
        }
    }

    pub(crate) fn pinned_call(&self) -> Option<PinnedPerson> {
        self.pinned_call
            .as_ref()
            .filter(|pin| self.pin_context_current(pin, false))
            .cloned()
    }

    pub fn pinned_call_person(&self) -> Option<AccountId> {
        self.pinned_call().map(|pin| pin.account)
    }

    pub fn can_pin_call_person(&self, owner: Owner, seat: &Seat, account: AccountId) -> bool {
        self.pin_context_current(
            &PinnedPerson {
                owner,
                seat: seat.clone(),
                account,
                show_person: true,
            },
            true,
        )
    }

    pub fn pin_call_person(
        &mut self,
        owner: Owner,
        seat: &Seat,
        account: AccountId,
        pinned: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let pin = PinnedPerson {
            owner,
            seat: seat.clone(),
            account,
            show_person: true,
        };
        if !self.pin_context_current(&pin, pinned) {
            return false;
        }
        if pinned {
            self.pinned_call = Some(pin);
        } else if self.pinned_call.as_ref().is_some_and(|current| {
            current.owner == owner && current.seat == *seat && current.account == account
        }) {
            self.pinned_call = None;
        } else {
            return false;
        }
        cx.notify();
        true
    }

    pub fn show_pinned_person(
        &mut self,
        owner: Owner,
        seat: &Seat,
        account: AccountId,
        show: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(pin) = self
            .pinned_call()
            .filter(|pin| pin.owner == owner && pin.seat == *seat && pin.account == account)
        else {
            return false;
        };
        self.pinned_call = Some(PinnedPerson {
            show_person: show,
            ..pin
        });
        cx.notify();
        true
    }

    pub(super) fn reconcile_call_pin(&mut self) {
        if self
            .pinned_call
            .as_ref()
            .is_some_and(|pin| !self.pin_context_current(pin, false))
        {
            self.pinned_call = None;
        }
    }
}
