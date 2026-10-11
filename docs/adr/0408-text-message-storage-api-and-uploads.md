# 0408. Message documents, history positions and resumable attachments

## Status

Accepted

## Context

ADR 0401 defines messages but leaves their stored format, pagination, upload protocol and
limits open. A message can be edited without keeping its earlier text. Storing whole
messages in the replay log would preserve earlier text after an edit or deletion. The
contact rule leaves access to earlier direct messages to this unit (ADR 0307).

## Decision

Messages and history:

- Text channels and voice-channel chat have the same message API. A direct conversation
  has exactly two accounts, one row per unordered pair, and no workspace.
- Reading, writing, reacting, uploading, downloading, read positions, snapshots and event
  delivery for a direct conversation require participation and the current contact rule.
  Earlier history is unavailable while that rule fails; it becomes available again when
  the rule holds. Administrator permission never opens somebody else's conversation.
- Opening a conversation records `conversation_opened` for its two participants.
- Message identifiers are UUID v7. Each channel or conversation has a separate rising
  integer public position. A public writer increments its place's row in the transaction
  that inserts the message. A private system line shares the latest public position and
  does not advance it. Its row lock lasts through commit, so a later position cannot commit first.
  Deletion does not reuse a position. The event cursor remains separate (ADR 0010).
- History accepts one exclusive `before` or `after` position and `limit` from 1 to 200,
  default 50. Alternatively, `before_message` or `after_message` names a visible message
  in that place and compares its position and identifier together. These cursors traverse
  private lines that share a public position. Without a cursor the latest page is returned.
  Pages ascend by position and identifier and state whether more messages exist in the
  requested direction. An invisible cursor returns `not_found`.
- A reply names a message in the same place. Removing a root with replies keeps an empty
  deleted marker; the replies and their references remain. A reader who cannot see the
  target receives no target identifier. A separate nullable thread-root
  column allows thread-only replies later (ADR 0403).
- The stored permanence flag and channel count follow ADR 0402 even though pins, threads,
  retention and the permanence interface are separate work. Permanent deletion records an
  audit entry and a system line.

Documents:

- Text is a typed JSON document, not HTML or Markdown. Blocks are paragraphs, quotes, code
  with an optional language, ordered or unordered lists, and checkbox lists. Inline nodes
  are text with bold, italic and strikethrough marks, code, links, and explicit account,
  workspace-role or everyone mentions. Checkbox changes are author edits.
- Documents contain at most 16 KiB of compact JSON, 100 blocks, 1,000 inline nodes and 100
  distinct mentions. Text refuses control characters except tab and newline. Unknown node
  kinds, unknown fields and repeated marks are refused. Links use HTTP, HTTPS or mailto,
  have no credentials or controls, and contain at most 2,048 bytes.
- An empty document needs an attachment. Account mentions in a channel name active
  workspace members; role mentions name roles of that workspace. Everyone mentions need
  `mention_everyone`, including a mention of the built-in everyone role. Direct conversations mention only their two participants.
- Reactions use the canonical spelling of Unicode emoji recognized by the `emojis` table.
  One account has one reaction per emoji per message, with at most 20 distinct emoji on a
  message. Adding and removing a reaction is idempotent. Responses give counts and the
  caller's own flag.
- Message changes and conversation opens share a per-account limit of 120 attempts per
  minute, using the bounded limiter of the server. Read-position changes share this limit.
  History reads have a separate limit of 120 per account per minute. Attachment mutations
  have a separate limit of 1,200 per session token per minute. Serializable retries count
  once.

Read positions and events:

- The last-read position is per account and place, moves forward only and cannot pass the
  latest position visible to that account. The snapshot supplies place, latest visible
  position, last-read position, unread count and mention count. An author's own messages
  and all system lines do not count unread. Sending a message, including a reply or direct
  message, advances the author's read position to that message in the send transaction;
  earlier messages therefore become read. Failed sends advance nothing. Counts are exact through 99 and capped at 100,
  meaning 100 or more. Indexed reads start after the read position; latest public position
  and bounded counts are loaded together without scanning earlier history. Mention counts
  read an index beginning with place and mentioned account, then position. Role and everyone
  recipients are fixed when the message is written or the mention is added by an edit.
  Later role membership changes neither add nor remove unread mentions; edits retaining
  the node retain its recipients. Removing the node removes those unread mentions.
  State is returned only while the account can currently read the place. Notification
  delivery and replay still check current roles, current mentions and current visibility.
- `message_changed` names the place, message and position and states deletion. Clients
  fetch current content or remove the deleted message. No message text or earlier edit is
  retained in events. Reactions use the same notice. Large messages need no event-size
  exception. The notice also makes the client refresh unread state for its place.
- `message_read` goes to the account's devices at every subscription level while the place
  remains accessible. Direct-message notices reach background connections.
- `message_mentioned` reaches background connections only if the recipient is currently
  mentioned in the current message and can read its channel. Role membership is checked at
  delivery and replay; editing or deleting the mention suppresses queued notices.
- A recipient is notified once when a message first mentions them. Edits validate only
  added mention nodes and notify only added recipients; reactions never create mentions.
  Existing nodes remain editable when an account leaves or a role is removed. One mention
  event stores its eligible recipients in a separate indexed table, inserted in one
  statement and removed with the event. Event size and statement count do not grow per
  recipient. Conversation lists and snapshots batch contact checks.
