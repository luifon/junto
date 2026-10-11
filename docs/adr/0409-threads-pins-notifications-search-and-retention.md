# 0409. Threads, pins, notifications, search and message retention

## Status

Accepted

## Context

ADRs 0402 to 0407 define the rules for discussion history and finding content. Their APIs,
copy ownership, notification precedence, pagination and retention execution need explicit
contracts. Stored events must not retain deleted text or reveal inaccessible content.

## Decision

Threads and forks:

- `GET` and `POST /api/v1/messages/{id}/thread` read and append thread replies. An authored
  channel message is the root. A deleted root remains as a marker while replies exist.
  Threads cannot nest and system messages cannot be roots. Direct conversations retain
  ordinary replies. Thread replies stay out of channel history; “also send to channel”
  remains deferred by ADR 0403. Thread reads use the history limits and exclusive cursors.
- A message carries its thread root, live reply count, latest reply identifier and pin flag.
  A reply creation or deletion sends a change notice for its root. Editing a reply uses the
  existing author and permanence rules. A thread page also lists readable fork channels.
  Reply count and latest reply are maintained when replies change, rather than counted on reads.
- Root authors, reply authors and explicitly named recipients follow a thread. Role and
  everyone mentions produce notices and unread mention counts without creating follower
  rows. Their recipients can read the thread without following it; later unnamed replies
  do not count as unread for them. `GET` and `PUT /api/v1/messages/{id}/thread/read` require
  current channel access and either thread participation or a received thread mention.
  Replies never count as unread channel messages. Their unread mentions contribute to the
  channel mention number until read in the thread. Account-only `thread_read` events
  synchronize read positions; ordinary and thread mention counts use separate indexed ranges.
- The snapshot contains the newest 200 unread threads, with `threads_more` and
  `threads_next`. `GET /api/v1/threads/unread?before=...&limit=...` continues that list,
  with a maximum of 200 and a default of 200. Only currently readable channels contribute.
  Ordering uses decreasing newest unread reply or mention identifier, then root identifier.
  The continuation stores both keys; each request reevaluates unread state and access.
  Followers use an index over cached latest reply positions. Other recipients use an
  indexed marker on their newest unread mention, with zero unread replies. Reading, editing
  and deleting mentions maintain that marker; no additional follower row is created.
- `POST /api/v1/messages/{id}/fork` creates a text or voice channel. It requires
  `manage_channel` in both the source channel and its workspace. The transaction copies
  the category and every channel override before publishing the new channel. It copies the
  root and every reply and attachment in batches of 200 within one transaction. A thread
  has one fork; repeated or concurrent requests return conflict after the first succeeds.
  Deleting the destination removes the fork marker in the same transaction and permits
  another fork. Archiving keeps the marker and refusal. The occupant-hiding setting is
  copied only to a voice destination. The new channel receives the next workspace position, as ordinary channel creation does.
- Copies retain authors, creation/edit times, documents, attachments and permanence, with
  fresh identifiers and destination positions. Reply references among copied messages are
  remapped. Reactions, pins and mention notifications are not copied. The destination
  inherits the source permanence setting and counts copied permanent messages. Copies
  change independently. The source thread retains a link to the destination. A channel read
  floor excludes the copied history from unread counts, including for later members. Mention
  nodes and their recipient history are copied so an edit does not repeat unchanged mentions.
- Attachment copies have independent metadata and chunk hashes over shared immutable file
  content. Each metadata row stores its content directory identifier. Cleanup removes that
  directory only when no live attachment uses it. Copying performs no file I/O inside a
  retried transaction. This replaces the possible hard-link approach described in ADR 0408.

Permanence and pins:

- Permanence continues to be a channel setting, not an individually reversible mark.
  Existing per-message flags, owner-only channel deletion, audit lines and counters apply
  to thread replies and fork copies too. No unmark endpoint weakens a permanent message.
