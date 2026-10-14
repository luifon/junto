# Scope

This file lists what the first version of Junto contains, what later versions can add, and what is not planned. The decisions behind each item are in [`adr/`](adr/README.md).

## Rules for the first version

- The application is put into use only after every first-version feature is built and tested end to end.
- The results of step 0 ([`step-0/README.md`](step-0/README.md)) can remove a feature from the first version. They do not add to it.

## First version

Text:

- Text channels
- Direct messages
- Replies
- Mentions
- Reactions
- Edit and delete
- Formatting with code blocks
- Attachments with image previews
- Threads
- Pins
- Unread signs
- Notification levels with mute and do not disturb
- Basic search over messages and transcripts
- Permanent-message channels
- Archive and export

Servers, accounts and permissions:

- Server roles and workspace roles with channel permissions
- Several workspaces per server
- The server book
- A device list per account
- Audit log

Calls:

- Voice channels with their own text chat
- Video
- Screen share with sound
- Meeting mode
- Voice channel lock and user limit
- Invite and move between voice channels

Recording and transcripts:

- Call recording
- Transcripts with speaker separation and a workspace glossary

Administration and automation:

- Administration screens
- Administrator access tokens
- The setup command-line tool
- Generated API documentation

Client:

- The update notice
- Interface scaling

## Limits of the first version stated in the records

- No guest accounts and no browser client (ADR 0105).
- No bot accounts, incoming webhooks or slash commands (ADR 0201).
- No permission overrides for a single member (ADR 0301).
- No presence indicators and no typing indicators (ADR 0401).
- No calls in text channels (ADR 0501).
- No recording and no meeting mode in direct-message calls (ADR 0501).
- Push-to-talk only while the application or the detached call window is focused (ADR 0507).
- No built-in recording player (ADR 0602).
- Screen reader support is not promised (ADR 0702).
- One theme and one interface language, English (ADR 0703).

## Later versions

- Search inside attached files
- Link previews
- Custom emoji
- Group direct messages
- Inline video and audio players
- A built-in recording player that jumps to a transcript line
- "Also send to the channel" for thread replies
- Bot accounts
- Incoming webhooks
- Slash commands
- Message buttons
- Bots in voice channels
- Moderation by report
- Opt-in compliance access to direct messages
- Stored voice profiles that name recurring speakers automatically
- A dark theme
- System-wide push-to-talk
- A dock or tray badge
- A browser client
- Mobile clients
- An RPM package
- An API for bots and bridges that receives the events of chosen channels, and with it a bridge to Matrix
- Login through an organisation's identity provider (OpenID Connect)
- Optional end-to-end encryption for direct conversations and calls; the server cannot search, record, transcribe or apply retention to them
- A push gateway for mobile clients that receives only event identifiers, never content
- Loading only a window of channels at the first start, if measurements show that large servers load too slowly

## Not planned

- Paid operating-system code signing
- Import of history from other chat services
- Recording of direct-message calls
