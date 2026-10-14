# 0788. An action that opens a microphone, a camera or a share names its call and seat

## Status

Accepted

## Context

A client is in one call at a time (ADR 0107), and one object holds that call (ADR 0734). A function of that object acted on the call that was active when the function ran. The window draws a control for the call that it shows, and the person uses the control later. Between the two the call of the device can become another one without an action of the person in that window: a person with "move members" moves the person (ADR 0511), the other window of the application joins another call (ADR 0772), or a request for a new token gives another seat (ADR 0785).

Three kinds of control then opened something in a call that the person did not choose it for. The dialog of a share stayed open over the change, and its confirm step shared the chosen screen and sound with the people of the new call. The camera control and the microphone control read the state at the click and did the opposite of it: a control that was drawn to stop a camera started one. The control "Unmute" of meeting mode (ADR 0739) opened the microphone in whatever call was active.

## Decision

A seat is a call on one server and the identity of the device in its room (ADR 0508): `junto_media::calls::Seat`.

Every function of the calls that opens a microphone, a camera or a share takes the seat that the person saw when the action was made: `start_share_in`, `start_camera_in`, `unmute_in` and `retry_microphone_in`. The session of a call compares the seat with its own call and identity at the moment at which it runs the command, which is after every change of its seat that came before. For another seat the command has no effect.

- The person is told. The view of the calls states the kind of the last action without effect (`CallsView::unused`) until `Calls::dismiss_unused`. The window shows one sentence with the other notices of a call and puts the statement away.
- Without a call nothing is told: the end of the call was told.
- A close is not bound to a seat: `mute`, `stop_camera`, `stop_share` and the sound setting act on the call of the moment. A close in another call than the intended one stops something that the person wanted stopped, or nothing.
- A close by the person does not wait for an open that is on its way. The calls count the closes of the microphone by the person; an open carries the count of its moment, and the session opens nothing when the count is another one.
- The setting "microphone closed" still belongs to the client and not to a call (ADR 0734). `unmute_in(None)` is the action of a person who is in no call: it opens the setting for the next call. When a call began after the control was drawn, it opens nothing and the person is told.
- A renewal that keeps the identity keeps the seat. A renewal that gives another identity is another seat.
- Push-to-talk is not bound: the key opens the microphone only while the person holds it. The choice of an input device is not bound: it is a setting of the installation.

The functions without a seat (`start_share`, `start_camera`, `set_muted`, `retry_microphone`) exist only with the feature `test-support`, for a test that acts at the moment of the call. The application is built without that feature, so its code cannot open something without a seat.

In the window a control holds its action from its drawing:

- The view that the window draws holds the seat and the action of the microphone control (`Controls::seat`, `Controls::microphone_action`): close, or open at the seat that is drawn. The listener of the control holds that value. The sound control holds the value that it sets.
- The camera control holds "stop", or "start at the seat", from the one reading of the calls that gives its look.
- The share control ends a share or opens the dialog by the view that the window draws.
- The control "Unmute" of meeting mode and the control that opens a failed microphone again hold the seat that is drawn.
- The dialog of a share holds the seat that the window draws at its opening. The main window and the call window close it when the device is at another seat, and the person is told that nothing is shared. The confirm step passes the seat, so a confirmation that runs before the window has closed the dialog shares nothing either. The sentence is shown also when the call has ended.

## Consequences

- A person who is moved while a share dialog is open chooses again in the new call.
- A click on a control that was drawn for another call does nothing but a close, and the person reads why.
- A control of the microphone or of the camera that two windows show does in each window what that window showed. A second click on a control that another window has already used does not undo it.
- A request for a new token that gives another identity gives another seat. The camera of the device is published at that seat when the session admits it (ADR 0794).
- Tests: `an_action_for_another_seat_opens_nothing` and `an_unmute_without_a_call_is_for_no_call` in `calls::ordering_tests` of `crates/media`; `a_share_asked_for_in_one_call_does_not_start_in_the_next` in `crates/media/tests/calls`; `a_share_dialog_belongs_to_the_call_and_the_seat_of_its_opening` and `the_controls_hold_the_action_and_the_seat_that_they_show` in `crates/client`; in the window tests with a media server `a_share_choice_cannot_cross_a_call_move`, `a_rendered_camera_stop_never_starts_a_replacement_call` and `a_drawn_unmute_does_not_open_the_microphone_in_a_replacement_call`.
