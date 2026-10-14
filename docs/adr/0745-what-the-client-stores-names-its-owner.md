# 0745. What the client stores of a server names its owner

## Status

Accepted

## Context

A client holds several servers at once (ADR 0715), and one entry of the server book can have one account after another: an account is removed and the person joins again, or the device is linked to another account. A server is not trusted (ADR 0714): it can state an identifier that it copied from another server, in a message that is valid in every other respect. Three stores of the client were named by such an identifier alone. The window held the decoded pictures of messages by the identifier of the file, so a message of a second server with a copied identifier showed the picture of the first server, and the next account of an entry saw a picture without a request to the server. The unfinished file of a download was named by the name and the identifier of the file (ADR 0721), so a second server could ask for the bytes after the ones that the first server had sent, and could cut or replace them. The unfinished file of an export was named in the same way, and a save removed what it found under that name.

## Decision

The owner:

- An owner is one entry of the server book, by its server identifier and its generation (ADR 0710), and the account that the access flow of that entry shows as logged in (ADR 0712). The client core gives the owner of a server, and gives none in every other state of the flow and for an entry that was removed. The owner stays while an ended session of the same account is renewed.
- What the client stores of a server belongs to one owner, and its key names that owner: a file, an entry of a cache, a remembered choice, and a prompt that waits. Nothing that was stored for one owner is shown, continued, counted or removed for another.
- An identifier of a server (a file, a message, an export) is never a key by itself. Two servers can state the same identifier. A list of the window that is keyed by the server identifier and an identifier of that server holds entries of one owner only: it is emptied for a server at the moment the server has another owner, or none.

Pictures of messages:

- The window holds a decoded picture under its owner, the identifier of the file and the hash that the message states. A message with a copied identifier, the same file under another account, and a file with another hash are read from their server.
- Each read has a number. Its answer is held only while the picture still waits for the read with that number: an answer for a picture that left the list, or that was asked for again, is discarded.
- When a server has another owner, or none, the window drops the pictures of the owner before. A picture is read for the account of its owner only; the read does not start when the server has another owner by then.
- At most 4 pictures of one owner are read at one time, so a server that answers slowly delays its own pictures only. A picture that finds no free read is asked for again when the window is drawn after a read has ended. The read of one picture ends after 10 minutes, with its tries; a picture whose read failed is not read again while the window holds its entry, as before this record.
- The window holds 48 pictures (ADR 0740). The reads start after a drawing of the window has ended, when every picture that the drawing uses is known. A new picture takes the place of the oldest one that is not being read and that this drawing did not ask for. A read that runs is never forgotten, so no second read of the same picture starts beside it.

Unfinished files:

- The name of the unfinished file of a download is `<name>.<mark>.part`. The mark is 32 hexadecimal digits of a SHA-256 hash over the owner, the kind of the file (a file of a message, a file of a recording segment, the text of a transcript, an export), its identifier, the length that the server states and the hash that the server states. Two unfinished files have one name only when all of these are equal.
- A download belongs to the owner of its start. Each of its requests is made for the account of that owner and fails when the device has another account. A download does not start while the device is not logged in.
- The unfinished file of an export save has the same form of name. A save removes only an unfinished file of its own owner.
- An upload belongs to the owner of its start in the same way: each of its requests is made for that account, and it does not start while the device is not logged in.
- The window looks at the owner of each server at every change of the state, and before it starts a download. For a server whose owner is another one than at the last look, or none, it ends the downloads of files of messages and of files of recordings that it follows, and forgets what it shows of them and the quoted messages that it read. The next owner sees neither their progress, nor the path of a saved file, nor a quoted text. A download is not started while the device is not logged in.
- The rules of ADR 0721 for the unfinished file stay: a regular file with one name, locked while a download holds it, and deleted only by the download that opened it.

## Consequences

- A download that was cut continues only for the same entry and the same account. After a server was removed and added again, and after the device got another account, the file is read from its first byte, and the unfinished file of the owner before stays in the directory until the person deletes it.
- An unfinished file that a build before this record left has a name of the earlier form and is not continued.
- A view that shows more than 48 pictures at one time shows the others as not yet arrived, until the window shows fewer. A message has at most 10 files unless the administrator of its server raised the limit (ADR 0401).
- The next account of an entry reads each picture again, also the pictures that the account before had read.
- The owner is the key for the stores of the window that later records name: the remembered channel of a workspace, the progress of uploads, the read positions and the acknowledgements of a recovery.
- A test holds two owners with one identifier of a file for the pictures, for the downloads and for an export save, and a test of the window against the real server shows that the next account of an entry reads a picture again.
