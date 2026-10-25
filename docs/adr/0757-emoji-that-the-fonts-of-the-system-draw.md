# 0757. Emoji that the fonts of the system draw

## Status

Accepted

The measurement, the picker and the stand-ins are tested in the Linux build image with its fonts, and with the text system of the test platform of gpui. The measurement with the text systems of macOS and Windows is not run by a test of this record: the tests that lay out text with the text system of the platform are left out on macOS, where that text system is made on the main thread only, and they were not run on Windows.

## Context

The emoji picker offers the table of the crate `emojis` in the version that the server takes (ADR 0408, ADR 0740), which is Unicode 17.0. The window draws emoji as text with the fonts of the system:

- macOS: Apple Color Emoji. gpui marks a glyph as a colour glyph by the PostScript name of its font, `AppleColorEmoji`. CoreText draws a sign that no font has with its font of last resort, whose glyph is a box with the code point.
- Windows: Segoe UI Emoji. gpui draws the layers of a COLR glyph of any font, and takes a fallback family only from the fonts of the system. Segoe UI Emoji has no picture of a flag.
- Linux: gpui draws colour only from a font with the PostScript name `NotoColorEmoji`. For a sign that the font of the text does not have, its text system (cosmic-text) tries a fixed list of families, and DejaVu Sans, which most Linux systems have, is in that list before Noto Color Emoji. DejaVu Sans has many emoji of the first versions, in one colour. The Linux build image has DejaVu Sans and Noto Color Emoji 2.042 of Debian 12.

A system font can be older than the table. Then an emoji is drawn as an empty box (a sign without a glyph), or a sequence (a skin tone, a joined sequence, a flag) falls apart into several pictures or letters. The first live test showed boxes in the picker on Windows; in the Linux build image the picker showed boxes for the emoji of Unicode 16 and 17. The same happens to an emoji in a message and in a reaction, which another system sent.

The category tabs of the picker were buttons of the kind for one icon, 26 by 26 units, with the name of the group as their label. The labels were wider than the buttons and were drawn over each other.

## Decision

Measurement:

- The module `emoji` of the client lays out each emoji of the table once, at 20 pixels, in the family of the text of messages, with the text system of the application, and keeps the result for the life of the process. Only the emoji that a screen needs are measured: the groups and the search results of the picker, and the emoji that the drawn messages and reactions contain.
- The result is one of three:
  - Broken: no glyph, a glyph with the number 0 (the missing glyph of a font), or two or more glyphs on a line at least 1.5 times as wide as the reference, the grinning face of the first emoji version. A sequence that falls apart is two or more pictures wide. One glyph is never Broken by its width: DejaVu Sans draws the sleeping face as one wide glyph. On macOS an emoji that the colour font does not draw is Broken too, because the font of last resort has a glyph for every code point; Apple Color Emoji draws every emoji of the table that the system knows.
  - Colour: the first glyph is a colour glyph.
  - Text: a sign of a font without colour.
- The width of the reference is that of its one glyph. When no font draws the reference as one glyph, the size of the type is the reference.

The picker:

- It offers the emoji that are not Broken. A group without such an emoji has no tab. When no group has one, the picker states that the system has no font that draws emoji.
- The tabs are icons of the design (16 by 16 units, one colour) in buttons of 26 by 26 units, with the name of the group as their tooltip and their accessible name, as in Slack and Discord. Nine tabs take 250 units; a dialog is 500 units wide, or the width of the window when that is less.

Messages and reactions:

- The text of a message, the quoted line above a reply and the sign of a reaction show an emoji that is Broken as its short code between colons (`:brazil:`), or as its name between colons when it has no short code (`:face with bags under eyes:`). The accessible label and the tooltip of a reaction use the same text.
- Only the full form of an emoji in the table is replaced. A sign such as © without the selector of the emoji form is text and stays as it is.
- The message that the server holds is not changed: another device with a newer font draws the emoji.

The colour of the emoji of the first versions on Linux is not changed:

- The window does not name the family Noto Color Emoji, neither as the family of an emoji nor as a fallback family of a font. gpui's Linux text system removes a requested font face from its font database when the face has no glyph for the letter m, and Noto Color Emoji has none. After such a request no emoji is drawn in colour for the rest of the process: a layout in the Linux build image gave the missing glyph for 👍, which Noto Color Emoji draws before the request.
- On a Linux system with DejaVu Sans, the emoji that DejaVu Sans has are drawn in one colour, as before this record. They are drawn whole, so the picker offers them.

A bundled colour emoji font was considered and is not used:

- gpui draws colour only from the system fonts named above (by PostScript name on macOS and Linux), and on Windows only the COLR format and fallback families from the fonts of the system. A bundled font would be drawn without colour on macOS and Linux, and would not be a fallback on Windows, without a change of three platform crates of gpui (`gpui-pre-macos`, `gpui-pre-wgpu`, `gpui-pre-windows`) in `vendor/`.
- No COLR font of version 0, the one format that the three text systems draw, covers Unicode 17. Twemoji Mozilla ends at Emoji 14; a font of the current Twemoji or Noto pictures would have to be built from their SVG files.
- The measurement is needed with a bundled font as well: a message can hold an emoji that is newer than any font of the client.

## Consequences

- The picker of each system offers a different set: the emoji that its fonts draw. A person cannot react with an emoji that the own system does not draw.
- A reader on an older system sees the short code of a newer emoji. On Windows a flag is expected to fall apart into two letters and to be shown as its short code, and the group of flags to have no tab; this was not run on Windows.
- An emoji that is newer than the table of the client is not in the table, is not measured, and is drawn as the fonts of the system draw it.
- The measurement of a group of the picker takes one layout per emoji at the first opening, up to a few hundred layouts; a later opening reads the kept results.
- The emoji of the first versions stay in one colour on Linux with DejaVu Sans until gpui's Linux text system keeps a requested face without the letter m, or the client carries a copy of `gpui-pre-wgpu` in `vendor/` with that correction (`vendor/README.md`, ADR 0733).
- A later bundled font needs copies of the three gpui crates in `vendor/`, with their corrections in `vendor/README.md` and ADR 0733, and its size counts against the measurements of ADR 0914.
- The text of a display name, a channel name and a notice is not measured: an emoji there that the fonts do not draw is drawn as a box.
