# 0513. Speaking marks for the people outside a voice channel

## Status

Accepted

## Context

ADR 0507 decides that the channel list shows, for the voice channels that a user is not in, a mark for who is speaking, that the clients in the voice channel report this, and that a voice channel that hides who is inside shows nothing. A client inside a call has the levels from the media server; a client outside has no connection to the room. Speaking changes several times per second. The event log keeps 5000 positions and a reconnect replays at most 1000 events (ADR 0207).

## Decision

Reports:

- A device in a call reports whether its own person speaks, with `PUT /api/v1/call/speaking` and a truth value. It reports no level, and a level in the body is ignored.
- A device reports for its own person only.
- One session sends at most 20 reports per 10 seconds. A further one gets `rate_limited`.
- A report that the person speaks lasts 5 seconds. A device renews it while the person speaks, and ends it with a report that the person does not speak.
- The server keeps the reports in memory.

Relay:

- At each interval of one second, and for each call that has a report or whose last event carried a speaker, the server computes the set of speakers from the record of that moment: the people who report that they speak, whom the media server lists, whose own microphone setting is not muted and who have no microphone block. The set has at most 16 accounts.
- The server compares that set with the set that the last event of the call carried, and records the event `call_speaking` with the whole set when the two differ. It does not compare the reports.
- A block, a removed block, a mute, a leave and a change of the hide setting therefore change the marks at the next interval, also while the reports stay the same.
- One call gets at most one such event per second.
- The event goes to the accounts that may see who is inside the voice channel (ADR 0514), and is decided when it is delivered (ADR 0011).
- The set of a voice channel that hides who is inside is empty, so the server records no speaker of it. It records no speaking event for a direct-message call. The people inside have the levels from the media server.

## Consequences

- A mark in the channel list follows the speech with a delay of up to one second. A block removes the mark of a person who keeps reporting within one second.
- Each call with a report costs one read transaction per second.
- A call with changing speakers uses up to one position of the event log per second. Three such calls fill the 5000 kept positions in about 28 minutes, and a client that was disconnected for longer than about 5 minutes of such traffic loads a snapshot instead of a replay (ADR 0207). A snapshot carries no speaking marks; the next event of a call carries its whole set.
- A modified client can report that its own person speaks. It cannot report for another person, and not while it is muted or blocked.
- A restart of the server loses the reports. The next report of each device restores them.
- The reports and the set of each call are per process (ADR 0011).
