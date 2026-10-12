# 0734. The call of a client: join, one call over all servers, reports, blocks and reconnect

## Status

Accepted

## Context

A server decides who is in a call and gives the token for the media server (ADR 0508, ADR 0510). The engine carries the media of one connection and knows no rule of a server (ADR 0733). ADR 0107 states that a client is in one call at a time over all its servers; ADR 0720 left that rule to the part that holds the media connection. The server expects from the device in a call: its own microphone and sound setting (ADR 0510), and whether its person speaks (ADR 0513). A microphone block is in the state of the server and must stop the microphone (ADR 0503); a sound block is applied by the client. The media server can restart, and a token is usable for about two minutes (ADR 0506). ADR 0507 leaves open how the client decides when a microphone is sent.

## Decision

The calls of a client:

- One object per client, in the crate `junto-media`, holds the call that the device is in. It asks a server for a place, starts an engine with the answer, and ends the engine when the call ends. An interface reads its view and its levels and calls its functions.
- The person's own settings "microphone closed" and "sound off" belong to the client and not to a call. They hold outside a call and are used by the next call.

Join, and one call per client:

- A join asks the server for the place first. A refusal leaves a call that the device is in as it is, and is told with the code of the server.
- When the place is given, the earlier call ends, and then the media connection of the new call starts. A call on another server is left with a request to that server. A call on the same server was ended by that server when it gave the new place (ADR 0510), so no request is sent for it.
- A connected media transport is shown as `entering` until the server lists the current place identity among the participants. The calls object tracks this admission separately from the engine connection. A renewed token for the same identity preserves admission; a replacement identity needs its own listing.
- One join runs at a time. A second one is refused as busy.
- A leave ends a join that waits for the answer of its server. Nothing is shown as asked for from then on, and the join returns as cancelled. When the server gives the place afterwards, the client gives it up with a leave request and starts no media connection.
- The client joins on the selected server, whose event stream is live (ADR 0720).

The order of the requests for a place:

- A join, a leave and a request for a new token each ask a server about the place of the account. The client runs them one at a time, in the order in which they were started, and each one handles its answer before the next one starts.
- An answer that gives a place which no call of the client uses is followed by a leave: the person left while the server was asked, or the call ended while a new token was asked for.
- A leave ends the call on the device at once. What it tells the server waits for its turn.
- The leave request of the client names the identity that this device was given, so the server ends only that place (ADR 0510). A place that another device or session of the account took stays. When the request gets no answer, the server ends the place by its check, because no media connection uses it (ADR 0509).
- When the application quits, it leaves in the same way and waits at most 2 seconds for this work.

Whether the place still exists:

- The calls read the state and the events of the server (ADR 0719). The place of the device is the own call of the account with the identity that the join returned. Another identity is a call on another device or session.
- A state names the last event that it holds, and an event has a number. The calls put both in one order, because they read them from two sources and the state can be ahead of the events.
- The numbers count within one history of the server, which its epoch names (ADR 0010). A state with another epoch starts anew: what the earlier history told about the place is not used, and events of the earlier history that are read later are not used.
- An event that shows the place, followed by an event that the account is in no call, ends the place at once, also in the first seconds after the join.
- Otherwise, after the state or an event has shown the place once, or 10 seconds after the join, a live state without the place means that the place has ended.
- The engine is closed at once, and the person is told why. The reason is the one of the event `own_call_changed`; the client waits for that event for at most 500 ms in all. When the account is in a call with another identity, the reason is that.
- While this client asks the same server for a place, by a join of the person or for a new token, another identity in the state is its own and ends nothing.
- While the event stream is not live, the state decides nothing.
- A person who left by an action of their own is told nothing.

Reports:

