# Junto

Junto is a self-hosted application for team text chat, voice and video calls, screen sharing, and call recording with transcripts.

It consists of a server written in Rust with PostgreSQL, a native desktop client built with gpui for macOS, Windows and Linux, LiveKit as the media server, a recorder, and a transcription worker.

## Status

Every feature of the first version that [`docs/scope.md`](docs/scope.md) lists is built: in the server, in the desktop client for macOS, Linux and Windows, in the recorder and in the transcription worker. Continuous integration builds and tests the project on the three systems ([ADR 0905](docs/adr/0905-continuous-integration.md)).

There is no release yet. The project publishes no client package and no server image, and the document that describes a release is not written. A server is installed from this repository with [`docs/install.md`](docs/install.md). The client packages are built from this repository with the release tooling in [`packaging/`](packaging/README.md).

## Main design points

- **Self-hosted.** All data stays on the hardware of the person who operates the server. The application sends no telemetry.
- **Independent servers.** Servers do not communicate with each other. One client connects to several servers and keeps a server book.
- **Transport encryption only.** Connections use TLS and LiveKit encrypts media. There is no end-to-end encryption, because the server reads messages for history, search, export and the API.
- **Device keys, no passwords.** The client generates a separate device key for each server and logs in by signing a challenge.
- **One public API.** The client uses only the public REST and WebSocket API. The API documentation is generated from the server's definitions.
- **Two levels of roles.** Server roles and workspace roles, with channel overrides, evaluated by one function for every check.
- **Private direct messages.** Direct messages are closed to everyone except their two participants, administrators included.
- **Calls in voice channels and in direct messages.** Voice, video and screen share with sound, with an explicit choice of which sound a share sends.
- **Recording with transcripts.** One video per recording, one audio file per track, and a transcript: a line from a microphone carries the name of the member, the voices in shared sound are separated into speakers, and a workspace glossary guides the spelling of names and terms.
- **Native client.** One client built with gpui and the LiveKit Rust SDK. There is no browser client.

## Documentation

- [`AGENTS.md`](AGENTS.md): the guide to the repository: where each part is, how to build and test it, and the rules for a change.
- [`docs/adr/`](docs/adr/README.md): the architecture decision records, with an index.
- [`docs/scope.md`](docs/scope.md): what the first version contains, what later versions can add, and what is not planned.
- [`docs/step-0/`](docs/step-0/README.md): the eight technical tests that ran before product code, with their results.
- [`docs/install.md`](docs/install.md): how to install a server with Docker Compose from this repository, and how to back it up, restore it, upgrade it and remove it.
- [`docs/client-macos.md`](docs/client-macos.md), [`docs/client-linux.md`](docs/client-linux.md), [`docs/client-windows.md`](docs/client-windows.md): how to install the desktop client from its package, how it updates, and how to restore the previous version.
- [`docs/api/`](docs/api/): the API documents (`openapi.json`, `events.json`, `structure-file.json`), generated from the code of the server.

## Licence

Apache-2.0. See [`LICENSE`](LICENSE). The application icon in [`packaging/icon/`](packaging/icon/README.md) is the project's own work under the same licence. The fonts of the client are under the SIL Open Font License; their licence texts are beside them in `crates/client/assets/fonts/`. The icons of the client are icons of Lucide under the ISC licence, partly also under the MIT licence of Feather; the licence file is beside them in `crates/client/assets/icons/lucide/`. Each client package holds the licence texts of the third-party code in its programs in `licences/third-party-licences.txt`, and the server image in `/usr/share/licenses/junto-server/third-party-licences.txt`; the AppImage also holds the source of its runtime in `licences/appimage-runtime` (ADR 0801).
