# 0120. Private moderation notices and their retention

## Status

Accepted; preserves the audit-only reason rule of ADR 0106.

## Context

Workspace removal leaves the account active but its next snapshot omits the workspace.
Server removal and ban end sessions; their streams state removed or banned without the
moderation reason. Reasons otherwise remain in immutable audit entries. Session tokens
are held only in memory (ADR 0710), so a session-based notice cannot serve a restarted
client whose account was removed.

The existing login rejects banned keys and unknown or removed keys before issuing a
challenge. A login challenge names its account before proof. The client requires that
account to match its saved account and binds a successful session to it. Issuing a
historical account identifier would expose removal history before proof; an opaque login
challenge would change this existing contract. Notice retrieval therefore uses a separate
opaque challenge without changing login or admitting historical keys.

## Decision

Each workspace removal, server removal and ban records a private notice in its transaction.
It holds an identifier, kind, optional workspace identifier, the workspace or server name
at the time of the action, optional message for the person affected, and time. It has no
actor or other-account identifier. The request field `reason` remains private audit text,
never copied into the notice. The separate optional `recipient_message` has at most 1,000
characters without controls and is stored and returned as the notice `message`. The moderator chooses
this message knowing that the person affected can read it. Without it the notice has no
message. Audit text is never copied into a notice. The notice table has only the nullable
`message` column for this text; it has no audit-reason column. The action's audit entry also
retains `recipient_message` separately from `reason`, so audit readers can see whether text
was sent and what it said, including after notice retention ends. Older audit entries without
that optional field mean no recorded recipient message.
It is a historical statement, not the account's current standing.

An active account reads its own notices at `GET /api/v1/account/moderation-notices`.
The snapshot response includes the newest 100 in `moderation_notices` and a continuation
in `moderation_notices_next`. Existing removal and permission events prompt a refresh.
Notice pages have at most 100 entries, newest identifier first, with `next_before`.

At recording, the notice retains its account and device-key readers separately from login
devices. A key registered to another account loses its earlier reader rows in the same
transaction. Reads also exclude a key currently registered to a different account.
A device requests a `moderation_notices` challenge through the existing challenge route;
its account identifier is always null, even for a known key. The signed proof is submitted
to `POST /api/v1/auth/moderation-notices`. Only notices addressed to that proven key are
returned. A key with no notices receives an empty page. The proof retains origin, server,
operation, expiry and single-use checks of ADR 0108; it creates no session and grants no
other access. Challenge and proof requests use the existing challenge address limit.
Access tokens read neither notices nor snapshots.

Explicit device deletion removes that key's historical notice readers in the same
transaction. Other devices retain their readers and the account retains its notices.
Server-account removal instead retains the readers for its ended devices, as required for
the private statement. Recovery retains registered device keys under the existing recovery
rule, so those keys retain notice access.

Private notices and their historical device-key readers last 30 days. Reads exclude them
at that boundary. The periodic maintenance retention pass deletes expired notices in
bounded batches and cascades their device readers. The same period applies to all three
kinds of notice. A client remembers which identifiers it has shown. No acknowledgement
route is required because the retention period bounds storage.

The separate existing records retain their existing rules: a banned account retains its
devices, memberships and roles while banned and restores them on unban; a removed account
loses devices, sessions, memberships and roles. Account marker rows and audit entries remain. Ended
session reasons are retained for the session lifetime, currently 30 days. Message and
recording retention remains independent. Historical notice readers grant no authority for login, joining, invite withdrawal, contact,
or banning a new account; device registration only removes obsolete reader rows.

## Consequences

A removed or banned person can read the private statement after a restart within 30 days
using the original device key. After the period, a banned person still learns the ban
from login, and a removed key is unknown; no private reason remains available through
notice retrieval. Audit readers retain their separately authorized history.

Workspace removal is stated even after the event log is trimmed. Other workspace members
receive no private reason through these notices. Retaining a historical device reader for
30 days is an explicit, bounded exception to deleting all device associations at removal;
it does not keep the device registered or prevent its use to join a new account.
