# Junto

Junto is a self-hosted application for team text chat, voice and video calls, screen sharing, and call recording with transcripts.

It consists of a server written in Rust with PostgreSQL, a native desktop client built with gpui for macOS, Windows and Linux, and LiveKit as the media server.

## Status

Design reviewed. The technical tests of step 0 are in progress. The server exists in `crates/` as far as this: accounts, device keys, profiles, invites, roles, channels, the audit log, the event stream, and the server side of calls (admission, room privacy, meeting mode, moderation and direct-message calls) behind the public API. Text messages, recording and transcripts are not built. A first desktop application exists in [`crates/client`](crates/client/README.md): it joins a server, shows the servers, workspaces, channels and the people in voice channels, and holds a voice call in a voice channel, without messages, cameras and screen share. There is still no usable application.

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
- [`docs/install.md`](docs/install.md): how to install a server with Docker Compose from this repository, and how to back it up, upgrade it and remove it.
- [`docs/client-macos.md`](docs/client-macos.md), [`docs/client-linux.md`](docs/client-linux.md), [`docs/client-windows.md`](docs/client-windows.md): how to install the desktop client from its package, how it updates, and how to restore the previous version.

## Licence

Apache-2.0. See [`LICENSE`](LICENSE).
