# 0730. Application crate: the window, its state over the client core, and its tests

## Status

Accepted

## Context

The client core holds storage, keys, the flows, the event stream and the state of each server, and has no interface (ADR 0715). The desktop client is a gpui window (ADR 0701) whose thread must never wait for the network (ADR 0719). The window shows several servers at once (ADR 0703), and every further screen of the product is added to it. A window that reads the client core from its drawing code cannot be tested without a server and cannot be drawn in a state that is hard to reach, such as a ban. The tests of the client must run against the real server (ADR 0902) and must not need a display.

## Decision

Crate:

- The crate `junto-client` holds the desktop application. Its binary is `junto`. It depends on `junto-client-core`, on `junto-media` and on the gpui set of ADR 0701, and holds no rule of joining, of login, of the event stream or of a call.
- Of the component library it uses the unstyled layer `gpui-base`: the root view with the Tab order and the focus trap, the scroll bar, the text input and the slider. The message list is the list element of gpui, and the composer is the text area of the same layer (ADR 0740); the styled layer is not used.
- The arguments of `junto` are `--data-dir`, `--scale` and `--no-activate`. The data directory is the argument, then `JUNTO_DATA_DIR`, then the directory of the user (ADR 0710). A data directory that cannot be opened ends the start with the reason on standard error; nothing in it is changed.

State:

- One entity, the application state, owns the client. It holds what the person selected (a server, a workspace per server, a channel per workspace), the settings of the interface, and the tasks that keep the window current.
- It wakes on `Client::changes` and on the `watch` of each saved server (ADR 0719), and then tells the window to draw. It never asks at an interval for a state.
- A function without side effects derives what the window shows from a copy of what the client core holds of each server, the selection, the names that were read, and what the calls of the client hold (ADR 0734): the call of the device, the levels as lit segments of the meters, the sound settings and the devices. The drawing code takes that data and holds no rule about a server. The function is tested without a window and without a server.
- A selection that the state no longer holds is replaced when the view is derived: the first server, the default workspace, the first text channel.
- The server that is shown is the selected server of the client core (ADR 0107). The members of the shown workspace are loaded and released as ADR 0719 states.
- A person in a voice channel is named from the members of the workspace. An account that the list does not hold is read from its profile route. A read that gets no answer is repeated with a later change of the state, at most three times for one account; a refusal is final.
- The lists and the name of an account are shown only while the state of the event stream belongs to the account of the session. After a login to another account, a logout, a ban or a removal, the state of the account before is not shown, although the event stream can still hold it for a moment (ADR 0718).
- A login that gave the device another account than the one of its last contact is told by a notice in the notice area of the server (ADR 0712).
- Every write of a file of the data directory that the window starts runs on a background thread: the settings, the name of the preset, the connection mode and the removal of a server. The thread of the window takes no lock of the data directory after the start.
- These writes are one queue of the application and run in the order in which the person asked for them. The queue does not belong to a window: a write that was accepted is done after its window has closed. When the application quits, it waits for the queue, at most 5 seconds. A write takes that long only while another program holds the lock of the data directory; after 5 seconds the application ends and that write is lost.
- A removal of a server that is asked for while a removal of the same server runs is not started.
- The result of a write is used only where it still applies. A name that was stored marks the name field as saved only when the field still holds that name.
- The argument `--scale` sets the interface scale of one start and is not stored. A scale that the person chooses in the settings is stored and ends the scale of the start.
- What the application does by itself: it contacts every saved server at the start; it checks a join request or a device link that waits every 5 seconds, as `junto-cli --wait` does; and every 20 seconds it contacts a server on which this device has an account and no session.
- A recovery is acknowledged only by the button of its notice (ADR 0118). A notice that was drawn is not acknowledged.

Window:

