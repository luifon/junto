# 0735. The screens of a voice call

## Status

Accepted

## Context

This version has the voice of a call and no camera, no screen share and no text messages (ADR 0903). ADR 0730 lets the window draw no control of a function that the build lacks, with the exception of the profile strip. The call can later be moved into a window of its own (ADR 0507).

## Decision

Entering:

- A click on a voice channel shows the channel and joins nothing. Its header has one "Join call" button, with a small arrow attached to it whose menu has "Join after a device check" (the device check below); the header has no sentence about the chat or the people, and the states of the call (recording, lock, the count against a user limit) follow the title. While the room is locked or full for the person, the same place has "Ask to join" in the place of "Join call", with the state of the request (ADR 0739). Reason: "Check devices" and "Join call" were two buttons for one intent, and "Join call" on a locked room was offered and refused. Tests: `device_checks::channel_pair_and_group_calls_offer_a_guarded_check_before_joining` (the press on the arrow opens only its menu and joins nothing, and the menu opens the check, also from the call of a pair and of a group conversation) and `live::device_checks::`. A double click on a voice channel that this device is not in also asks for a place in its call, through the same request as "Join call": a locked or a full room refuses it and the refusal is told, and from a call in another voice channel the device moves into this one, as the button does (`Calls::join`, one call of a client over all its servers). Its first click shows the channel. A double click on a text channel does nothing more than a click. The channel list is the one place that lists voice channels to enter; the notices and the administration list them for other purposes. Tests: the window test `calls::a_double_click_on_a_voice_channel_asks_for_a_place_and_a_single_click_only_shows_it` and the window live tests `live::rules::a_double_click_enters_a_voice_channel_as_the_join_button_does` and `live::rules::a_double_click_on_a_voice_channel_of_another_server_moves_the_call_there`, which also has a full room refuse the double click while the call on the first server goes on. The same place shows the state of its call: recording, meeting mode, lock, count. For the call of this device the header has the controls of the room in their place (ADR 0739).
- A join that the server refuses is told by a notice in the notice area of the shown view, with the name of the channel and what the person can do. The notice stays until the person closes it.

The call in the main area:

