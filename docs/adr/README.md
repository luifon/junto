# Architecture decision records

Each record states one decision that can be changed independently: its context, the decision as rules, and its consequences.

Numbers are grouped in ranges by area. A new record takes the next free number in the range of its area.

Status values:

- **Accepted**: the decision is in force.
- **Accepted, pending step 0**: the decision is in force, and a technical test of step 0 decides or tests part of it. See [`../step-0/README.md`](../step-0/README.md).

## 0001–0099 Architecture and hosting

| Number | Title | Status | Summary |
|---|---|---|---|
| [0001](0001-system-components.md) | System components and data location | Accepted | Rust server, PostgreSQL, files on disk, LiveKit, recorder, transcription worker and one desktop client, all on the operator's own hardware. |
| [0002](0002-transport-encryption-only.md) | Transport encryption only | Accepted | TLS and LiveKit media encryption, no end-to-end encryption, because the server must read messages. |
| [0003](0003-deployment-with-docker-compose.md) | Deployment with Docker Compose in two forms | Accepted, pending step 0 | Separate containers; one host, or a data host and a public host joined by a private link; the recorder runs on the data host. |
| [0004](0004-server-address-and-network-paths.md) | Server address and network paths for calls | Accepted, pending step 0 | A server address is a domain name with a valid certificate; media uses UDP with a TCP fallback; three services share port 443 by hostname. |
| [0005](0005-backups-and-upgrades.md) | Backups and upgrades | Accepted | The server provides consistent backup and restore commands; migrations go forward only, under a lock, after a copy. |
| [0006](0006-storage-guard.md) | Storage guard | Accepted | Recordings and exports reserve disk space; below a reserve the server refuses new uploads, recordings and exports. |
| [0007](0007-monitoring-and-no-telemetry.md) | Monitoring and no telemetry | Accepted | A health endpoint and an optional OpenTelemetry export; nothing is sent to the authors or a third party. |
| [0008](0008-server-crates-and-process.md) | Server crates, configuration and process behaviour | Accepted | Three crates in one workspace; settings from environment variables; structured logs on standard error and the owner code on standard output; host commands in the same binary. |
| [0009](0009-serializable-transactions-and-retry.md) | Serializable transactions with a bounded retry | Accepted | Every request is one serializable transaction that is run again up to eight times when it loses against a concurrent one; no lock order; PostgreSQL limits each statement, lock wait and idle transaction. |
| [0010](0010-event-positions-assigned-at-commit.md) | Event positions are assigned at the commit, under one lock | Accepted | A deferred trigger gives each event its position when its transaction commits, under one advisory lock, so a higher position is never visible before a lower one; a cursor is an epoch and a position, and a host command sets a new epoch after a restore. |
| [0011](0011-events-written-with-the-change-and-authorized-at-delivery.md) | Events are written with their change and authorized when they are delivered | Accepted | The event table is the outbox and the replay log; each connection reads it in bounded batches and decides every event with the permission function in the same transaction, valid for one second from before its snapshot; deliveries are at least once; a stop loses nothing. |
| [0012](0012-health-route.md) | Health route | Accepted | `GET /api/v1/health` without a session answers 200 when the database answers and 503 with the error `unavailable` otherwise; the body also states whether the last admission check reached the media server and how many call rooms keep failing their check. |
| [0013](0013-request-bodies-are-read-whole.md) | A request body is read whole before the answer, up to a bound | Accepted | A body has at most 2 MiB; a larger one is read to its end, up to 16 MiB, and then answered with the error body, so that the client receives the answer and not a reset connection; the server holds at most 64 MiB of bodies, 4 MiB per client address, and holds nothing for a method without a body. |

## 0101–0199 Accounts and servers

