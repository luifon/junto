# 0787. A login without an answer keeps the state of the session

## Status

Accepted

## Context

When a server no longer accepts the session of a device, the client core logs in again once with the device key (ADR 0109, ADR 0712). The answer decides the state: a session, `banned`, or `no account` after a removal. A login that failed for any other reason left the device `logged out`: also when the server was not reached, did not answer in time, or answered that it failed. Such a failure tells nothing about the account.

`logged out` ended what belongs to the account on that device. The media session ended the call of the device (ADR 0785), the window stopped showing the state of the server (ADR 0730), and the event stream client had no session to connect with. A person in a call lost the call because one request of the client did not reach the server, while the media connection, which goes to another server, still worked.

## Decision

A login after an ended session has three outcomes.

- The server answers about the account. A session gives `logged in`; `account_banned` gives `banned`; `unknown_device` gives `no account` with the notice "removed" (ADR 0712). Unchanged.
- The server refuses the login for another reason, for example `origin_rejected`. The device is `logged out`, as before. A call of the device on that server ends.
- The login gets no answer about the account: the server was not reached, the answer was lost, the server answered `busy`, `timeout`, `internal` or `unavailable` or with a status of 500 or above that the client cannot read, or the server limited the request. The access flow stays in the state that it had, `logged in`, and the client core keeps the session that the server ended. The request that found the end of the session fails with the error of the login.

Who logs in again after the third outcome:

- The next request of the account is sent with the kept session. The server answers that the session has ended, and the client core logs in again, as for the first end.
- The event stream client stops when its session ends and waits for another session (ADR 0718). The client core now tells it that the login got no answer. The stream client then tries the kept session again after the wait that the end of the session set, which is the wait of a lost connection and doubles up to its bound. A server that cannot be reached gives the waits of a lost connection. A server that answers again refuses the session, the stream client reports the end again, and the client core logs in again.
- After a login without an answer, the client core sends no further login of this kind before a wait has passed: the time that the server named, or 1 second, which doubles with each further login without an answer, up to 60 seconds. A request or a report of the stream client that finds the ended session during the wait fails at once with an error of the kind "not reachable", and no login is sent. A session ends the wait. Reason: several requests of a call are repeated every few seconds, and each would send a login to a server that limits logins.
- While the login runs, the device has no session. The level of the server is not announced to the event streams during that time, so the stream client keeps its state and its waits; the end of the login announces the level.

The call during that time follows the rule that ADR 0734 states for an interrupted event stream: the call is kept, a place that the server ended is not noticed, and no new token is asked for. The media session changes nothing without a live state: the admission and the blocks stay as the last state told them (ADR 0785), also while the device has no session because a login runs. The first state after the stream is live again is applied by its position.

The media engine takes the account of a call from the session of the device, and, while a login runs and the device holds no session, from the access flow, which still shows the account as `logged in`. A join whose answer arrives in that time starts its call; a move of the account that the stream tells in that time is followed, by the account of the call; and a direct call names the other account of the conversation. A login that gives another account ends the call when the media session reads the state. Reason: the media engine read the account of the session only, so the answer of a join that arrived during a login started no call, the join was reported as ended by the person and its place was given up, a move in that time was not followed, and a direct call could name the own account as the other one. Tests: `a_join_answer_that_arrives_while_a_login_runs_starts_the_call` and `a_move_that_is_told_while_a_login_runs_is_followed` in `crates/media`.

What reads `logged out`, and what a login without an answer changes for it:

- The media session ends a call when the access flow is not `logged in` for the account of the call. It keeps the call now.
- The window shows a server as "logged out" when the flow is `logged out` and the server answers, and as "not reachable" when the flow is `logged out` and the server does not answer. Now the flow stays `logged in`: the window shows the last known state with the mark of an interrupted connection.
- The window contacts every `logged out` server again at an interval. The stream client does that for a kept session.
- The server book stores the membership, not the session. Unchanged.
- The command-line tool prints the state of the flow. It prints `logged in`.

## Consequences

- A person stays in a call while the API of the server cannot be reached, for as long as the media connection works. The reports of the call to the server fail during that time and are sent again.
- A device can show `logged in` with a session that the server has ended. Each request of the account fails until a login succeeds, with the error of that login.
- A device that the server removed or banned while the server cannot be reached learns it at the first login that gets an answer.
- A login that the server limits is counted as "no answer": the limit of the server tells nothing about the account.
