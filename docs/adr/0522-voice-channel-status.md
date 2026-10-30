# 0522: Voice channel status

Status: Accepted

## Decision

A voice channel has a plain text status, separate from its name and topic. The status is trimmed, contains at most 500 Unicode characters on one line, and stays until somebody changes or clears it. Leaving the call does not clear it. Text channels have no status.

`PUT /api/v1/channels/{channel_id}/voice-status` accepts `SetVoiceChannelStatus` and returns the channel. An empty status clears it. Every change requires `SetVoiceChannelStatus`; somebody outside the channel also needs `ManageChannel`. A person is inside only after the admission check has confirmed their media connection. A token or an unconfirmed seat does not count. Hidden, deleted and archived channels use the existing access rules of the call routes. The route shares the rate limit of call actions.

The permission rule and 500-character bound follow [Discord's channel status API](https://docs.discord.com/developers/resources/channel#modify-voice-channel-status) and [voice channel guide](https://support.discord.com/hc/en-us/articles/19583625604887-Voice-Channels-FAQs). The status states the activity of a channel, so it belongs to channel state rather than to the structure file that sets up a deployment.

A change records `voice_channel_status_updated` in the audit log without adding a system message to channel chat. It publishes `channel_updated`, whose channel carries `voice_status`. Snapshots and channel reads carry the same value. An empty value can be omitted, and readers default it to empty for earlier servers. Event revision 4 records the addition (ADR 0206).

The channel column draws the status below the voice channel name in ordinary, favourite and personal sections. Its context menu copies the current text or opens the editor when allowed; the channel menu also opens the editor. The editor saves, clears or cancels, keeps typed text on failure and supports retry. It closes when the account, channel or required permission is lost, including leaving a call without channel management. An asynchronous result only affects the editor and account that sent it.

The CLI's `call status <channel> <text>` sets or clears the value. `state` appends a status line after the room settings only when the status is nonempty, preserving the default state output of earlier versions. Watch input joins the words of status text and offers `clear-status`; its 4096-byte line bound admits a 500-character UTF-8 status with the command and channel identifier.

## Verification

Server tests cover a pending seat, a confirmed connection, channel management outside the call, explicit denial, hidden and archived channels, text channels, Unicode bounds, control characters and exclusion from structure export. Window tests edit, copy, clear and revoke access; the live window test changes status as a connected member, receives another person's update and closes the editor after leaving. The initial window test failed because the status editor was absent.
