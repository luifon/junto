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
