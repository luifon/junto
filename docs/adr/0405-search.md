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

Text is optional when at least one content, mention, place, author or time filter
is supplied. A request with neither text nor criteria is refused. Filter-only
searches use the same authorization and cursor rules; a picture needs no caption.
`conversation_id` cannot be combined with `channel_id`. File and picture results
include retained attachment metadata, fetched in one batch for authorized messages.

`files=true` explicitly selects a file list with one channel or conversation
scope, empty text and no other search criteria. The scope is authorized through
the common place resolver as a history read, without enumerating other places.
An index on live attachments orders each place by its message creation time and
message identifier. This time is recorded on attachment association, including
permanent messages. Duplicate attachments of one message occupy one result.
The cursor records those chronological keys, rather than a computed text rank;
a page applies private-reader and deletion checks before limiting messages and
renders text only for the selected page. Removed attachments are excluded.

The `search` object inside the general `GET /api/server` `capabilities` object
reports support for content/mention/direct filters, filter-only requests and
explicit file lists (ADR 0201). An absent object or flag means unsupported;
unknown fields are ignored. There is no separate search-capability route.
Clients check these flags before sending extension parameters;
they never simulate server filters by filtering a returned page. Older servers
continue to accept ordinary text, channel, author and date searches.

Filter-only results use the same bounded, spoiler-concealing message preview as text searches. Matching a link inside a spoiler does not reveal its label or address in the result. The API search regression `a_filter_only_link_search_conceals_spoiler_labels_and_urls` covers this combination.

Explicit Files lists use that same spoiler-concealing message preview and its 512-character bound for channel, pair and group scopes. The selected page reads the source documents after attachment ordering and authorization; concealed text and links remain hidden without changing attachment metadata, chronological pagination or the sparse-history index. API regressions cover all three scopes, nested spoilers and Unicode truncation.
