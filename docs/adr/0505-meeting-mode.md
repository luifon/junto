# 0505. Meeting mode

## Status

Accepted

## Context

In a call where one person presents to many, open microphones interrupt the presenter. A presenter needs to silence all other participants with one action and to let one participant speak on request. A mode that silences participants must end when the presenter is gone, or the participants stay silenced with nobody able to end it. Microphone blocks are also set by moderators (ADR 0503).

## Decision

- The person who shares a screen can turn meeting mode on. This needs the "run meeting mode" permission, which all members have by default.
- One person runs meeting mode at a time.
- While meeting mode is on, every other microphone is blocked.
- A bar tells every participant that meeting mode is on and who runs it.
- A participant raises a hand, and the presenter removes that person's block.
- Meeting mode ends when the presenter turns it off, ends the last of the presenter's shares, or leaves and does not return within a short time. A presenter who runs several shares (ADR 0502) keeps meeting mode while one of them runs.
- Ending meeting mode removes every microphone block in that voice channel, including a block that a moderator set earlier.
- A person whose own microphone setting is on is then heard again, and the client tells that person so.
- Turning meeting mode on and off is written into the text chat of the voice channel.
- Direct-message calls have no meeting mode (ADR 0501).

## Consequences

- A person who does not share a screen cannot turn meeting mode on.
- A moderator's block that was set before meeting mode does not survive its end. The moderator sets the block again if it is still needed.
- When meeting mode ends, a microphone that its user did not mute sends audio again without an action by that user. The client's message informs that user.
- The blocks are microphone blocks, so the server enforces them (ADR 0503).
- The time that meeting mode waits for a presenter who left is fixed by ADR 0512: 60 seconds.
