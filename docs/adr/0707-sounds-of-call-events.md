# 0707. Sounds of the application

## Status

Accepted

## Context

The client played one sound: the tone of an incoming direct-message call, a 440 Hz sine at 0.08 of full scale, 0.8 s in every 3 s, on its own thread (ADR 0733). A first set of sounds covered the events of a call: a person who joins or leaves, the own microphone and sound, a share that starts, the end of the call, and an invitation. A person told that the sounds of the microphone and of the sound were too close to each other, and asked for the sounds that Discord and Slack have: a message, a mention, and every action of a call. Prepared sounds with melodies were tried and judged unpleasant: a sound that plays at every change must be short, quiet and plain.

The list of actions comes from Discord (its sound settings: Message, Mute, Unmute, Deafen, Undeafen, Voice Disconnected, PTT Activate and Deactivate, User Join, User Leave, User Moved, Incoming and Outgoing Ring, Stream Started and Ended, Viewer Join and Leave), Slack (the notification sound of messages, the huddle sounds of people who join and leave, the ring) and Teams (the ring, notifications, a person who waits in the lobby).

## Decision

Sounds:

- Each sound is a sequence of plain sines, glides and pauses. A glide moves its frequency evenly from one value to another; its phase is the integral of the frequency, so the wave stays continuous. Each tone and glide rises and falls in 10 ms (a raised cosine), so that the device does not click. The peak of every sound is 0.03 of full scale (about −30 dBFS), 8.5 dB below the tone of an incoming call. The frequencies are between 262 and 880 Hz, and a sound lasts from 40 to 360 ms.
- A kind of event has a pattern of its own, so that a person can tell the event without looking: the microphone two short tones in the upper register; the sound of the person three short tones in the lower register; "off" falls in pitch and "on" rises; people who join and leave one glide that rises or falls; a share two equal tones; a request three equal tones; a recording two equal tones and a third that rises or falls; a message one tone and a mention two tones with a wide rise.

  | Sound | Event | Parts (Hz, ms) |
  |---|---|---|
  | Message | a new notice of a message for all or of a reply in a followed thread | 740 for 90 |
  | Mention | a new notice of a mention, a direct message or a missed call | 659 70, pause 20, 880 110 |
  | CallJoined | the person enters a call (the server lists the person) | glide 330→494 110, pause 20, 659 90 |
  | CallLeft | the person is no longer in the call | 440 70, pause 20, glide 392→262 140 |
  | Moved | a moderator moves the person into another voice channel | glide 392→523 90, pause 30, glide 392→523 90 |
  | ConnectionLost | the media connection of the call goes from connected to reconnecting | 330 120, pause 60, 330 120 |
  | PersonJoined | another person enters the call | glide 392→587 150 |
  | PersonLeft | another person leaves the call | glide 587→392 150 |
  | MicrophoneOff | the microphone closes, by the person or by a block | 659 55, pause 25, 523 55 |
  | MicrophoneOn | the microphone opens | 523 55, pause 25, 659 55 |
  | SoundOff | the sound of the person turns off, by the person or by a block | 440 45, pause 20, 370 45, pause 20, 294 45 |
  | SoundOn | the sound of the person turns on | 294 45, pause 20, 370 45, pause 20, 440 45 |
  | TalkStarts | the key of push-to-talk is pressed in a call | 880 40 |
  | TalkEnds | the key of push-to-talk is released | 587 40 |
  | ShareStarted | a share starts in the call | 494 60, pause 40, 494 60 |
  | ShareEnded | a person in the call stops sharing | 392 60, pause 40, 392 60 |
  | ViewerJoined | a person starts to watch the share of the person | glide 523→659 80 |
  | ViewerLeft | a person stops watching the share of the person | glide 659→523 80 |
  | Invitation | an invitation into a voice channel arrives | 523 80, pause 20, 659 80, pause 20, 784 110 |
  | Request | a person asks to enter the call; a join request for a person who decides it | 659 60, pause 60, 659 60, pause 60, 659 60 |
  | RecordingStarted | a recording of the call starts, or the person enters a recorded call | 587 80, pause 40, 587 80, pause 40, 784 120 |
  | RecordingStopped | a stop of the recording of the call is asked for, or it ends | 587 80, pause 40, 587 80, pause 40, 440 120 |
  | HandRaised | a person raises a hand, for a person who may lower it | glide 523→784 120 |

- The sounds are made by the program (`junto_media::audio::sounds`); the client carries no sound files.
- No sound for an outgoing direct call: only the person called rings (test `state::direct::tests::an_outgoing_ring_makes_no_local_ring_sound`). No sound for a camera, a reaction or a sent message: none of Discord, Slack and Teams plays one.

Who hears the sounds of a call:

- Only the device that is in the call plays them: the decision reads the call of this device (`CallsView`) and what the server lists at its place (`call_sounds::Listed`): the people, their shares and raised hands, the requests to enter, the recording, and a move of the account to that place. Another device of the same account, and a device whose person is outside the call, play nothing.
- A join or a leave is a change of the accounts that the server lists in the call. The list counts only from the moment the server lists the person of this device; that moment plays the sound of the entry, and of a recording that runs. The people who are in the call before the person enters play no sound of their own.
- A new seat can arrive with its participant state already present. That first observation plays the entry, or the move, and the recording-start sound if the call is recorded; it does not require an earlier observation without the participant.
- A seat that a moderator's move began plays the sound of the move at once and no sound of the entry; the seat keeps that it began with a move while the move ends before the server lists the person. A seat that the person took in another call plays the sound of the entry there.
- The microphone is closed when the person closed it or a block of a moderator or of meeting mode holds it; the sound is off when the person turned it off or a block holds it. A change of either plays its sound.
- A share that ends while its person stays in the call plays the sound of the end; a person who leaves while sharing plays the sound of a person who leaves.
- A raised hand plays for the presenter of meeting mode and for a person who holds "mute members" in the channel: the people who may lower it (ADR 0516).
- A request to enter the call plays for the people inside, to whom the server gives it (ADR 0514).
- While the sound of the person is off, only the sound of the change of the sound plays, and no other sound of the application.

