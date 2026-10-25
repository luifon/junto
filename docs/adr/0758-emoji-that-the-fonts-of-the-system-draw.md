# 0758. Emoji that the fonts of the system draw

## Status

Accepted

The test target `emoji_fonts` of the client lays out emoji with the text system of the system on the main thread and checks facts of each system; the CI jobs of Linux, macOS and Windows run it (ADR 0905). It passed in the Linux build image and on Windows 11 with DirectWrite and Segoe UI Emoji; on macOS it runs in the macOS CI job.

## Context

The emoji picker offers the table of the crate `emojis` in the version that the server takes (ADR 0408, ADR 0740), which is Unicode 17.0. The window draws emoji as text with the fonts of the system:

- macOS: Apple Color Emoji. gpui marks a glyph as a colour glyph by the PostScript name of its font, `AppleColorEmoji`. CoreText draws a sign that no font has with its font of last resort, whose glyph is a box with the code point.
- Windows: Segoe UI Emoji. gpui draws the layers of a COLR glyph of any font, and takes a fallback family only from the fonts of the system. Segoe UI Emoji has no country flags: DirectWrite draws a country flag as two letters without colour. It has the other flags, such as the chequered flag. A headless gpui platform of Windows has a text system without fonts; the platform that can open windows has DirectWrite.
- Linux: gpui draws colour only from a font with the PostScript name `NotoColorEmoji`. For a sign that the font of the text does not have, its text system (cosmic-text) tries a fixed list of families, and DejaVu Sans, which most Linux systems have, is in that list before Noto Color Emoji. DejaVu Sans has many emoji of the first versions, in one colour. The Linux build image has DejaVu Sans and Noto Color Emoji 2.042 of Debian 12.

A system font can be older than the table. Then an emoji is drawn as an empty box (a sign without a glyph), or a sequence (a skin tone, a joined sequence, a flag) falls apart into several pictures or letters. The first live test showed boxes in the picker on Windows; in the Linux build image the picker showed boxes for the emoji of Unicode 16 and 17. The same happens to an emoji in a message and in a reaction, which another system sent.

The category tabs of the picker were buttons of the kind for one icon, 26 by 26 units, with the name of the group as their label. The labels were wider than the buttons and were drawn over each other.

## Decision

Measurement:

- The module `emoji` of the client lays out every emoji of the table, with each skin tone, at 20 pixels in the family of the text of messages, with the text system of the application, on the background executor. It starts at the first use: the first drawing of a message list, a reply or the picker. When it ends, the results are kept for the life of the process and every window is drawn again. Until then every emoji counts as drawn. The layouts of the measurement are dropped with its own text system; only the result of each emoji is kept.
- The result is one of three:
  - Broken: no glyph, a glyph with the number 0 (the missing glyph of a font), a glyph of the font that draws a code point of private use (on macOS the font of last resort; on Linux and Windows such a code point has the glyph 0), or a sequence that falls apart. A glyph takes place when its advance, the distance to the next glyph or to the end of the line, is more than a quarter of the width of the grinning face of the first emoji version; a joiner, a variation selector, a tag and a combining keycap take no place. Two or more glyphs that take place fall apart when one of them has no colour, such as the two letters of a country flag on Windows (9.7 units each at 20 pixels, 18.8 together against 27.5 for the face), or when the line is at least 1.5 times as wide as the face. The colour glyphs of one picture can overlap within its width: Segoe UI Emoji draws a family sequence as three overlapping colour glyphs on a line 25.1 units wide. One wide glyph, such as the sleeping face of DejaVu Sans, is not Broken.
  - Colour: the first glyph is a colour glyph.
  - Text: a sign of a font without colour.

The picker:

