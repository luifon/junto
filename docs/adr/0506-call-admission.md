# 0506. Call admission by repeated checking

## Status

Accepted, pending step 0

## Context

The server decides who may be in a call, and LiveKit admits a participant who presents a token that the server issued. A LiveKit token stays valid until it expires. LiveKit renews the token of a connected client. LiveKit's notifications to the server can be lost. A single check at the time of joining therefore does not keep a removed person out of a call.

## Decision

- The server enforces call admission, by repeated checking.
- Join tokens are valid for about a minute.
- A removal or a ban removes the participant through LiveKit's API.
- The server compares the participants of every active LiveKit room with the people who are allowed in it. It does this at short intervals and whenever its connection to LiveKit returns.
- The server removes anyone who is not allowed.
- The decision whether a person is allowed uses the permission function of ADR 0301.

## Consequences

- Known limit: a removed person can be back in a call for the seconds until the next check.
- Known limit: in the two-host deployment form (ADR 0003), no check runs while the link between the two hosts is down.
- The server does not depend on LiveKit's notifications for admission. A lost notification is corrected at the next check.
- The server keeps a current list of the people who are allowed in each active call, including people who entered by invitation or move through a lock or a user limit (ADR 0504).
- The repeated comparison costs requests to LiveKit's API for every active call at every interval.
- Step 0 observes what happens to calls and to admission checks when the link between the hosts drops (`docs/step-0/README.md`, test 5).
- The length of the interval is not fixed by this record.
