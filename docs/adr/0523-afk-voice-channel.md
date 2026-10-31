# 0523. The AFK voice channel of a workspace

## Status

Accepted

## Context

A person who leaves a computer while connected to a voice channel can remain in a call without participating. Presence alone does not distinguish a person speaking while using no keyboard or mouse from a person who has left. Moving a person must preserve the permission and admission checks of an ordinary call move (ADR 0511), and the destination must prevent publication through the media server.

## Decision

Each existing workspace receives an AFK voice channel through migration 0056. Creating a workspace creates its AFK channel as well. AFK is enabled by default, with an idle time of 15 minutes. A member with `manage_channel` in the workspace can disable it, select another open voice channel of the same workspace, and choose an idle time of 5, 15, 30 or 60 minutes. The ordinary channel settings rename the selected channel. A selected channel must be replaced before it is archived or deleted. Deleting the workspace removes its channels normally.

A device in a workspace voice call reports idle only after both system input inactivity and microphone silence have lasted for the configured time. The native activity reader supplies elapsed system idle time; presence keeps its five-minute threshold. The audio capture path retains the last time that the sent microphone signal counted as speech, including a short utterance between window updates. Muted audio counts as silence. Counting begins on entry to a call. A call in meeting mode sends no idle report. The AFK channel, meeting mode, direct calls and group calls are exempt. An unavailable system activity source never qualifies a device as idle. The client checks settings and eligibility at most every 30 seconds while connected.

`GET /api/server` advertises `capabilities.afk_channel`. An absent flag enables no AFK controls or idle request. An older server therefore offers no AFK feature. An older client never reports idle and is never moved automatically.

An idle report names the call identifier and the media identity of the reporting seat, and states the two elapsed durations. The server verifies the current session and device, the connected seat, the workspace setting and timeout, the source exemption and the person's current access to the destination. Physical input and microphone silence are observations of the device. A report creates an ordinary stored move with an internal automatic-AFK mark; it does not grant `move_members`. The destination token and the admission check recheck that the same workspace still has this AFK destination enabled, that the reported durations reach its current timeout, and that the source is outside meeting mode. Admission also rechecks the source membership and destination access. A settings or permission change can therefore cancel a move before completion. The usual move events and the two channel-scoped audit records apply.

Settings changes send an empty workspace-scoped `workspace_afk_changed` invalidation at both connection levels. The media client rereads the listening-only state of its current seat without reconnecting; a change during a read requires a later read, so a delayed answer cannot undo the change. The open administration form also refreshes.

Event revision 17 adds that invalidation and the optional `afk` mark and the destination identity of an automatic AFK move to `CallMove`. A delayed move event can therefore name the current destination seat exactly. The client prepares its notice from that event, so the ordering of the event stream and the HTTP answer does not decide whether the person is told. Completion shows an AFK notice with one control to rejoin the earlier channel. That control names the current AFK seat and uses normal call joining, which checks current permissions, lock and capacity. Leaving or changing the seat dismisses the notice. The ordinary move sound of ADR 0707 applies, under the person's sound settings.

While enabled, the AFK destination grants no publication source: microphone, camera, screen picture or screen audio. The same restriction applies to ordinary channel joins, move tokens, renewed tokens and admission updates. State and speech reports cannot reopen the microphone or claim speech there, and share reports are refused. Meeting mode cannot start in AFK; a channel with an active meeting cannot be selected as an enabled AFK destination. The own-call state and join response state `listen_only`, defaulting to false for an older server, so the media client also closes these controls. Disabling AFK ends the automatic moves and the publication restriction.

Structure format 6 adds the optional `afk` settings of each workspace: a channel entry identifier, the enabled state and the idle minutes. Export states all three. Plan resolves the channel in the same workspace, rejects a text channel or one listed for deletion, and applies the setting after channel creation through the same server checks. When an import supplies an AFK channel for a workspace created by that same change, the temporary default channel is removed in the transaction after selecting the imported destination. Existing channels are preserved unless the file lists their deletion. The settings participate in the structure revision. Formats 1 to 5 remain readable; an absent setting preserves an existing workspace's values and leaves a new workspace at its defaults.

## Consequences

The server remains authoritative for membership, settings and publication. A device cannot use an idle report to move another account or a different seat, to enter a forbidden destination, or to pass a publication restriction. The feature needs no new channel kind, permission value or block-reason value that older fixed-enum readers could reject.
