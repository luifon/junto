# 0721. Messages in the client core: the state, the reads that events ask for, sending, and files

## Status

Accepted

## Context

ADR 0408 defines the message routes. Its events carry no content: `message_changed` names a place, a message and a position, and the client fetches the current content. The server has no key that makes a send repeatable. ADR 0719 holds one state per server with one writer, the stream task of ADR 0718, and a rule for parts that are read from their routes. A window draws that state and must never wait for the network (ADR 0715). A server can be hostile, so what the client holds of it needs a bound (ADR 0714). An attachment has up to 1 GiB, and the HTTP client reads a whole response of at most 4 MiB in 40 seconds.

## Decision

Routes:

- The API object of a server has one function for each message route: the history of a channel and of a direct conversation with its four cursors, send, get, edit, delete, the two reaction routes, the read position, the direct conversations, the attachment limits, and the upload and download routes.
- A place is named by one type: a channel, or a direct conversation. The chat of a voice channel is the place of its channel. ADR 0722 adds the thread under a message of a channel as a third kind.
- An emoji in a path is written as escapes of its bytes, because an emoji can contain `#` and `*`.

State (ADR 0719):

- The state of a server gets: the unread summary of each place from the snapshot, the direct conversations (the direct places of the snapshot, and `conversation_opened`), the attachment limits, the timelines of the places that a reader has open, and the messages that wait to be sent.
- A timeline is one range of the history of a place without a hole, in the order of the server (position, then identifier), with the facts whether the history has more before and after it. Each message is shared between a copy of the state and the next one.
- A timeline holds at most 10,000 messages. A page that passes the bound drops messages at the other end and marks that end as having more. ADR 0747 adds a bound on the weight of the messages of a timeline, with the same rule, and bounds one message.
- A reader opens a place and releases it. A place with unread messages opens with the page before and the page after the read position; every other place opens with its latest page of 50. A place that the account never read has no read position: it opens with its latest page, and not at the start of a history that can be years long. The read position of that moment stays in the timeline, for the line of the first unread message.
- The attachment limits are read when a reader opens a place, and again after a reload while a place is open. `attachment_limits_changed` replaces them.

Reads that events ask for:

- `message_changed` asks for the unread summary of its place. When a timeline of the place holds the message, or the message lies inside its range, or after its end while the history has no more after it, the event also asks for the message. ADR 0722 adds further holders that ask for a message, and puts a reply of a thread into the timeline of its thread only.
- An answer `not_found` removes the message from the timeline. A deleted message that the server keeps as a marker arrives with its `deleted` mark and replaces the content.
- `message_read` moves the read position. When the position is the latest one, the counts are zero without a read; otherwise the summary is read. ADR 0722 reads the summary also at the latest position when it showed a mention. `message_mentioned` asks for the summary.
- These reads follow the rule of ADR 0719 for parts in one changed form: an event that arrives while such a read is on its way is kept; the answer is applied, and then the kept events start the read of what they name again. The answer can be older than those events, and no event carries the content that could be applied to it.
- The answer to an edit, a deletion or a reaction of a reader is not put into the state: the change arrives with its event and is read as this section states (ADR 0746).
- A read of a summary or of one message that is on its way is not started a second time. At most 4 reads of single messages are on their way; the others wait in a queue.
- A read of a summary or of a message that gets no answer is sent again after 2 seconds, with waits that double. No event tells the change a second time. A read of a message is sent 5 times; after that the timeline is marked as not current, and a reader offers to read it again. A read of a summary has no last try and is sent every 64 seconds after the fifth: no reader can ask for a summary, and a place that nobody has open has no timeline to mark.

Reload:

- After a snapshot replaced the state, the timelines that readers hold stay in the new state with the mark `stale`, and the waiting messages stay. Each timeline is read again as one page that replaces its messages: the latest page when its reader follows the end, else the page from the position that the reader named last.
- ADR 0719 discards every part at a reload. A timeline is kept for display only: its content is not trusted, the mark says so, and the page that replaces it is asked for at once. The messages that the unread threads and the notices of the new state name are kept in the same way, and each is read again (ADR 0742).

Read position:

- A reader names the position that the person has seen. The client sends one request per place at a time, the newest position, at most one in a second. A position at or below the read one is not sent.
- The answer to the request is the summary of the place at the time of the request. When an event changed the summary of the place while the request was on its way, the answer can be older than that event: it is not applied, and the summary is read.
- The message-change limit of the server (120 in a minute, ADR 0408) is shared by sends, reactions and read positions; the spacing keeps reading from using it up.

Sending:

