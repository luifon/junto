# 0723. Recordings and transcripts in the client core

## Status

Accepted

## Context

ADR 0604 to ADR 0608 define the routes of a recording, of its transcript and of its files. A recording is posted in the chat of its voice channel as a system message that names it; its segments, its transcript and the lines of the transcript are read from their routes. The events `recording_updated`, `recording_removed`, `transcript_line_corrected` and `transcript_speaker_renamed` tell a change. The file of a segment has no hash, has up to several hundred megabytes, and is served with byte ranges. The text form of a transcript is made for each request, states no length and serves no range. ADR 0719 holds one state per server with one writer and parts that a reader opens, and ADR 0721 saves a file through an unfinished file that is locked. A server can be hostile, so what the client holds of it needs a bound (ADR 0714).

## Decision

Routes:

- The API object of a server has one function for each route that a member uses: the recordings of a workspace, one recording with its segments, its removal, the transcript, a new request for the transcript, the lines of the transcript, the transcript as text, the correction of a line, the name of a speaker, and the file of a segment from a byte on.

State (ADR 0719):

- A reader opens a recording by its identifier and releases it, as the pin list of a channel (ADR 0722). The state then holds the recording with its segments, the transcript without its lines, the lines that a reader asked for, and why the last read failed, or that the server has no such recording for the account.
- An answer for another recording than the one that was asked for is not taken (ADR 0714).
- The state holds at most 2,000 segments and 10,000 speaker labels of one recording, 200 terms of each glossary list, and 20,000 lines of one transcript. A page has 200 lines.
- Lines are read only when a reader asks: the first page, and the page after the last held line. The state says that the transcript has more lines when a page says that lines follow it, and when a read of the transcript states another count of lines than the read before and more lines than the state holds. The lines of a recording that is gone are not taken.
- `recording_updated` reads the recording and its transcript again: the event carries no segment and no transcript. `transcript_line_corrected` and `transcript_speaker_renamed` change a held line and a held label without a request; the text of the worker of a corrected line is the text that the line held before. `recording_removed`, and the deletion of the channel, mark the recording as gone. `channel_updated` reads the open recordings of the channel again: a channel that hides who is inside gives a recording of its running call only to the people inside (ADR 0604), and the event does not tell what the account may read.
- After a reload the held data stays with the mark that it is not current. The recording and its transcript are read again, and as many lines as were held.
- A read of a recording that gets no answer is sent again 5 times; the reader is told at the first failure. A page of lines that failed is read again when the reader asks. An answer `not_found` marks the recording as gone.
- At most 4 reads of recordings and of pages of lines are on their way at one time; the others wait in the order in which they were asked for.

Files:

- The file of a segment is saved through an unfinished file with the rules of ADR 0721: a regular file with one name, locked from its opening, continued after a cut connection and after a restart with a byte range, and given a name that no file has. Its length is compared with the bytes that the segment states. An answer whose stated range starts at another byte than the one that was asked for ends the download as damaged; this holds for the file of a message too.
- The text of a transcript is read from its start at every try, and at most 64 MiB of it.
- A download ends as failed after 5 answers in a row after which the file holds no more bytes than it held at most before, and the text of a transcript after 5 answers in a row that were cut: a server cannot make the client ask without an end (ADR 0714). This holds for the file of a message too.
- The unfinished file is deleted when no later try can continue it: its bytes are not the file that the server states, the download ended as above, or the server answers that it has no such file (404, 410) or no byte after the held ones (416). The file is locked for the deletion, so a file that another download holds stays.
- The caller gives the name of a saved file without its ending, because the routes of a recording know no name of a channel or of a person. The client core adds `.mp4`, `.ogg` or `.txt` and reduces the name as the name of an attachment.

Tool:

- `junto-cli recordings` has `list`, `show`, `remove`, `transcript`, `request-transcript`, `lines`, `text`, `correct`, `rename-speaker` and `download`.

## Consequences

- A change of a recording costs two requests for each open recording. A correction and a rename cost none.
- A continued download whose unfinished file was damaged is saved as it is, and so is one whose server sends other bytes than the file under a correct range: the file of a segment has no hash to compare. A hash in the segment would end both.
- The size of a download is the size that its segment states. The client has no bound of its own for it and does not read the free space of the disk.
- The state holds the start of a long transcript, not a range around a line. A reader that shows one line of a long transcript reads the pages up to it.
- The end-to-end test runs the server of this repository inside the test process, with its recorder in memory, because the server binary has no such recorder: a recording is made, its files are processed by the real tools, and a client reads the detail, saves each file and compares it with the file of the server, reads the lines, follows a correction and a rename of a second client, and sees the removal. The window tests use the same support file.
- `junto-server` is a development dependency of the client core and of the desktop client, for that test server.
