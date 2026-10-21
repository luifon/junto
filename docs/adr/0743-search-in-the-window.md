# 0743. Search in the window

## Status

Accepted

## Context

ADR 0405 and ADR 0409 define the search: full-text search over messages and transcript lines with the web-search syntax of PostgreSQL and the `simple` configuration, filters for a channel, an author and a time span, pages with an opaque continuation, and results that are decided by what the account can read at the time of each request. A result holds plain text, a place identifier, an author and a time; a message result names its message, and a transcript result names its recording and its line. ADR 0722 keeps no search in the state of a server. The approved design draws the search field in the header of a channel and the results in the side panel (view H). The search is counted against the limit of 120 history reads in a minute.

## Decision

Query:

- The header of a channel and of a direct conversation has the search field. Enter sends the text; the text is not sent while the person types. The search covers every place of the shown server that the account can read.
- A text of more than 1,024 bytes is refused before a request, and an empty text ends the search.
- The panel has three filters: a channel of the shown workspace, a member of the shown workspace as the author, and a time span of "any time", "today", "last 7 days" or "last 30 days". "Today" starts when the day began in the time zone of the person. A changed filter sends the search again.

State:

- The window holds one search: its server, its text, its filters, the results of the pages that were read, and whether the server has a further page. An answer is used only while its request is the last one of the window.
- The results of the same text stay on screen while its next answer is on its way. Another text starts with an empty panel.
- "Show more results" reads the page after the read ones. The window holds at most 1,000 results of one search, and of the text of one result its first 600 characters.
- After the state of its server was replaced (ADR 0722), the first page of a search that the side panel shows is read again; a further page is read when the person asks. The side panel, the filters and a jump that is on its way stay as they are. A search that the side panel does not show sends no request then: its results are dropped.
- A result whose place the state of the server does not hold is not listed. A person who loses a channel loses its results on screen without a request. A filter for a channel that left the state is taken away, and the search is read again.
- A result names the workspace of its channel (ADR 0413). While the search is open, the window holds the archived list of each workspace whose results name a channel that is in no channel list (ADR 0762), and a result of an archived channel is listed with its place and leads to it. A page of results is listed when every result of it is placed. A result whose archived list has not answered holds back its page and the later ones: a first page that waits is shown as searching, and a later one as a page on its way under "Show more results". The page then appears whole, as a page appears when it arrives, so no row of a listed page moves while the messages stand above the transcripts (ADR 0730). A result of a workspace that the state does not hold waits for nothing and is not listed. When the read of an archived list fails, its results are not listed, and the area of the notices above the results states it with "Try again"; a new search reads every failed archived list of the server again.
- The search ends when another server is shown, and with its close control, which empties the field.

Panel:

- The side panel shows one list: a search closes a thread, the pinned messages, a transcript and the notices. A thread and a transcript that a result opens lie over the results, and closing them shows the results again. The notices take a search out of the side panel; its text stays in the field, and Enter sends it again.
- The results stand in the order of the server under "Messages" and under "Transcripts", each with its count. A row shows the place, the day, the author, the time and the text. The words of the search are marked in the text: the words and the words of the phrases of the query, without a word or a phrase after a minus and without `or`, compared as whole words without their case, as the server compares them.
- The panel states a search that runs, one that found nothing, a refusal with its reason, and a request without an answer with "Search again".

Jump:

- A message result states its place, its position and the root of its thread (ADR 0413). A press on it shows the message as a notice does (ADR 0742), without a read of the message: its place is selected, the thread of a reply is opened, the messages around the position are read when the list does not hold the message, and the list goes to the message and marks it. When the list holds the messages around the position and not the message, the message is gone: it leaves the results, and the panel says so.
- A result of a server that states no position reads the message first, as before ADR 0413. A person who goes to another place before the message was read stays there.
- A transcript result shows the voice channel of its recording, the time of its line from the start of the recording, and opens the transcript in the side panel at its line (ADR 0744). A server that states no time gives the number of the line.

## Consequences

- Each search and each further page is one counted request. Typing sends nothing.
- The count beside the title is the count of the listed results, with a plus sign while the server has more, also at the bound of 1,000.
- The filters offer the channels and the members of the shown workspace. A result of another workspace of the server is listed, and cannot be chosen as a filter from there.
- A transcript result shows the number of its line and not its time in the recording, because the result does not hold that time.
- The results are what the account could read at the time of their request. A result of a place that the state still holds stays listed when the server stops giving its message or its recording to the account; opening it is refused then.
- The marks follow the words of the query and not the match of the server. They can differ for a query with nested operators, and for a script whose words hold combining marks.
- The window tests against the real server cover a query that finds messages and a transcript line, the filters, the jump to a message, a search without a result, a refused text, a further page, and the loss of the results of a channel with the access to it.
