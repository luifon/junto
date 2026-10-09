# 0504. Voice channel privacy, invite and move

## Status

Accepted

## Context

Voice channels are permanent and visible in the channel list (ADR 0501), so a member with access can enter at any time. People in a call sometimes need to keep others out for the length of that call without changing roles or channel permissions. A person inside a call needs a way to bring another member in. The move function of the media server is not available in the self-hosted version of LiveKit.

## Decision

Private use of a voice channel:

- A voice channel has a user limit.
- A person inside a voice channel who has the "lock room" permission can lock it. All members have this permission by default.
- A locked voice channel unlocks when it is empty.
- A voice channel has a "hide who is inside" setting.
- Changing the user limit and the hide setting needs "manage channel".
- Only people who are invited or moved in can enter a locked or full voice channel.
- A person outside can ask to enter. Anyone inside can accept. The first answer applies. The request expires after 60 seconds. The request appears as a line in the text chat of the voice channel.

Invite:

- Anyone inside a voice channel can invite a member who has access to that channel. The member gets a prompt and enters on accept.
- An invitation never passes a permission. It works only for a person who already has access to the channel. It passes the lock and the user limit, and nothing else (ADR 0301).

Move:

- Moving a person without a prompt needs "move members".
- The server performs a move itself. The client joins the destination first and leaves the source only after the join is confirmed.
- If the join fails, nothing changes and both sides are told.

## Consequences

- A lock and a user limit do not change who has access to the channel. They stop entry without an invitation or a move.
- A lock cannot stay on an empty voice channel.
- An invited or moved person can make a voice channel hold more people than its user limit.
- A person without access to a channel cannot be invited into it. Access is given by a role or an override (ADR 0301).
- During a move a client is connected to two LiveKit rooms for a short time.
- A voice channel that hides who is inside shows no occupants, no level meters and no speaking marks to people outside it (ADR 0507).
- Moves are written to the audit log (ADR 0303).
- A person can be dragged between voice channels in the client. Each drag action also exists in a menu that the keyboard can reach (ADR 0703).
