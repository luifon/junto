# 0006. Storage guard

## Status

Accepted

## Context

Uploads, recordings and exports write large files to the disk of the server. The final size of a recording or an export is not known when it starts. A recording that stops because the disk is full leaves incomplete files. A large upload that fails must not start again from its first byte.

## Decision

- Each recording and each export reserves disk space when it starts.
- The number of recordings and exports that run at the same time is limited.
- The server keeps a reserved amount of free disk space. Below that amount, the server refuses new uploads, new recordings and new exports, and warns the administrators.
- A recording that reaches the reserve is stopped in order, so that its files are complete.
- Uploads are sent in chunks and can resume.

## Consequences

- A recording or an export does not start when the server cannot reserve space for it.
- A user can be refused an upload, a recording or an export while free disk space is below the reserve.
- A long recording can be stopped by the server before the call ends. Its files up to that point are complete.
- The reservation of a recording covers its video twice. The video is recorded as segments, which were 6.0 to 6.5 % larger than the MP4 in the measured recordings, and the segments and the MP4 that is built from them exist together until the segments are deleted (ADR 0602).
- What the recorder does when the disk is full was not tested. The server stops a recording before that point.
- A chunked upload stays under a size limit per request that a proxy in front of the server applies.
- The reserved amount and the limit on concurrent jobs are not fixed by this record. ADR 0605 states them, and how a reservation is stored.
