# 0501. Voice channels and direct-message calls

## Status

Accepted

## Context

Calls need a place in the structure of a server. Recordings, the glossary and retention are defined per workspace channel (ADR 0406, ADR 0603), and a direct message belongs to no workspace (ADR 0302). A call also produces text: links, notes and system lines about what happened in the call.

## Decision

Voice channels:

- A voice channel is a permanent place for calls in the channel list. The channel list shows who is inside.
- Each voice channel has its own text chat.
- Text channels have no calls in the first version.

Direct-message calls:

- A direct-message call has voice, camera and screen share.
- A direct-message call has no recording and no meeting mode.
- People who need a recording use a voice channel, which can be private or locked (ADR 0504).

Media:

- A participant can send a microphone, a camera, and a screen share of a full screen or of a single window, with sound (ADR 0502).

Event lines:

- Call start, recording start and stop, a member accepting a call invitation, and the start of a screen share appear as system lines in the text chat of the voice channel.

A call invitation is the invitation of an existing member to a call (ADR 0506). It is distinct from an invite code that joins a server (ADR 0105); using that code writes no call-join system line.

## Consequences

- A call in a voice channel starts when a member enters it; no person schedules or creates a call.
- The members of a workspace can see who is in each voice channel, unless the channel hides who is inside (ADR 0504).
- The text chat of a voice channel holds the recording and transcript of a call as a system message (ADR 0602).
- Recording of direct-message calls is not planned (`docs/scope.md`).
- A fork of a thread can create a voice channel (ADR 0403).