- It offers the emoji that are not Broken. A group without such an emoji has no tab. When no group has one, the picker states that the system has no font that draws emoji.
- The tabs are icons of the design (16 by 16 units, one colour) in buttons of 26 by 26 units, with the name of the group as their tooltip and their accessible name, as in Slack and Discord. The open tab is marked selected for a screen reader, and the name of the open group is a line above the grid, also for a person who moves through the tabs with the keyboard. Nine tabs take 250 units; a dialog is 500 units wide, or the width of the window when that is less.

The stand-in of an emoji that is Broken:

- Its short code between colons (`:brazil:`), or its name between colons when it has no short code, with the colon of the name of a skin tone written as a comma (`:thumbs up, medium skin tone:`). It is drawn in the colour of secondary text, so that it differs from text that a person typed.
- It replaces the emoji in the text, the link text and the inline code of a message, the quoted line above a reply, the line "Replying to" above the composer, the sign, the tooltip and the accessible label of a reaction, and the text of a search result and of a notice, whose marked matches follow the replacement.
- The full form of an emoji of the table is replaced, and a sequence of several code points that lacks some of its selectors is replaced as the emoji of the table. A single sign such as © without the selector of the emoji form is text and stays as it is.
- The message that the server holds and the emoji that a reaction sends are not changed: another device with a newer font draws the emoji.

The colour of the emoji of the first versions on Linux is not changed:

- The window does not name the family Noto Color Emoji, neither as the family of an emoji nor as a fallback family of a font. gpui's Linux text system removes a requested font face from its font database when the face has no glyph for the letter m, and Noto Color Emoji has none. After such a request no emoji is drawn in colour for the rest of the process: a layout in the Linux build image gave the missing glyph for 👍, which Noto Color Emoji draws before the request.
- On a Linux system with DejaVu Sans, the emoji that DejaVu Sans has are drawn in one colour, as before this record. They are drawn whole, so the picker offers them.
- A colour font with another name than Noto Color Emoji is not drawn in colour by gpui on Linux. If its glyphs are bitmaps only, gpui draws them empty, while the measurement counts them as drawn. This case is not detected.

A bundled colour emoji font was considered and is not used:

- gpui draws colour only from the system fonts named above (by PostScript name on macOS and Linux), and on Windows only the COLR format and fallback families from the fonts of the system. A bundled font would be drawn without colour on macOS and Linux, and would not be a fallback on Windows, without a change of three platform crates of gpui (`gpui-pre-macos`, `gpui-pre-wgpu`, `gpui-pre-windows`) in `vendor/`.
- No COLR font of version 0, the one format that the three text systems draw, covers Unicode 17. Twemoji Mozilla ends at Emoji 14; a font of the current Twemoji or Noto pictures would have to be built from their SVG files.
- The measurement is needed with a bundled font as well: a message can hold an emoji that is newer than any font of the client.

## Consequences

- The picker of each system offers a different set: the emoji that its fonts draw. A person cannot react with an emoji that the own system does not draw.
- A reader on an older system sees the stand-in of a newer emoji. On Windows a country flag is shown as its stand-in, and the group of flags offers the flags that Segoe UI Emoji has.
- The measurement of the table of 3,944 emoji took 2.3 seconds in the Linux build image (two processors) and 0.3 seconds on Windows 11, in debug builds, on a thread other than the window thread, once per process. Until it ends, a newer emoji can be drawn as a box and the picker offers every emoji.
- An emoji that is newer than the table of the client is not in the table, is not measured, and is drawn as the fonts of the system draw it.
- The emoji of the first versions stay in one colour on Linux with DejaVu Sans until gpui's Linux text system keeps a requested face without the letter m, or the client carries a copy of `gpui-pre-wgpu` in `vendor/` with that correction (`vendor/README.md`, ADR 0733).
- A later bundled font needs copies of the three gpui crates in `vendor/`, with their corrections in `vendor/README.md` and ADR 0733, and its size counts against the measurements of ADR 0914.
- Not replaced: the text of a display name, a channel name and other labels of the interface, and the text that a person types into the composer. An emoji there that the fonts do not draw is drawn as the fonts draw it.
