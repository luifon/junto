# 0401. Messages and attachments

## Status

Accepted

## Context

Text channels, the text chat of each voice channel and direct messages carry messages with formatting and files. The rules for editing and deleting decide what the server must store and what other members can rely on. Attachments use disk space on the server, so their number and size need limits that the administrator of a server controls.

## Decision

Messages:

- A message can be a reply to another message.
- An author can edit their own message at any time. An edited message shows "edited". No edit history is kept.
- An author can delete their own message. Deleting another person's message needs "manage messages".
- A deleted message and its files are removed for everyone.

Mentions and reactions:

- A message can mention a person, a role, or everyone in the channel. Mentioning everyone in the channel needs the "mention everyone in a channel" permission.
- Reactions use standard emoji.

Formatting:

- Bold, italic, strikethrough, inline code, code blocks with syntax colours, quotes, lists, checkbox lists and links.
- Ticking a checkbox is an edit of the message, so only the author of the message can do it.

Attachments:

- A message has up to 10 files, and a file has up to 100 MB, by default. The server administrator can change both limits.
- An image shows a preview. Any other file shows as a row with its name and size.
- Uploads are sent in chunks and can resume (ADR 0006).

Pins:

- Each channel has pins.

Indicators:

- People show online, idle, do not disturb, or offline. The person can choose Online, Idle, Do not disturb, or Invisible per server account. Invisible is shown only to that account; everyone else sees offline.
- A connection of an authenticated session counts as online, including a background connection after the last window closes. An account is active while any valid connection is active, otherwise idle while any valid connection exists, otherwise offline. Expired sessions, removed devices and inactive accounts do not count, even before connection cleanup. Manual idle and do not disturb override activity. Invisible hides every connection. Reconnection keeps the manual choice. No last-seen time is stored or returned.
- `accounts.presence_mode` keeps online, idle or invisible. The existing account notification setting `do_not_disturb` supplies the do-not-disturb choice, so notification suppression and the public status use one value. `PUT /api/v1/account/presence` changes the mode and this bit, preserving daily quiet hours and the other notification settings. A per-account asynchronous gate covers each choice transaction through its registry visibility update, and stream registration rereads the committed choice under the same gate. Registration and choice writes therefore have one order, including when no connection existed at the write. The gate registry keeps weak references and prunes released gates. The existing notification settings can still change the bit. Daily quiet hours suppress notifications without changing the public status.
- `POST /api/v1/presence` reads at most 100 explicit account IDs and reports this session's idle flag. It requires an own-account session. It batches workspace acquaintance and session validation, and calls the same `authorize(ViewAccount)` policy as profiles for each account. Unknown, inaccessible, inactive and invisible accounts all answer offline. It returns the caller's own manual choice separately. A read is limited to 600 per minute per account, idle transitions to 60, and choice writes use the existing account-change limit.
- Event revision 11 adds optional `subscribe.presence` (default false) and the empty `presence_changed` control message. It requests a fresh presence read and carries no identity, activity, last-seen time or durable event position. Connection and idle changes of invisible accounts send no public invalidation. Notifications coalesce through a separate watch channel per connection. Successful presence reads register only authorized requested identities on connections of that exact session, with the owner always watched. Each connection retains at most 1024 recently requested identities and a changed set within that bound; a repeated inaccessible read removes its old watch. Changes of an unwatched identity wake no connection. Before a public invalidation is written, the connection rechecks current profile authorization in batches of at most 100, through its bounded database lane. Membership or role revocation therefore also cancels a stale watch. The thirty-second client refresh repairs evicted interest as well as missed hints. Older clients and clients that do not opt in receive no extra frames. A client enables the extension only when the server advertises revision 11 or later. Presence does not enter the durable event log or the snapshot.
- The desktop reads the explicitly shown people and its own account in batches of 100, with at most four requests across servers and one request per server. A rotating account cursor prevents frequent invalidations from starving later IDs. A 250 ms coalescing delay joins invalidations, and a 30 second refresh also repairs missed notifications. System inactivity is checked every five seconds. Failed reads retain the shown values and expose retry in the status dialog; the periodic refresh retries them. A failed choice keeps the saved choice and offers retry. A replaced account, server entry generation, permission change or snapshot cancels obsolete reads and clears their values. Closing lookup dialogs releases their interest. The decoded-avatar bound of ADR 0783 does not limit presence reads.
- The status dialog opens from the own profile strip and offers the four choices, read/save errors and retry, and an explanation when automatic idle is unavailable. Shared avatar and call-tile components carry the badge to members, messages, replies, threads, direct conversations, contact results, call participants, invitations, selectors, mentions, administration, search results, notices and transcripts. Audit actors with account IDs carry a badge beside their heading. Account mentions and system-line actor/subject names with account IDs use the same status shapes in text. Merged system lines retain the IDs of each shown name. Historical names without account IDs, unrecognized shared-audio speakers and profile presets have no presence; the capture owner's identity in a shared-audio source line does.
- Automatic idle starts after five minutes without input in the desktop session, including activity outside Junto. Native reads run on a separate bounded worker and never collect input content. macOS uses `CGEventSourceSecondsSinceLastEventType` for combined-session input; Windows uses `GetLastInputInfo` and wrapping `GetTickCount` arithmetic. Linux X11 uses the ScreenSaver extension's idle counter with one-second connection/query deadlines, including abstract and filesystem sockets. Wayland uses `ext-idle-notify-v1` per seat (version 2 input idle ignores inhibitors; version 1 follows the compositor's inhibitors), with Mutter's `org.gnome.Mutter.IdleMonitor.GetIdletime` as fallback. Without a supported source, Online stays active and the person can choose Idle manually. Dropping the reader requests worker shutdown without blocking the window.

