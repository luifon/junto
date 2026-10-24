# 0411. Exports: request, content, file, build, download and removal

## Status

Accepted

## Context

ADR 0406 decides that an export is one ZIP file with the messages as JSON and as HTML, the attachments, the recordings and the transcripts; that it is built in the background, checked again at the download, valid for 7 days, removed when an item of it is deleted, and guarded by the storage guard. ADR 0302 gives each participant an export of their direct messages. What a person reads in a channel depends on the person: the private lines of a call in a channel that hides who is inside (ADR 0408) and the recording of such a call while it runs (ADR 0604). A file of an export can be larger than the memory of the server and larger than 4 GiB. The transactions of the server are short (ADR 0009).

## Decision

Request and ownership:

- `POST /api/v1/channels/{channel_id}/exports` needs sight of the channel and `export_channel` in it. `POST /api/v1/conversations/{conversation_id}/exports` needs that the caller is a participant who may reach the other account (ADR 0307).
- An export belongs to the account that asked for it. `GET /api/v1/exports`, `GET` and `DELETE /api/v1/exports/{export_id}` and `GET /api/v1/exports/{export_id}/file` serve that account only; every other account gets `not_found`, an administrator included.
- While an export of the same place waits or is written for the caller, a request returns that export with the status 200.
- An account has at most 3 exports that wait or are written and at most 10 stored exports; a further request gets `limit_reached`.
- A request gets `storage_low` while the free space of the exports is at or below the reserve (ADR 0605), and `unavailable` on a server without `JUNTO_EXPORTS_DIR`.
- The request of a channel export records the audit entry `export_requested` about the channel. The request of a direct-message export records none: the entry would tell the readers of the audit log that the conversation exists (ADR 0302).

Content:

- An export holds the items of its place whose identifier is not greater than the identifier of the export. Both are UUIDs of version 7 that the server generates, so these are the items that existed when the export was asked for.
- Messages: every message of the place that is not deleted and that the account reads, thread replies included, in the order of their positions.
- Attachments: the files of those messages. Recordings (channels only): each processed recording that the account may read by the rule of ADR 0604, with its files, its description and its transcript as text.
- The file is a ZIP archive: `export.json` (what the file is), `messages.json` (an array of the `Message` objects of the API), `messages.html`, `attachments/<attachment>/<name>`, `recordings/<recording>/recording.json`, `transcript.txt` and one file per segment.
- In `messages.html` every text of a person and every name is escaped. The page has a content policy that allows no script and no address outside the file. A link is written only for `http`, `https` and `mailto`.
- The name of an attachment becomes a name of a file without the characters of a path; `.` and `..` become `file`. The writer refuses an entry name that is not a relative path.

Build:

- One task writes one export at a time for all server processes. A process holds an export with a lease that it renews every 15 seconds; a lease older than 90 seconds lets another process take the export. An export that was taken three times without an end fails with `interrupted`.
- The place is read in pages: 200 messages, 50 attachments or 1 recording per transaction. Every such transaction first decides again that the account may export the place, and reads only what the account reads then. When it may not, the export fails with `access_ended` and its partial file is deleted. The transaction that states the export as ready decides the same again; when the account may not export the place then, the finished file is deleted and the export fails with `access_ended`.
- The build runs twice over the place with the same code. The first run writes nothing and counts the bytes of the file. Then one transaction reserves that number of bytes (ADR 0605); without the space the export fails with `storage_low`. The second run writes the file. The reservation ends with the build.
- The content can grow between the two runs: a message is edited, or a recording finishes its processing. Before the second run writes a page, it adds the bytes of the page to the bytes of the pages before it. When the sum is above the reservation, one transaction raises the reservation to the sum; the free space must then hold the reserve of the server, every other reservation and every byte of the sum that the files do not hold yet. That includes what earlier pages stated and what is written later: the readable page and the directory are copied into the archive at their end. Without the space the export fails with `storage_low`. A file of a recording that is larger than its segment states fails the export. So the files of a build never hold a byte that is not reserved.
- Nothing is held in memory beyond one page, one part of a file of 1 MiB and the open entry. The page of a recording holds the text of its transcript. The readable page is collected in a second file, and the directory of the archive in a third, until the end.
- The archive is written by the server's own writer: entries stored without compression, each described in the ZIP64 form, so that a file and the archive can pass 4 GiB. No dependency was added. Attachments and recordings are compressed formats already.
- The files of an attempt have the number of the attempt in their names, so two processes never write the same file.

Download, lifetime and removal:

- The download needs the state `ready`, an expiry in the future, and the permission of the request, decided at each download. It serves one byte range like the download of a recording (ADR 0608).
- A recording of a call that still runs in a channel that hides who is inside is read by the people inside only (ADR 0604). When the run that writes the file puts such a recording into it, the export stores the call; the run that counts stores nothing, and each attempt starts without a stored call. While that call runs and hides who is inside (ADR 0517) and the account is not inside, the download gets `not_found`. The export stays, and the download works again when the call has ended.
- A download that runs is decided again every 5 seconds, with the session or the token of its request. The 5 seconds are measured on the clock of the server and on a clock that a change of the time of the machine does not move; the earlier one decides. When the export is removed, has expired, or its account may no longer download it, the response ends without the rest of the file.
- An export that has finished, ready or failed, is removed 7 days later by maintenance.
- A deletion of a message, of the files of a message (also by file retention, ADR 0409) or of a recording removes, in its transaction, every export of the same place whose identifier is not smaller than the identifier of the deleted item: every export that holds the item or, while it is written, can hold it.
- A ban and a removal of an account remove its exports. The deletion of a channel removes the exports of the channel (ADR 0017).
- A removed export is shown by no route from the transaction of its removal on. The task of the exports then deletes its directory and its row.

Events:

- `export_updated` with the export, at the request and at each change of its state, and `export_removed` with the reason. Both go to the account of the export only.

## Consequences

- Two people who export the same channel can get different files.
- A person who loses `export_channel`, or the sight of the channel, cannot download an export made before, and can again when the permission returns within the 7 days. A download that runs at the loss sends at most 5 further seconds of the file.
- A message that is edited after its page was read is in the export with its earlier text. A message that is deleted removes the export.
- A deletion in a busy channel removes the exports that were asked for after the deleted message was written, also the ones that a later page would not have held.
- An export needs its size twice at most during the build: the archive, and beside it the readable page and the directory.
- A program that reads the file must read ZIP64. Current versions of the common tools do.
- The export of a large channel reads its messages twice.
- An administrator does not see which exports other accounts hold, except through the audit entries of channel exports.
- Tests: `exports` in the API tests, among them `rule_e16_the_export_of_a_direct_conversation_holds_the_lines_of_its_calls`: the export of a direct conversation holds the lines of its calls as its history shows them, and its page states each line with the account that started the call and the duration.
