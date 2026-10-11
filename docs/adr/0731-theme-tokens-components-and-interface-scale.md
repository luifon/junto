# 0731. Theme tokens, components, the interface scale and the rule for a narrow window

## Status

Accepted

## Context

ADR 0703 fixes one theme in one file, an interface scale from 90% to 150%, and a text catalog, and leaves open which panel gives way when the window is narrow. ADR 0702 requires a keyboard path and a label for every control. The approved design gives its values as CSS: colours, type sizes, radii and the measures of each part. Every later screen is built from the same parts, so a part that is defined twice or half is paid for on each screen. gpui draws an SVG file in one colour and has no screen of its own to compare a drawing with the design.

## Decision

Theme:

- One value, the theme, holds every colour, the three font families, the text styles, the radii and the fixed measures of the layout. Drawing code names no colour and no font.
- A measure is given in the pixels of the design at 100% and becomes rems. The size of one rem is 16 pixels times the interface scale. A border is one pixel at every scale.
- The fonts of the design are carried in the binary: Atkinson Hyperlegible Next for text and controls, with its italic faces for the italic text of a message (ADR 0740), Atkinson Hyperlegible Mono for addresses and codes, Marcellus for titles of 18 pixels and more. Each is under the SIL Open Font License, and its licence text is beside its files.
- The icons are the drawings of the design as SVG files in the binary. An icon is drawn in one colour that the caller gives.
- No interface text is smaller than 12 pixels, and running text has at least 14 (ADR 0703). A test holds both.

Generated avatars:

- An avatar without an image is a square of tiles that is mirrored on both axes: 5 by 5 tiles in three colours for a person and a server, 3 by 3 tiles in two colours for a workspace.
- The pattern and the colour set follow from a hash of the identifier, so an account has the same avatar on every device. Every colour of the set is in the pattern.
- The edges of the tiles lie on pixels of the display, and the line between two tiles has at least one such pixel.

Components:

- Each component has one definition: icon, avatar, tessera band and selection mark, level meter, button, text field, segmented control, switch, choice box, tab, volume control, badge, dot, chip, flag, recording mark, status line, the rows of the two lists, the tile of a person in a call, category header, section label, panel header, notice, wait box, state panel, menu, dialog, tooltip and scroll area.
- A control is a tab stop, is activated with Enter or Space, has a role and a label for the accessibility tree, and shows a mark while it holds the keyboard focus: a ring of 2 pixels, in terracotta on stone and in ochre on the server book. A text field shows its caret and the border colour of its focus, as the design draws it. A disabled control is no tab stop.
- A dialog and a menu hold the keyboard focus while they are open. Tab stays inside, the arrow keys move between the items of a menu, Escape closes, and the focus returns to where it was. A click outside a menu and on the cover of a dialog closes it. A dialog that must be answered has no close control. When the control that holds the focus leaves the dialog, as a button does that is disabled while its action runs, the focus returns to the dialog.
- The menu of a server in the collapsed server book opens when the pointer is over the icon and stays for 220 milliseconds after the pointer left, so that the pointer can move into it. Enter on the icon opens the same menu and moves the focus into it.
- The text field takes its editing from the text input of the component library and its frame, colours and type from the theme.

Scale and narrow window:

- The interface scale is one of 90, 100, 110, 125 and 150 percent. It is a setting of the installation in `settings.json` (ADR 0710), with the form of the server book. A change of one setting is written as a change of that setting, under the lock of the data directory. The window uses the new value at once. When the write fails, the settings dialog says so until that setting is in the file, and the next change of any setting writes it again.
- The layout reacts to the width of the window in design pixels, which is its width divided by the scale. Below 1200 design pixels the window is narrow.
- In a narrow window the server book collapses to icons, and the side panel lies over the content at the right edge, with a flat cover behind it; a click on the cover closes the panel. The person can expand the server book in a narrow window; that choice lasts until the window is wide again.
- The channel column keeps its width at every scale.

Pictures of the screens:

- The tool `junto-screens` draws each screen and state with the real window code on gpui's test platform and its headless renderer, and writes PNG files. It opens no window and takes no keyboard focus. A developer compares the files with the design.

## Consequences

- A second theme is a second value of the theme. A colour that a screen needs and the theme lacks is added to the theme.
- At 150% in a window of 1440 pixels the layout has 960 design pixels and is narrow, as the design draws it.
- A window of 1320 pixels at 100% is wide and gives the content 544 pixels while the side panel is open.
- The width of the scroll bar and of a border does not follow the scale.
- The drawing of a screen can be compared with the design on a machine without a display. The window controls of the operating system are not in such a picture.
- An avatar is not the avatar of the design for the same name: the design drew fixed patterns, and the product derives them.
- An image that an account has set as its avatar is not shown yet; the generated avatar stands in its place until the avatar images are built.
