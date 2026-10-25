# 0703. Interface layout, visual design, theme and language

## Status

Accepted

## Context

The client shows several servers, the channels of one workspace and the content of one channel or call at the same time. A client that is first built for one server needs its state rebuilt when a second server is added. Colours, fonts, spacing and interface text that are spread over the code cannot be changed in one place, which a second theme or a second language would require.

## Decision

Layout:

- The layout has three columns: server book, channel list, content. This is the arrangement that Discord uses.
- The server book collapses to icons with a hover menu.
- The state and the actions of a voice channel are in the call header.
- Call settings and voice channel settings are in a Settings tab next to the Chat and Members tabs.
- The user's own call controls are next to the profile strip.

The title bar of the main window:

- macOS: the window asks for a transparent title bar; the server book reaches the top edge, and the system draws the window buttons over its top strip. The top strip of the server book and the headers at the top of the channel column, of the main area and of the side panels act as the title bar. The header of a dialog and of the administration and account layers does not: it lies inside the window, and a press there is a press on the layer. A press there and a move of the pointer move the window (`start_window_move`), and a second press of a double press does what the system setting of a double press on a title bar names (`AppleActionOnDoubleClick`: zoom, fill, minimize or nothing; gpui reads it). A control in a header keeps its press: gpui gives a press to the focusable element under the pointer first and marks the press as handled, and a header acts only on a press that no control took. The window takes the move of the title bar from the system (`app_owns_titlebar_drag`): otherwise the system reads a press on the transparent title bar itself, a double press zoomed the window only on its edge and along one axis, and a click in the title bar waited for the system to tell a double press. While a layer is open it covers the headers; a band of the height of the top strip of the server book then lies over the layer at the top of the window and acts as the title bar, a press there does not close the layer, and the panel of a dialog or of a layer starts under the band.
- Windows: the window asks for no title bar of the system, and draws a strip of 32 pixels above the columns, with the name of the application and the buttons minimize, maximize (restore when maximized) and close at its right end, each 46 pixels wide. These are the measures of Windows 11 in pixels of the display: the strip follows the scale of the display, not the interface scale, as the title bars of other programs do. gpui answers the hit test of Windows with the area of each part (`WindowControlArea`): the buttons are the caption buttons of the system, and the rest of the strip is the caption. Windows therefore moves the window from the strip, snaps it at a screen edge, maximizes and restores it at a double press, shows its window menu, offers the snap layouts on the maximize button, and acts on a button at the release of the press; the window draws the colours under the pointer and while pressed (the close button red, as on every window of Windows). The left, right and bottom borders of the frame of the system resize the window. When the window is not maximized gpui answers the top rows of the strip, of the thickness of the frame, as the top border, also over the buttons, as the title bar of the system does. A maximized window fills the work area of its monitor and leaves the taskbar free, and the top row of the screen at the close button closes it. The call window keeps the title bar of the system.
- Linux: X11 and a Wayland compositor that draws the decorations draw the title bar. A Wayland compositor without server decorations (`xdg-decoration`), such as GNOME, leaves them to the program: then each window, the main window and the call window, draws the same strip. The buttons are those that the compositor supports, the window handles them, a press and a move on the strip move the window, a double press maximizes and restores it, and a press of the right button opens the window menu of the compositor. A band of 4 pixels inside each edge, and of 12 pixels at each corner, resizes the window, except at an edge that the compositor tiled and in a maximized window. The bands lie inside the content, so the program needs no transparent border around the window.
- A dialog or a layer of the window covers the columns and not the strip: the window buttons stay usable while a dialog is open.

Client state:

- Client state is built for several servers from build step 1, although the screens for several servers come in build step 6 (ADR 0903).

Visual design:

- Surfaces are light and stone-coloured. The server rail is dark.
- Accent colours are terracotta, green and ochre.
- Red is reserved for recording, leaving a call and errors.
- Avatars without an image are generated tile patterns.
- Text and controls use the Atkinson Hyperlegible typeface, with at least 14px for body text.
- No information is carried by italics or small capitals.

Theme, scaling and language:

- The first version has one theme. All colours, fonts and spacing are in one theme file.
- The interface scales from 90% to 150%. The scale changes the rem size of the window, and text and controls of the component library follow it.
- The client gives its own sizes in rems. A size that is given in pixels does not scale: a dialog with a width in pixels kept that width at 150% while its text grew.
- The interface language is English. All interface text is in one catalog.

Drag and drop:

- A person can be dragged between voice channels, a channel can be dragged to a new position or category, and a file can be dragged from the desktop into a chat.
- Each drag action also exists in a menu that the keyboard can reach (ADR 0702).
- Attaching a file does not depend on a file drop: a file chooser attaches a file on every system. On X11 a file drop from another application did not reach the drop handler of gpui in three of three attempts, and the cause was not found. A drop from a file manager was not tested on any system (`docs/step-0/README.md`, test 2).

## Consequences

- A second theme requires a second theme file and no change to the code that draws the interface. A dark theme is in the later list of `docs/scope.md`.
- A second interface language requires a second catalog.
- The cost of state for several servers is paid in build step 1, before any screen uses it.
- No other element of the interface uses red.
- At 150% in a window 1320 points wide, the three side columns at fixed rem widths left 180 points for the content. The client needs a rule for which panel collapses at each scale and window width. ADR 0731 fixes that rule.
- The interface scale is separate from the scale of the desktop, which gpui reads from the system. Both were observed at 150%.
- The drag of a file from the desktop is available only where the drop reaches the client. Until a drop from a file manager is tested on each system, the file chooser is the path that works everywhere.
- The permissions for invite and move apply to a drag of a person between voice channels in the same way as to the menu action (ADR 0504).
- A press that Windows takes on the strip, such as the move of the window, ends without a release in the window. A menu that such a press closed opens again at the next press on its button: each press, not only a release, ends the press that closed a menu.
- The close button of the strip is red under the pointer, an exception to the use of red that follows the convention of Windows.
- On Windows and on a Wayland compositor without server decorations the columns are 32 pixels lower. The strip of the server book where macOS draws its window buttons stays 10 pixels high there.
- On Linux the band of an edge lies over the content: a press at the outermost 4 pixels of a scroll bar at the right edge resizes the window.
- The window tests draw the strip on every system with a global of the tests (`DrawnFrameInTests`, which also states the tiled edges and the maximized state on Linux) and read what the title bar asks of the system (`TitleBarRequests`: move, double press, minimize, zoom, resize from an edge, window menu), because the test window of gpui carries out no such request: the strip and its buttons at their places above the columns; the buttons, the double press, the move and the right press of the strip handled by the window, also while a dialog is open; a press in each band and corner, no band at a tiled edge and none when maximized; the headers that act as the title bar only in a window whose content reaches it, a double press and a move there, a press on the button of the server menu that stays the button's, a dialog header that makes no request, and the band over an open dialog that moves the window without closing the dialog. On Windows the main window draws the strip without that global. The check of the Windows package tests the answer of the real window to the hit test, its borders, its buttons and its maximized rectangle (ADR 0755). The action of the system setting at a double press on macOS and the strip on GNOME are checked by a person.
