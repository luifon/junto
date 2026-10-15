# 0771. Direct call lifetime and local history

## Status

Accepted

## Context

An unanswered direct call may have no invitation because the recipient muted the conversation or is quiet. The caller must not learn which rule suppressed the ring. Invitation timestamps belong to the server clock, which may differ from the device clock.

## Decision

- An outgoing direct call waits at most sixty seconds for the other participant, whether or not an invitation exists. The device measures this interval with a monotonic clock. It leaves when the interval expires and shows No answer.
- Once the other participant has joined, their departure ends this device's direct call. A temporarily unavailable participant snapshot does not count as a departure.
- Before the other participant has joined, a decline or an expiry of the ring of the call ends the outgoing call at once, with the outcome Declined or No answer. The ring belongs to the call when its invitation names the call that the server state names as the own call of the account, and that own call names the seat of this device in the call; the server sends that call in the same change as the ring. Each new place of the account in a call has a new seat (ADR 0508), so an own call that a server state still holds from an older call at the same place, for example after it missed the leave of that call, does not supply the ring of the new call. The device does not decide this when the resolution arrives, but each time it reconciles its direct calls: after each event, each change of its own call, and each second. A resolution that arrives before the answer to the join, or before the media connection, therefore ends the call as soon as the call exists on the device. Each way of ending records the same outcome in the history, so the history does not depend on whether the server or the device reached the deadline first.
- An incoming invitation gets a local monotonic ringing interval bounded by the duration supplied by the server and by sixty seconds. Absolute server timestamps do not control the device's ring timer.
- The server remains authoritative for acceptance, refusal and expiry. A reload that no longer contains an invitation stops its ring without inventing a missed-call outcome. A later authoritative resolution can supply the outcome.
- Decline stops the local ring immediately. An incoming caller cancellation is a missed call for the recipient. Outgoing unanswered calls say No answer.
- The incoming view identifies the caller and offers Accept and Decline. It suppresses the conversation's start-call action while that invitation waits.
- Call history is local to the current client session until the server supplies durable call-history events.

## Consequences

Quiet and muted conversations have the same unanswered behavior for the caller. A device that receives an old pending snapshot can ring for at most one local interval, and subsequent authoritative events or reloads end it sooner. Local history is not a durable record and does not infer what happened on another device.

Tests: `state::direct::lifetime::tests` and `state::direct::tests::an_own_call_held_from_an_older_call_at_the_same_place_does_not_end_a_new_call` in `crates/client`, and in the window live tier `live::direct::an_outgoing_call_that_the_server_finds_unanswered_first_ends_with_no_answer` and `live::direct::a_decline_before_the_answer_to_the_join_ends_the_outgoing_call`, which holds the answer of the server to the join until the device holds the decline.
