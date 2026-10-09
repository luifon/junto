# Junto

Junto is a self-hosted application for team text chat, voice and video calls, screen sharing, and call recording with transcripts.

It consists of a server written in Rust with PostgreSQL, a native desktop client built with gpui for macOS, Windows and Linux, and LiveKit as the media server.

## Status

Design reviewed. The technical tests of step 0 are in progress. The server base exists in `crates/`: accounts, device keys, invites, roles, channels and the audit log behind the public API. There is no client yet, and there is still no usable application.

## Main design points

- **Self-hosted.** All data stays on the hardware of the person who operates the server. The application sends no telemetry.
- **Independent servers.** Servers do not communicate with each other. One client connects to several servers and keeps a server book.
- **Transport encryption only.** Connections use TLS and LiveKit encrypts media. There is no end-to-end encryption, because the server reads messages for history, search, export and the API.
- **Device keys, no passwords.** The client generates a separate device key for each server and logs in by signing a challenge.
- **One public API.** The client uses only the public REST and WebSocket API. The API documentation is generated from the server's definitions.
- **Two levels of roles.** Server roles and workspace roles, with channel overrides, evaluated by one function for every check.
- **Private direct messages.** Direct messages are closed to everyone except their two participants, administrators included.
- **Calls in voice channels.** Voice, video and screen share with sound, with an explicit choice of which sound a share sends.
- **Recording with transcripts.** One video per recording, one audio file per track, and a transcript with speaker names and a workspace glossary.
- **Native client.** One client built with gpui and the LiveKit Rust SDK. There is no browser client.

## Documentation

- [`docs/adr/`](docs/adr/README.md): the architecture decision records, with an index.
- [`docs/scope.md`](docs/scope.md): what the first version contains, what later versions can add, and what is not planned.
- [`docs/step-0/`](docs/step-0/README.md): the eight technical tests that run before product code, with their pass and fail gates.

## Licence

Apache-2.0. See [`LICENSE`](LICENSE).
