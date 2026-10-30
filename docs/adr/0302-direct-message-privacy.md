# 0302. Direct messages and their privacy

## Status

Accepted

## Context

A direct message is a private conversation between people and belongs to no workspace, so workspace roles and channel permissions do not apply to it. The administrator permission passes every channel check (ADR 0301). The server has no end-to-end encryption (ADR 0002), so the content of a direct message is readable by the server and by the person who operates it.

## Decision

Scope:

- A direct message is one-to-one or a named group.
- Two accounts exchange direct messages only while the rule of ADR 0307 holds for them.
- A direct message exists inside one server and is shared across that server's workspaces.
- Direct messages are not deleted automatically by default. One server-level retention setting applies to them.
- Each participant can export their own direct messages.

Privacy:

- Direct messages are closed to everyone except their current participants. This check runs before the administrator permission (ADR 0301).
- There is no administrator read feature and no administrator export for direct messages.
- The join screen and the documentation state that the person who operates a server has technical access to its data.

### Named group conversations

Clients send `Junto-Group-Conversations: 1` on HTTP requests and the WebSocket upgrade to opt into group values in message and call places. The new `/group-conversations` routes opt in by their path. Existing routes and streams without the header retain their pair and channel kinds: groups, their calls, message summaries, search hits, notices, files and exports are omitted or unavailable. Internal admission and retention support groups independently of a reader's capability. The capability belongs to the complete connection, including its spawned delivery reads. This avoids sending an unknown enum value in an existing API v1 field to an older client (ADR 0206).

- A named group starts with three to ten distinct active accounts, including its creator. Its name has one to eighty characters without controls. Every member can rename it, add an eligible account or leave. The identifier stays the same after a departure, including when only two or one remain; it never becomes the pair conversation of the remaining accounts.
- Membership is separate from the pair identifiers. A group message and a group call name the conversation identifier. The pair routes and the unique conversation between two accounts retain their formats and behavior.
- Only current members can read the group. Membership is checked before administrator permissions, for history, message and attachment lookup, uploads, search, notices, exports and every event delivery, replay and snapshot. An added member can read the existing history. A member who leaves loses access to all of it, including an export requested earlier.
- All active members must be mutually reachable by the contact rule of ADR 0307. Creation and additions require this rule. A loss of contact between any active members makes the group unavailable until contact returns or the affected membership ends. An existing member can leave an unavailable group. Inactive and deleted accounts keep their attribution and do not prevent the remaining active members from exchanging messages.
- Former membership is kept separately for attribution. Current members can read the departed profile of a former author; former members gain no group access from that record. Group departure events are recorded with their group scope and recheck current group access at delivery and replay. The workspace, pair and moderator audiences retain ADR 0121.
- Batched event visibility reads collect candidate group identifiers in the existing standing query. Actor and contact checks run only for accounts with candidates, keeping ordinary channel and pair delivery within its statement bound.
- The client withdraws group history, named excerpts, notices and unread summaries when membership or contact access ends, and invalidates earlier reads before it loads another snapshot. Group member menus capture the conversation, member and account owner; a changed membership or account ends the menu.
- Group calls use the existing admission, device-seat, invitation and one-call-per-account rules. Joining rings the other current active members, respecting quiet time, direct mutes and the recipient's group mute. A group mute is an account-scoped notification setting keyed by the conversation and suppresses message notices and future rings. Leaving the group marks its room for an admission check. Group calls have no meeting mode or recording. Pair-call ring outcomes and lifetime remain specific to pairs.

## Consequences

- An administrator cannot read, search, export or moderate a direct message through the application.
- The protection is a rule of the application. A person with access to the database or the disk of the server can read direct messages.
- A recovery gives a new device access to an account's direct messages. For this reason only the server owner can perform it, and each recovery is recorded and shown to the account (ADR 0104).
- A direct message has no workspace, so a call in a direct message has no recording and no meeting mode (ADR 0501).
- Moderation by report and opt-in compliance access to direct messages are in the later list of `docs/scope.md`.
