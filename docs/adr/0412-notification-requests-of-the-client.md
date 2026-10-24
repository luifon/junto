# 0412. Notices read in one request, the choice to follow a thread, a default that is not set, and the quiet time by time zone

## Status

Accepted

## Context

ADR 0404 and ADR 0409 define notices, levels and the quiet time. Four parts did not serve a client. A client marked each notice read with its own request, against the limit of message changes. ADR 0404 names threads among the places of a notification, and the server had no setting for one thread: an account that wrote one reply was notified of every later reply. The route of the notification default of a workspace answered "mentions only" also when no default was stored, so a client could not show that none was set, and nothing removed a default. The daily quiet time stored a distance from UTC, so it moved by one hour on the clock of the person at each change for daylight saving time.

## Decision

Every notice in one request:

- `PUT /api/v1/notifications/read` marks every unread notice that the caller reads now and that was created at or before `until`. The body states `until`, the time of creation of the newest notice that the person saw; without it, and for a later time, the time of the request is used. A notice that arrived later stays unread.
- The request states a time and no notice identifier, because the identifier of a notice is the identifier of its message (ADR 0409).
- A notice that no list returns to the account now, because the account cannot read its place or muted it (ADR 0409), stays as it is. The number that the answer states and the event therefore tell nothing of such a place.
- One statement marks them. It reads the unread notices of the account through their index and decides each one as the list does. An account has at most 500 notices after each maintenance run (ADR 0409).
- When a notice was marked, the account gets `notifications_read` with `until` and `read_at` at every connection level. The HTTP response also returns the number changed by the request.
- The request counts against the limit of message changes (ADR 0408).

The choice to follow a thread:

- An account has one of three states for a thread: no choice, follow, do not follow.
- Without a choice the rule of ADR 0409 holds: the root author, the authors of replies and the accounts that a reply names follow the thread.
- "Do not follow": the replies of the thread do not count as unread for the account and create no thread notice for it. A mention of the account in the thread still notifies and still counts (ADR 0404).
- "Follow": the replies count as unread and create thread notices for the account, also when it never wrote in the thread. The account follows from the moment of the choice: the replies before it are read.
- A choice stays when the account writes in the thread or is named in it afterwards. Only the account changes or removes it. Removing the choice returns to the rule; an account that follows again by the rule has read the replies up to that moment.
- `PUT /api/v1/messages/{id}/thread/follow` with `following` stores the choice and `DELETE` on the same path removes it. Both need what the read position of a thread needs: the caller sees the channel, and the message is an authored root (ADR 0409).
- The state of a thread carries `following` and `follow_setting`. A change records `thread_follow_changed` for the account, at every connection level.
- A reply that makes an account follow a thread by the rule also records `thread_follow_changed` for that account, with `following` true and no choice: the author of a first reply, the root author at the first reply of the thread, and an account that a reply or the edit of a reply names. The server compares the followers among these accounts before and after the reply in the same transaction, so a second reply and a named account that chose not to follow record nothing. The event is delivered only while the account may see the channel of the thread (`view_channel`), as the audience of an event about a place is (ADR 0510): a reply can name a member of the workspace who does not see the channel, and the root author can have lost the channel before the first reply; they follow by the rule and are told nothing. Without the event the other connections of the account, and the follow control of a window that shows the thread, keep the state from before the reply.

A default that is not set:

- `GET /api/v1/workspaces/{id}/notification-default` returns the stored level, or `level: null` when the workspace has none. The snapshot carries the same value for each workspace.
- The path names a workspace that the caller sees; another identifier gets `not_found`.
- `PUT` sets a level and refuses a body without one. `DELETE` on the same path removes the default; it needs the administrator permission, as `PUT` does, and records the audit action `notification_default_removed`.
- A change records `notification_default_changed` with the workspace and the level, or without a level after a removal, for the accounts that see the workspace, at every connection level. The event `notification_settings_changed` is no longer recorded for this change; it stays the event of the own settings of an account.
- The order of ADR 0409 does not change: without a default of the workspace, the default of the account applies, then "mentions only".

The quiet time by time zone:

- A daily quiet time states its start, its end and `time_zone`, the name of a zone of the IANA time zone database. The field `utc_offset_minutes` no longer exists.
- The server accepts a name that its database lists as a time zone (`pg_timezone_names`). Another text gets `invalid_request`, also a text that PostgreSQL would read as a rule of its own, such as `XYZ3`.
- The quiet time holds while the clock of the zone shows a time inside the window. On the day on which the clock skips an hour, a window inside that hour does not occur. On the day on which the clock repeats an hour, a window inside that hour occurs twice.
- A stored zone that the database does not know, after a restore onto a database with other zone data, gives no daily quiet time: the account is notified. Do-not-disturb by hand does not depend on the zone. The statement that asks does not fail.
- The migration turns each stored distance from UTC into the same window on the clock of `UTC`: start and end minus the distance. The quiet time then holds at the same instants as before. A client that knows the zone of the person stores it at the next change.

## Consequences

- A client marks all notices read with one request and one event.
- A person leaves a thread without leaving its channel, and follows a thread without writing in it.
- A reply in a thread costs a read of its root author, and a read of the follow state of at most its author, the root author and the accounts that it names, before and after it. An edit of a reply costs the read of the follow state of the accounts that it names, before and after it.
- A client shows "not set" for the default of a workspace and offers its removal.
- A quiet time from 22:00 to 07:00 stays at these hours of the clock of the person through the year.
- A quiet time that the migration converted shows hours of UTC until the person sets it again.
- A client of the earlier form that sends `utc_offset_minutes` gets `invalid_request`.
- The check of the quiet time is a database function with a handler for an unknown zone. It cannot be made part of the statement that calls it, as the earlier form could.

The event `notifications_read` retains `until` and `read_at` in API version 1. The server stores the newest account read boundary and applies it when creating a notice whose timestamp is at or before that boundary, including a transaction that commits after the mark. Notice insertion reads that indexed boundary in the serializable transaction, so concurrent marks cause a retry where needed. Existing hidden notices are unchanged by mark-all-read, and creation still requires current visibility and notification policy. The HTTP count names rows changed by that request, not notices created by another transaction afterwards.

The boundary uses the server timestamp captured at request start, not commit order. If that clock moves backward after a mark, notices timestamped within the already-read interval are read even when created later in real time. An older mark never moves the stored boundary backward. Concurrent notice creation and marks can retry under serializable isolation; the request retains its original timestamp on retry.
