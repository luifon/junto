# 0702. Accessibility scope of the first version

## Status

Accepted, pending step 0

Open: the accessibility tree was not read on Windows. On macOS and Linux the tree was read through the accessibility interface of the platform, and no screen reader program was started (`docs/step-0/README.md`, test 2).

## Context

The client is built with gpui (ADR 0701). The interface uses hover menus and drag actions, which a person who uses only the keyboard cannot reach unless another path exists.

gpui builds an accessibility tree and gives it to the platform: to the accessibility interface of macOS, and to AT-SPI on Linux. An element is in the tree when it has an identifier and a role. Step 0 read the tree of a test application on both systems (`docs/step-0/README.md`, test 2):

- In the tree, with label and state: buttons, switches, check boxes, text inputs, headings, lists with their items, menus and menu items. An activation from the accessibility side runs the click handler of the control.
- Not in the tree: the text of messages. The Markdown text view of the component library adds nothing to the tree, and the message list exposes only its buttons. Plain text inside an element without a role is not in the tree either.
- The tree on Linux has the same nodes as the tree on macOS. On Linux gpui publishes the tree only while the accessibility status of the session is enabled.

## Decision

- Screen reader support is not promised in the first version.
- A screen reader can use the labelled controls of the client and cannot read a conversation. Message text reaches the accessibility tree only if the client adds an accessible node for each message. The first version does not promise that node.
- The first version provides legible type, interface scaling and a keyboard path for every hover menu and every drag action.
- Each drag action also exists in a menu that the keyboard can reach (ADR 0703).
- The interface scales from 90% to 150% (ADR 0703).
- Text and controls use the typeface and the minimum size of ADR 0703, and no information is carried by italics or small capitals.

## Consequences

- A person who depends on a screen reader can operate buttons, switches, inputs and menus of the first version, and cannot read messages with it.
- A control reaches the accessibility tree only when the client gives it a role and a label. A control with an icon and no text needs an explicit label.
- The keyboard paths were tested on macOS and on Linux in an X11 session: the Tab order, a shortcut bound to an action, and two keyboard paths to a hover menu worked.
- Every feature that adds a hover menu or a drag action must add the keyboard path with it.
- What a screen reader program speaks was not tested. The statement above describes the content of the tree.
- Keyboard shortcuts are not fixed by this record.
