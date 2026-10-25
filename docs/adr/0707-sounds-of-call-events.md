# 0707. Sounds of call events in the client

## Status

Accepted

## Context

The client played one sound: the tone of an incoming direct-message call, a 440 Hz sine at 0.08 of full scale, 0.8 s in every 3 s, on its own thread (ADR 0733). A person in a call learned only from the window that another person joined or left, that a share started, or that the own microphone or sound changed. The rules of the system notifications kept one call point for the sound of an invitation into a voice channel, which played nothing (ADR 0739). Prepared sounds with melodies were tried and judged unpleasant: a sound that plays at every change of a call must be short, quiet and plain.

## Decision

Sounds:

- Each sound is one or two plain sines in sequence, with a rise and a fall of 10 ms (a raised cosine) at the start and the end of each tone, so that the device does not click. The peak of every sound is 0.03 of full scale (about −30 dBFS), 8.5 dB below the tone of an incoming call. The frequencies are between 330 and 587 Hz. Two events that are a pair (on and off, joins and leaves) have different sounds; no two events have the same sound.

  | Event | Tones | Length |
  |---|---|---|
  | Another person joins the call | 392 Hz 70 ms, pause 20 ms, 523 Hz 70 ms | 160 ms |
  | Another person leaves the call | 523 Hz 70 ms, pause 20 ms, 392 Hz 70 ms | 160 ms |
  | The person's microphone turns off | 392 Hz | 80 ms |
  | The person's microphone turns on | 523 Hz | 80 ms |
  | The person's sound turns off | 370 Hz | 150 ms |
  | The person's sound turns on | 494 Hz | 150 ms |
  | A share starts in the call | 440 Hz 60 ms, pause 40 ms, 440 Hz 60 ms | 160 ms |
  | The person is no longer in the call | 330 Hz | 200 ms |
  | An invitation into a voice channel arrives | 587 Hz | 120 ms |

- The sounds are made by the program (`junto_media::audio::sounds`); the client carries no sound files.

Who hears them:

- Only the device that is in the call plays the sounds of the call: the decision reads the call of this device (`CallsView`) and the people whom the server lists in that call. Another device of the same account, and a device whose person is outside the call, play nothing.
- A join or a leave is a change of the accounts that the server lists in the call; a share that starts is an account of the list whose share was absent before. The list counts only from the moment the server lists the person of this device. The people who are in the call before the person enters, and the people whom a call that hides who is inside lists at once when the server has seen the device (ADR 0517), play no sound. A person who joins between the seat of the device and the listing of its person plays no sound either.
- The microphone and the sound are the person's own settings in the calls (`muted`, `deafened`). A block of a moderator plays nothing; a close of the microphone by a rule of the calls plays the sound of a closed microphone, because the person's setting changed.
- The end of the call of this device plays one sound, whatever ended it. A move into another call, and a new seat in the same call, play nothing; the list of the new call counts from its start.
- While the sound of the person is off, only the sound of the change of the sound plays. The tone of an incoming call keeps its own rules (ADR 0515, ADR 0733): only the called account rings, not the caller (test `state::direct::tests::an_outgoing_ring_makes_no_local_ring_sound`).
- The invitation sound plays at the call point of ADR 0739, outside quiet times and while the sound of the person is on.

Setting:

- One switch, "Call sounds" in the Voice section of the settings, turns every sound of this record off. It is stored in `voice.json` as `call_sounds`, on by default; a file of version 3 is read with the sounds on (ADR 0710). It does not change the tone of an incoming call.

Player:

- `SoundPlayer` opens the output device on a thread of its own, plays one sound, writes 150 ms of silence after it so that the device has played the last samples, and closes the device. A request only sends the sound to that thread and returns at once, as the requests of the ring tone do (ADR 0733). The device is the output device of the call, or the chosen output device when no call has one. Requests that arrive while a sound plays are played in order after it, each sound at most once, and at most four.
- The decision is a function of the state before and after a change, `state::call_sounds::sounds`, called at each change of the calls and of the state of a server.

Tests:

- `audio::sounds::tests::every_sound_is_short_and_quiet_and_starts_and_ends_in_silence` renders each sound: the sound that is not silence lasts 60 to 210 ms, the peak is at most 0.03 and less than half of the ring, and the first and the last millisecond stay below a tenth of the level, which fails without the rise and the fall. `no_two_events_have_the_same_sound` compares the sounds. `a_sound_reaches_the_output_once_and_the_output_closes_after_it` plays a sound through a synthetic output and checks its length, its peak and that the output closes. `a_device_that_is_slow_to_open_does_not_hold_the_caller` holds the open of the device and fails when the request has not returned within 10 seconds. `a_device_that_does_not_open_does_not_stop_the_next_sound` checks the next sound after a missing device.
- `state::call_sounds::tests` check each event, and that nothing plays for the people present at the person's own entry, for a call that hides who is inside, for a move, for a device outside the call, while the sound is off, and with the setting off.
- The window test `calls::the_sound_settings_of_the_application_are_kept_over_a_restart` turns the switch off and reads it from the file after a restart.
- The window tests play the tones on a synthetic output of their own (`Window::tones`), apart from the output of the calls (`Window::sound`): the synthetic host keeps one record of what was played, which a second output stream would clear and interleave with the sound of the call that the tests measure. The live test `live::tones::the_person_in_the_call_hears_a_tone_for_each_event_and_none_for_the_people_present` enters a call in which the other person already is and requires that no tone plays, then requires the tone of a closed microphone, of a person who leaves, of a person who joins and of the end of the call, each at its frequency, with a real media server.

## Consequences

- The level of a sound on real speakers depends on the volume of the system and of the device; the tests check the samples, not the loudness that a person hears.
- Each sound opens and closes the output device. On a system that is slow to open a device, a sound starts later than its event; the window does not wait for it.
- A person who joins before the server lists the person of this device is not heard as a join.