- The header has the title and the state of the call. While the call is restored, it shows "Reconnecting…" and the statement that the room controls return.
- The stage shows the people as tiles: the generated avatar, and a plate with the name and the level meter or the mark of the microphone. The tile of a person who speaks has the bright edge. Tile sizes are defined in `crates/client/src/ui/tile.rs`.
- The tiles of the stage fill it, as in the grids of Discord and Meet (`ui::tile::grid`): each tile has the largest 16:9 size with which all tiles fit in the free area of the stage, with 8 design pixels between them. A wide stage has no more rows than columns and a tall stage no more columns than rows, so one person is drawn at the width of the stage, two people side by side at about half of it each, and four and nine people in two and three rows. Below the size of a tile of the strip (148 by 84) the tiles keep that size and the stage scrolls. The stage builds only the lines of tiles in view (`ui::in_view`): the rows of the grid, or the focused tile and the rows of the strip under it. The people under a voice channel in the channel list are built in the same way, one row of the height `occupant_row_height` per person. Such a column of lines takes the full height of its lines in the layout, so the scroll area that holds it keeps its length and its offset. When gpui prepaints it, the column reads the part of it that the areas around it show (the content mask, which a scroll area sets to its bounds), builds the lines in that part, lays each out at its height and places it in the column, in the same frame, as the uniform list of gpui does for a list that scrolls by itself. A scroll therefore shows built lines at once, without a margin of lines out of view, and a column can lie inside a scroll area with other content above and below it, as the people of a voice channel lie between the channels. A row that is not in view has no element: the keyboard focus of a row of a person that leaves the view is lost, as in the lists of gpui, and the move of the focus to the next row scrolls the list and builds the rows after it. Before, a call of 601 people built and laid out a tile and a row of the channel list for each person at each frame: about 460 ms per frame on two processors of the Linux host, of which about 220 ms were the rows of the channel list (`docs/client-performance.md`). A first version built the lines that were in view at the previous paint, with half a view more on each side, and asked for another frame after a scroll; it took 38 ms per frame on that host and showed empty space for a frame after a long scroll, so the lines are now chosen in the prepaint of the frame that draws them. Tests: `ui::in_view::tests` (the lines chosen for a part of the column are those that lie in it, for lines with and without gaps, and a column of 601 rows of 28 design pixels builds the ten rows of a view of 280), and the window test `calls::a_call_of_many_people_draws_the_tiles_and_the_rows_in_view` (601 people with held profiles in a window of 1600 by 900: fewer than 150 tiles and fewer than 150 rows of the channel list exist at the start, the tile and the row of the last person do not, and after one turn of the wheel to the end of the stage, and of the channel list, each is in view and the tile or the row of the first person no longer exists). The stage measures its free area when it is drawn and draws the tiles again at a new size, so a resize or a statement under the tiles changes the size of the tiles in the next frame. People that "Hide people without video" leaves out are not counted, so the others grow. In the focus the focused tile has the largest size above the rows of tiles of the size of the strip, which hold the other people: the strip wraps into as many rows as its tiles need across the stage, and the focused tile leaves room for each of them (`ui::tile::strip_height`; before, the focused tile left room for one row while the strip wrapped into more, and the rows under it ended outside the stage). Test: `ui::tile::tests::the_strip_reserves_each_row_that_its_tiles_wrap_into`. The strip under a share keeps its own size (ADR 0738). Reason: the tiles had a fixed size of 222 by 126, so two people were two small tiles in the middle of a large empty stage. Tests: `ui::tile::tests` (1, 2, 4 and 9 tiles in a stage of 1176 by 746 and of 656 by 380, and many tiles), and `live::cameras::the_focus_draws_the_camera_of_the_person_who_speaks_large_and_the_grid_returns`, which reads that two tiles are each between 40% and 50% of the width of the stage.
- While the first media connection is made, and while the call is restored, the stage shows that statement in place of the tiles.
- "Hide people without video" (Settings, "Your view") leaves out the tile of every person without a camera in each layout of the stage: the grid, the focus, the strip under a share (ADR 0738) and the call window (ADR 0772). A person has video while a camera of that person is published in the call; the camera of this device counts while it starts or runs. A camera counts before its first picture arrives, because only a drawn tile asks the media server for its picture (ADR 0770): a rule by the picture never drew a camera that started while the setting was on. The setting also leaves out every tile when nobody has a camera, and the stage then states it. A statement with the avatars of the first three people left out says how many the stage leaves out ("2 people without video are hidden") and that they are listed in Members. Reason: the setting first applied only while at least one picture was drawn, so in a call without cameras it changed nothing, and a person saw no effect. Test: `live::cameras::the_people_without_video_are_hidden_in_every_layout` reads the statement and the avatars of the people left out in the grid, the focus, the strip and the call window.
- The panel has the tabs Chat and "In the call". Chat shows the chat of the voice channel with its composer (ADR 0740). "In the call" lists the people in the call and the members of the workspace outside the call with the voice channel that they are in, and lists people only. A row of a person in the call is one line with the marks of the microphone and of the sound at its end; a second line names only the states that have no mark: connecting or reconnecting, a raised hand, a share. The volume of a person is in the menu of the person, which is the same menu on the row of the channel list, on the tile and in the list: the volume, Pin where the call is drawn, the moderation group and the identity actions (ADR 0739). Reason: each row had a volume slider and was three lines high, and a crossed microphone was drawn twice in one row.
- A row of the person's own controls lies under the stage, in the main window and in the call window (`Shell::render_own_controls`): microphone, sound, camera, share, raise hand (in a call with a hand), View, the devices (the sliders icon, which opens Settings at "Voice and video"; in the call window the main window opens them and comes to the front, ADR 0772), the separate window or its return, and Leave, each an icon with a tooltip. Some are also on the call bar and the profile strip, on purpose, so the call can be seen and left from any channel. On a stage narrower than 520 design pixels the row keeps microphone, sound, camera, share and Leave, and the others move into "⋮" at the end of the row in the order of the row; the call window has "⋮" in every width, with "Keep on top" and "Full screen" (ADR 0772). "View" opens a menu: Grid, Speaker (the person who speaks in a large tile), "Hide people without video" and "Hide my tile", which read and write the settings of "Your view"; "Hide my tile" is disabled while the camera is off, and the layout items while a person is pinned. The press on a button of the row opens only its menu. Reason: the controls of one call were split over the header, the call bar and the profile strip, and the call window placed them elsewhere again; the row puts the own controls in one place under the call, as Discord and Slack huddles do. The header of the call holds only what everyone in the call shares: the recording control, which looks the same for everyone ("Record", or the mark of the recording with its time and "Stop"), enabled with "record calls" and disabled for the others; meeting mode (with an icon of its own, a presentation board) and the lock as toggles for a person who may change them, and for the others a mark ("Meeting mode", "Locked") only while the state is on; and the count against a user limit. The header has no settings button. The call window has the same header with the labels of its shared controls; only a call window narrower than 520 design pixels draws them with their icons and states alone (`SHARED_LABELS_FROM`). The call bar keeps its controls; its grid button shows the grid or the large tile with two icons, and its separate-window button has the form of the bar. Tests: `own_controls::the_row_of_own_controls_lies_under_the_call_and_narrows_into_its_menu`, `live::detached::` (the window controls in "⋮"), and the header tests of `header::` and `badges::`.
- In a narrow window the panel starts closed, two buttons of the header open its tabs, and it lies over the stage (ADR 0731).

