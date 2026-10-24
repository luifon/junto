# 0214. Event cursors in change responses

## Status

Accepted

## Context

A response and the events of its transaction can arrive in either order. A later read of
the global head includes unrelated transactions. The last position of the whole transaction
also counts events for other accounts and can disclose private activity to its caller.

## Decision

A request authenticated with a verified, active session can receive the `Junto-Event-Cursor`
header on a successful POST, PUT, PATCH or DELETE response. A request without a session
credential, including join and login, and a request with an access token receive no header.
Only credentials that may open the server event stream qualify. A successful GET that
commits a writing transaction can have the header too; ordinary reads and errors do not.

The value is a JSON cursor `{"epoch":"...","position":123}` using ADR 0010. It names the end
of the last change committed by the request of which the caller itself may receive an event,
decided by the same function as event delivery. Both a transaction without events and a
transaction whose events all go to others yield `null`. They cannot be distinguished through
the header. A later eventless or hidden-only transaction does not erase an earlier visible
receipt.

An absent header means **no position stated**, not that the request wrote nothing or failed.
Receipt lookup is best effort after the change commits. Any failure of that lookup, including
its read-only commit, logs a warning and leaves the normal success status and body intact,
without the header. It never becomes a busy or timeout response and never repeats the writing
transaction. If one lookup fails in a request with several commits, the whole request omits
the header; an earlier or later cursor cannot establish the missing boundary.

The deferred position trigger records the position of the change in a connection setting. It
does not read the epoch table. Each writing attempt clears the setting. After a successful
commit, the request retains that connection and reads it. Failed attempts and rollbacks
contribute nothing. Unverified requests skip this read, and a transaction without events
needs no epoch or delivery read.

A change is decided in a fresh read-only transaction. It loads the account's current
access and the current epoch, reads only the events of the change in bounded
batches, and uses the delivery decision at full level, independent of any connection's
subscription level. The same decision checks audiences, current roles and memberships,
notification visibility and quiet time. Subscription replay, delivery and receipt selection
use one function for their SQL audience/class filter. The read transaction is retained because
access, log state, candidates and Standing must share one snapshot after the commit;
an own-account address alone cannot bypass current-account or quiet-time checks. Positions
of hidden events never become the receipt.
The cursor is stored only after this read succeeds. If the range was trimmed or a relevant
event cannot be read, the success response omits the header instead of supplying a false
cursor. An epoch reset requires client resynchronization.

A client that has processed its stream through this position has processed every event of
its change that it may receive under that authorization decision. This is the whole promise;
it says nothing about events for other accounts. Later permission changes are still decided
under ADR 0011. A background connection can reach the boundary through a checkpoint, and a
snapshot at or beyond it also establishes the boundary. A response receipt never advances
the client's stream cursor. A null receipt introduces no event version and must not replace
newer event state. When the header is absent, the client uses the successful response normally
and does not repeat the change for lack of a receipt. If it needs a known reconciliation
boundary, it loads a fresh snapshot after that response; an existing cursor or an in-flight
checkpoint does not establish the missing position. A call observation cursor still describes
the state of its observation.

## Consequences

Response bodies and empty 204 responses are unchanged. Old readers ignore the header.
A writing HTTP transaction resets one setting. An authenticated request reads it after
commit and, for a change with events, pays for the same access decisions as delivery. If that read
fails, the server logs the failure and omits the header while preserving success.

The header exposes no event position beyond what the caller's own full stream can deliver.
It adds no boundary for trailing hidden events. The positions of the stream count changes,
not events (ADR 0010), so neither the header nor the gaps between the events of one change
tell how many events of the change went to other accounts (ADR 0011).
