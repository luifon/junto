# 0513. No speaking state outside a call

## Status

Accepted. Replaces the relay of speaking marks to the people outside a voice channel.

## Context

ADR 0507 first decided that the channel list shows, for the voice channels that a user is not in, a mark for who is speaking, reported by the clients inside. The server kept those reports in memory, computed the set of speakers of each call once per second with a read transaction, and recorded the event `call_speaking` for the accounts that may see who is inside. The people inside a call never used it: they draw the levels that the media server gives them. A person outside a call does not take part in it, and who speaks there is not their information.

## Decision

- Only the people in a call see who speaks in it. The channel list, the stage and the panel of the call of the device draw the level meter of each person from the levels that the media engine measures (ADR 0507, ADR 0734).
- For any other call the rows of the people keep their other marks (microphone muted or blocked, sound off, share, raised hand, connecting) and draw no meter. An open microphone has no mark there.
- The client reports nothing about speaking. The server has no speaking state: no report route, no relay task and no event.
- The route `PUT /api/v1/call/speaking` and the event `call_speaking` are removed from API version 1 (ADR 0201). A 0.2 client that reports speaking to a server of this version gets `not_found` for the unknown path. Its media code treats the answer as a failed report and sends it again every 3 seconds while it is in a voice channel call; the request changes nothing on the server, and the call is not affected. It receives no speaking event, so it draws no speaking mark outside its call. A client of this version that is connected to a 0.2 server skips the event `call_speaking`, as it skips every event type that it does not know (ADR 0718), and sends no report.

Tests: `shell::channels::tests::the_level_meter_is_drawn_only_for_the_people_of_the_call_of_this_device`, the view tests `a_voice_channel_shows_its_people_with_their_marks_and_the_state_of_its_call` and `the_call_of_this_device_shows_measured_levels_and_the_panel_of_the_call` (no speaking mark outside the call), and the server test `calls_review::the_retired_speaking_report_is_an_unknown_path`.

## Consequences

- A call no longer costs one read transaction per second on the server, and changing speakers no longer use positions of the event log.
- A modified client cannot show false speaking marks to people outside a call.
- A person outside a call sees who is in it, not who talks.