- Each channel has at most 100 pins. Pin and unpin require `manage_messages`, are idempotent,
  and operate on authored messages, including permanent messages and thread replies.
  System messages cannot be pinned. Deletion removes a pin; edits change what a pin shows.
  The pin list returns current messages. Changes use `message_changed`; no-ops emit nothing.

Notifications:

- Notification policy resolves channel preference, personal workspace preference,
  administrator workspace default, personal account/server default, then mentions-only.
  Account defaults are local to this server. The nil scope UUID names them; workspace and
  channel identifiers name their respective scopes. A PUT replaces a policy, and an absent
  level inherits. Role and everyone mention switches default on and are channel-specific.
- Channel mute is separate from level. A supplied expiry represents one hour, eight hours,
  or the client's next-day boundary. An absent expiry means indefinite. Expiry is evaluated
  using the server clock, without waiting for maintenance. A false mute ends it.
- Manual do-not-disturb and a daily interval with UTC offset are synchronized account data.
  Equal daily interval endpoints are invalid. The server suppresses notification, mention and call-invitation delivery while quiet,
  including replay, and creates no direct-call invitation for a quiet recipient. Durable
  notice data, message events and unread signs continue updating. Clients also apply the
  synchronized policy to OS notifications for already received content. Notices created
  while quiet do not become delayed alerts when the quiet period ends. Read-state changes
  still synchronize while quiet. Direct-message notices use the existing
  conversation mute of ADR 0515. Call invitations retain their existing events and snapshot.
- Ordinary messages notify at level all. Mentions notify at all or mentions-only, subject
  to the role/everyone switches. Explicit account mentions do not use those switches.
  Thread replies notify prior writers, including the root author, unless muted or at level
  nothing. Direct messages notify the other participant unless the conversation is muted.
  Senders do not receive their own notices. System lines create no message notifications.
- There is one durable notice per recipient and message. Its identifier is the message
  identifier within that account's notice collection. It contains no message text.
  `notification_changed` prompts a refresh of the caller's notices. Ordinary-message
  and thread notices reach full connections; only mention and direct notices reach background.
  Read state is idempotent and shared across devices. `notification_settings_changed`
  refreshes policy at every connection level. Advancing a channel, conversation or thread
  read position marks its notices read in the same transaction. Maintenance keeps at most
  500 notices per account and removes notices older than 90 days in bounded batches.
  Inserts queue their recipients for cap maintenance; unchanged accounts are not scanned.
  The ordered account index serves the cap, and the age index serves lifetime removal.
  Unknown and foreign notification cursors are invalid requests.
- Each event records the recipients newly notified by that change. An edit adding a
  mention can upgrade an existing general or thread notice once, reset its read state and move it
  to the front of the notice list. Pages order by notice creation time and identifier.
  Unchanged mentions do not notify accounts that joined a mentioned role after the send.
- Notice reads, snapshots, delivery and replay check current channel access, direct
  participation/contact, current mention membership/text and mute policy. Deleting content
  removes its notices. No notice opens another person's direct conversation to an admin.
  Creation first selects mentioned accounts, prior writers, direct peers and possible
  all-message subscribers; it checks current access before evaluating their policy or
  writing notices. Account-level all-message candidates must belong to the source workspace.
  Delivery reads the event's receiver list before authorizing notice content, so unrelated
  connections do not perform those authorization queries. Default mentions-only sends do
  not evaluate policy for every member.
- Raw unread and mention counts preserve ADR 0408. Additional `unread_sign`, `muted` and
  `notification_mentions` fields apply policy without losing the raw read position or counts.
  A muted channel has no sign or notification mention count. Role/everyone switches affect
  those signs while explicit mentions continue to count.

Search:

- `GET /api/v1/search` uses PostgreSQL full-text search with the `simple` configuration and
  web-search query syntax. Queries have 1 to 1,024 bytes, pages 1 to 200 results (default 50).
  Search includes authored messages, thread replies, reachable direct messages and ready or
  partial transcript lines. Messages extract each text node once. Transcript lines store
  current searchable text and an indexed vector; corrections replace that text atomically.
  Waiting, running and failed transcripts are excluded; NUL query bytes are invalid requests.
