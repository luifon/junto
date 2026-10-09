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
- The interface scales from 90% to 150%.
- The interface language is English. All interface text is in one catalog.

Drag and drop:

- A person can be dragged between voice channels, a channel can be dragged to a new position or category, and a file can be dragged from the desktop into a chat.
- Each drag action also exists in a menu that the keyboard can reach (ADR 0702).

## Consequences

- A second theme requires a second theme file and no change to the code that draws the interface. A dark theme is in the later list of `docs/scope.md`.
- A second interface language requires a second catalog.
- The cost of state for several servers is paid in build step 1, before any screen uses it.
- No other element of the interface uses red.
- The permissions for invite and move apply to a drag of a person between voice channels in the same way as to the menu action (ADR 0504).
