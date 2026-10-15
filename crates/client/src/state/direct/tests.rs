use super::*;
use junto_client_core::protocol::call::CallPlace;
use junto_client_core::protocol::ids::CallId;

fn ring() -> (ServerId, AccountId, CallInvitation) {
    let caller = AccountId::generate();
    let callee = AccountId::generate();
    let now = OffsetDateTime::now_utc();
    (
        ServerId::generate(),
        callee,
        CallInvitation {
            id: CallInvitationId::generate(),
            call_id: CallId::generate(),
            place: CallPlace::direct(caller, callee),
            account_id: callee,
            invited_by: caller,
            created_at: now,
            expires_at: now + time::Duration::seconds(60),
        },
    )
}

#[test]
fn incoming_direct_call_rings_until_resolved() {
    let (server, own, invitation) = ring();
    let mut calls = DirectCalls::default();
    assert!(!calls.ringing());
    calls.see(server, own, &invitation, Instant::now());
    assert!(calls.ringing());
    calls.resolve(server, invitation.id, InvitationOutcome::Accepted);
    assert!(!calls.ringing());
    assert_eq!(
        calls.rings[&(server, invitation.id)].outcome,
        RingOutcome::Accepted
    );
    calls.see(server, own, &invitation, Instant::now());
    assert!(
        !calls.ringing(),
        "an older snapshot cannot ring a resolved invitation again"
    );
}

#[test]
fn direct_call_declined_and_missed_are_distinct() {
    let (server, own, invitation) = ring();
    for (outcome, wanted) in [
        (InvitationOutcome::Declined, RingOutcome::Declined),
        (InvitationOutcome::Expired, RingOutcome::Missed),
        (InvitationOutcome::Cancelled, RingOutcome::Cancelled),
    ] {
        let mut calls = DirectCalls::default();
        calls.see(server, own, &invitation, Instant::now());
        calls.resolve(server, invitation.id, outcome);
        assert_eq!(calls.rings[&(server, invitation.id)].outcome, wanted);
        assert!(!calls.ringing());
    }
}

#[test]
fn a_reloaded_snapshot_cannot_ring_a_resolved_call() {
    let (server, own, invitation) = ring();
    let mut calls = DirectCalls::default();
    calls.see(server, own, &invitation, Instant::now());
    calls.reconcile_server(server, &[]);
    calls.see(server, own, &invitation, Instant::now());
    assert!(!calls.ringing());
    assert_eq!(
        calls.rings[&(server, invitation.id)].outcome,
        RingOutcome::EndedElsewhere
    );
}

#[test]
fn an_outgoing_ring_makes_no_local_ring_sound() {
    let (server, _, invitation) = ring();
    let mut calls = DirectCalls::default();
    calls.see(server, invitation.invited_by, &invitation, Instant::now());
    assert!(!calls.ringing());
    let held = &calls.rings[&(server, invitation.id)];
    assert!(!held.incoming);
    assert_eq!(held.other, invitation.account_id);
    calls.expire(Instant::now() + std::time::Duration::from_secs(60));
    assert_eq!(
        calls.rings[&(server, invitation.id)].outcome,
        RingOutcome::Missed
    );
}

#[test]
fn forgetting_a_server_discards_its_ring_history() {
    let (server, own, invitation) = ring();
    let mut calls = DirectCalls::default();
    calls.see(server, own, &invitation, Instant::now());
    calls.forget_server(server);
    assert!(calls.rings.is_empty());
    assert!(!calls.ringing());
}

#[test]
fn direct_ring_resolution_before_snapshot_is_not_lost() {
    let (server, own, invitation) = ring();
    let mut calls = DirectCalls::default();
    calls.resolve(server, invitation.id, InvitationOutcome::Declined);
    calls.see(server, own, &invitation, Instant::now());
    assert!(!calls.ringing());
    assert_eq!(
        calls.rings[&(server, invitation.id)].outcome,
        RingOutcome::Declined
    );
}

#[test]
fn a_voice_invitation_and_another_accounts_ring_are_not_direct_rings() {
    let (server, _, mut invitation) = ring();
    let mut calls = DirectCalls::default();
    calls.see(server, AccountId::generate(), &invitation, Instant::now());
    assert!(calls.rings.is_empty());
    invitation.place = CallPlace::Channel {
        workspace_id: junto_client_core::protocol::ids::WorkspaceId::generate(),
        channel_id: junto_client_core::protocol::ids::ChannelId::generate(),
    };
    calls.see(server, invitation.account_id, &invitation, Instant::now());
    assert!(calls.rings.is_empty());
}

#[test]
fn an_answered_direct_call_records_its_end() {
    let (server, own, invitation) = ring();
    let mut calls = DirectCalls::default();
    calls.see(server, own, &invitation, Instant::now());
    calls.resolve(server, invitation.id, InvitationOutcome::Accepted);
    calls.ended(server, invitation.call_id);
    assert_eq!(
        calls.rings[&(server, invitation.id)].outcome,
        RingOutcome::Ended
    );
}

#[test]
fn a_server_clock_offset_does_not_silence_or_extend_an_incoming_ring() {
    for offset in [-86_400, 86_400] {
        let (server, own, mut invitation) = ring();
        invitation.created_at += time::Duration::seconds(offset);
        invitation.expires_at += time::Duration::seconds(offset);
        let local = Instant::now();
        let mut calls = DirectCalls::default();
        calls.see(server, own, &invitation, local);
        assert!(calls.ringing(), "clock offset {offset}");
        calls.expire(local + std::time::Duration::from_secs(60));
        assert!(!calls.ringing(), "clock offset {offset}");
    }
}

