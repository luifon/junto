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

## 0101–0199 Accounts and servers

| Number | Title | Status | Summary |
|---|---|---|---|
| [0101](0101-independent-servers.md) | Independent servers without federation | Accepted | Servers never communicate with each other; the client keeps a server book and uses each server's API. |
| [0102](0102-workspaces.md) | Several workspaces per server and the default workspace | Accepted | An account belongs to the server and membership is per workspace; the first start creates a usable default workspace. |
| [0103](0103-device-keys-and-login.md) | Accounts, device keys and login | Accepted, pending step 0 | One device key per server, no passwords, login by a signed single-use challenge; step 0 decides where keys are stored. |
| [0104](0104-device-linking-and-recovery.md) | Device linking and recovery | Accepted | A second device is linked with a one-time code for all servers; only the server owner performs a recovery, and it is recorded. |
| [0105](0105-invites-and-joining.md) | Invites and joining | Accepted | Joining by invite code or by approval; codes have an expiry, a use limit and workspaces; no guest accounts. |
| [0106](0106-removal-and-bans.md) | Removal and bans | Accepted | A ban applies to the account, blocks its keys and cancels its invites; a return needs a new code. |
| [0107](0107-connections-and-background-servers.md) | Connections, background servers and one call at a time | Accepted | One WebSocket per saved server with server-side filtering for background servers; one call per client and per account. |

## 0201–0299 API and automation

| Number | Title | Status | Summary |
|---|---|---|---|
| [0201](0201-public-application-api.md) | One public application API, versioning and administration in the client | Accepted | REST and a WebSocket event stream with no private endpoints; the client supports the current and the previous version. |
| [0202](0202-generated-api-documentation.md) | Generated API documentation | Accepted | OpenAPI and event schemas are generated from the server's definitions, and CI fails on a difference. |
| [0203](0203-access-tokens.md) | Administrator access tokens | Accepted | Tokens with an expiry and a set of actions, checked against the creator's current permissions on each request. |
| [0204](0204-setup-tool.md) | Setup from a file with a command-line tool | Accepted | `export`, `plan` and `apply` for the server structure, with stable identifiers and an all-or-nothing endpoint. |

## 0301–0399 Permissions and moderation

| Number | Title | Status | Summary |
|---|---|---|---|
| [0301](0301-roles-and-permission-evaluation.md) | Two levels of roles and one evaluation rule | Accepted | Server roles and workspace roles, channel overrides, one evaluation function, and limits that apply to administrators too. |
| [0302](0302-direct-message-privacy.md) | Direct messages and their privacy | Accepted | One-to-one direct messages inside one server, closed to everyone except their two participants. |
| [0303](0303-audit-log.md) | Audit log | Accepted | A read-only log of administrative actions, visible according to channel access, with system lines in the channel. |

## 0401–0499 Text

| Number | Title | Status | Summary |
|---|---|---|---|
| [0401](0401-messages-and-attachments.md) | Messages and attachments | Accepted | Edit and delete rules, mentions, reactions, formatting, attachment limits and pins. |
| [0402](0402-permanent-messages.md) | Permanent messages | Accepted | A channel setting that makes each message sent under it uneditable and excluded from automatic deletion. |
| [0403](0403-threads-and-forks.md) | Threads and forks | Accepted | Replies attached to one message; a thread can be forked into a text channel or a voice channel. |
| [0404](0404-notifications.md) | Unread signs and notifications | Accepted | Server-stored read positions, a level per channel, mute, do not disturb, and rules for direct messages and threads. |
| [0405](0405-search.md) | Search | Accepted | PostgreSQL full-text search over messages and transcripts, limited to the channels a user can read. |
| [0406](0406-archive-export-and-retention.md) | Archive, export and retention | Accepted | Manual archive with deletion after a set time, ZIP export with an expiring link, retention per workspace. |

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

## 0601–0699 Recording and transcripts

| Number | Title | Status | Summary |
|---|---|---|---|
| [0601](0601-recording-job-model.md) | Recording start and the recording job model | Accepted, pending step 0 | A recording starts by permission with an indicator and is a tracked job with segments and states. |
| [0602](0602-recording-output-and-layout-page.md) | Recording output and the layout page | Accepted, pending step 0 | One MP4 composed by the project's own layout page, plus one audio file per track as timed segments. |
| [0603](0603-transcripts-diarization-and-glossary.md) | Transcripts, diarization and glossary | Accepted, pending step 0 | Voice detection before Whisper, speaker names from tracks, optional diarization of shared sound, a workspace glossary. |

## 0701–0799 Client

| Number | Title | Status | Summary |
|---|---|---|---|
| [0701](0701-native-client-and-dependencies.md) | Native client with gpui and its dependencies | Accepted, pending step 0 | A native Rust client with gpui and the LiveKit Rust SDK on macOS, Windows and Linux; step 0 fixes the dependency revisions. |
| [0702](0702-accessibility-scope.md) | Accessibility scope of the first version | Accepted, pending step 0 | Legible type, scaling and keyboard paths are provided; screen reader support is not promised. |
| [0703](0703-interface-layout-and-theme.md) | Interface layout, visual design, theme and language | Accepted | Three columns, one theme in one file, scaling from 90% to 150%, English text in one catalog. |
| [0704](0704-updates-and-code-signing.md) | Client updates and code signing | Accepted, pending step 0 | Updates from the release page verified against the project's keys; no paid operating-system code signing. |
| [0705](0705-supported-systems.md) | Supported systems and packages | Accepted, pending step 0 | macOS 26 on Apple Silicon, Windows 11 on x86-64, Linux on x86-64 with PipeWire; tarball and AppImage. |

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
