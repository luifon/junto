# 0784. The list of recordings in the window

## Status

Accepted

## Context

A server keeps the recordings of calls of a workspace and lists them, the newest first, in pages (ADR 0604). The list holds the recordings of the voice channels that the account can view. The window showed a recording only while it ran in the call of this device (ADR 0738). The state of a server holds a recording only while it runs (ADR 0719), so a list of past recordings is not in the state. The server tells a change of a recording with the events `recording_started`, `recording_updated` and `recording_removed`. The approved design has no view for the list. The administration has a layer with pages, and its audit log is a list that is read in pages on demand (ADR 0761).

## Decision

The page:

- The administration has the page "Recordings" in the group of the shown workspace. Every member has it: the server answers with what the account may see.
- The page lists the recordings of the shown workspace, the newest first, in the order of the server.
- A row shows:
  - the channel, by its name in the state, or "A channel that is not shown" when the state does not hold it. The page holds open the archived list of the workspace, so an archived channel is named;
  - the start: the moment the recorder started, or the moment the recording was asked for when the recorder has not started;
  - the length, when the recording has ended and has a start;
  - the size, when the server has processed the files;
  - the state as words, for every state of a recording. "Partly failed" names the failed files and all files. "Failed" names the reason when this build knows it;
  - who started it, by a name that the state holds, or "an account";
  - the state of the transcript with the number of its lines.
- The page has no player and no control that saves or removes a recording.
- The names of a channel and of a person are text of a server: each is drawn without control characters, cut at 64 characters, in a line of its own that is cut at the width of the row.

Reads:

- The list is no collection of the client core. The page asks the route of the list for 50 recordings, and "Load older" asks for the 50 after the loaded ones.
- The page keeps at most 2,000 recordings. Then a sentence takes the place of "Load older", and no older page is read.
- A read is one request. While a read is on its way no older page is asked for.
- An empty list is stated as a sentence. A refusal and a server that cannot be reached are stated as their sentences with "Read again"; the loaded rows stay.
- A recording in an answer that names another workspace than the one that was asked for, or that the list holds already, is left out.

Changes:

- While the page is open, an event about a recording of its workspace reads the list again from the start. A state that replaced the state before, and a reader of the event stream that missed steps, do the same, because either can hide such an event. These two, a change of the permissions of the account in the workspace or on the server, and a deleted channel of the workspace also empty the list first and take nothing of a read that started before them (ADR 0748).
- One read of a list runs at a time. Changes that arrive while a read is on its way cause one further read from the start after it. Reason: the list takes the answer to its newest read only, so a read that started during another one would drop the answer of the first, and a server that tells changes faster than it answers would never be shown.
- A read that a change causes starts not sooner than one second after the start of the read before. A change that arrives sooner waits for the end of that second, and further changes in that time start no further read.
- A read from the start replaces the loaded rows with the first page.

Whose list it is:

- A list belongs to one server, one workspace and one account. A request is made for the account that the device had when the read started; it fails when the device has another account by then.
- An answer is taken only when it is the answer to the newest read, the page still shows that server and that workspace, and the device still has that account. Every other answer is dropped.
- The list is put away when the page changes, when the layer closes, and when the layer closes because its server shows another account (ADR 0761).

## Consequences

- A member finds the recordings of a workspace and their states without being in a call.
- A recording cannot be played, saved or removed in the window yet. The API of the server has the routes for it (ADR 0604).
- The name of a person who is no member of a loaded list is "an account": the page starts no read of a profile.
- A person who loaded older pages returns to the first page when a recording of the workspace changes.
- A server that sends events without end causes at most one request for the list in a second and never two at once, each answer is shown, and the page holds at most 2,000 rows.
- The page shows what the server answered at the last read. A change is shown after the next read.
