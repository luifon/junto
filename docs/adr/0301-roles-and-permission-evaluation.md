# 0301. Role and person permissions at each level

## Status

Accepted

## Context

A server has several workspaces. Each workspace needs independent roles, category defaults,
channel exceptions and exceptions for individual members. The API, events, downloads, search,
exports and calls must use the same evaluation. Administration cannot open direct messages or
change permanent content. Permissions follow Discord's override order, with Junto's separate
server and workspace scopes and existing grant ceilings.

## Decision

Server permissions are administrator, create workspace, create invites, approve join requests,
ban members and view server audit log. Server roles grant these permissions. A person can have
an allow and deny set on the server; evaluation combines roles, removes person denies and adds
person allows. The owner bypasses this result. Only the owner can remove an administrator role's
administrator pass with a person deny. Direct grants do not create a role position (ADR 0309).

Workspace permissions retain their existing bits: view channel, manage roles, manage channel,
manage messages, mention everyone, manage glossary, record calls, manage recordings, export
channel, move members, mute members, deafen members, disconnect members, lock room, run meeting
mode, view audit entries and admit to full call (bits 0–16).

Bits 17–30 are, in order: send messages, send messages in threads, create threads, read message
history, attach files, add reactions, mention roles, connect, speak, video, share screen, use
voice activity, set call user limit and set voice channel status. No route checks set voice
channel status yet; the permission is stored, shown and evaluated. Camera and screen publication
have separate permissions; screen sound requires both speak and share screen, because the server cannot tell screen sound from a voice (ADR 0510).

Everyone and workspace roles grant workspace permissions. A workspace person override applies
after their union: deny then allow. Category and channel overrides exist for everyone, roles and
people, using disjoint allow and deny sets. Neutral removes an override for that permission.
Workspace administration checks workspace scope; a channel grant cannot grant workspace authority.

A channel uses its category's overrides while synced, and otherwise its own complete set.
The chosen set applies in this order: everyone deny and allow, the union of held role denies,
the union of held role allows, then person deny and allow. Role positions do not change this
order. Categories use the same order. Overrides do not grant workspace membership.

A new channel is synced. Its first effective override change copies the category set and detaches
it atomically; a no-op does not detach. Moving a synced channel selects the new category's set.
An unsynced channel keeps its set. Sync again replaces the detached set after checking the grant
ceiling. Category deletion preserves the effective sets of synced children before clearing their
category, preventing deletion from exposing their contents.

Every channel action requires view channel. Top-level sends and thread replies have distinct
permissions; a thread inherits its channel's permissions. Attachments and mentions require the
send permission of their destination as well as their own permission. Reading stored messages,
threads, pins, attached files, search results and exports requires read message history. Without
that permission, a single message read requires creation after the start of the caller's current
live stream connection and current channel visibility. The server records the connection start;
replay grants no older message. Reconnect establishes a new start while the client retains
messages already held. Search, pins, exports, file lists and thread history never use this
exception. The time bound requires no stored list of message grants. Channel references respect
visibility; @everyone requires mention everyone. Invite links use create invites.
The voice user limit has its own permission; other channel settings retain manage channel.

Connecting requires view channel and connect. Microphone, camera and screen publication each
require connect and their corresponding permission; use voice activity also requires connect. Recording retains record calls and the
in-call rules. The media server restricts track sources. Without use voice activity the supported
client forces push-to-talk; a media server cannot prove that audio came from a physical key press.
Priority-speaker attenuation, link previews, external emoji, stickers, soundboards, webhooks,
application commands, scheduled events, polls and slow mode have no permissions because the
application does not implement these features. Existing message management covers pins and
thread management. Meeting mode retains Junto's hand and presenter rules.

Limits run before the administrator pass: direct conversations belong only to their participants;
nobody edits permanent messages or another author's message;
recovery, owner transfer and permanent-content deletion belong to the owner; owner protection,
self-moderation restrictions and access-token restrictions remain. Administrator then passes other
permission checks, including private channels. Role order still restricts administration actions.
An invitation or move cannot grant permission; entry still checks the destination permissions.

Default everyone grants include view channel, lock room, run meeting mode and the channel-use
permissions except set call user limit. Admit to full call, recording and other management
permissions remain ungranted by default. Migration 0042 preserves earlier access, copies the
old manage-channel authority into set call user limit, widens the stored bit range, and renews
the event epoch. ADR 0304 defines storage and ceilings; ADR 0310 defines effective snapshot lists.

## Consequences

The same person can have different access in separate workspaces, categories and channels,
without creating a role for each exception. Removing a deny can grant access and therefore
requires the same ceiling as adding an allow. A category is shown only with a visible channel
or category management authority. Hidden identifiers receive the same answer as absent ones.
The client uses the server's effective lists and withdraws content on permission changes
(ADRs 0748 and 0790). The server decides every request again in its transaction.