Native API references: [Windows session input](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getlastinputinfo), [Wayland idle notification protocol](https://gitlab.freedesktop.org/wayland/wayland-protocols/-/blob/main/staging/ext-idle-notify/ext-idle-notify-v1.xml), and [Mutter idle interface](https://github.com/GNOME/mutter/blob/main/data/dbus-interfaces/org.gnome.Mutter.IdleMonitor.xml).
- Typing indicators are transient signals for a text or voice channel chat, a direct conversation, or one thread. The signal carries an account identifier and display name, never draft content.
- Clients watch at most eight open message places over their current full event subscription. Actual composer edits announce activity at most once every two seconds. Clearing, sending, previewing, leaving the input or closing its window stops that composer. Restoring a saved draft announces nothing. Several local composers and devices of one account aggregate; stopping one source leaves other active sources visible.
- A signal lasts at most eight seconds. The server bounds active entries and revision keys to 8192, broadcasts to a queue of 256 signals, accepts at most eight places per connection, and rates changes to eight per account per second. Pending watch acknowledgements coalesce to the latest set. Disconnected connections remove only their own sources. Closing a connection and publishing its authorized activity use the same registry guard, so an interrupted read cannot revive a closed source.
- Sender and recipient sessions and place permissions are read for each delivery. Typing requires an open writable place; a new thread also needs Create threads. A recipient needs current place access and the thread root must satisfy the same live-history boundary as messages. Other accounts receive the signal only for watched places; the viewing account is excluded. Queued obsolete signals and signals whose lifetime elapsed are dropped.
- Typing activity has no database row, persistent event, cursor, snapshot field, audit record or replay. Reconnection clears the client display and watches again. Expiry bounds a lost stop signal. Limits bound memory and database work independently of message persistence.

## Consequences

- The earlier text of an edited message cannot be recovered or shown.
- A checkbox list in a message cannot be ticked by other members.
- A deleted file is removed from disk, and a finished export that contains a deleted message or file is deleted (ADR 0406).
- Messages in a channel with the permanent-message setting follow the stricter rules of ADR 0402.
- The server refuses new uploads while free disk space is below the reserve (ADR 0006).
- Custom emoji, link previews, inline video and audio players, and message buttons are in the later list of `docs/scope.md`.
- The format of message identifiers is not fixed by this record.
