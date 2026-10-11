# 0715. Client core library, several servers at once, and the event stream attachment

## Status

Accepted

## Context

The desktop client is built with gpui (ADR 0701), which has its own executor and no tokio runtime. A command-line tool must operate a server before the desktop client exists, and the tests must drive the client without an interface (ADR 0902). Client state is built for several servers from the first build step (ADR 0703), and each saved server has its own connection (ADR 0107). The event stream of the server does not exist yet, but the place where its client attaches must not require a later change of the server book or of the session handling.

## Decision

Crate:

- The crate `junto-client-core` holds everything of the client that is not interface and not media: the data directory, the device keys, the API client, the flows and the manager of servers. It has no dependency on gpui. The media engine is the crate `junto-media`, which builds on it (ADR 0733).
- The binary `junto-cli` is in the same crate and uses only the public interface of the library. Its commands are: `servers`, `add`, `remove`, `status`, `claim`, `join`, `request`, `login`, `link`, `approve-link`, `link-offer`, `link-all`, `approve-all`, `devices`, `recoveries`, `acknowledge-recovery`, `account`, `invite`, `requests`, `approve-request`, `profile`, `state`, `watch` and `call` (ADR 0720).
- Each command of `junto-cli` is one process. A command that logs in ends its session before it exits. `watch` is the one command that runs until it is stopped.
- An owner code, an invite code, a link code and a link request are never accepted as arguments, because an argument is visible to other programs of the user and stays in the history of the shell. The tool reads them from standard input: one line when standard input is not a terminal, and a prompt with the echo off when it is. An option that names a code is refused.
- On a terminal the tool turns the echo off first and shows the prompt after that, and restores the echo when the line was read or the read failed.
- A line is read byte by byte into one buffer of a fixed size that is overwritten when it is dropped: 256 bytes for a code, and the size of a link text for an offer or a request. Nothing after the end of the line is read, and a longer line is an error. The text stays in such a buffer until a type of the protocol crate takes it. On Windows the bytes pass through the input buffer of the standard library.
- After every contact that can log in, the tool prints the recoveries that this device has not acknowledged, each with its identifier and once per process, before the output of the command. This holds for the commands of one server and for `link-all` and `approve-all`.
- The tool acknowledges a recovery only with the command `acknowledge-recovery <identifier>`. A printed line does not prove that a person read it, because a script can run the tool and discard its output (ADR 0118). Until the acknowledgement, every command that logs in prints the recovery again. `recoveries` lists them.
- `profile` shows and sets the name and the avatar of the local preset. `account` shows the account of this device on a server and sets its name and its avatar there; with an account identifier it shows the profile of that account. `--save-avatar` writes the image of an avatar to a file that does not exist yet, and never replaces a file.
- The tool prints text that a server sent (names of accounts, devices and join requests, an invite code, the hash of an avatar) without control characters, so that a server cannot move the cursor of a terminal or erase a line that the tool printed before, such as a recovery (ADR 0714).
- `approve-all` prints the account of this device with every server. `--yes` approves for the accounts that were printed and never agrees to another account (ADR 0716).

Runtime:

- The client holds a tokio runtime with two worker threads, or uses the runtime of the caller when the caller has one. The media engine runs its tasks on the same runtime.
- Every public operation that uses the network starts a task on that runtime and returns a future that only waits for the task. Any executor can poll that future.
- A started action of a flow runs to its end when its future is dropped (ADR 0712).

Several servers:

- The manager holds one object per saved server. Each has its own reachability, API version, access flow and session, and loads only its own key. No server shares a session, a key or a state with another.
- The reachability of a server is one of: not contacted yet, reachable with its API version, unreachable, incompatible (ADR 0713), another server (ADR 0714).
- A failure of one server changes that server only. Connecting all servers works on at most four servers at a time, and the result of each is reported by itself.
- The operations of a logged-in account are written once. A saved server gives them with a session that it renews with the device key when the server ended it (ADR 0712).
- One request of an account does not wait for a request of the same account. A renewal of the session and an action of the access flow do not run at the same time.
- A server is removed only while its access flow runs no action. The removal deletes the entry and the key under one lock of the data directory and names the generation of the entry, so it cannot delete the key of an entry that another program added meanwhile (ADR 0710).
- An interface learns of a change through one notification for the whole client and then reads the states that it shows.

Event stream attachment:

- Each server has a connection level that the client derives: `off`, `background` or `selected` (ADR 0107).
  - `selected`: the device is logged in and the server is the selected one.
  - `background`: the device is logged in, the server is not selected, and its connection mode is "always" or this device is in a call on it (ADR 0720).
  - `off`: every other case.
- The event stream client (ADR 0718) is a part of this crate. The client core tells it through one operation that the level or the session of a server changed. With a level other than `off` it receives the address, the API version, the session token, and the account and the device of the session.
- The client core calls it after a login, a logout, a renewal of the session, a ban, a removal, a change of the selected server, a change of the connection mode, and when this device enters or leaves a call.
- The stream client tells the client core two things: that the server no longer accepts a session, and whether this device is in a call.
- The cursor is not written to the server book (ADR 0718).

## Consequences

- The desktop client adds an interface on top of the library and needs no second implementation of joining, login or storage.
- A server can be operated from a terminal with `junto-cli` before the desktop client exists.
- A script gives a code to `junto-cli` through a pipe. A code cannot be passed on the command line.
- The console code that turns the echo off on Windows is unsafe code of the tool and is not executed by any test yet (ADR 0711).
- The library runs inside gpui without a tokio runtime in the interface code, at the cost of two more threads.
- The event stream client needs no access to keys or to the book. It gets a token and reports an ended session; the client core logs in.
- A session token that is renewed reaches the stream client as a new call with the same level.
- An interface that shows many servers reads all their states after each notification. The notification does not say what changed.
