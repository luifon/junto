# 0799. The logo of a server in the window

## Status

Accepted

## Context

A server has a public logo that an administrator sets and removes (ADR 0101). The public information of the server names its path, the event `server_logo_changed` states its description, and the snapshot does not carry it. The client core holds what the events stated and reads the image without a session (ADR 0795). The server book of the window drew a generated picture for every server (ADR 0731, ADR 0780). The window reads and decodes the pictures of people with a cache of plain data (ADR 0783).

## Decision

Reading:

- The window holds at most one image per server, with the hash that the client computed of its bytes. The rules are a cache of plain data, as for the pictures of people; the state of the application starts the request that the cache names and gives it the answer.
- An event that states no logo drops the image. An event that states a description with the hash of the held image reads nothing. An event that states another description reads the image, and the held image stays shown until the new one arrives.
- Without a statement of an event, the image is read once when the server book names a path. After a reload of the state and after missed notices it is read again once, because the events between were not delivered. A read that finds no logo drops the image.
- One request per server at a time. A read without an answer, or one that the server limits, is tried again with a later change of the state, at most three times; the held image stays. Bytes that are no image are not used and not asked for again. The image is decoded away from the window, at the size of an avatar image.
- The answer to a request that started before a newer statement is not used.

Server book:

- A server with an image shows it in place of the generated picture, at the same size and with the same corners, in the expanded row, in the collapsed icon and in the head of the menu of a server. The image is an image in the accessibility tree with the label "Logo of" and the name of the server (ADR 0702). The generated picture has no label.

Administration:

- The page "Server" exists for an administrator. It shows the logo or states that the server has none, and has "Choose image…" and "Remove logo". The removal is a question in place of the two actions.
- A file is read away from the window, at most 10 MiB, and cut in the dialog of ADR 0119; the cut is checked before a request: at most 2 MiB, and a PNG, JPEG or WebP image by its first bytes. The page states the bounds, and that the logo is public and visible before a person joins.
- A refusal of the server is one sentence under the actions.

## Consequences

- A member sees a new logo without an act: the event states it, and the window reads the image once.
- A server with a logo costs each window one request of its image at the start, one after the first state of the server, and one after each later reload of that state.
- The picture of a server that the window cannot draw, or that it has not read yet, is the generated picture.
- The page "Server" is designed in this record.
