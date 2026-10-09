# 0102. Several workspaces per server and the default workspace

## Status

Accepted

## Context

One server can serve several groups that need separate channels, roles and settings. Running one server per group multiplies the hosting work. A server with no workspace and no channel cannot be used until an administrator creates them.

## Decision

- A server hosts several workspaces and starts with one.
- The first start of a server creates a default workspace with one text channel and one voice channel, so that a new server is usable.
- The default workspace can be renamed.
- An account belongs to the server. Membership is per workspace.
- Creating a workspace needs the "create workspace" server permission (ADR 0301).

## Consequences

- One account can be a member of several workspaces of a server, with different workspace roles in each.
- Functions that are not inside one workspace are held at server level: accounts, invites, approvals, bans, the server audit log (ADR 0301) and direct messages (ADR 0302).
- Settings that differ between groups are per workspace: workspace roles, the glossary (ADR 0603), retention and the archive period (ADR 0406), and the notification default (ADR 0404).
- An invite code or an approval that names no workspace grants the default workspace (ADR 0105).