- A message that the person sends is in the state from that moment as a waiting message: `waiting for files`, `sending`, `not confirmed`, or `failed` with the code of the refusal.
- One send per place is on its way, in the order in which the person sent, so the messages of a place get their positions in that order. A message that waits for its files is the exception: it does not hold the messages after it, and it gets its position when its files are complete. An upload can take an hour, and a text that the person sends meanwhile is not kept back for it.
- The answer to a send ends the wait. It puts the message into the timeline only while the state does not hold the events of the send by the receipt of the answer (ADR 0763); otherwise the events decided, also a later edit or deletion. An answer without a stated position puts the message in.
- A refusal, and a connection that was never made, leave the message as `failed`. The person sends it again or discards it.
- A lost answer leaves the message as `not confirmed`: the server may hold it. The client does not send it again by itself.
- A message `not confirmed` is confirmed when a loaded message of the own account has the same document, the same reply and the same files, and a position after the latest position of the place at the first attempt. One message of the server confirms one waiting message, and a message that was the answer to another send confirms none.
- "Send again" for a message `not confirmed` waits until 30 seconds after the request, the time after which the server ends a request (ADR 0008), reads the history after the position of the first attempt, up to 1,000 messages, and sends only when the history ended there and no message confirms it. When the history goes on after the 1,000 messages, the server may hold the message further on: nothing is sent, the message stays `not confirmed`, and the next "send again" reads on from the last message that was read.
- A reader draws a message that is `sending` only while no loaded message matches it: the event of the message can be followed before the answer arrives.
- A send that the server answers with an ended session was not taken. It is sent with the next session.
- An idempotency key in the send route would replace the comparison by content. The server of this version has none.

Files:

- An upload reads the file once for its length and its SHA-256 hash, starts the upload, and sends parts of 1 MiB at the offset that the server reports. After a request without an answer it waits (1 second, doubling to 30 seconds), reads the status and continues at the received length. `busy` is waited out. A refusal ends the upload with its code. Cancelling ends it and deletes what the server holds: the parts of an upload that runs, and the file of a complete upload that no message has. A complete upload without a message counts as one of the 20 uploads of the account (ADR 0408), so a file that the person takes out of a message is deleted and not left to its expiry.
- A message with files is a waiting message from the moment of the send and is sent when every upload is complete. The first upload that fails leaves the message as `failed`, also while another upload of the message still runs.
- A download writes to a file `<name>.<mark>.part` in the target directory, asks for the bytes after the ones that the file holds, compares the length and the SHA-256 hash with the attachment, and then gives the file the name of the attachment, or that name with a number. The name is taken by creating an empty file that the finished file replaces, so no file of another origin is replaced.
- The mark states the entry of the server book, the account and the file that the unfinished file belongs to, so a download continues only a file of its own owner (ADR 0745). A download is made for the account of its start.
- The `.part` file must be a regular file with one name. A symbolic link at its path, and a file that has a second name, is refused before anything is written, so a download changes no file of another origin. The file is locked from its opening until it has its final name: a second download of the same attachment into the same directory is refused, also from another process. A download deletes a `.part` file only when it opened that file itself.
- The name of an attachment comes from another account. It is reduced to one file name without a path, without a leading dot, without characters that a file system refuses, and without a reserved device name. A name that ends with `.part` gets a `_` at its end: no saved file has the name of an unfinished one, so a download never continues in a file that a person saved.
- A picture for display is read into memory, at most 8 MiB, and compared with its hash. It is read for one owner (ADR 0745).
- The transport gets two operations for files, with default implementations for tests: a request that carries one part of a file has 10 minutes; a download is read as a stream without a limit for the whole request and ends when no byte arrives for 40 seconds. The HTTP library uses a second client object with the settings of the first, as the event stream does (ADR 0718).

Tool:

- `junto-cli messages` has the actions `list`, `show`, `send` (text and files), `edit`, `delete`, `react`, `unreact`, `read`, `unread`, `conversations`, `open`, `limits`, `set-limits`, `upload` and `download`. `state` prints the conversations and the unread summaries, and `watch` prints the message events.

## Consequences

- A reader draws messages from the state and sends commands. It holds no rule about events.
- A change of a message costs the client one or two requests: the summary of the place, and the message when a reader has it loaded.
- A place that nobody has open costs one request for each of its message events, and holds no message.
- After a reload an open place shows messages that can be out of date for the time of one request.
- A message whose answer was lost can be sent a second time only by the person, and not before 30 seconds have passed. Two messages of one account with the same content from two devices in that time can confirm each other; the person then sees one waiting message less than was sent, and the history holds both.
- With the connection mode "connect only when selected", the state of a server that is not selected is discarded, and its waiting messages with it.
- An upload whose start was answered and lost leaves an upload without a file on the server until it expires (ADR 0408); it counts as one of the 20 of the account.
- A download that was cut continues from its part file, also after a restart of the client, for the same entry of the server book and the same account (ADR 0745).
- The stream client is tested against played routes for the reads, the bound, the read position, the reload and the lost answer, and against the real server for two clients, an offline client, pages, files and two devices of one account.
