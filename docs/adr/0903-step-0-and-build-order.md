# 0903. Step 0 before product code, and the build order

## Status

Accepted

## Context

Several decisions depend on behaviour of operating systems and third-party components that documentation does not settle: sound capture, the state of gpui on each system, the recorder, the macOS key store and permissions after an update, the network paths, the capacity of a media host, and the diarization component. Product code that is built on a wrong assumption about one of these has to be written again. Features also depend on each other: calls need accounts and permissions, and recording needs calls.

## Decision

Step 0:

- Step 0 is a set of eight technical tests that run before product code. Product code starts after the results of step 0 are recorded in `docs/step-0/README.md` and in the affected ADRs.
- Each test has a stated result that passes or fails.
- A failure changes the design or removes a feature from the first version. The results of step 0 can remove a feature from the first version. They do not add to it.
- The tests are:
  1. Sound capture on each of the three systems (ADR 0502).
  2. gpui on each system, and one fixed set of dependency revisions (ADR 0701, ADR 0702).
  3. The recorder on the data host, connected to the media server on the public host (ADR 0003, ADR 0601, ADR 0602).
  4. The macOS update with the project's own certificate (ADR 0103, ADR 0704).
  5. The network: TURN over TLS only, and a drop of the link between the hosts (ADR 0004, ADR 0506).
  6. The capacity of the media host (ADR 0003).
  7. The diarization component (ADR 0603).
  8. A packaged install on a clean system of each kind (ADR 0705).

Build order after step 0:

1. Base: the server with accounts and device keys, invite codes, both levels of roles and the permission rule as one function, the default workspace, and the storage of the audit log; the client window in the visual design of ADR 0703, with state for several servers; CI on three systems.
2. Calls: voice channels, microphone and sound, screen share with sound on each system, the media server, admission checks.
3. Recording: the layout page, tracked recording jobs with segments, files, transcripts, the result posted in the channel.
4. Text: messages, formatting, attachments, replies, reactions, threads, pins, unread signs, notifications, search.
5. Administration: screens for roles and overrides, several workspaces, permanent messages, access tokens, the setup tool.
6. Several servers: the server book, joining by approval, background connections, device linking across servers, the update notice.
7. The rest: archive and export, meeting mode, lock, limit, hidden occupants, invite and move by dragging, the detached call window.

Use:

- The application is put into use only after every first-version feature is built and tested end to end.

## Consequences

- No product code exists while step 0 runs.
- An ADR with the status "Accepted, pending step 0" can change when the result of its test is recorded.
- Calls and recording are built before text.
- The permission function and the audit log storage exist before the features that use them.
- Client state for several servers is built in step 1, and its screens in step 6 (ADR 0703).
- There is no period in which the application is used with part of the first-version features.