#[test]
fn a_ring_absent_after_reload_stops_without_claiming_it_was_missed() {
    let (server, own, invitation) = ring();
    let mut calls = DirectCalls::default();
    calls.see(server, own, &invitation, Instant::now());
    calls.reconcile_server(server, &[]);
    assert!(!calls.ringing());
    assert_ne!(
        calls.rings[&(server, invitation.id)].outcome,
        RingOutcome::Missed
    );
}

#[test]
fn a_voice_invitation_resolution_does_not_enter_direct_call_history() {
    let (server, own, mut invitation) = ring();
    invitation.place = CallPlace::Channel {
        workspace_id: junto_client_core::protocol::ids::WorkspaceId::generate(),
        channel_id: junto_client_core::protocol::ids::ChannelId::generate(),
    };
    let mut calls = DirectCalls::default();
    calls.see(server, own, &invitation, Instant::now());
    calls.resolve(server, invitation.id, InvitationOutcome::Declined);
    assert!(calls.early.is_empty());
}

#[test]
fn an_invitation_that_is_not_shown_names_no_caller_for_the_profile_reads() {
    use crate::state::view::{Invited, Known, Selection, derive};
    use junto_media::calls::CallTarget;
    let (server, own, invitation) = ring();
    let caller = invitation.invited_by;
    let mut calls = DirectCalls::default();
    calls.see(server, own, &invitation, Instant::now());
    let view = || {
        let mut view = derive(&[], &Selection::default(), &Known::default());
        view.invitations.push(Invited {
            id: invitation.id,
            caller,
            target: CallTarget {
                server_id: server,
                place: invitation.place,
            },
            by: None,
            channel: None,
        });
        view.wanted_people
            .entry(server)
            .or_default()
            .accounts
            .push(caller);
        view
    };
    let shown = calls.filter_view(view());
    assert_eq!(shown.invitations.len(), 1);
    assert_eq!(shown.wanted_people[&server].accounts, [caller]);
    // The person declined: the invitation leaves the view, and its caller with it.
    calls.resolve(server, invitation.id, InvitationOutcome::Declined);
    let hidden = calls.filter_view(view());
    assert!(hidden.invitations.is_empty());
    assert!(hidden.wanted_people[&server].accounts.is_empty());
}

#[test]
fn what_is_held_of_a_server_is_dropped_with_its_account() {
    let (server, own, invitation) = ring();
    let other = AccountId::generate();
    let held = |calls: &mut DirectCalls| {
        calls.see(server, own, &invitation, Instant::now());
        calls.finished.insert((server, other), RingOutcome::Missed);
    };
    let mut calls = DirectCalls::default();
    held(&mut calls);
    // The same account: nothing is dropped.
    calls.own(&[(server, own)].into_iter().collect());
    calls.own(&[(server, own)].into_iter().collect());
    assert_eq!(calls.finished.len(), 1);
    assert_eq!(calls.rings.len(), 1);
    // The device has another account on the server.
    calls.own(&[(server, AccountId::generate())].into_iter().collect());
    assert!(
        calls.finished.is_empty() && calls.rings.is_empty(),
        "the next account sees the outcome of a call of the account before it"
    );
    // The device has no account there, and the server is not saved any more.
    for accounts in [
        HashMap::new(),
        [(ServerId::generate(), own)].into_iter().collect(),
    ] {
        let mut calls = DirectCalls::default();
        calls.own(&[(server, own)].into_iter().collect());
        held(&mut calls);
        calls.own(&accounts);
        assert!(calls.finished.is_empty() && calls.rings.is_empty());
    }
}

#[test]
fn the_outcome_of_a_call_is_the_one_of_its_newest_outgoing_ring() {
    let (server, callee, first) = ring();
    let caller = first.invited_by;
    let mut calls = DirectCalls::default();
    calls.see(server, caller, &first, Instant::now());
    calls.resolve(server, first.id, InvitationOutcome::Declined);
    // A later ring of the same call waits.
    let later = CallInvitation {
        id: CallInvitationId::generate(),
        created_at: first.created_at + time::Duration::seconds(5),
        expires_at: first.expires_at + time::Duration::seconds(5),
        ..first.clone()
    };
    calls.see(server, caller, &later, Instant::now());
    // A declined ring of an older call at the same place, and an incoming ring of this
    // call, say nothing of this call.
    let older = CallInvitation {
        id: CallInvitationId::generate(),
        call_id: CallId::generate(),
        created_at: later.created_at + time::Duration::seconds(5),
        ..first.clone()
    };
    calls.see(server, caller, &older, Instant::now());
    calls.resolve(server, older.id, InvitationOutcome::Declined);
    assert_eq!(
        calls.outgoing_outcome(server, first.call_id),
        Some(RingOutcome::Waiting)
    );
    let incoming = CallInvitation {
        id: CallInvitationId::generate(),
        account_id: caller,
        invited_by: callee,
        created_at: later.created_at + time::Duration::seconds(10),
        ..first.clone()
    };
    calls.see(server, caller, &incoming, Instant::now());
    calls.resolve(server, incoming.id, InvitationOutcome::Declined);
    assert_eq!(
        calls.outgoing_outcome(server, first.call_id),
        Some(RingOutcome::Waiting)
    );
    calls.resolve(server, later.id, InvitationOutcome::Expired);
    assert_eq!(
        calls.outgoing_outcome(server, first.call_id),
        Some(RingOutcome::Missed)
    );
    assert_eq!(calls.outgoing_outcome(server, CallId::generate()), None);
}
