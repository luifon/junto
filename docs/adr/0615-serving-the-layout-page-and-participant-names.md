# 0615. Serving the layout page and the participant names in join tokens

## Status

Accepted

## Context

The recorder loads the layout page of the project from the server (ADR 0602). The page is built from `recording-layout/` with Node.js into a directory of static files (ADR 0609). The recorder adds the address of the media server, its own room token and the layout to the query of the page address, and it cannot send a header. The page labels a tile with the display name of its participant and never with an identity (ADR 0609). The server logged the whole address of each request, with its query. The Rust build of the server needs no Node.js.

## Decision

Serving:

- The server serves the built page at `GET /recording-layout/` and its files at `GET /recording-layout/<file>`, from the directory in `JUNTO_RECORDER_LAYOUT_DIR`. Without the setting, both paths answer `not_found`.
- The setting names an absolute directory that contains `index.html`. Another value stops the start with a message that names the variable (ADR 0008).
- The build of the server does not include the page. A deployment builds `recording-layout/dist` and puts it beside the server; `JUNTO_RECORDER_LAYOUT_URL` (ADR 0604) is the address of `/recording-layout/` as the recorder reaches the server.
- The routes ask for no credential. The files are the same for every caller and are public in the repository, and the recorder cannot send one. The token in the query is a token of the media server; the page uses it in the browser of the recorder, and the server does not read it.
- The routes are not part of the public API and not in the OpenAPI document, like `/internal/` (ADR 0508). The entry host does not need to forward them.
- A file is served when its path consists of plain file names, none of which starts with a dot, when its extension is one of the page (`html`, `js`, `css`, `json`, `map`, `woff2`, `svg`, `png`, `txt`, `LICENSE`), when no part of its path below the directory is a link, when it is a regular file, and when it has at most 16 MiB. The server checks this on the opened directories and on the opened file. It does so on Unix systems; on another system it serves no file of the page, as it reads no file of a recording there (ADR 0606). Every other path answers `not_found`, with the response of an unknown path.
- A response states the media type by the extension, `X-Content-Type-Options: nosniff` and `Cache-Control: no-cache`.

Log:

- The log record of a request states the method and the path, and not the query (ADR 0008). The room token of the recorder is therefore not written to the log of the server.

Names:

- A join token carries the display name of its account in the claim `name`. The server reads the name in the transaction of the join.
- The name is the one at the time of the join. A person who changes the name has the new name in a call after joining again.

## Consequences

- A recording shows the display names of the participants.
- Every participant of a call sees the display names of the others through the media server. They see each other in the call in any case, and the profile of a fellow member is readable (ADR 0119).
- A server without the directory records no composite: the recorder gets no page. The setting of the address and the setting of the directory are given together in a deployment that records.
- A person who reaches the server address can load the page. The page shows nothing without a room token of the media server.
- An entry host that logs requests must leave the query of `/recording-layout/` out of its own log, when it forwards that path at all.
- A change of the page needs a new build and a copy of the directory, and no new server binary.