| Number | Title | Status | Summary |
|---|---|---|---|
| [0101](0101-independent-servers.md) | Independent servers without federation | Accepted | Servers never communicate with each other; the client keeps a server book and uses each server's API. |
| [0102](0102-workspaces.md) | Several workspaces per server and the default workspace | Accepted | An account belongs to the server and membership is per workspace; the first start creates a usable default workspace. |
| [0103](0103-device-keys-and-login.md) | Accounts, device keys and login | Accepted, pending step 0 | One device key per server, no passwords, login by a signed single-use challenge; step 0 decides where keys are stored. |
| [0104](0104-device-linking-and-recovery.md) | Device linking and recovery | Accepted | A second device is linked with a one-time code for all servers; only the server owner performs a recovery, and it is recorded. |
| [0105](0105-invites-and-joining.md) | Invites and joining | Accepted | Joining by invite code or by approval; codes have an expiry, a use limit and workspaces; no guest accounts. |
| [0106](0106-removal-and-bans.md) | Removal and bans | Accepted | A ban blocks the account and its keys and cancels its invites; an unban restores the same account; otherwise a return needs a new code and gives a new account. |
| [0107](0107-connections-and-background-servers.md) | Connections, background servers and one call at a time | Accepted | One WebSocket per saved server with server-side filtering for background servers; one call per client and per account. |
| [0108](0108-login-challenge-and-signature.md) | Wire format of the login challenge and the limit on failed attempts | Accepted | The signed bytes name server, origin, operation, account, device and challenge; a challenge lives 60 seconds and is used when its operation commits; no lock per key; a ceiling on outstanding challenges. |
| [0109](0109-session-tokens.md) | Session tokens | Accepted | Opaque random tokens stored as hashes, valid 30 days, checked inside the transaction of each request, ended per device, by a ban, a removal and a recovery. |
| [0110](0110-one-time-codes.md) | Format and storage of owner, invite and link codes | Accepted | Codes of 5-bit symbols in groups of four, stored as hashes; each start without an owner issues a new owner code. |
| [0111](0111-invite-rules-and-attempt-limits.md) | Invite and join request rules, and the limits on attempts | Accepted | Defaults and bounds of invites, atomic use counting, join requests with a kept history and a hard ceiling, and limits per client address with bounded memory that trust a forwarded address only from a configured proxy. |
| [0112](0112-device-link-flow-and-recovery-notices.md) | Device link flow, device removal and recovery notices | Accepted | The new device gets a code, a logged-in device approves it with the public key; a recovery ends every session of the account and each device keeps a notice of it. |
| [0113](0113-account-states.md) | Account states for removal and ban | Accepted | Accounts are active, banned or removed and are never deleted; a ban keeps the keys blocked until an unban, a removal frees them. |
| [0114](0114-host-commands-for-the-server-owner.md) | Host commands for the server owner | Accepted | Two commands of the server binary assign a new owner or issue an owner code, on the database only, with an audit entry. |
| [0115](0115-existing-accounts-and-further-workspaces.md) | Adding an existing account to further workspaces | Accepted | A logged-in account redeems an invite, or an administrator adds the account; both are in the audit log. |
| [0116](0116-event-stream-connection-limits.md) | Limits of the event stream: connections, heartbeat and slow clients | Accepted | Two connections per device and ten per account, counted until their tasks end; a ping every 25 seconds; unfragmented client frames counted at the socket; no queue per connection; a client that takes no message for 10 seconds is disconnected and resumes from its cursor. |
| [0117](0117-public-origins-in-the-signed-challenge.md) | The signed challenge names the public origin of the server | Accepted | A device signs the origin that it connected to, and a server accepts a proof only for one of its configured public origins, so a challenge that another server relays gives that server no session; loopback origins serve local development only. |
| [0118](0118-recovery-notices-until-acknowledged.md) | A recovery notice stays until its device acknowledges it | Accepted | A login removes no notice; a device reads its notices and acknowledges each one, and the reason of an ended stream is one of them. |
| [0119](0119-account-profile-and-avatar.md) | Account profile: display name and avatar | Accepted | An account sets its name and an avatar of at most 256 KiB and 1024 by 1024 pixels, a still PNG, JPEG or WebP recognized by its bytes and stored in the database; a profile is served to the account, to fellow members and to moderators. |

## 0201–0299 API and automation

