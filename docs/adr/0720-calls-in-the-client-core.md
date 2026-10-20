# 0720. Calls in the client core: typed routes, the connection of a server in a call, and the commands of the tool

## Status

Accepted

## Context

The server decides who is in a call and gives out the token for the media server (ADR 0506, ADR 0508, ADR 0510). The client core needs every call route before the media engine and the desktop client exist, and a way to use them by hand. ADR 0107 states that a call goes on while another server is shown and that a client is in one call at a time. ADR 0715 left open how the server of a call keeps its connection in the mode "connect only when selected". A place in a call belongs to one session (ADR 0510), and each command of `junto-cli` is one process that ends its session (ADR 0715).

## Decision

Routes:

- The API object of a server has one function for each route of the calls: the call of the account, its own state, its screen share, its hand and its speaking report; the call of a voice channel, its settings, its lock and meeting mode; requests to enter and their answers; invitations and their answers; moves, their join and their failure; the two blocks, the lowering of the hand of another person (ADR 0516) and the removal of a person; direct-message calls and their refusal; and the mute of a direct-message conversation. A further function reads the snapshot (ADR 0207).
- A join returns the address of the media server, the identity and the token. The token is held by the secret type of the protocol crate, and the body of the response by a buffer that is overwritten (ADR 0714).
- A call request is sent again only after `busy` and `timeout`, as every request without a signature (ADR 0714).
- This unit connects to no media server.

One call per client (ADR 0107):

- The server enforces one call per account on one server (ADR 0510). The rule over several servers is applied by the part that holds the media connection, when it joins a call. The client core gives it the call of the account per server in the state and the function that leaves a call. ADR 0734 fixes the rule.

Connection of a server in a call:

- When the state of a server holds a call of the account on this device, the stream client tells the client core, and the server has at least the connection level `background`, also in the mode "connect only when selected" and while another server is selected.
- The rule ends when that call ends or moves to another device of the account. A call of the account on another device does not keep a connection of this device.
- The server sends every event of the call to a connection of an account in the call at any level (ADR 0116), so the state of the call stays current at the background level.

Commands of `junto-cli`:

- `state` logs in, starts the event stream, prints the state of the server once and ends its session.
- `watch` prints the state and then every notice until its time has passed, the line `quit` arrives or the process is interrupted. Each other line of standard input is a call action that runs with the session of the watch.
- `call <action>` runs one call action in a process of its own. A join in this form ends with the command, because the end of the session removes the place (ADR 0509).
- A join prints the token only with `--show-token`.
- The three commands end their session through an operation of the server object that waits for a running action or renewal of the session and then logs out. The event stream can start a login at any moment (ADR 0718), and a logout that is refused as `busy` would leave the session of that login on the server.

## Consequences

- The media engine and the desktop client use the same functions as the tool.
- The state follows the recordings of the server as far as a call shows them: `recording_removed` removes a recording from the call that shows it, and the events of transcripts and of the glossary change nothing, because their collections are read from routes (ADR 0719).
- A person tests a call by hand with one `watch` process per account: the process holds the place, and its standard input takes the actions of that place.
- A server in the mode "connect only when selected" that is not selected has no connection, so the client learns of a call of this device there only from a join that it made while the server was selected. A join through the API without a connection is not followed by the rule; the desktop client joins on the selected server.
- The live tier of the client tests runs the routes against a real media server with a real participant (ADR 0902). The plain tier covers the routes that need no media server and the refusal `media_unavailable`.
