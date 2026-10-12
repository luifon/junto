# 0712. Client flows as state machines

## Status

Accepted

## Context

Adding a server, joining, asking for access, linking a device and logging in each take several requests and can stop at a point where a person or another device must act (ADR 0105, ADR 0112). The desktop client and the command-line tool both drive these steps. A flow that is written inside the interface code cannot be tested without the interface, and a flow that waits inside a function for the user blocks the program that called it. A request can also fail after the server has done the work, so the client can hold a state that differs from the state on the server.

## Decision

Contract of every flow:

- A flow is in one state at a time. Each state is one that an interface can show and that lasts until an action.
- Each state accepts a fixed set of actions. The flow reports the state, the accepted actions, the action that runs now, and the reason why the last action failed.
- An action runs to its end and leaves the flow in a state: the next state after a success, the state before after a failure, with the reason. The next action clears the reason.
- While an action runs, the flow accepts no other action and reports none. A second action gets the error `busy` and changes nothing. An action that the state does not accept gets `not available` and changes nothing.
- No flow waits for the interface. Waiting for a person is a state, not a running action.
- A server object offers a second way to run an action of the access flow: it waits for the action or the renewal of the session that runs now and then runs the action. An interface uses it for what a person asked for; `end_session` is that way for a logout.
- An action runs as a task of the client (ADR 0715). It runs to its end when the interface drops its future. If the task itself stops, the flow returns to its state with the reason `stopped` and accepts actions again.
- Text that the user typed is checked before any request: a name has 1 to 64 characters and no control character, as on the server.
- The reason of a failure holds no secret. It names the error code of the server, or the cause of a failed connection, and holds no text of the server, with one exception.
- A refusal because a limit is reached keeps its code beside the wait that the server names. An interface states a full list of waiting join requests or device links (ADR 0111, ADR 0112) with a sentence of its own, apart from a limit on attempts.
- The exception is a refusal with a code that the client does not know. The client has no text of its own for it, so the reason holds the status of the response and the message of the server, cut to 200 characters without control characters (ADR 0209, ADR 0714). An interface shows the message as a quotation of the server.
- The helper that waits for a decision goes on after such a refusal with a status of 500 or above, and waits for the stated time after one with the status 429, as it does for the known codes of those groups.

Flow "add a server":

- States: `enter address`, `preview`, `key of another address`, `saved`.
- `fetch` reads the address (ADR 0714), asks it for the public server information, and moves to `preview` with the name, the logo address and the usable API version. Nothing is stored and no key is created.
- `fetch` fails when the server identifier or the address is already saved.
- When this device holds a key of the reported server identifier that was created for another address, `fetch` moves to `key of another address`, which shows that address. The server cannot be saved from there. The one action besides going back is `discard key`, which deletes that key; the flow then shows the preview (ADR 0711).
- `save` stores the entry with the chosen connection mode (ADR 0107). A server whose API versions the client cannot use can still be saved (ADR 0713).

Access flow of one saved server:

- States: `no account` (with a notice: removed, request denied, request expired, link expired, key missing), `waiting for approval` (with the request identifier once the server has answered), `waiting for link` (with the link code, the public key of this device and the expiry), `logged out`, `logged in` (with the recoveries that this device has not acknowledged), `banned`.
- Actions: `check`, `join` with an invite code, `claim owner` with an owner code, `request access`, `request link`, `log in`, `log out`, `forget account`.
- `no account` accepts `check`, `join`, `request access`, `request link` and `claim owner`. The two waiting states accept `check`, `join`, `claim owner` and `forget account`; `waiting for link` also accepts a new `request link` and `request access`. `logged out` accepts `log in`. `logged in` accepts `check` and `log out`. `banned` accepts `check` and `forget account`.
- Before any contact the state comes from the membership in the server book (ADR 0710): a member starts as `logged out`.

What the server says decides the state:

