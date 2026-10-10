# 0506. Call admission by repeated checking

## Status

Accepted, pending step 0

Open: no admission check ran during or after a loss of the link between the two hosts, and requests to the LiveKit API through the link were not tested (`docs/step-0/README.md`, test 5, third check). The token facts and the behaviour of calls at a loss of the link are recorded.

## Context

The server decides who may be in a call, and LiveKit admits a participant who presents a token that the server issued. LiveKit's notifications to the server can be lost. A single check at the time of joining therefore does not keep a removed person out of a call.

Step 0 measured how long a token admits a participant (`docs/step-0/README.md`, test 5):

- A join token stays usable after its stated lifetime. A token with a lifetime of 60 seconds was accepted 54 seconds after its expiry and refused 65 seconds after it.
- The LiveKit server sends every connected client a new token with a lifetime of 600 seconds, whatever the lifetime of the join token.
- A removal through LiveKit's API ends the connection and does not end the validity of any token. A removed participant joined again with the original token, and 80 seconds after the removal with the token of 600 seconds. Each new connection gets a new token of 600 seconds.

## Decision

- The server enforces call admission, by repeated checking.
- Join tokens are valid for about a minute. The server treats a token with a stated lifetime of N seconds as usable for N + 60 seconds.
- A removal or a ban removes the participant through LiveKit's API.
- The server compares the participants of every active LiveKit room with the people who are allowed in it. It does this at short intervals and whenever its connection to LiveKit returns.
- The comparison runs for as long as the room exists. It does not stop when the join token of a removed person has expired, because that person can hold a longer token from the LiveKit server, and each successful return gives a new one.
- The server removes anyone who is not allowed, each time that person is found.
- The comparison skips the participants of the recorder. The recorder is in the room as one participant per recording job, and the participant of a recorder that crashed stays in the room for about 20 seconds (ADR 0601).
- The decision whether a person is allowed uses the permission function of ADR 0301.

## Consequences

- Known limit: a removed person can be back in a call for the seconds until the next check, and again after each removal, for as long as the room exists.
- The lifetime of the join token limits only the first join. It does not limit how long a removed person can return.
- Known limit: in the two-host deployment form (ADR 0003), a check runs during a loss of the link between the two hosts only when the server's requests to the LiveKit API do not pass through that link. The LiveKit API on the public address of the media server answered normally during a loss of the link of 30 seconds. The path of the server's requests is not tested, so this record keeps the limit that no check runs while the link is down.
- Calls continue while the link is down (ADR 0003), so a person who was removed during that time stays in the call until the first check after the link returns.
- The server does not depend on LiveKit's notifications for admission. A lost notification is corrected at the next check.
- The server keeps a current list of the people who are allowed in each active call, including people who entered by invitation or move through a lock or a user limit (ADR 0504).
- The repeated comparison costs requests to LiveKit's API for every active call at every interval.
- Not tested: ending a room and continuing the call in a room with a new name, which would make every old token useless, and a LiveKit setting that shortens the token of 600 seconds.
- The length of the interval is fixed by ADR 0509: 2 seconds.
