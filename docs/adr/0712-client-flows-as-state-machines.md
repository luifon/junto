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
- A server object offers a second way to run an action of the access flow: it waits for the action or the renewal of the session that runs now and then runs the action. An interface uses it for what a person asked for; `end_session` is that way for the logout of a program that ends.
- The actions that wait in this way take the lock of the actions in the order in which they were asked for, and each holds the lock from its turn to its end. An action that `dispatch` asks for after the running action has ended finds the lock held by the first waiting action and gets `busy`. Reason: an action that waited released the lock before it ran, so an action asked for later could take the lock in that gap, and the waiting action waited again with no bound on how often. Test: `an_action_that_waits_for_the_running_one_runs_before_a_later_action` in `crates/client-core`, which failed before the change.
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

- States: `no account` (with a notice: removed, request denied, request expired, link expired, key missing), `waiting for approval` (with the request identifier once the server has answered), `waiting for link` (with the link code, the public key of this device and the expiry), `logged out`, `logged out by the person`, `logged in` (with the recoveries that this device has not acknowledged), `banned`.
- Actions: `check`, `join` with an invite code, `claim owner` with an owner code, `request access`, `request link`, `log in`, `log out`, `log out by the person`, `forget account`.
- `no account` accepts `check`, `join`, `request access`, `request link` and `claim owner`. The two waiting states accept `check`, `join`, `claim owner` and `forget account`; `waiting for link` also accepts a new `request link` and `request access`. `logged out` accepts `log in` and `log out by the person`. `logged out by the person` accepts `log in`. `logged in` accepts `check`, `log out` and `log out by the person`. `banned` accepts `check` and `forget account`.
- Before any contact the state comes from the membership in the server book (ADR 0710): a member starts as `logged out`, or as `logged out by the person` when the book holds the logout of the person.

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

Logout:

- `log out` ends the session of the program that runs. A program that ends uses it (`end_session`), and the next start of a client logs in by itself.
- `log out by the person` is the logout that a person asks for. The state in memory becomes `logged out by the person`, the book stores the logout with the membership, and then the server is told (`DELETE /api/v1/auth/session`). The session ends on this device also when the book cannot be written or the server does not answer: the token is dropped from memory, and the action reports the first failure. A session that the server was not told of ends at its expiry (ADR 0109).
- The account, the membership and the key of this device stay. Reason: a person who logs out keeps the way back to the account without a link or a recovery, and a person who wants to give up the account uses `forget account` or removes the server.
- In `logged out by the person` the client logs in only with `log in`. `connect`, `connect_when_free` and the start of a client send no request for that server, and an interface does not log in by itself (ADR 0730).
- The contact that the client makes by itself (`connect`, `connect_when_free`) chooses its action, `log in` or `check` or none, from the state after it holds the lock of the actions, not when it is asked for. Reason: a contact that waits for a running action could otherwise run a `log in` that was chosen before that action, and a logout of the person that ends meanwhile would be undone by a login that the person did not ask for. Test: `a_contact_that_waits_for_an_action_does_not_undo_a_logout_of_the_person` in `crates/client-core`, which failed with the choice before the lock. Reason: a client logs in at each start without an action of the person (ADR 0109), so a logout that lasted only until the next start or the next retry would not be a logout.
- `log in` from that state first ends the logout in memory and in the book, then logs in. A failed write of the book leaves the logout as it was. After that the state is `logged out` when the login fails, and the client tries again by itself as for every member.
- Every answer of the server that settles the state also ends the logout in the book, because such an answer comes only after `log in`.
- `junto-cli logout` is the logout of the person, and `junto-cli login` ends it. Every other command of an account refuses while the person is logged out, and says that `login` logs in. A command ends its own session with `log out` when it ends, which keeps no logout.
- Each program reads the logout from the book when it starts. A program that runs while another one logs out or logs in on the same data directory keeps its own state until its next start.

Ban and removal:

- `banned` keeps the key. `check` finds an unban and logs in to the same account.
- `forget account` deletes the key of this device for the server and clears the membership, in one step under the lock of the data directory. The device can then join with a new invite code and gets a new account with a new key (ADR 0113). It is also the way to give up a waiting request or link.
- In `waiting for approval`, and for an inactive account, whose state a client that restarted without a connection shows also while a request to return waits, `forget account` first withdraws the request on the server with the key (ADR 0111) and deletes the key only after the server confirmed it. When the server cannot be reached or refuses, nothing changes on the device and the action fails with that error. When the server answers `key_already_registered`, the request was approved: the client checks, logs in, keeps the key, and the action fails as `not available`. Reason: a key that is deleted while its request waits can still be approved into an account that no device holds, and the person who asks again with a new key makes a second request; two approvals then make two accounts.
- Consequence: `forget account` in these states, "Cancel request" and the deletion of a key of another address (ADR 0711) need a server that answers the withdrawal route. A server that cannot be reached, and a server of a release without the route (it answers `not_found`), make these actions fail each time; the API version does not tell the two releases apart. A person whose server is gone for good removes the server instead, which keeps the key file (below). A release that adds the route therefore upgrades the servers before the clients.
- Removing a server in one of these states withdraws the request first, after any running action. When the server answers `key_already_registered`, the request was approved during the removal: the client checks, logs in, starts the event stream of the server as after the login of an action, and the removal stops with `has account`; the server and its key stay, and the window shows the new account without a failure sentence. The same holds for a stale inactive state whose account is active or banned now: the check shows that state. When the server does not confirm the withdrawal, or the key cannot be read, the entry is removed and the key stays in the key directory; the same server added again at the same address uses that key, `check` finds the waiting request (the rule for a request that the book does not know, above), and a new request returns it with the name that the server holds. The book records the kept key with an end: the lifetime of a join request (7 days, ADR 0111) and one day for a clock that differs, from the removal (book format 6). The same server added again uses the key and ends the record. Each start of the client deletes the kept keys whose end has passed and whose server the book does not hold, with their records; after the end no approval of the request is possible, so the key cannot give access to an account. A deletion that fails keeps its record for the next start. The book records at most 64 kept keys; the oldest record gives way, and its key then stays without an end. Test: `manager::tests::a_kept_key_is_recorded_and_deleted_after_its_request_ended`. A removal that cannot wait for the server (`remove_server`) keeps the key in these states. `junto-cli remove` uses the removal that waits. Tests: `manager::tests::a_cancelled_request_is_withdrawn_before_the_key_is_discarded`, `a_withdrawal_that_the_server_does_not_confirm_keeps_the_request_and_the_key`, `removing_a_waiting_server_withdraws_its_request_or_keeps_its_key`, `a_removal_during_an_approval_keeps_the_server_and_the_key_of_the_new_account`, `a_request_that_already_waits_is_shown_with_the_name_that_the_server_holds` and `a_second_press_while_the_request_is_sent_makes_no_second_request` in `crates/client-core`. The window test `flows::a_removal_that_meets_an_approval_keeps_the_server_and_tells_no_failure` holds the withdrawal of the removal at a proxy, approves the request meanwhile, and requires no failure sentence, the server in the book and the account opened with its event stream; it failed before the stream was started there: the server stayed in the state `Standby`, logged in without a connection.
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
- After the logout of the person no request of that server is sent until `log in`: the window shows no lists of that server and gets no event or notice of it.
