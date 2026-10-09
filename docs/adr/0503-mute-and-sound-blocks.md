# 0503. Mute, sound blocks and moderation in a call

## Status

Accepted

## Context

A participant controls their own microphone and their own sound. A moderator also needs to silence a participant or to stop a participant from hearing a call. If a moderator's action and a participant's own setting were one state, removing the moderator's action would turn on a microphone that the participant had muted. The media server can stop a participant from sending microphone audio. For incoming media it can only stop all of a participant's incoming streams, video included.

## Decision

- There are two kinds of mute, one for the microphone and one for sound.
- A person can mute their own microphone and turn off their own sound.
- A moderator can block a person's microphone, with the "mute members" permission, or a person's sound, with the "deafen members" permission.
- The interface shows a block differently from a person's own setting.
- Removing a block returns the microphone or the sound to the person's own setting.
- The server enforces a microphone block by removing the person's permission to send microphone audio.
- A sound block is applied by the client only.
- Moderation in a call has three permissions: "mute members", "deafen members" and "disconnect members".

## Consequences

- A blocked microphone cannot send audio, whatever client the person uses.
- A modified client could ignore a sound block, so the server cannot guarantee that a person with a sound block does not hear the call.
- A person whose block is removed is heard only if their own microphone setting is on.
- Meeting mode uses microphone blocks, and ending it removes every microphone block in the voice channel, including one that a moderator set earlier (ADR 0505).
- A person with "disconnect members" can remove a participant from a call. The participant is removed through LiveKit's API (ADR 0506).