Sounds of messages and requests:

- A new notice plays a sound when it passes the rules of a system notification of ADR 0742 (new, unread, outside "do not disturb" and the daily quiet time, its place not muted, not shown in the active window), also above the bound of three system notifications in ten seconds. One change of the state plays one sound for its notices: the mention sound when one of them is a mention, a direct message or a missed call, otherwise the message sound. Two sounds of messages are at least 2 seconds apart.
- A new join request plays the request sound for a person who may decide it, outside the quiet times, also while a window is active.
- An invitation into a voice channel plays its sound outside the quiet times (ADR 0739).

Settings:

- The section "Sounds" of the settings has one switch "All sounds" and one switch per sound, as Discord has: messages; mentions and direct messages; the ring of an incoming call; the own entry, leave, move and lost connection; another person who joins; another person who leaves; microphone off; microphone on; sound off; sound on; push to talk pressed; push to talk released; a share that starts; a share that ends; a viewer who starts watching; a viewer who stops watching; an invitation into a call; a request to enter a call or to join a server; a recording that starts; a recording that stops; a raised hand. "All sounds" off turns every sound off, the ring of an incoming call too; each switch keeps its choice and is drawn off and held while all sounds are off. Turning a switch off drops the sounds that wait in the player and silences the one that plays (a generation of the player that each request carries; test `audio::sounds::tests::a_cancel_drops_the_waiting_sounds_and_silences_the_one_that_opens`). Each sound has its own switch and no switch changes another sound (`audio::sounds::tests::every_sound_can_be_switched_off_without_changing_another`).
- The switches are stored in `voice.json` version 6 as `sounds`: `all` and the list `off` of the sounds that are off, so that a sound that a later version adds starts on. Version 5 stored one switch per group of sounds; a group that was off becomes every sound of that group off, and nothing of the group is turned on (`voice::tests::version_five_expands_disabled_groups_without_enabling_any_of_their_sounds`). A file of version 4 with the switch of call sounds off keeps every sound of a call off and the sounds of messages and the ring on (ADR 0710).

Player:

- `SoundPlayer` opens the output device on a thread of its own, plays one sound, writes 150 ms of silence after it so that the device has played the last samples, and closes the device. A request only sends the sound to that thread and returns at once, as the requests of the ring tone do (ADR 0733). The device is the output device of the call, or the chosen output device when no call has one. Requests that arrive while a sound plays are played in order after it, each sound at most once, and at most four.
- The decision of a call is a function of the state before and after a change, `state::call_sounds::sounds`, called at each change of the calls and of the state of a server. The sounds of notices and requests are decided with the rules of the system notifications (`state::notices::alerts`).

Tests:

- `audio::sounds::tests`: `every_sound_is_short_and_quiet_and_starts_and_ends_in_silence` renders each sound (30 to 380 ms heard, peak at most 0.03 and less than half of the ring, the first and last millisecond below a tenth of the level, which fails without the rise and the fall). `the_four_sounds_of_the_microphone_and_the_sound_differ_in_count_register_and_direction` measures the rendered sounds as a person hears them (the tones between pauses, the pitch of each half of a tone from the crossings of zero): the microphone two tones, the sound three tones, "off" falling and "on" rising, every microphone tone above every sound tone, and no two of the four alike; it fails on the first set, whose four sounds were one tone each. `no_two_events_have_sounds_that_are_alike` compares every pair of sounds by what is heard. `a_glide_moves_in_pitch_from_its_start_to_its_end` and `every_sound_is_recognised_at_the_rates_of_devices` check the measure. The player tests are unchanged.
- `state::call_sounds::tests` check each event of a call: the own entry and not the people present, the microphone and the sound and their blocks, shares that start and end, the end of the call, a switch into another call, a move, a lost connection, a raised hand for a person who may lower it and not for another, a request to enter, a recording that starts, stops and runs at an entry, a device outside the call, and that nothing else plays while the sound is off.
- `state::notices::tests::a_change_with_new_notices_has_one_sound_under_the_rules_of_a_system_notification` and the join request test check the sounds of notices and requests.
- Window tests: `notices::a_notice_plays_its_sound_under_the_rules_of_a_system_notification` (a mention, an active window that shows the place, a channel with "all messages", a muted channel), `settings::the_switches_of_the_sounds_are_kept_and_all_sounds_holds_every_kind`. The window tests play the sounds on a synthetic output of their own (`Window::tones`), apart from the output of the calls (`Window::sound`), and name the sound that was played with `junto_media::audio::sounds::heard::recognise`. The live test `live::tones::the_person_in_the_call_hears_a_tone_for_each_event_and_none_for_the_people_present` enters a call in which the other person already is and requires the sound of the entry alone, then the sounds of a closed microphone, of a person who leaves, of a person who joins and of the end of the call, with a real media server.

## Consequences

- The level of a sound on real speakers depends on the volume of the system and of the device; the tests check the samples, not the loudness that a person hears. Whether a person tells the sounds apart is checked by measuring their pattern; a person confirms it at the live test.
- Each sound opens and closes the output device. On a system that is slow to open a device, a sound starts later than its event; the window does not wait for it.
- A person who joins before the server lists the person of this device is not heard as a join.
