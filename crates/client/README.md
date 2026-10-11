# junto-client

The Junto desktop application: a gpui window on `junto-client-core`. The binary is `junto`. The design decisions are in [`docs/adr/`](../../docs/adr/README.md): ADR 0730 describes this crate, ADR 0731 its theme and components, and ADR 0732 joining a server in the window.

This version shows the servers of the server book, their workspaces and channels, the people in voice channels, and the state of each server. It joins a server with a code or a request for access. It has no text messages and joins no call. It is built and tested on macOS and builds for Linux; Windows is not supported yet.

## Run it locally

A server for development, with a temporary database in Docker:

```sh
tools/dev-server.sh
```

The script prints the address (`http://127.0.0.1:8080`) and the server prints its one-time owner code below it. Control-C ends the server and removes its database. `--port`, `--name` and `--media` (a LiveKit server for calls) are its options.

The application, with a data directory of its own:

```sh
cargo run -p junto-client -- --data-dir /tmp/junto-first
```

At the first start the application asks for a name. Type the address into "Add server" of the server book, then the owner code into "I have a code". The first device that presents the owner code is the server owner.

## Two people on one machine

Each instance needs its own data directory: the directory holds the server book, the profile and the device keys of one installation (ADR 0710).

```sh
cargo run -p junto-client -- --data-dir /tmp/junto-first
cargo run -p junto-client -- --data-dir /tmp/junto-second
```

The second person needs an invite code or an approval. The command-line tool of the client core works on the data directory of the first person, also while the application runs:

```sh
# An invite code for the second person, created by the owner:
JUNTO_DATA_DIR=/tmp/junto-first cargo run -p junto-client-core --bin junto-cli -- invite

# Or: the second person presses "Request access", and the owner approves the request:
JUNTO_DATA_DIR=/tmp/junto-first cargo run -p junto-client-core --bin junto-cli -- requests
JUNTO_DATA_DIR=/tmp/junto-first cargo run -p junto-client-core --bin junto-cli -- approve-request <id>
```

With more than one saved server, `junto-cli` names the server with `--server <identifier, address or name>`; `junto-cli` without arguments prints its commands.

## Options

| Option | Meaning |
|---|---|
| `--data-dir <directory>` | The data directory. Default: the variable `JUNTO_DATA_DIR`, then the directory of the user (ADR 0710). |
| `--scale <percent>` | The interface scale of this start, from 90 to 150. The stored setting stays. |
| `--no-activate` | Open the window without bringing it to the front. |

The variable `JUNTO_LOG_LEVEL` sets the level of the log on standard error (`warn` by default).

## Tests

```sh
tools/with-test-postgres.sh tools/serial.sh cargo test -p junto-client
```

The tests in `tests/app` open the window code on gpui's test platform, with a client on a data directory of its own, against the server binary of this checkout. They click what is drawn and read what the window shows. A second client without a window is the administrator of the server. The test with people in a voice channel needs a LiveKit server and the participant tool of `spikes/test-participant`:

```sh
tools/with-test-postgres.sh tools/with-test-livekit.sh tools/serial.sh \
    cargo test -p junto-client --test app live:: -- --ignored
```

## Screens as pictures

`junto-screens` draws every screen and state without a window and writes one PNG file per screen. Nothing appears on the display.

```sh
cargo run -p junto-client --features test-support --bin junto-screens -- <directory>
```

## Layout of the crate

| Path | Content |
|---|---|
| `src/theme.rs` | Every colour, font, type size, radius and fixed measure, and the interface scale. |
| `src/text.rs` | The catalog of the interface text, with the sentence for each failure of a flow. |
| `src/assets.rs`, `assets/` | The fonts and the icons, carried in the binary. The fonts are under the SIL Open Font License; the licence texts are beside the files. |
| `src/ui/` | The components. |
| `src/state/` | The state of the application over the client core, and the derivation of what the window shows. |
| `src/shell/` | The main window. |
| `src/bin/junto-screens.rs` | The tool that draws the screens. |