| Number | Title | Status | Summary |
|---|---|---|---|
| [0201](0201-public-application-api.md) | One public application API, versioning and administration in the client | Accepted | REST and a WebSocket event stream with no private endpoints; the client supports the current and the previous version. |
| [0202](0202-generated-api-documentation.md) | Generated API documentation | Accepted | OpenAPI and event schemas are generated from the server's definitions, and CI fails on a difference. |
| [0203](0203-access-tokens.md) | Administrator access tokens | Accepted | Tokens with an expiry and a set of actions, checked against the creator's current permissions on each request. |
| [0204](0204-setup-tool.md) | Setup from a file with a command-line tool | Accepted | `export`, `plan` and `apply` for the server structure, with stable identifiers and an all-or-nothing endpoint. |
| [0205](0205-api-conventions.md) | API conventions: paths, identifiers, encodings and errors | Accepted | Routes under `/api/v1`, UUID version 7 identifiers, RFC 3339 times, one error body with stable codes for every error, identical responses for absent and hidden objects, and a generated OpenAPI file checked by a test. |
| [0206](0206-event-envelope-and-version-rule.md) | Event envelope and the version rule of the event stream | Accepted | One JSON object per text frame with a `type`; within an API version the server only adds, and a reader skips what it does not know; the schemas are generated into `docs/api/events.json`. |
| [0207](0207-cursor-replay-bound-and-snapshot.md) | Cursor, replay bound and snapshot | Accepted | The device keeps its cursor; the server replays up to 1000 events that the account may receive from a log of 5000 positions and otherwise answers `resync`; the snapshot route returns the state with the cursor that continues it; states what positions tell a recipient. |
| [0208](0208-events-of-the-server-base.md) | The events of the server base and who receives them | Accepted | Each event goes to the accounts that the matching REST route serves; a permission change is a notice that names only the workspace; removal, ban and recovery end the connection with a reason. |
| [0209](0209-readers-accept-unknown-error-codes-and-audit-actions.md) | A reader accepts error codes and audit actions that it does not know | Accepted | An unknown error code and an unknown audit action are read as an explicit `unknown` value and never written; secrets erase their text on drop; the OpenAPI derivation of the protocol crate is a cargo feature. |

## 0301–0399 Permissions and moderation

| Number | Title | Status | Summary |
|---|---|---|---|
| [0301](0301-roles-and-permission-evaluation.md) | Two levels of roles and one evaluation rule | Accepted | Server roles and workspace roles, channel overrides (none per category), one evaluation function, and limits that apply to administrators too. |
| [0302](0302-direct-message-privacy.md) | Direct messages and their privacy | Accepted | One-to-one direct messages inside one server, closed to everyone except their two participants. |
| [0303](0303-audit-log.md) | Audit log | Accepted | A read-only log of administrative actions, visible according to channel access, with system lines in the channel. |
| [0304](0304-permission-evaluation-details.md) | Details of the permission function: view permission, grant rule and stored sets | Accepted | A "view channel" permission, the grant rule including what the channel overrides of a role allow, the permissions for actions that ADR 0301 does not name, and permission sets stored as fixed bits. |
| [0305](0305-audit-log-storage.md) | Storage and reading of the audit log | Accepted | Typed events stored with the change they describe, a trigger that refuses changes, and one read route per level with visibility decided per channel from one snapshot. |
| [0306](0306-ceilings-on-roles-and-invite-workspaces.md) | Ceilings on roles, role assignments and the workspaces of an invite | Accepted | 250 roles per workspace, 64 roles per member, 64 server roles per account and 100 workspaces per invite, so that no list in an event grows without a bound. |
| [0307](0307-contact-rule-for-direct-messages-and-direct-calls.md) | Which two accounts may exchange direct messages and direct calls | Accepted | Two active accounts with a common workspace, or of which one administers the server; decided by the permission function at the start of a call, at every admission check, at the delivery of every event of the call and in the snapshot; seeing a profile does not allow a call. |

## 0401–0499 Text

| Number | Title | Status | Summary |
|---|---|---|---|
| [0401](0401-messages-and-attachments.md) | Messages and attachments | Accepted | Edit and delete rules, mentions, reactions, formatting, attachment limits and pins. |
| [0402](0402-permanent-messages.md) | Permanent messages | Accepted | A channel setting that makes each message sent under it uneditable and excluded from automatic deletion. |
| [0403](0403-threads-and-forks.md) | Threads and forks | Accepted | Replies attached to one message; a thread can be forked into a text channel or a voice channel. |
| [0404](0404-notifications.md) | Unread signs and notifications | Accepted | Server-stored read positions, a level per channel, mute, do not disturb, and rules for direct messages and threads. |
| [0405](0405-search.md) | Search | Accepted | PostgreSQL full-text search over messages and transcripts, limited to the channels a user can read. |
| [0406](0406-archive-export-and-retention.md) | Archive, export and retention | Accepted | Manual archive with deletion after a set time, ZIP export with an expiring link, retention per workspace. |
| [0407](0407-deleted-channels-and-permanent-message-count.md) | Deleted channels keep a marker row, and a channel counts its permanent messages | Accepted | A deleted channel stays as a row for the audit log; the count of permanent messages decides the owner-only deletion. |

## 0501–0599 Calls

