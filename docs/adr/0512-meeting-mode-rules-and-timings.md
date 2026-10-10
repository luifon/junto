# 0512. Meeting mode: who starts it, its blocks and its timings

## Status

Accepted

## Context

ADR 0505 decides meeting mode: the person who shares a screen turns it on, one person runs it at a time, every other microphone is blocked, the presenter removes a person's block, and it ends when the presenter turns it off, stops sharing, or leaves and does not return within a short time. It leaves that time open. Its end removes every microphone block in the voice channel, the ones of moderators included. A microphone block is enforced through what a participant may publish (ADR 0510).

## Decision

Start:

- A person turns meeting mode on in the voice channel that the person is in, with the permission "run meeting mode".
- The person shares a screen: the person's device has reported the share, and the media server lists the screen track (ADR 0510).
- A person whose microphone is blocked cannot turn meeting mode on. The end of meeting mode removes every microphone block of the call (ADR 0505), so a blocked person who could start it could end it at once and remove the block that a moderator set.
- A second person who tries while meeting mode is on gets `conflict`.

While it is on:

- Every member except the presenter who has no microphone block gets one with the reason `meeting`. A block that a moderator set before stays, with its own reason.
- A person who enters the call gets the block on entry, in the join token as well.
- A person raises and lowers a hand. The hand is a field of the participant.
- The presenter removes the microphone block of a person, whatever its reason. A moderator with "mute members" can do the same.
- A moderator can block the presenter's microphone.

End:

- The presenter turns meeting mode off from inside the call.
- Meeting mode ends at once when the presenter's device reports the end of the share, and when the admission check ends the share because the media server has listed no screen track for 10 seconds.
- Meeting mode ends when the presenter has not been in the call with a listed screen share for 60 seconds. The time starts when the presenter leaves the call. A presenter who returns has the rest of the 60 seconds to share again.
- Meeting mode ends with its call: when the last person leaves a call in which it is on, the server records `call_meeting_ended` with `presenter_left` and the note of the end before it ends the call.
- The end removes every microphone block of the call, and records `call_meeting_ended` with the reason: `turned_off`, `share_stopped` or `presenter_left`.

## Consequences

- A person who does not share a screen cannot turn meeting mode on, and neither can a blocked person. A blocked person cannot use meeting mode to remove the own block.
- Any member with "run meeting mode" who shares a screen can turn meeting mode on and off, and that removes the blocks that moderators set (ADR 0505). All members have the permission by default. An operator who needs lasting blocks takes "run meeting mode" from the "everyone" role, or denies it in the channel.
- The microphones are open again at the next admission check after the end: at most 2 seconds later.
- A presenter whose client loses its connection for less than 20 seconds stays in the call (ADR 0509), and meeting mode continues.
- Meeting mode without a presenter in the call lasts at most 60 seconds plus one check.
- Direct-message calls have no meeting mode (ADR 0501): the routes exist for voice channels only, and the database refuses a meeting on such a call.