A direct-message call:

- A direct conversation whose direct-message call this device is in is drawn as the call of a voice channel: the stage with the people, the notice area at the end of the stage, and the panel with Chat and Members. Chat is the conversation with its composer. Members lists the people in the call with the volume control of the other person. The devices and the settings of the own share and of the view are in the settings of the application at "Voice and video", as for a voice channel. A direct-message call has no room (ADR 0501): the header, the panel and the call window have no room controls, no recording, no meeting mode and no invitation.
- The header has the name of the other person and the state of the call: "Calling…" until the other person has a place in the call, then "Direct call". While the call rings, the stage states that it calls the person, and Members lists the person outside the call.
- A share is drawn as in a voice channel: the bar of the shares when both people share, in which a card chooses the watched share, the line of the watched share with its sound and its volume, and the quality of the own share in the settings of the application (ADR 0738).
- Outside its call, a direct conversation has the button that starts a call above its messages, and the outcome of the last call (ADR 0771). When the device enters the call of the conversation or leaves it, and when the call moves into the call window or back (ADR 0772), the main area changes from one body to the other. That is a change of the whole body: its controls can change position (ADR 0730), as at the join of a voice channel.

Outside the main area:

- The channel list shows, for the call of the device, the measured level of each person (ADR 0507), and "connecting…" or "reconnecting…" beside the person of the device.
- The panel of the call (Chat, Members) closes with the button at the end of its tabs, in the main window and in the separate call window, as the server book collapses; the stage then takes its width. The buttons of the two tabs in the header open it again, and the Chat button counts the messages of the chat that arrived while it was closed. Whether the panel beside the stage is closed is kept in `settings.json` for each of the two windows and used after a restart. In a window too narrow for the panel beside the stage, the panel lies over the stage while it is open, as before, and that state is not kept. `Shell::toggle_call_panel` closes or opens it; the shortcut list (branch D) binds a key to it. Test: `panel_widths::the_panel_of_a_call_closes_and_opens_again_and_stays_closed_after_a_restart`.
- The strip of the call is above the profile strip while the device is in a call, whatever server and channel are shown. It names the voice channel, leads to it, shows the time since the call started, and has "Leave". Its share button starts and ends a share (ADR 0738), and its hand button raises and lowers the hand of the person (ADR 0739). Its camera button is drawn disabled, so that the strip has its final form.
- The microphone and the sound button of the profile strip work in a call and outside one. A microphone that the person closed has the crossed mark on terracotta; a microphone that the server holds closed has the lock mark and takes no click.
- A line above the profile strip tells who closed the microphone or the sound, and that the microphone cannot be used, with "Try again".
- The settings of the application have the section "Voice and video": the devices of the person (the component of the device check below), the input mode, the key of push-to-talk, the three parts of the processing, "Your screen share" (the resolution and the frame rate of the next share, ADR 0738) and "Your view" ("Hide people without video" and "Hide your camera tile"). Test: `device_checks::the_settings_of_a_share_and_of_the_view_are_in_voice_and_video`.

