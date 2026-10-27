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
- The window checks ownership before deriving the shown place and starting its readers. A changed owner ends the window's message, pin and member reader records and its message anchors, because the stream client has dropped those subscriptions. The shown place opens them for the new owner even when the window did not observe the intermediate logged-out state. The account-switch file-dialog test waits for the new account in the client core before letting the window run, then checks that messages, pins and members load again.

Pictures of messages:

- The window holds a decoded picture under its owner, the identifier of the file and the hash that the message states. A message with a copied identifier, the same file under another account, and a file with another hash are read from their server.
- Each read has a number. Its answer is held only while the picture still waits for the read with that number: an answer for a picture that left the list, or that was asked for again, is discarded.
- When a server has another owner, or none, the window drops the pictures of the owner before. A picture is read for the account of its owner only; the read does not start when the server has another owner by then.
- At most 4 pictures of one owner are read at one time, so a server that answers slowly delays its own pictures only. A picture that finds no free read is asked for again when the window is drawn after a read has ended. The read of one picture ends after 10 minutes, with its tries.
- A read that got no answer (the server was not reached, ended its answer early, failed with a status of 500 or more, or the 10 minutes passed) is read again by the first drawing after a wait: 30 seconds, and twice as long after each further read without an answer, up to 16 minutes. The window is drawn when the wait ends. A read whose answer decides (a refusal of the server, bytes that are not the file, bytes that are not a picture) is not read again while the window holds the entry. Each read again keeps the place of the picture in the list.
- The window holds 48 pictures (ADR 0740). The reads start after a drawing of the window has ended, when every picture that the drawing uses is known. A new picture takes the place of the oldest one that is not being read and that this drawing did not ask for. A read that runs is never forgotten, so no second read of the same picture starts beside it.

Unfinished files:

- The name of the unfinished file of a download is `junto-<kind>.<mark>.part`, where `<kind>` is `file`, `recording`, `transcript` or `export`. It holds no name that a server or a person chose: the name of the saved file of a recording comes from the name of its channel, and a rename of the channel gave the next download another unfinished file, so the download started again and the first unfinished file stayed in the directory. Test: `a_cut_download_continues_after_a_rename_of_its_channel` in `crates/client-core`. The mark is 32 hexadecimal digits of a SHA-256 hash over the owner, the kind of the file (a file of a message, a file of a recording segment, the text of a transcript, an export), its identifier, the length that the server states and the hash that the server states. Two unfinished files have one name only when all of these are equal.
- A download belongs to the owner of its start. Each of its requests is made for the account of that owner and fails when the device has another account. A download does not start while the device is not logged in.
- The unfinished file of an export save has the same form of name. A save removes only an unfinished file of its own owner.
- An upload belongs to the owner of its start in the same way: each of its requests is made for that account, and it does not start while the device is not logged in.
- The window looks at the owner of each server at every change of the state, and before it starts a download. For a server whose owner is another one than at the last look, or none, it ends the downloads of files of messages and of files of recordings that it follows, and forgets what it shows of them and the quoted messages that it read. The next owner sees neither their progress, nor the path of a saved file, nor a quoted text. A download is not started while the device is not logged in.
- The rules of ADR 0721 for the unfinished file stay: a regular file with one name, locked while a download holds it, and deleted only by the download that opened it.

## Consequences

- A download that was cut continues only for the same entry and the same account. After a server was removed and added again, and after the device got another account, the file is read from its first byte, and the unfinished file of the owner before stays in the directory until the person deletes it.
- An unfinished file that a build before this record left has a name of an earlier form and is not continued.
- A view that shows more than 48 pictures at one time shows the others as not yet arrived, until the window shows fewer. A message has at most 10 files unless the administrator of its server raised the limit (ADR 0401).
- The next account of an entry reads each picture again, also the pictures that the account before had read.
- The other stores of a server follow the same rule:
  - The window remembers the channel of a workspace under the server and the workspace; the same identifier of a workspace on two servers names two workspaces. The remembered workspace, channel and direct conversation of a server are dropped when the server has another owner, or none.
  - The progress of the uploads of waiting messages is held under the server and the number of the upload: each server numbers its uploads from 1.
  - The window keeps no record of the read positions that it sent. The event stream client of a server sends a position once for the account that it has, and leaves out a position that the account has read already (ADR 0721); its record ends with the account.
  - An acknowledgement of a recovery that is on its way, and its failure, are held under the owner in the window. The client core remembers an acknowledgement under the account, the device and the recovery; a recovery that the server states to another account or another device of the entry is not hidden, and an answer to an acknowledgement changes the notices only while the device has the account and the device that sent it. The command-line tool prints a recovery once for each owner.
- After a logout and a login of the same account, the server shows its first workspace and the first channel of it.
- A test holds two owners with one identifier of a file for the pictures, for the downloads and for an export save, and a test of the window against the real server shows that the next account of an entry reads a picture again.