- `request access` stores the intent in the server book before the request is sent. When the answer is lost and the result of the request cannot be read either, the flow is `waiting for approval` without an identifier, also after a restart, and `check` asks the server. When the server refused the request, the intent is removed again.
- `check` and `log in` ask the server and take its answer as the truth. With a stored join request the client asks for the result of the request first: a waiting, denied or expired request decides the state. For an approved request, and in every other case, it tries a login.
- When the book knows no account and no request, a key exists, and the login answers `unknown_device`, the client also asks for the result of a join request of that key. A request that waits moves the flow to `waiting for approval`. This finds a request that the server holds and the book lost.
- A login that succeeds gives `logged in`, whatever the state before was. This is how an approved link, an approved request and an unban are found.
- The error `unknown_device` for a device that was a member gives `no account` with the notice "removed". The error `account_banned` gives `banned`.
- An operation whose answer was lost or could not be read although its status says success (ADR 0714), or that the server refused because the key already belongs to an account, is followed by a login. A session means that the device has its account, and the flow moves to `logged in`. Otherwise the first error stays.
- A session that the server ends is found at the next request of the account. The client logs in again once with the device key. A ban or a removal found then moves the flow to `banned` or `no account`, and the request fails with the error "access lost".
- The state `logged in` shows the recoveries that the server told at the last login. The server tells every recovery that this device has not acknowledged, at every login (ADR 0118), so the client keeps no list of its own, and a restart loses none.
- After the interface has shown a recovery to the person, it acknowledges it through the server object. The client sends the acknowledgement for the account that the flow shows. When the server has removed the notice, the recovery leaves the state. When the server answers that this device has no notice of that recovery, the recovery leaves the state too and the call fails with that answer. Every other failure leaves the recovery in the state.
- The client remembers the recoveries that it acknowledged since its start and leaves them out of the list of a later login. The answer of a login can be older than an acknowledgement, and the server never gives a device a second notice of one recovery.
- The error `origin_rejected` is a reason of its own: the saved address is not a public origin of the server (ADR 0117). It changes no state. The membership and the key stay, and the next `check` or `log in` asks again.
- A request belongs to the account that the device had when the request started. When the renewed session belongs to another account, the flow moves to `logged in` for that account with the account before, and the request fails with the error "account changed" and is not sent again. What a user confirmed for one account is never done for another.
- The client does not hold a device to the account identifier that it stored. A login for another account than the stored one is accepted and shown with the account before. A removed device can be linked or recovered into another account (ADR 0112), and the server already decides which account a key belongs to.
- The state in memory changes first and the membership in the book second. When the book cannot be written, the action reports that and the entry stays marked as unsaved; every later action of the server writes it again until a write succeeds (ADR 0710).

Ban and removal:

- `banned` keeps the key. `check` finds an unban and logs in to the same account.
- `forget account` deletes the key of this device for the server and clears the membership, in one step under the lock of the data directory. The device can then join with a new invite code and gets a new account with a new key (ADR 0113). It is also the way to give up a waiting request or link.
- After a removal the key stays, and `join` with a valid invite code creates a new account with the same key (ADR 0113).

Flow "approve a device link" on a logged-in device:

- States: `enter code`, `confirm`, `outcome unknown`, `approved`.
- `look up` asks the server for the public key and the device name behind a code and moves to `confirm`, which shows both. `confirm` belongs to the account that the device had at the lookup.
- `approve` sends the code and the public key that `confirm` showed, for that account only. The server adds the device only when the key behind the code is that key (ADR 0112). When the device has another account by then, nothing is sent and the flow returns to `enter code` with the error "account changed".
- When the answer to `approve` is lost, cannot be read although its status says success, or the server no longer knows the code, the client reads the list of devices of the account. A device with the confirmed public key means `approved`. Without such a device the approval did not happen: after a lost answer the flow stays at `confirm`, and after an unknown code it returns to `enter code`.
- When the list of devices cannot be read either, the flow is `outcome unknown`. Its action `check` reads the list again and moves to `approved` or back to `confirm`.

Waiting:

- The client offers a helper that runs `check` at an interval until a waiting state ends. The first `check` runs after one interval. Dropping the helper stops it. The flow itself starts no timer.

## Consequences

- The flows are tested without an interface, against a server in memory and against the real server.
- An interface draws a state and offers its actions. It holds no join or login logic.
- The command-line tool and the desktop client behave the same, because both run the same flows.
- A client that stopped between the answer of the server and its own write finds its account or its waiting join request again at the next `check`.
- A `check` of a device without an account that holds a key costs one more signed request.
- A link approval is never reported as failed while the device was added, and never done for an account other than the one that the user saw.
- Linking a device on all servers at once and sending the profile preset are flows of their own (ADR 0716, ADR 0717).
- A device that was removed does not learn whether its account or only the device was removed. Both show "removed".
- A login of a banned or removed account is tried once per ended session, not in a loop.
- `forget account` cannot be undone on the device. An unban after it restores the account, and this device reaches it only through a link or a recovery.
