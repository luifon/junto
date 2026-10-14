# 0794. A close stops frames at once, and a device does not open by itself

## Status

Accepted

## Context

The engine of a call runs as one task that handles one message at a time (ADR 0733). A close reached the media only when that task took its message: the mute of the person, a microphone block of the server, the stop of the camera or of the share, and the end of the call. Some messages take long, because they open a sound device or leave a room. During that time the view said "muted" while the microphone was sent.

Three paths opened something without an action of the person. After a failure of the input stream the engine opened the default input of the system when the chosen microphone was gone, and sent from it (ADR 0733). After a new media connection the engine published the camera again, also at a seat that the server had not listed (ADR 0770). After the media server had removed the device, the session asked for a new token as after a lost connection, and the event of the removal could arrive later (ADR 0734).

## Decision

A close does not wait for the engine task.

- The microphone has a count of closes from outside the engine task (`CaptureLink::stop_now`). The thread of the input device and the queue of waiting frames read it with the gate of the engine: from the moment of a close they send silence. The gate opens again only when the engine task has taken a command that was sent at or after the close, so what the engine held before the close cannot open it.
- The close of the person (`Calls::mute`) and the release of the push-to-talk key count at once, before the session and the engine take them. A press of the key carries the count of the moment of the press, so a press that the engine takes after the release does not open. Controls of the session that close the microphone (the person's setting, a block, a seat that is not admitted) count when the session hands them to the engine.
- Controls that open the microphone carry the count that the session read together with the person's setting, under the lock that a close of the person holds. A close that follows that reading is later than those controls, so they do not open the gate after it.
- The camera and the share each have a count of stops at the handle of the engine. A capture holds the count of the moment at which its start was asked for; its pictures and its sound are dropped when the count is another one. A stop that is asked for after a start therefore stops that start too.
- The end of a call counts all three.
- An open is the direction that waits: it takes effect when the engine task takes its command.

A device does not open, and nothing is sent again, without the person.

- A microphone that the engine opens on another device than the call used before sends nothing: after a failure of the input stream, and also after a new media connection or the end of a block, when the device of the call is gone by then. The engine states it (`EngineState::input_replaced`), the session closes the person's own setting and tells the person why (`MicrophoneClosed::DeviceChanged`), and the person opens the microphone or chooses a device. A stream that is opened again on the same device sends as before, and so does the device that opens when the person chooses one or asks for the microphone again. The device that the person chose in the sound settings is not a replacement either: when it was not connected and the call sent from the default, and it is connected at a later open, the call sends from it.
- The camera is published at a seat only while the session admits the seat (`Controls::camera_held`). After a new connection for the same seat it returns as ADR 0770 states; at another seat it waits until the server lists the seat and the events up to the answer that gave it are read (ADR 0785). When the admission of a seat ends, the track of the camera is withdrawn and the capture goes on for the next publication. A publication that the media server completes after that is withdrawn too: each request for a publication has a number, and only the result with the number that the camera waits for is its publication.
- A camera whose media connection is lost is captured for 30 seconds more, counted from the first loss until a connection is made, however many tokens are tried in between. Then the camera ends with "interrupted", and the person starts it again. Before, the capture had no limit while the connection was lost.
- After the media server removed the device (the participant was removed, the room was closed or deleted, or the identity joined again elsewhere), the session asks for no new token until a state that was loaded after the loss shows the place. A removal by a rule of the server is then read before a token is asked for, also when its event is late. One state is asked for per loss, also when the engine tells the same loss again after the token has arrived. An end of the place that the events tell while that state is loaded ends the call at once, and the state of the same history keeps the reason that the events told.
- A change of the default output of the system is not a failure of the output stream: the stream follows it. A change that an input stream reports stays a failure, so the microphone is handled as above.
- Sound settings that cannot be read give the defaults, as before; the window states that they were not read, on the settings page and in a notice, and the microphone setting starts closed, because the defaults send an open microphone all the time.

## Consequences

- "Muted" on the screen and silence on the wire begin together.
- A person whose headset is removed during a call is muted on the microphone of the computer, and reads why.
- After a removal by the media server a call returns one snapshot later than after a lost connection.
- A reload of the state of the server follows each removal by the media server.
- Tests: `a_close_from_outside_the_engine_task_stops_the_frames_until_the_engine_has_taken_it`, `controls_that_open_do_not_open_after_a_close_that_followed_their_reading`, `closing_the_microphone_stops_the_gate_of_the_call_at_once`, `a_press_of_the_talk_key_does_not_open_after_its_release`, `a_removal_that_the_events_tell_ends_the_call_with_its_reason_while_a_state_is_loaded`, `a_microphone_on_another_device_closes_the_setting_and_is_told`, `a_removal_by_the_media_server_waits_for_a_new_state_before_a_token` and `a_seat_given_by_a_renewal_sends_nothing_before_the_events_reach_its_answer` in `calls::ordering_tests` of `crates/media`; `a_changed_default_output_is_not_a_failure_of_the_stream` in `audio::cpal_host`; `a_chosen_microphone_that_is_there_again_is_not_a_replacement` and `a_publication_that_ends_after_a_hold_is_not_the_publication_of_the_camera` in `engine`; with a media server `the_chosen_devices_are_opened_and_a_missing_one_gives_way_to_the_default`, `a_microphone_that_is_gone_at_a_new_connection_is_not_replaced_without_the_person`, `a_chosen_microphone_that_is_there_again_is_used_and_the_setting_stays_open` and `a_camera_without_a_media_connection_ends_after_its_limit`; in the window tests `sound_settings_that_cannot_be_read_are_told_and_close_the_microphone`.