- All events commit with their change and are authorized again at delivery (ADR 0011).

System messages:

- Channel audit actions become immutable system messages with their action, actor and safe
  subject account, without audit entries or permission details. Call notes become system
  messages in the same transaction. Existing call notes and channel audit entries are
  converted by the migration. Recording start and stop use their notes only, avoiding
  duplicate lines from their audit entries.
- Each system line fixes its audience when written. A public line stays public. A line
  written while occupants are hidden is visible only to the accounts inside at that
  instant, including after the call ends. A later join does not grant earlier lines.
  Setting changes affect subsequent lines in either direction; they never change earlier
  positions, counts or audiences. This persistent-message rule is separate from dynamic
  delivery of live call events (ADR 0514). A hidden call line written with no participants,
  including `call_ended`, the recording post and later lines, is visible to every account
  that participated at any time while occupants were hidden. `call_message_members`
  stores that set; public-only participants are excluded. Ended calls keep their final
  privacy setting, so a later call or setting cannot alter their history.
  Historical settings and admission evidence determine migrated lines; missing evidence
  grants no private access. Reaction and deletion events retain the line's audience.
- The finished recording post refers to the recording identifier; its video, audio and
  transcript use the existing authorized routes. Deleting the post uses
  `manage_recordings` and removes the recording. Removing the recording removes the post.
  Other system lines stay, with an absent recording reference when the recording has gone.
- Deleting a channel removes its messages and call notes and queues attachment removal.

Attachments:

- `JUNTO_ATTACHMENTS_DIR` names a dedicated existing absolute directory. Limits are
  `JUNTO_ATTACHMENT_BYTES` (default 100,000,000, range 1 to 1,073,741,824) and
  `JUNTO_ATTACHMENTS_PER_MESSAGE` (default 10, range 1 to 100). Uploads cannot start
  without configured storage and return `unavailable`. An administrator can replace the limits through `PUT
  /api/v1/attachment-limits`; authenticated clients read them with GET. Database overrides
  survive a restart and record an audit entry and `attachment_limits_changed` for every
  active account. Lowering the byte limit affects new uploads; lowering the
  attachment-count limit affects subsequent sends. The storage guard reserves the whole
  declared file size and shares the reservation table with recordings; chunks also stop at
  the free-space reserve.
- An upload belongs to its author and one place. It declares a basename of 1 to 200
  characters, byte length and lowercase SHA-256. Paths, control characters, invisible
  formatting and directional controls are refused. An account has at most 20 unattached
  uploads. They expire after 24 hours.
- Chunks contain 1 MiB except the final chunk. A request names its byte offset. Only the
  next offset is accepted. Repeating identical bytes is idempotent; different bytes at
  that offset are refused. The upload status gives the durable received length.
- Each chunk has a durable intent before any file write. The server writes and syncs a new
  staging file, links it to the immutable chunk name, and records completion after
  rechecking the session and access. All paths use server identifiers and offsets, never
  the supplied filename. Opens and writes refuse symbolic links using the recording-file
  helpers. A crash before completion allows the same chunk to be sent again.
- Completion reads and verifies all chunks and the declared hash in a background job that
  survives a request deadline; status polling and retries reuse completed verification.
  A hash mismatch is recorded durably and releases the reservation; later completion
  attempts return that failure without reading or hashing the file again.
  Still PNG, JPEG and WebP files up to 8 MiB with dimensions up to 1,024 per side get
  preview metadata using the avatar header checker. Other files remain downloads with
  `application/octet-stream`. The server does not decode images. Clients treat attachments
  as untrusted files.
- Sending claims complete uploads once, in the message transaction. Upload ownership,
  place and expiry are checked again. An attachment cannot be reused by another message.
- Downloads require a session and current access to the message, use `nosniff`, `no-
  store`, an attachment disposition and the verified hash as ETag. Uploads have four
  places with at most two per account, separate from eight download places with at most
  two per account. Downloads use
  the recording download's single-range parser and bounded producer; a stalled producer
  releases capacity after the configured download stall interval. Each 1 MiB chunk is
  verified before transmission.
- Cancellation, expiry, message deletion and channel deletion make files immediately
  unavailable through the API. Maintenance removes their directories and releases any
  remaining reservation in bounded batches. Deletion queues removal in its own transaction;
  the expiry query uses the partial expiry index without scanning live attachments. It waits at least one minute after the last
  chunk intent, beyond the request deadline, before removing a directory. Partial staging
  files go with the upload directory; completed uploads also have abandoned staging files
  removed after this delay. File operations run outside the async runtime. Active upload
  work delays cleanup; a failed item is deferred so later items can be processed.

Read cost and future copies:

- History loads rows, reactions and attachments in batches independent of page size.
  Snapshot summaries aggregate authorized channels in one statement, reusing the structure
  already authorized for the snapshot.
- A future fork can duplicate attachment metadata and hard-link immutable chunks into a
  new attachment directory. Removing either copy leaves the other intact; editing or
  removing copied messages requires no shared mutable attachment ownership.

## Consequences

- An old direct conversation is preserved but inaccessible during loss of contact.
- A client renders document nodes and escapes their text; the API sends no rendered HTML.
- Message notices may require an additional request, and always yield current content.
- Unread summaries count messages up to the cap; notification levels and channel mute
  policy are later work.
- Attachment backups include the configured directory and database metadata together.
- Files are inaccessible immediately after deletion and removed from disk by maintenance.