Structure:

- The functions that draw the call take the derived view (ADR 0730) and the handlers of the window, and read from the main window only what belongs to the drawing: the open tab, the open list of devices, the positions of the volume controls, the scroll positions of the lists, and whether the panel is open in a narrow window. A second window draws the same view with them. The member list of the panel has one list state for each server, account and workspace, and one for the direct calls of an account; a list that is not drawn keeps only its scroll position and not its rows, the lists of a server go when the server has another account or none, and the list of a workspace goes when the server does not list the workspace any more. Reason: one list state served every workspace, so the member list of a call in another workspace opened at the scroll position of the last one (`badges::the_member_list_of_a_call_in_another_workspace_opens_at_its_top`).
- The derived view has one panel of a call for both places: the people of the call, its media and settings, and its place. The place is a voice channel, with its name, the members outside the call, the recording and the room, or a direct-message call, with the other account and whether that account has a place in the call. The functions that draw the stage, the shares and the panel read the people from the panel, so they draw both kinds of call.

The notice area is the fixed scrolling area at the end of a call stage, the area above a message list and under its direct-call controls, or the part below the actions of a state panel (ADR 0730).

## Consequences

- A person sees the people of a call that has no share. The state uses the same tiles, name plates and meters as a call with a share.
- The devices are chosen in the settings of the application, before a call or during one, or in the device check before a join. The choice is stored, and a running call uses it at once.
- The volume control is the unstyled slider of the component library with the theme of ADR 0731. The arrow keys change the volume by 10 %. In a menu, such as the menu of a person whose first item is the volume, Left and Right change it and Up and Down move to the other items (`VolumeSlider::in_menu`); otherwise the keyboard could not pass the volume to reach "Invite" or "Move to". Tests: `ui::menu::tests::menu_arrows_reach_an_explicit_volume_focus_handle`, `live::rules::dragging_a_person_invites_or_moves_and_the_keyboard_reaches_the_same_actions`.
- The list of devices is drawn at its box and above the settings or the device check. It holds the keyboard focus while it is open.
- When a control that holds the keyboard focus leaves the window, such as "Join call", the focus returns to the window, so that Tab and the key of push-to-talk still reach it.
- The window draws one disabled button of a function that this version lacks, the camera, as an exception to ADR 0730.
- The time of a call is computed from the time of the server and the clock of the machine. A machine whose clock is behind shows 00:00:00 at first.
- During a direct-message call the conversation is read in the panel of the call, which is narrower than the main area. A person who wants the conversation in the whole main area detaches the call (ADR 0772) or leaves it.

Tests: `live::direct::a_direct_call_has_the_chat_the_volume_the_devices_and_the_share_controls_of_a_call` in the window live tier of `crates/client` starts a direct-message call in the window, reads the ringing call, sets the volume of the other person, reads and writes the conversation in the panel, chooses the microphone, the speaker and the camera in the settings of the application at "Voice and video", shares a screen with its sound, watches the share of the other person, sets the volume of the watched share on its line and the quality of the own share in the settings.


A device check is offered by the arrow of each join control (`Shell::join_control`): "Join call"
of a voice channel, "Call" of a pair or group conversation, and the answer to an invitation or a
ring ("Join", "Accept"). The arrow is a button of its own attached to the join button; its
press opens only its menu, whose one item "Join after a device check" opens the check, and
joins nothing. Quick join remains the join button itself.
The check and the settings at "Voice and video" draw one component of the devices
(`Shell::render_device_checks`), which behaves the same in both places. Its
Join action retains the exact server owner, call place and invitation identifier. Closing it
joins nothing. Losing that owner, leaving the target group, or resolving the invitation closes
the check, its previews and the menu of the arrow; a stale Join action is refused. A real join
cancels all local tests before the call takes the devices, and a camera preview never enables
the call camera. While already in a call, the check behaves as the settings do in a call
(below).

