# 0405. Search

## Status

Accepted

## Context

Members need to find earlier messages and the content of recorded calls. All structured data is in PostgreSQL (ADR 0001), and the server can read message and transcript text (ADR 0002). A search that returns content from a channel the user cannot read would bypass the permission system.

## Decision

- The server provides full-text search over messages and transcripts in the channels that a user can read.
- A search has filters for channel, author and date.
- Results follow permissions. The permission function of ADR 0301 decides which results a user receives.
- The search uses the full-text search of PostgreSQL.
- Permission checks and rank ordering select a materialized page before message documents are converted to text. Only that page, including its continuation row, reads message and transcript text by identifier. A common word can match the whole history without rendering every matching document. Rank, identifier and transcript line keep the cursor order.

## Consequences

- No separate search service is deployed or kept in step with the database.
- A user who loses access to a channel no longer receives results from it.
- A recorded call can be found by words spoken in it, after its transcript is ready (ADR 0603).
- The first version has basic search only. Search inside attached files is in the later list of `docs/scope.md`.
- A search result for a transcript line does not open the video at that moment; the built-in recording player that does so is in the later list.
