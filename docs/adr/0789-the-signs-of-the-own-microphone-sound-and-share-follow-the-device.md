# 0789. The signs of the own microphone, sound and share follow the device

## Status

Accepted

## Context

The window shows each person of a call with a microphone sign, a sound sign and a share mark: on the tile of the stage, in the list of the call and in the channel column (ADR 0735). For the other people these come from the state of the server. For the person of the device they came from the state of the server too, as soon as the server listed the person.

What the server lists for a person is the last report of that person's device (ADR 0510). The device sends a report after a change, at most one at a time, 500 ms apart, and again after 3 seconds when it failed (ADR 0734). The report and the event that carries it back can be late, and they do not arrive while the event stream is interrupted. During that time the tile of the person said "microphone closed" while the device sent the microphone. The control of the profile strip, which reads the calls of the device, said "open" at the same time.

## Decision

The row of the person in the call of this device is derived from the calls of the device (`junto_media::calls::CallsView`):

- The microphone sign is "held" when a block is known: the block that the calls hold, or the block that the server lists for the person. A block is a rule of the server and is shown as one.
- Without a block the sign is "open" when the person's own setting is open and the microphone of the engine runs, and "closed" otherwise: while the device enters or connects again, and while the system gives no microphone. A block whose end waits for its event is still shown as a block (ADR 0785).
- The sound sign is the person's own setting, or a sound block.
- The share mark is the share of the engine.
- The level and the speaking mark are the ones of the engine, as before.
- The hand is the one that the server lists: the server holds it.

The settings that the server lists for the person ("muted", "sound off") and its share mark are not shown for the device that sent them. They are shown for the other people, and for the person when this device is not in that call: the row is then the seat of the account on another device.

## Consequences

- The tile, the row of the list and the row of the channel column of the person say "open" only while the device sends the microphone. The control of the profile strip shows the person's setting and a block, as before: it says "open" for an open setting also while the microphone does not run, and the line above it tells a microphone that cannot be used.
- The other people see the person as the server lists the person, so their signs can be older than the sign that the person sees.
- The sign "closed" is shown from the moment at which the person closes the microphone. The engine closes its gate after that, in its own task.
- Tests: `the_listed_own_tile_tracks_local_controls_before_server_reports` in `crates/client/src/state/view/tests.rs`, and `the_own_tile_is_open_while_the_report_of_the_state_waits` in the window tests with a media server.
