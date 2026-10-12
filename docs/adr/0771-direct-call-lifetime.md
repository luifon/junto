# 0771. Direct call lifetime and local history

## Status

Accepted

## Context

An unanswered direct call may have no invitation because the recipient muted the conversation or is quiet. The caller must not learn which rule suppressed the ring. Invitation timestamps belong to the server clock, which may differ from the device clock.

## Decision

- An outgoing direct call waits at most sixty seconds for the other participant, whether or not an invitation exists. The device measures this interval with a monotonic clock. It leaves when the interval expires and shows No answer.
- Once the other participant has joined, their departure ends this device's direct call. A temporarily unavailable participant snapshot does not count as a departure.
- An incoming invitation gets a local monotonic ringing interval bounded by the duration supplied by the server and by sixty seconds. Absolute server timestamps do not control the device's ring timer.
- The server remains authoritative for acceptance, refusal and expiry. A reload that no longer contains an invitation stops its ring without inventing a missed-call outcome. A later authoritative resolution can supply the outcome.
- Decline stops the local ring immediately. An incoming caller cancellation is a missed call for the recipient. Outgoing unanswered calls say No answer.
- The incoming view identifies the caller and offers Accept and Decline. It suppresses the conversation's start-call action while that invitation waits.
- Call history is local to the current client session until the server supplies durable call-history events.

## Consequences

Quiet and muted conversations have the same unanswered behavior for the caller. A device that receives an old pending snapshot can ring for at most one local interval, and subsequent authoritative events or reloads end it sooner. Local history is not a durable record and does not infer what happened on another device.
