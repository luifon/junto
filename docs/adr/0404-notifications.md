# 0404. Unread signs and notifications

## Status

Accepted

## Context

A member of several channels, workspaces and servers receives more messages than need attention. The client must show where new content is and must interrupt the user only for content that the user selected. A user works on several devices, so the read position cannot be stored on one device. Silencing one channel, lowering its notification level and silencing the whole application for a time are three different needs.

## Decision

Unread signs:

- The server stores the last-read position per user and channel.
- A channel with new matching content shows an unread sign.
- Mentions show a sign with their number.

Notification level:

- Each channel has a level: all messages, mentions only, or nothing.
- An administrator sets the workspace default. A user can set a personal default for the workspace. A channel setting overrides both.
- A mention of the user always counts as a mention.
- Mentions of a role and mentions of everyone in the channel can be switched off per channel.

Mute:

- Mute is separate from the level.
- A muted channel sends no notification and shows no unread sign. Only the weight and the colour of its name change.
- A mute lasts 1 hour, 8 hours, until the next day, or until it is turned off.

Do not disturb:

- Do not disturb applies to the whole application. The client shows no operating-system notifications, and unread signs still update.
- It is switched by hand or on a daily schedule.

Direct messages, call invites and threads:

- Direct messages and call invites notify unless do not disturb is on or the conversation is muted.
- The mute of a direct-message conversation is stored on the server. While it lasts, the server makes no call invitation for the account that muted (ADR 0515).
- A reply in a thread notifies the people who wrote in that thread.

## Consequences

- The read position is the same on every device of a user.
- The level and the mute of a channel are stored separately, so ending a mute returns the channel to its level.
- Do not disturb also stops notifications for direct messages and call invites.
- Servers that are not selected deliver direct messages, mentions and call invites to the client, so their signs and notifications work without the full event stream (ADR 0107).
- A dock or tray badge is in the later list of `docs/scope.md`.
- Notification sounds and join sounds are not fixed by this record.