In every list of devices, in the settings and in the device check, the selected device has a check
mark and every other choice has the icon of its kind of device: a camera for a camera, a
microphone for an input and headphones for an output (`DeviceKind::icon`). Test:
`device_checks::each_device_list_of_the_voice_settings_has_the_icon_of_its_kind` in the window
tests.

A device choice is a setting of the person, and it works in every place and at every time. In
the settings at "Voice and video" and in the device check, a choice from the camera, input or
output list is stored and used at once, also by a running call (`choose_input`,
`choose_output`, `choose_camera`, which change the one stored setting). Only the tests differ
in a call: the microphone test, the output test and the camera preview of the check are not
offered, because each opens a capture or a stream of its own beside the call, and the call
holds the devices. In a call the input meter of the component shows the level of the call's own
microphone, the preview shows the picture that the call sends (the own camera of the call,
which the window asks the media engine for while the component is shown, also when the own tile
is hidden or the call is not drawn: `AppState::call_devices_shown`; the size of the preview
when no own tile states one; a window that shows the devices without the call converts only
that picture), and the component shows the state of the microphone of the call and its
permission with "Try again". A statement at the top of the component says so. Out of a call the
tests and the preview work as before, with "Check". Before 2026-11-09 the three lists were
unavailable in a call and the Settings tab of the call chose the devices; reason for the change
in the paragraph of the panel above. A list of the settings opens also when no check session
runs, because a device choice is a setting of the person and not a check: a change of the
account of the selected server, a join and the end of a call end the session while the section
stays shown (ADR 0733). In the component, a press on the box of another list while one list is
open closes the open list and opens the other one, and a press on the box of the open list
closes it. Tests:
`device_checks::the_device_lists_of_the_settings_open_without_a_check_session`,
`device_checks::in_a_call_the_device_lists_of_the_settings_open_and_choose_the_devices`, and
the window live test `live::a_person_joins_a_call_in_the_window_speaks_hears_mutes_and_leaves`,
which opens the settings from the call, reads the level of the call's microphone there, and
chooses another microphone, which the call opens at once.

Each person can pin one participant, including themselves, to their own person stage from the
tile or member menu. A pin overrides speaker focus and the grid, including for an avatar without
video and when other audio-only tiles are hidden. It follows the account rather than a tile's
position. Camera and speaking changes keep the pin. A banner names the pinned person and offers
Unpin; the account menu offers the same action. The automatic-layout control is disabled while
a person is pinned.

The pin and the choice between its person stage and the screen-share stage live in the shared
application state of the local call, so the main and detached windows keep the same choice.
Pinning shows the person; Show screen shares and Show pinned person switch the stage without
changing the selected watched share. Unpin returns to the ordinary stage and layout. The pin
belongs to the exact server owner and local call seat. Ending or replacing that seat, changing
its owner, or the participant leaving clears it; a connection retry at the same seat keeps it.
It is local presentation state, with no server spotlight or change to recording layout. Channel,
pair and group calls use the same state and renderer.

Tests: `live::pinning::` exercises avatar and video pins, speaking changes, share selection,
main/detached transfer, both unpin controls, hidden audio-only people, person/group departure,
pair-call end, and rejection of old owners and seats.

## Local input while muted

A connected call shows the notice "You are muted" with Unmute when the person's explicit
mute is on (or a key of push to mute is held) and the person keeps talking: the
independently echo-filtered local input reaches the segment containing -45 dBFS, the
speaking threshold used by the media engine, for 2.5 seconds, where pauses shorter than
0.8 seconds keep the speech continuous. It then stays until 5 seconds after the last speech,
so the pauses between words do not hide it, and it goes at once when the person unmutes or
the call is no longer connected. The next mute needs new speech. One state machine in the
application state decides it (`state::muted_speech::MutedSpeech`), stepped with each change
of the levels and of the calls, and with a timer at the moment when the drawn notice can
change without them (`MutedSpeech::wake`): the levels change only when the input changes, so
a steady level is speech that goes on, and silence changes nothing until the hold ends.
Reason: the notice followed the level of each moment, so each pause between words hid it,
each next word showed it again, and the controls around it moved with every word. It keeps
its place above the call bar (in the call window, the notice area of the stage), where it
covers no control; it moves the call bar once per period of speech.

