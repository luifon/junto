# 0406. Archive, export and retention

## Status

Accepted

## Context

Channels that are no longer used fill the channel list, and their files, recordings and transcripts use disk space. Members need a copy of a channel's content outside the server. An export is a second copy of content, so it can keep a message that was deleted and can be downloaded by a person who lost the permission after it was built. The HTML form of an export contains text that users wrote, which a browser would interpret if it were not escaped.

## Decision

Archive:

- Archiving is manual.
- An archived channel is read-only and leaves the channel list.
- An archived channel is deleted, with its files, recordings and transcripts, a set time after archiving. The time is 30 days by default and is adjustable per workspace, including "never".
- People with access to the channel get a notice 7 days before the deletion.
- Automatic deletion skips a channel that contains permanent messages (ADR 0402).

Export:

- An export is one ZIP file with the messages as JSON and as readable HTML, the attachments, the recordings and the transcripts.
- All content in the HTML is escaped.
- The content of an export is a copy made when it is built.
- The server builds the export in the background and provides a download link.
- The download link checks the permission again and expires after 7 days.
- When a message or a file inside a finished export is deleted, the server deletes that export.
- An export needs the "export channel" permission, which administrators have by default.
- An export reserves disk space when it starts, and the number of exports that run at once is limited (ADR 0006).

Retention:

- Retention of files and recordings is a setting per workspace. By default nothing is deleted automatically.
- File retention skips the attachments of permanent messages (ADR 0402).

## Consequences

- No channel is archived without an action by a person.
- With the default setting, the content of an archived channel is deleted 30 days after archiving and cannot be restored from the server afterwards, except from a backup (ADR 0005).
- A person who loses the "export channel" permission after requesting an export cannot download it.
- A deletion in a channel removes every finished export that contains the deleted item. The export must be built again.
- An export that is not downloaded within 7 days must be built again.
- Exports are written to the audit log (ADR 0303).
- Direct messages have their own export for each participant and their own server-level retention setting (ADR 0302).
- Import of history from other chat services is not planned (`docs/scope.md`).
- ADR 0410 states the routes and the rules of archived channels, with the smallest period of 7 days. ADR 0411 states the routes, the content and the build of exports.
