# 0001. System components and data location

## Status

Accepted

## Context

Junto provides text chat, voice and video calls, screen sharing, call recording and transcripts. Each function needs a component, and the division between components determines which component stores data and which component enforces permissions. The system is self-hosted, so every component must run on hardware that the operator of a server controls.

## Decision

The system has these components:

| Component | Role |
|---|---|
| Server (Rust) | Accounts, permissions, messages, files, REST API, WebSocket event stream |
| PostgreSQL | All structured data, including full-text search |
| File storage on disk | Attachments, recordings, transcripts |
| LiveKit server (the media server) | Forwards call audio and video between participants (SFU) |
| LiveKit Egress with Redis (the recorder) | Records calls |
| Recording layout page | A small web page, served by the server, that the recorder loads to compose the video |
| Transcription worker | Voice detection, Whisper for text, an optional diarization model for speaker separation |
| Desktop client (gpui) | The only client |

- All data is stored on the operator's own hardware. The system uses no hosted database and no hosted storage service.
- PostgreSQL holds all structured data. Full-text search uses PostgreSQL; there is no separate search service.
- Attachments, recordings and transcripts are files on disk.
- In these records, "server" means the Junto server, "media server" means the LiveKit server and "recorder" means LiveKit Egress.

## Consequences

- The operator provides the hardware and the disk space, and is responsible for backups (ADR 0005).
- The server is the only component that enforces permissions. The media server admits a participant on a token that the server issues (ADR 0506).
- The desktop client is the only client. A browser client and mobile clients are in the later list of `docs/scope.md`.
- A deployment runs PostgreSQL, the media server, the recorder with Redis and the transcription worker in addition to the server (ADR 0003).