- When the media connection is made, and after each change, the device reports "muted" and "sound off". At most one report is on its way, and two reports are at least 500 ms apart.
- A person whose microphone cannot be used (no permission, no device) is reported as muted. The person's own setting is not changed by that.
- In a voice channel the device reports whether its person speaks: a change at once, at most one report in 700 ms, and a renewal every 3 seconds while the person speaks (ADR 0513).
- A report that failed is sent again after 3 seconds.

Blocks:

- A call starts with the blocks that the server states in its answer to the join, and to a request for a new token (ADR 0510). From the moment the state of the server lists the person, the blocks are the ones of the state.
- A microphone block closes the microphone and withdraws its track; without the block the microphone returns to the person's own setting. The permission of the participant in the room closes the microphone too (ADR 0733).
- A sound block turns the sound off; the person cannot turn it on.
- In a call that hides who is inside, the answer of the server states no block for a place whose device the server has not seen in the room, and its join token allows pictures only (ADR 0517). The calls keep the sound off from the start of the media connection until the state of the server lists the person with the identity of this place, so a sound block holds from the first moment as in every other call. A listing with the place of another device of the account does not end the wait. While the event stream is not live, the state lists nothing, and the sound stays off. The engine opens the microphone when the media server reports the changed permission (ADR 0733), which the admission check sets when it has seen the device.
- An answer to a request for a new token that tells no blocks, for the same reason, leaves the blocks that the call holds.

When a microphone is sent (ADR 0507):

- Voice activation, the default: the open microphone is sent all the time, and the level decides the speaking mark. No gate cuts the sound below a level. A gate with a fixed level removes quiet speech and the start of words; noise suppression and the silence detection of the codec already reduce what silence costs.
- Push-to-talk: the microphone is sent while the key is held, and the window has the focus. The key is one of two modifier keys, Alt (Option on macOS) or Control: a modifier types nothing into a focused field.

Reconnect:

- While the call library restores a connection, the call is shown as reconnecting.
- When the connection has ended, the calls ask the server for a new token: first after 1 second, then with a doubled wait up to 5 seconds, until the media connection is made, the server ends the place, or the person leaves. The wait grows with each request, whether the server refused it or the media connection with its token failed, and starts at 1 second again when a media connection is made. A server that cannot be reached, a limit, and `media_unavailable` are waited for; another refusal ends the call with its reason.
- A new token is asked for only while a live state of the server shows the place of this device. A place that the server ended, or has not shown yet, is not asked for, so a person whom a moderator removed does not return by this path.
- A new token can name a new identity: the earlier place had ended. The call continues with it.

Sound settings:

- The file `voice.json` of the data directory holds the chosen devices, the three parts of the processing, the input mode, the key, and the volume per account of a server (ADR 0710). A change is used at once and written in the background.

## Consequences

- A refused join on another server costs the person nothing: the first call goes on.
- Between the answer of the new server and the start of the new media connection the device is in no room. When that connection then fails, the person is in the new call without media and sees "reconnecting".
- A device whose call moved to another device of the account is told so and does not take the call back.
- After a restart of the media server each device returns by itself, within the waits above and the time that the server needs to observe the room again (ADR 0509).
- While the event stream of a server is interrupted, a place that the server ended is not noticed, and no new token is asked for. A lost media connection returns when the stream is live again and the server still holds the place.
- A request for a place waits while an earlier one has no answer. A server that does not answer holds the next request for at most the time limit of one request, 40 seconds.
- A person with a sound block who enters the call again hears nothing at any moment, and a device that leaves late ends no place of another device.
- A person who enters a call that hides who is inside hears the call and is heard from the moment at which the server lists the person, which is after the media connection is made.
- The speaking mark of the person follows what is sent: a muted microphone and a released key give none.
- Push-to-talk does not work while another application has the focus (ADR 0507).
- The share of the device and the shares of the other people use the same object (ADR 0737).
- A move between voice channels, the acceptance of an invitation and the entry after an accepted request use the same object (ADR 0739). Direct calls use it too; the window has no place that starts one yet.
