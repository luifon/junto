# 0746. An answer belongs to the owner and the place that asked

## Status

Accepted

## Context

The window sends requests and opens dialogs of the system whose answers arrive later. Between the request and the answer the person can select another place or another server, and a server can change what the window shows without the person: it removes the account, deletes the channel, or ends the session. Two paths took an answer for whatever the window showed at the moment of the answer. The answer to an edit of a message was put into the state of the server (ADR 0721), outside the order of its events: an answer that arrived after the message had been deleted put the message back, also into the state of the next account of the entry. The file dialog of the composer attached its files to the place that the composer showed when the dialog was answered; the dialog of macOS does not stop the window, so that place could be a place of another server.

## Decision

- An answer, a dialog of the system and a refusal belong to the owner (ADR 0745) and to the place that asked. When the owner or the place is another one at the moment of the answer, the answer changes nothing of what the window shows now.
- Where the person would lose an action without learning of it, the window says so at the place that it shows now.

Changes of a message:

- The window sends an edit, a deletion and a reaction for the account that the device has at that moment. A request fails when the device has another account by then.
- The answer to an edit is not content of the state. The change of a message arrives with its event and is read in the order of the events (ADR 0721), as the change of a deletion and of a reaction does. The client core has no operation that puts a message of an answer into the state from outside the stream task.
- A refusal of such a request is told only while the server still has the owner that asked. It is told above the composer that the window shows at that moment, also when that is the composer of another place; this record does not change that.

The file dialogs:

- The file dialog of the composer is opened for the place that the composer writes to and for the owner of its server at that moment. Its answer is taken only while the composer of that pane still writes to that place and the server still has that owner.
- Otherwise nothing is attached and nothing is uploaded, and the composer states that the files were chosen for another place. The person attaches them again where they belong.
- The file dialog of the logo of a server is opened for the owner of the server whose page "Server" is open. Its file is sent only while that page of that owner is still open. Otherwise nothing is sent; the page that is open then shows its own logo unchanged.
- The file dialog of the picture of an account compares the server identifier and the account of its layer, as ADR 0781 states.

## Consequences

- An edit shows on screen when its event has been followed, and not at its answer. On a connection that works, the event arrives with the answer.
- After a cut connection an edit shows when the stream has continued from its cursor or has loaded a snapshot.
- A file that the person chose in a dialog is never uploaded to a place that the person did not see when the dialog was opened.
- The answer to a send of the stream task is still put into the timeline when it arrives, as ADR 0721 states. It is the answer to a request of the stream task itself, and it ends with the account. A deletion that the events tell between the storing of the message and a late answer to its send is shown again until the place is read again.
- Tests of the window against the real server hold the answer to an edit at a proxy until the message was deleted, for the same account and for the next account of the entry, and hold the file dialog of the test platform while another server or another place is selected, and the file dialog of the logo while the page of another server is opened.