| Number | Title | Status | Summary |
|---|---|---|---|
| [0501](0501-voice-channels-and-direct-message-calls.md) | Voice channels and direct-message calls | Accepted | Calls happen in permanent voice channels with their own text chat; direct-message calls have no recording and no meeting mode. |
| [0502](0502-screen-share-sound.md) | Screen share with sound | Accepted, pending step 0 | The sharer chooses the sound explicitly, the application's own sound is excluded, and shared sound is its own track. |
| [0503](0503-mute-and-sound-blocks.md) | Mute, sound blocks and moderation in a call | Accepted | A person's own mute and a moderator's block are separate states; the server enforces microphone blocks only. |
| [0504](0504-voice-channel-privacy-invite-and-move.md) | Voice channel privacy, invite and move | Accepted | User limit, lock, hidden occupants and requests to enter; invitations pass the lock and limit only; the server performs moves. |
| [0505](0505-meeting-mode.md) | Meeting mode | Accepted | The person sharing a screen blocks every other microphone; ending it removes every microphone block in the voice channel. |
| [0506](0506-call-admission.md) | Call admission by repeated checking | Accepted, pending step 0 | Short-lived join tokens, removal through LiveKit's API, and a repeated comparison of participants with the people allowed. |
| [0507](0507-call-window-push-to-talk-and-levels.md) | Call window, push-to-talk, levels and volume | Accepted | A detachable call window, push-to-talk while focused, level meters and speaking marks, a volume per person. |
| [0508](0508-media-server-client-and-room-names.md) | Media-server client, join tokens and room names | Accepted | The server reaches LiveKit through one trait with its own Twirp client, over a private network without TLS in the first version; a join token names one room, one identity and the sources to publish; a room has a random name per call; notifications are verified by their signature on an internal route, processed once and limited in number. |
| [0509](0509-admission-check-interval-and-limits.md) | Admission check: interval, triggers and known limits | Accepted | Every rule changes the server's record of a call, and the end of a session removes its place in the same transaction; the full check takes the rooms in turn, 50 per part every 2 seconds and 10 at one time, and a targeted check takes the rooms that requests changed; states how long a room waits; failures stay within one room and one action; only absence that the server observed of a room counts. |
| [0510](0510-call-state-one-call-per-account-and-events.md) | Call state: the record, who is in a call, one call per account, blocks and events | Accepted | A person is in a call when the record allows the identity and the media server lists it; an account has one place on the server, a seat belongs to one session, and reading or changing the own call needs current access; blocks belong to the call and close every sound source; lists the events; an event for one account is delivered only while the account may be at the places it is about, and an account that loses a place gets one event without names and no outcome. |
| [0511](0511-move-between-voice-channels.md) | The move of a person between voice channels | Accepted | The server stores the move, the device joins the destination with a second identity, and the admission check completes the move when it sees that join, the move has not expired and both accounts are still allowed; only the session in the source gets the token and reports a failure; after 30 seconds nothing has changed and both sides are told; the audit log gets one entry per channel, and neither names the other channel. |
| [0512](0512-meeting-mode-rules-and-timings.md) | Meeting mode: who starts it, its blocks and its timings | Accepted | A person with a listed screen share and no microphone block turns it on; it ends when the share ends or when the presenter has been away for 60 seconds, and its end removes every microphone block. |
| [0513](0513-speaking-marks.md) | Speaking marks for the people outside a voice channel | Accepted | A device reports whether its own person speaks, at most 20 times per 10 seconds; once per second the server computes the set of speakers of a call from the record and records it when it differs from the set last sent, and never for a channel that hides who is inside. |
| [0514](0514-hidden-occupants-entry-requests-and-invitations.md) | Hidden occupants, the lock, requests to enter and invitations | Accepted | A hidden voice channel shows the people outside only that it is in use, moderators and administrators included; a request to enter and an invitation wait 60 seconds, an accepted person enters within 60 seconds, and a new invitation of an account replaces the earlier one with an event. |
| [0515](0515-direct-call-rings-and-muted-conversations.md) | Rings of direct-message calls: limit, pause after a decline, and muted conversations | Accepted | One account rings another at most 3 times in 10 minutes and not for 5 minutes after a decline, counted in the database for every session; an account mutes a conversation on the server and then gets no call invitation from the other account; no block list. |

## 0601–0699 Recording and transcripts

