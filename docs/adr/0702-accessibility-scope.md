# 0702. Accessibility scope of the first version

## Status

Accepted, pending step 0

## Context

The client is built with gpui (ADR 0701). The screen reader support of gpui is recent and was reported not to work on macOS. The interface uses hover menus and drag actions, which a person who uses only the keyboard cannot reach unless another path exists.

## Decision

- Screen reader support is not promised in the first version.
- Step 0 tests what a screen reader reads on each system (`docs/step-0/README.md`, test 2).
- The first version provides legible type, interface scaling and a keyboard path for every hover menu and every drag action.
- Each drag action also exists in a menu that the keyboard can reach (ADR 0703).
- The interface scales from 90% to 150% (ADR 0703).
- Text and controls use the typeface and the minimum size of ADR 0703, and no information is carried by italics or small capitals.

## Consequences

- A person who depends on a screen reader may not be able to use the first version. The result of step 0 states how much a screen reader can use.
- Every feature that adds a hover menu or a drag action must add the keyboard path with it.
- Keyboard shortcuts are not fixed by this record.