- The main window is one view. It holds what belongs to the window: the layer that is open (a dialog or a menu), the text of its fields, and where the keyboard focus returns to.
- A channel list groups the channels without a category by kind, under "Text channels" and "Voice channels", and then lists each category that holds a visible channel, in the order of the server.
- A text channel shows its messages and the composer (ADR 0740). A voice channel shows the chat of the channel, the state of its call, the action that enters it, and the call when the device is in it (ADR 0735). The window draws no control of a function that the build does not have, with one exception: the camera button of the strip of the call is drawn disabled, so that the strip has its final form.
- In the call of the device the level meter of a person shows the level that the media engine measures. In another voice channel it shows three of five segments while the server reports the person as speaking (ADR 0513), and none otherwise.
- The window follows the levels of a call only as far as they change a meter: a level is turned into lit segments before it is compared with what is drawn.
- The call of the device ends with the application: the server is told when the application quits, with a wait of at most 2 seconds.
- The interface text is in one module, with one sentence for every failure of a flow and for the error codes of joining. A test fails when the client core adds a failure without a sentence.
- A device name that the application sends is the name of the product and of the operating system. It holds nothing of the machine or of the person.

Notice areas:

- A notice, error or bar that arrives after a view is drawn must preserve the bounds of its existing controls. Its region either keeps a fixed height or grows from the edge opposite the content anchor. The notice area scrolls when its contents do not fit; notices are not drawn over controls.
- State panels put notices in the growing part under their actions (ADR 0732).
- A message area reserves a scrolling notice area above the message list, under the controls of a direct call. The list keeps both its bounds and its scroll position when a notice arrives, whether it is at its end or scrolled away. Changing the top edge and compensating the scroll would preserve row coordinates but would clip the rows under the new notice; it would not preserve the control under the pointer.
- A call reserves a scrolling notice area at the end of its stage column, including the meeting bar. The side panel keeps its bounds. This area has a fixed height even when empty: the tiles are centred inside the stage and a shared picture fits its height, so taking height from either edge would move them. The separate call window follows the same rule.
- The channel column places entry notices and connection, sound and update notices below its scrolling list and above the call controls. Its list keeps its top anchor and its call controls keep their bottom anchor.
- A form reserves a scrolling status and error slot wherever a result can arrive before another control. Pending requests and explanations of an absent value share that reserved geometry with their completed state. The slot keeps its height when empty and when its text wraps. This also keeps the height and position of a centred dialog. A request result may instead follow all controls in a region whose top position is fixed.
- Changes that replace the whole body, such as a server state panel becoming channel content after its snapshot, are separate view transitions. Controls may change position at that transition, including the recovery acknowledgement. The notice remains until acknowledged; drawing or replacing a view never acknowledges it. Preserving a control across unrelated bodies would require a permanent shared location for that control, which this layout does not provide.

Tests:

- The tests of the window run the real window code on gpui's test platform against the server binary of this checkout. A test puts text into a field, clicks an element by its name, presses keys, and reads the data that the window drew last. It does not read the state of the client core in place of the window: the client core can be ahead of the window for a moment. A second client without a window acts as the administrator.
- An element has a fixed name for the tests. The row of a channel and the row of a member are named by the identifier of the channel and of the account, because a server can hold two channels or two members with one name.
- A test that needs a request to be on its way at a known moment holds it at the proxy of the test and releases it. It does not rely on a wait of a fixed length.
- The client core answers from its own threads at real times. A test allows that in gpui's scheduler and waits for a state with a bound of 60 seconds per step.
- The application state can be given fixed data of servers in place of the client core, and a fixed state of the calls. Only the build with the feature `test-support` has this. The tool of ADR 0731 uses it.
- A test gives the application sound devices that exist only in the program: a tone as the microphone and a sink that measures (ADR 0733). The test with a call needs a media server and is ignored in the plain run.

## Consequences

- A further screen is a part of the one window and reads the same derived data.
- A state of a server that is hard to produce is drawn from fixed data, and its derivation is tested as a function.
- The window follows the event stream without a reload: a change of a channel, a member or a call is in the next drawn frame.
- At the background level the workspaces of a server that is not selected are as old as its last snapshot (ADR 0107). The server book lists them from that state.
- The name of an account that is in a voice channel and is not a member of the workspace costs one request, and at most three when the server does not answer.
- A test that starts the application a second time on one data directory closes the first window before, and checks that the state of the application ended with it.
- The tests of the window need the server binary and PostgreSQL, as the tests of the client core do.
- The crate has code for one system in two places: the height of the strip for the window controls of macOS, and the options of the title bar. Windows-specific code is not written; nothing in the crate prevents a Windows build of the same code.