| Number | Title | Status | Summary |
|---|---|---|---|
| [0601](0601-recording-job-model.md) | Recording start and the recording job model | Accepted | A recording starts by permission with an indicator and is a tracked job with segments and states. |
| [0602](0602-recording-output-and-layout-page.md) | Recording output and the layout page | Accepted | One MP4 composed by the project's own layout page, plus one audio file per track as timed segments. |
| [0603](0603-transcripts-diarization-and-glossary.md) | Transcripts, diarization and glossary | Accepted | Voice detection before Whisper, speaker names from tracks, optional diarization of shared sound, a workspace glossary. |
| [0604](0604-recording-control-on-the-server.md) | Recording control on the server: the record, the controller, routes and events | Accepted | A recording and its segments are rows; the states follow one order; a start stores the recording and does not wait for the recorder; a controller compares each recording with its room once per second and after notifications, starts a job per audio track, stops jobs and names the main share; a recording is started and stopped by a participant with `record_calls`; its events carry no segments and reach the people who may see who is inside while its call runs, and the people who see the channel when the call has ended; a recording ends only when the room holds no participant of the recorder. |
| [0605](0605-storage-guard-reservations-and-values.md) | Storage guard: reservations, values and the stop at the reserve | Accepted | A recording reserves 8 GiB in the transaction of its start, the server keeps 5 GiB free and records 2 calls at one time; at the reserve a running recording is stopped in order; the health route and the log state when the storage is low. |
| [0606](0606-files-of-a-finished-recording.md) | Files of a finished recording: search, recovery, join, check and final state | Accepted | The server finds each file by its own identifiers and follows no link, takes the audio files that were saved of a lost recorder, joins the pieces of a composite by their bytes into one MP4 file, places each file on the timeline and checks it with `ffprobe` and `ffmpeg`; an incomplete attempt is repeated up to three times; the recording is `ready`, `partly_failed` or `failed` by its files. |
| [0607](0607-transcription-hand-over-and-transcript-storage.md) | Transcription hand-over, transcript storage, corrections and the glossary | Accepted | A transcript has its own state; the server runs the worker as a child process for one job at a time, with a lease among server processes, a time limit and three runs, and accepts only a transcript that agrees with its job; lines are rows, corrections and speaker names are appended rows with their author; a workspace glossary of at most 200 terms. |
| [0608](0608-recording-downloads-removal-and-upgrades.md) | Recording downloads, removal of a recording, and upgrades during a recording | Accepted | A file of a recording is downloaded whole or as one byte range by the rule of its recording; a removal deletes the rows in one transaction and the directory through a stored queue; the server does not start with a waiting migration while a recording runs. |
| [0610](0610-transcription-worker-language-and-isolation.md) | Transcription worker: language, process and isolation | Accepted | A Python 3.12 worker outside the Cargo workspace with pinned libraries; one process runs one job from files and opens no socket; every limit bounds the work and has a setting; the container has no network, no root user and a read-only file system. |
| [0611](0611-transcription-job-and-transcript-contract.md) | Transcription job and transcript contract | Accepted | One JSON job file in and one JSON transcript out, each with a format version and a schema that refuses unknown fields; files are opened through directory descriptors without following a link; an invalid job is refused as a whole, and each segment has its own result. |
| [0612](0612-model-installation-and-verification.md) | Model installation and verification | Accepted | A manifest lists every model file with its size, checksum and download address; one command downloads them over HTTPS and is the only code that uses the network; the worker checks every checksum before and after it loads the models and does not start when it can write to them. |
| [0613](0613-speech-clips-language-and-glossary.md) | Speech clips, language per clip and the glossary limit | Accepted | The worker builds clips of at most 29 seconds from Silero VAD with the measured rules, independent of the block length; Whisper detects the language of each clip or uses the language of the job; beam search with 5 beams; the glossary is cut to a token limit in order of priority. |
| [0614](0614-speaker-labels-for-shared-audio.md) | Speaker labels for shared audio | Accepted | Only shared audio is separated, per participant over all of its segments: TitaNet-Large embeddings of 2.5-second windows, average linkage on cosine distance cut at 0.6, labels `Speaker N` in the order of first speech, and a result per segment. |

## 0701–0799 Client

