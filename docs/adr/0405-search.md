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

The search also accepts independent `has_file`, `has_link`, `has_picture`,
`mentions_me` and `direct` boolean filters. They combine with each other and
with channel, author and time bounds before ranking and pagination. Files are
completed attachments still retained on a live message; pictures have an image
media type. Links include structured link nodes and URLs in ordinary text nodes.
Mentions use the current account and the existing account, role and everyone
mention rules, independently of notification preferences. Direct messages are
limited to conversations the account can read. These message filters exclude
transcript lines. Every existing visibility and private-reader check still
applies before a result is returned.

An empty query with `has_file` and one explicit channel or conversation scope
lists that place's files, including attachments of thread replies and messages
without text. The scope is checked as a history read before the query runs.
`conversation_id` narrows results to one authorized direct conversation; it cannot
be combined with `channel_id`. The response includes retained attachment metadata
for file and picture queries, fetched in one batch for authorized message IDs.
Other searches keep their existing statement count. Files of deleted messages,
removed attachments and private-reader messages outside the caller's audience
are excluded before pagination. Empty queries without a file scope are refused.