A separate echo canceller always takes a copy of the microphone and the actual output mix as
its reference, even when optional sending processing is off. It does not change the settings
input meter or the sent samples. Remote playback picked up by the microphone is filtered
before detection. The warning starts no capture and opens no sending gate. An isolated
virtual-device test routes playback into capture, then adds local speech beside it, with all
sending processing off and the sending gate closed. The threshold of -45 dBFS is to be checked
in real rooms, so that other people talking in the background do not start the notice.

The first row holds the statement and Unmute, with no close button. In push-to-talk mode
an explicit mute still warns, but waiting for the talk key alone does not; Unmute removes
the explicit mute and preserves the talk-key gate. A held push-to-mute key also warns, with
an explanation and a disabled Unmute until release; the callback checks the hold again so a
press drawn before the hold cannot bypass it. The second row, apart from Unmute, is the
check box "Tell me when I talk while muted" in a click area at least 24 design pixels high
(`ui::CheckBox`). Unchecking it turns the device setting off (`muted_speech_notice` in the
sound settings, on by default and when a file of 0.2 or earlier lacks it; the format version
stays 8, whose readers ignore the field) and the notice states "Turned off. Settings, Voice
and video turns it on again." for 3 seconds, then fades out in 0.4 seconds. Checking the box
again within that time turns the setting on and leaves the notice as it was. The 3 seconds
start again when the pointer leaves the notice and the check box loses the keyboard focus
in every window, so the statement does not go while the person reaches for it. Settings,
Voice and video has the same setting as a switch.

The main and detached windows show the same notice. Unmute names the seat drawn by the
control. No sound is sent or played because the notice appears.

Tests: `state::muted_speech::tests` (speech with pauses shorter than the limit shows the
notice after the onset and single words do not; a steady level shows it at its onset; pauses of two to four seconds keep it, and
it goes after the hold at the moment it asks to be woken; an unmute hides it and the next
mute needs new speech; the statement of a notice turned off lasts 3 seconds, waits while
held and fades), `voice::tests::the_muted_speech_notice_is_on_by_default_and_in_files_without_the_setting`
in `crates/client-core`, and the window live test
`call_notices::local_input_while_muted_shows_a_notice_in_both_windows` (no earlier than the
onset after a steady level, and gone after the hold), and the window live test
`live::muted_speech::local_speech_while_muted_offers_unmute_without_bypassing_held_keys`,
which reads that the notice comes no earlier than the onset after the mute, unchecks and
checks the box, waits until a notice turned off goes and stays away during speech, turns the
setting on again, keeps the notice through a pause and loses it after the hold, and keeps
its checks of remote speech, push to talk, push to mute and the call window.

## Hiding the own camera tile

"Hide your camera tile" in the call controls and the own tile's context menu hides the
local camera tile while its capture and publication continue. The people stage, camera
focus and the strip beside a share all omit it in both windows. Other people and the
Members panel stay complete. A mark states that the camera is on and hidden from the own
view, with Show; the control also restores it. A camera that is starting or reconnecting
uses a hidden-view statement without claiming that it is sending.

Hiding removes an own pin, and a hidden own camera cannot be pinned through the Members
panel. Other pins are kept. Turning the camera off restores the ordinary own avatar unless
"Hide people without video" also hides it. Starting the camera again uses the saved choice.
The installation stores `hide_own_camera` in sound settings version 8; earlier files default
to showing the own tile. Hiding requests no own preview size and changes no media gate.

The call side panel's close button returns keyboard focus to the call shell, so Tab can reach
the stage controls after the close button is removed. The share live check covers tile
and Members mute with Enter and Space, and context-menu share volume with arrow keys,
in the main and detached windows.