- Optional filters name a channel, an author, an inclusive RFC 3339 `from` time and an
  exclusive `until` time. Transcript authors are the recorded speaker accounts; transcript
  dates are recording request times. Results contain plain text, never rendered HTML.
- Order is decreasing PostgreSQL cover-density rank, identifier, and transcript line number.
  The opaque continuation value records these keys. A subsequent page reevaluates current
  content and permissions; it is not a retained snapshot of deleted or inaccessible text.
- Channel authorization uses the common permission function before selecting results.
  Direct places use current participation and contact checks. Private system lines are not
  searchable. Transcript visibility follows the recording route, including hidden active
  call occupants. One batched standing query uses the shared permission function across
  workspaces; delivery authorizes only its bounded candidate places. Statement count is
  independent of page size and workspace count; snapshot consumers share the same standing.

Retention and execution:

- Workspace attachment retention and server direct-message retention accept days from 1
  to 36,500, or null for never (the default). Reading or changing retention requires
  administrator; changes create audit entries and `retention_changed` events. Workspace
  notification defaults also require administrator and create audit entries.
- Maintenance removes due nonpermanent channel attachments without removing their messages.
  It preserves permanent attachments and marks surviving messages with
  `files_removed_by_retention`. Direct retention erases due content, references it
  with a deleted marker when replies need one, and removes its attachments and notices.
  Each transaction processes at most the configured maintenance batch for one place.
  An identifier continuation advances through places without globally sorting their backlog. Removal becomes
  effective through the API before the existing file-cleanup pass removes bytes.
  File and direct retention seek due rows by place and age indexes. Workspace recording
  retention has the same days range and default, with `GET` and `PUT` on
  `/api/v1/workspaces/{id}/recording-retention`. It seeks finished recordings by workspace
  and age and calls the same removal function as manual deletion, including file cleanup,
  transcript removal, events and an audit entry without a human actor. Active and processing
  recordings are retained until final; ready, partly_failed and failed recordings are due
  under the same indexed age rule. Recording policy changes have their own audit action.
- New content mutations share the message-write account limiter, and history/search/notice
  reads share the history limiter. Database transactions and event positions follow ADRs
  0009 to 0011. The snapshot carries current notices and personal notification policies.
- Archive/export APIs remain separate work. No scheduled
  archive deletion or export notice is produced before those features exist.

Notification delivery reads current visibility in one batched statement. Its join to
server state explicitly limits that singleton to one row, so PostgreSQL does not estimate
correlated authorization subqueries for nonexistent state rows and compile a large JIT
plan for each read. Quiet policy is read only for batches containing mentions, notices or call invitations. The authorization-validity window remains the production default in general tests.

The unread indexes include the place identifier so that excluding unreadable places does
not require heap reads. Mention rows remain stored for their original recipients; gaining
access later retains the existing unread-mention behavior. Thread-root mention lookups
and cascading deletion have a root/account index; head promotion seeks the newest unread
position for one recipient. Retention continues inclusively from the place just processed,
so full batches do not re-probe earlier places.

An account authorized to fork a thread can learn that its single fork already exists,
even after losing access to the destination. The repeated request returns 409 with no
identifier, name or membership of the destination; thread reads omit unreadable fork links.

## Consequences

- A channel fork cannot widen access at creation. Subsequent independent permission changes
  affect the destination as they affect any other channel; unreadable fork links are hidden.
- Files shared by copies are retained until the last live copy is removed. Content deletion
  does not make another copy unavailable.
- Search needs no service outside PostgreSQL and cannot preserve an earlier edit in an index.
- Clients fetch current notice data and render plain text. OS notification policy stays in
  the client while server policy and read state remain consistent across devices.
