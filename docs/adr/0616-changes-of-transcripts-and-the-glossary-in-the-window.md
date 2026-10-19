# 0616. Changes of transcripts and the glossary in the window

## Status

Accepted

## Context

Anyone who can see a recording corrects a line of its transcript and names a voice of shared sound; each change shows who made it, and the original text stays viewable (ADR 0603). The server keeps every correction and every name with its author and its time, and the routes of the lines and of the transcript return them (ADR 0607). A person with "manage recordings" asks for a transcript again when a recording with files has none or a failed one. The transcript states the glossary terms that the speech model received and those that did not fit, and the result of each audio file (ADR 0611, ADR 0613, ADR 0614). Each workspace has a glossary that every member reads and a person with "manage glossary" replaces as a whole list (ADR 0607). The person who starts a recording can set the language of the speech (ADR 0603). The server accepts a stop while the recorder starts a recording (ADR 0604). A segment states the time range that the recorder reported for it, also when it failed (ADR 0601).

The window showed the transcript and followed the changes of other clients, and offered none of these actions (ADR 0744): a recording was always started with the language `auto`, and the header of a starting recording had no control (ADR 0738). The client core had no function for the glossary. A control under the pointer must not move when an answer or an event arrives (ADR 0730).

## Decision

The glossary in the client core:

- The API object reads the glossary of a workspace and replaces it, with the list that the change was made from.
- The glossary of one workspace is a collection of the administration (ADR 0760), opened by a reader and read from its route. `glossary_replaced` names the workspace and carries no term: the collection of that workspace is read again. An answer for another workspace is refused (ADR 0714).
- The state holds at most 200 terms, the number of the server. An answer with more terms is held in part and marked as cut, and a reader makes no change from it: the replacement of the whole list would remove the terms that the state does not hold.

A replacement made from an older list:

- `PUT /api/v1/workspaces/{workspace_id}/glossary` takes `replaces`, the terms that the caller read. With it, the server replaces the glossary only while it holds exactly those terms in that order, and answers `conflict` otherwise. Nothing is stored, recorded in the audit log or told then. The comparison runs in the serializable transaction of the replacement (ADR 0009).
- Without `replaces`, the route replaces the glossary as before. A server of an older version ignores the field.
- The window always sends it. A change of the window therefore never undoes a change of another person that the window had not received.

The glossary page:

- The administration has the page "Glossary" in the group of the shown workspace, after "Recordings", for every member. It lists the terms with their position, the most important first.
- A person whose roles hold "manage glossary" adds a term at the end of the list, changes the text of a term in a form in its row, removes a term after a question in its row, and moves a term up or down by one place. The server decides.
- Each change is made from the list that the state holds when the person sends it, and names the term that the row showed: a list that holds another term at that position changed meanwhile, and nothing is sent. A form or a question whose row holds another term closes.
- The rules of a term are checked before it is sent: 1 to 200 characters after the space around it is removed, no control character, no `<|` or `|>`, and no term twice. A change that changes nothing sends nothing.
- One change is on its way at a time. After the answer, the page takes no change while the state still holds the list that the change replaced, because a change made from that list would be refused. The page takes changes again when the state holds another list.
- A new term goes to the end, below every control of the page. The answer of a form keeps the height of its row (ADR 0730). A removed or moved term moves the rows under it when the state arrives, as a row that the server removes does on the other pages.

The transcript in the side panel:

- Each line has "Correct". It opens a form in the line with the text of the line; the answer of the server closes it, and the row keeps the height of the form (ADR 0730). A correction has 1 to 2,000 characters after the space around it is removed and no control character; a correction equal to the shown text sends nothing.
- The mark "corrected" of a line opens and closes, under its text, the text of the speech model and who made the newest correction, and when.
- The head of the list has "Details". It shows the glossary terms that the speech model received and those over the limit, the voices of shared sound, and the result of each audio file: the file by its person and kind, how much of it was transcribed and why not all, and whether the lines of shared sound have voices and why not all.
- Each voice has "Name", or "Rename" once it has a name, which opens a form in its row; the name has 1 to 64 characters. The row states who gave the name, and when. The lines of the voice show the name with the label of the voice.
- The correction and the name are sent as requests of the window. The change arrives with the event of the server, which makes the client core read the line or the transcript again (ADR 0723). A refusal is stated in the form, in a slot that keeps its height (ADR 0730).
- A recording of an archived channel takes no change (ADR 0410): the controls are drawn disabled with the reason.

The card of a recording:

- The title line of the transcript has "Transcribe" when the files are processed and the recording has no transcript, and "Transcribe again" when its transcript failed. It is enabled for a person whose roles hold "manage recordings" in a channel that is not archived, and disabled with the reason otherwise; the server decides. A refusal is stated in the slot of the card. The new state of the transcript arrives with the event of the recording.
- "Parts" opens a list over the chat: every segment of the recording, the composite and each track, with its state and the time range that the recorder reported for it, in seconds of the time of day. A part that failed, that has no file, or whose file failed its check is marked. The list floats, so the rows of the chat do not move.

The recording of a call:

- "Record" opens the languages of the speech: "Detect the language of each sentence" first, then each of the 100 languages that the speech model knows, by its English name. The choice starts the recording with `auto` or with the code of the language. The codes are those of the speech model; the worker refuses a code that its model does not know (ADR 0613).
- While the recorder starts a recording, the header has "Cancel" after the mark, for a person whose roles hold "record calls". It stops the recording as "Stop" does.
- The mark of the recording of the call of this device has one width while the recording starts and while it runs: "Cancel" and "Stop" stand at the same place, and a press aimed at "Cancel" when the recorder starts lands on "Stop", which ends the recording as "Cancel" would. "Cancel" never stands where "Record" stood: a press on "Record" when another person starts a recording at that moment lands on the mark.

## Consequences

- A change of the glossary costs one request and the read of the glossary after its event. Two people who change the glossary at the same time: the second change is refused and is made again from the list that the page then shows.
- The CLI of the client core has no glossary command.
- The window offers no removal of the name of a voice: the server keeps the names of a voice and shows the newest, and takes no empty name.
- The page waits for the event of its own change before it takes the next one. A lost connection delays it until the state is read again.
- After a correction or a name, the window shows the text of before until the event of the server has been followed and the line or the transcript was read again.
- A language list of 100 entries is a scrolled list in the menu. The person chooses a language at each recording; the choice is not stored.
- The window tests make the recordings with the server of this checkout inside the test process (ADR 0723), and give the window a connected call through a function of the media engine for tests, because that server has a media server in memory: `transcripts::` corrects a line and names a voice and checks that the row under a form keeps its place; reads the glossary terms and the result of each audio file; asks for a failed transcript again; reads the parts of a recording that was saved in part; starts a recording in a chosen language, cancels it while the recorder starts it, and checks that "Stop" stands where "Cancel" stood; and reads, adds, changes, moves and removes glossary terms, with a change from an older list refused while the event stream of the window is held.
