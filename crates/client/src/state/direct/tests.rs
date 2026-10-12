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
    calls.see(server, own, &invitation, invitation.created_at);
    assert!(calls.ringing());
    calls.resolve(server, invitation.id, InvitationOutcome::Accepted);
    assert!(!calls.ringing());
    assert_eq!(
        calls.rings[&(server, invitation.id)].outcome,
        RingOutcome::Accepted
    );
    calls.see(server, own, &invitation, invitation.created_at);
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
        calls.see(server, own, &invitation, invitation.created_at);
        calls.resolve(server, invitation.id, outcome);
        assert_eq!(calls.rings[&(server, invitation.id)].outcome, wanted);
        assert!(!calls.ringing());
    }
}

#[test]
fn a_late_snapshot_cannot_ring_an_expired_call() {
    let (server, own, invitation) = ring();
    let mut calls = DirectCalls::default();
    calls.see(server, own, &invitation, invitation.expires_at);
    assert!(!calls.ringing());
    assert_eq!(
        calls.rings[&(server, invitation.id)].outcome,
        RingOutcome::Missed
    );
}

#[test]
fn an_outgoing_ring_makes_no_local_ring_sound() {
    let (server, _, invitation) = ring();
    let mut calls = DirectCalls::default();
    calls.see(
        server,
        invitation.invited_by,
        &invitation,
        invitation.created_at,
    );
    assert!(!calls.ringing());
    let held = &calls.rings[&(server, invitation.id)];
    assert!(!held.incoming);
    assert_eq!(held.other, invitation.account_id);
    calls.expire(invitation.expires_at);
    assert_eq!(
        calls.rings[&(server, invitation.id)].outcome,
        RingOutcome::Missed
    );
}

#[test]
fn forgetting_a_server_discards_its_ring_history() {
    let (server, own, invitation) = ring();
    let mut calls = DirectCalls::default();
    calls.see(server, own, &invitation, invitation.created_at);
    calls.forget_server(server);
    assert!(calls.rings.is_empty());
    assert!(!calls.ringing());
}

#[test]
fn direct_ring_resolution_before_snapshot_is_not_lost() {
    let (server, own, invitation) = ring();
    let mut calls = DirectCalls::default();
    calls.resolve(server, invitation.id, InvitationOutcome::Declined);
    calls.see(server, own, &invitation, invitation.created_at);
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
    calls.see(
        server,
        AccountId::generate(),
        &invitation,
        invitation.created_at,
    );
    assert!(calls.rings.is_empty());
    invitation.place = CallPlace::Channel {
        workspace_id: junto_client_core::protocol::ids::WorkspaceId::generate(),
        channel_id: junto_client_core::protocol::ids::ChannelId::generate(),
    };
    calls.see(
        server,
        invitation.account_id,
        &invitation,
        invitation.created_at,
    );
    assert!(calls.rings.is_empty());
}

#[test]
fn an_answered_direct_call_records_its_end() {
    let (server, own, invitation) = ring();
    let mut calls = DirectCalls::default();
    calls.see(server, own, &invitation, invitation.created_at);
    calls.resolve(server, invitation.id, InvitationOutcome::Accepted);
    calls.ended(server, invitation.call_id);
    assert_eq!(
        calls.rings[&(server, invitation.id)].outcome,
        RingOutcome::Ended
    );
}