| Number | Title | Status | Summary |
|---|---|---|---|
| [0701](0701-native-client-and-dependencies.md) | Native client with gpui and its dependencies | Accepted, pending step 0 | A native Rust client with gpui and the LiveKit Rust SDK on macOS, Windows and Linux; step 0 fixes the dependency revisions. |
| [0702](0702-accessibility-scope.md) | Accessibility scope of the first version | Accepted, pending step 0 | Legible type, scaling and keyboard paths are provided; screen reader support is not promised. |
| [0703](0703-interface-layout-and-theme.md) | Interface layout, visual design, theme and language | Accepted | Three columns, one theme in one file, scaling from 90% to 150%, English text in one catalog. |
| [0704](0704-updates-and-code-signing.md) | Client updates and code signing | Accepted, pending step 0 | Updates from the release page verified against the project's keys; no paid operating-system code signing. |
| [0705](0705-supported-systems.md) | Supported systems and packages | Accepted, pending step 0 | macOS 26 on Apple Silicon, Windows 11 on x86-64, Linux on x86-64 with PipeWire; tarball and AppImage. |
| [0710](0710-client-data-directory-and-file-formats.md) | Client data directory and file formats | Accepted | One private data directory per user with the server book, the profile preset and one key file per server; versioned JSON files that are replaced in one step under a lock; a corrupted file stops the start and is never overwritten. |
| [0711](0711-device-key-file-and-its-protection.md) | Device key file and its protection on each system | Accepted | Device keys are key files on all three systems in the first version, behind one interface; a key is stored with the origin that it was created for and given out for that origin only; files are opened through held directories without following a link and must belong to the user alone; the Windows code has not been executed yet. |
| [0712](0712-client-flows-as-state-machines.md) | Client flows as state machines | Accepted | Adding a server, access to a server and approving a device link are flows with states, a fixed set of actions per state and one running action; the answer of the server decides the state, a lost answer is resolved with a login, and the state `logged in` shows the recoveries until the device acknowledges them. |
| [0713](0713-api-version-negotiation-per-server.md) | API version negotiation per server | Accepted | The client speaks its current API version and the one before, picks per server at the first contact and at each check, and shows a server without a common version as incompatible with the reason. |
| [0714](0714-client-requests-and-trust-in-a-server.md) | Client requests: addresses, proofs, repetition and what a server may cause | Accepted | An address is an HTTPS origin; the device signs a challenge only after it compared it with its request, over bytes that name the origin of the saved address; one place repeats requests after `busy`, `timeout` and a lost answer of a signed request; responses are bounded, an unknown error code is grouped by its status, and secrets are held in buffers that are overwritten. |
| [0715](0715-client-core-library-and-several-servers.md) | Client core library, several servers at once, and the event stream attachment | Accepted | One crate without interface code holds storage, keys, the API client, the flows and the manager; `junto-cli` uses its public interface, reads codes from standard input and acknowledges a recovery only by its own command; each saved server has its own state and session; the event stream client attaches through one interface. |
| [0716](0716-linking-a-device-on-all-servers.md) | Linking a new device on all servers in one flow | Accepted | The first device gives an offer with its servers and the new device gives a request with one link code and public key per server; both show a fingerprint; servers are worked on four at a time, each with its own result, and a device is added only to an account that the user saw. |
| [0717](0717-profile-preset-for-a-new-account.md) | Profile preset for a new account: the client sends the avatar once | Accepted | The server book records per server whether the avatar of the preset waits; the client sends the avatar, not the name, to an account that this device created; a failed try is counted and repeated, and an image that the server refuses is not sent again. |

## 0801–0899 Licence and reference code

| Number | Title | Status | Summary |
|---|---|---|---|
| [0801](0801-licence-and-reference-code.md) | Licence and the reference-code process | Accepted | Apache-2.0 with a CI licence check; GPL code is a reference only through three separate sessions and a similarity check. |

## 0901–0999 Process and testing

| Number | Title | Status | Summary |
|---|---|---|---|
| [0901](0901-repository-rules-and-feature-process.md) | Repository rules and the process per feature | Accepted | No personal information in tracked files; each feature passes a fixed sequence of tests, checks and an ADR before a push. |
| [0902](0902-testing-strategy.md) | Testing strategy | Accepted | CI on three systems, a test participant for calls and recordings, and tests of real capture on real systems. |
| [0903](0903-step-0-and-build-order.md) | Step 0 before product code, and the build order | Accepted | Eight technical tests run before product code; seven build steps follow. |
| [0904](0904-server-tests.md) | Server tests against PostgreSQL | Accepted | Integration tests over HTTP against a real PostgreSQL in Docker, one database per test, a replaceable clock, fixed orders for races, one table of every protected operation, and time limits that a test shortens only for the step that it examines. |
