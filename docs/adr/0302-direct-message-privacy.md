# 0302. Direct messages and their privacy

## Status

Accepted

## Context

A direct message is a conversation between two people and belongs to no workspace, so workspace roles and channel permissions do not apply to it. The administrator permission passes every channel check (ADR 0301). The server has no end-to-end encryption (ADR 0002), so the content of a direct message is readable by the server and by the person who operates it.

## Decision

Scope:

- A direct message is one-to-one.
- A direct message exists inside one server and is shared across that server's workspaces.
- Direct messages are not deleted automatically by default. One server-level retention setting applies to them.
- Each participant can export their own direct messages.

Privacy:

- Direct messages are closed to everyone except their two participants. This check runs before the administrator permission (ADR 0301).
- There is no administrator read feature and no administrator export for direct messages.
- The join screen and the documentation state that the person who operates a server has technical access to its data.

## Consequences

- An administrator cannot read, search, export or moderate a direct message through the application.
- The protection is a rule of the application. A person with access to the database or the disk of the server can read direct messages.
- A recovery gives a new device access to an account's direct messages. For this reason only the server owner can perform it, and each recovery is recorded and shown to the account (ADR 0104).
- A direct message has no workspace, so a call in a direct message has no recording and no meeting mode (ADR 0501).
- Group direct messages, moderation by report and opt-in compliance access to direct messages are in the later list of `docs/scope.md`.
