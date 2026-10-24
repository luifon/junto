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
- Each change is made from the list that the page shows when the person sends it, and names the term that the row showed: a list that holds another term at that position changed meanwhile, and nothing is sent. A form or a question whose row holds another term after a change of another person closes, and its row keeps the height of the form until the person acts (ADR 0730).
- A form that the page cannot send, because the permission was taken while it was open, the list is read again, or the list is held in part, disables its button and states the reason in its slot. The send checks the permission of the roles again, so an Enter in the field sends nothing either.
- The sentence that the roles do not hold "manage glossary" stands below the list, so a permission given or taken while the page is open moves no row (ADR 0730).
- The rules of a term are checked before it is sent: 1 to 200 characters after the space around it is removed, no control character, no `<|` or `|>`, and no term twice. A change that changes nothing sends nothing.
- One change is on its way at a time. The answer carries the glossary after the change, and the window asks the client core for a read of the glossary at once (ADR 0760): the event of the change, a change of another person and their reads can all come before a slow answer, and no event would follow to read the list again. The page shows the answered list, and makes the next change from it, until the state shows a read of the glossary that started after the answer arrived: the client core states when the read that a collection shows started, and such a read holds the change or a later one. A refused change puts the answered list away, and the page shows the state. The page never waits for a list of a given content: another person can put the list back to the one that the change replaced.
- A new term goes to the end, below every control of the page. The answer of a form keeps the height of its row (ADR 0730). A removed or moved term moves the rows under it when the state arrives, as a row that the server removes does on the other pages.

The transcript in the side panel:

- Each line has "Correct". It opens a form in the line with the text of the line; the answer of the server closes it, and the row keeps the height of the form (ADR 0730). A correction has 1 to 2,000 characters after the space around it is removed and no control character; a correction equal to the text that the line shows now sends nothing.
- The form keeps the value that it was made from and sends it as `replaces` (ADR 0607). When the line or the name changes on the server while the form is open, by another person or by the same account on another device, the form states it without naming who: a text that the person had not changed is replaced by the new value, and a changed text stays while the form names the new value; the form is made from the new value from then on. A change that the server refuses because the value changed meanwhile is stated in the form. When the new value arrives after that refusal, the form names it in place of the refusal, so a second Save replaces a value that the person has seen. A send puts away the change that the form named.
- The mark "corrected" of a line opens and closes, under its text, the text of the speech model and who made the newest correction, and when.
- The head of the list has "Details". It shows the glossary terms that the speech model received and those over the limit, the voices of shared sound, and the result of each audio file: the file by its person and kind, how much of it was transcribed and why not all, and whether the lines of shared sound have voices and why not all.
- Each voice has "Name", or "Rename" once it has a name, which opens a form in its row; the name has 1 to 64 characters. The row states who gave the name, and when. The lines of the voice show the name with the label of the voice.
- The correction and the name are sent as requests of the window. The change arrives with the event of the server, which makes the client core read the line or the transcript again (ADR 0723). A refusal is stated in the form, in a slot that keeps its height (ADR 0730). The server answers a change in an archived channel with `channel_archived` and a stale value with `conflict` (ADR 0410), and the form states the two apart; for a server that answers `conflict` in both cases, a channel that the state shows as archived decides.
- A draft, a shown original text and open details belong to the owner of the server that made them (ADR 0745): for another account of the server, or none, the panel shows none of them, and the requests of the owner before are told to nobody. The answer to a request for a transcript or a removal of a recording that the owner before sent changes nothing in the window, and a refusal that the window showed for a recording is put away when the owner changes.
- A recording of an archived channel takes no change (ADR 0410): the controls are drawn disabled with the reason.

The card of a recording:

- The title line of the transcript has "Transcribe" when the files are processed and the recording has no transcript, and "Transcribe again" when its transcript failed. It is enabled for a person whose roles hold "manage recordings" in a channel that is not archived, and disabled with the reason otherwise; the server decides. A refusal is stated in the slot of the card. The new state of the transcript arrives with the event of the recording.
- "Parts" opens a list over the chat: every segment of the recording, the composite and each track, with its state and the time range that the recorder reported for it, in seconds of the time of day. A part that failed, that has no file, or whose file failed its check is marked. The list floats, so the rows of the chat do not move.

The recording of a call:

- "Record" opens the languages of the speech: "Detect the language of each sentence" first, then each of the 100 languages that the speech model knows, by its English name. The choice starts the recording with `auto` or with the code of the language. The codes are those of the speech model; the worker refuses a code that its model does not know (ADR 0613).
- While the recorder starts a recording, the header has "Cancel" after the mark, for a person whose roles hold "record calls". It stops the recording as "Stop" does.
- The mark of the recording of the call of this device has one width while the recording starts and while it runs: "Cancel" and "Stop" stand at the same place, and a press aimed at "Cancel" when the recorder starts lands on "Stop", which ends the recording as "Cancel" would. "Cancel" never stands where "Record" stood: a press on "Record" when another person starts a recording at that moment lands on the mark.

## Consequences

- A change of the glossary costs one request, the read of the glossary after its answer, and the read after its event; an event that arrives while the read after the answer is on its way ends that read and starts another. Two people who change the glossary at the same time: the second change is refused and is made again from the list that the page then shows.
- The CLI of the client core has no glossary command.
- The window offers no removal of the name of a voice: the server keeps the names of a voice and shows the newest, and takes no empty name.
- The page takes the next change at once after an answer. A page that shows its answered list while another person changed the glossary meanwhile gets a refusal at the next change, and then shows the state.
- Two people who correct the same line or name the same voice at the same time: the second change is refused, and its form shows the first one; a second Save replaces it knowingly.
- After a correction or a name, the window shows the text of before until the event of the server has been followed and the line or the transcript was read again.
- A language list of 100 entries is a scrolled list in the menu. The person chooses a language at each recording; the choice is not stored.
- The window tests make the recordings with the server of this checkout inside the test process (ADR 0723), and give the window a connected call through a function of the media engine for tests, because that server has a media server in memory: `transcripts::` corrects a line and names a voice and checks that the row under a form keeps its place; reads the glossary terms and the result of each audio file; asks for a failed transcript again; reads the parts of a recording that was saved in part; starts a recording in a chosen language, cancels it while the recorder starts it, and checks that "Stop" stands where "Cancel" stood; shows in an open form a correction and a name of another person; names the new text in place of a refusal of a stale correction, with the event stream of the window held at a proxy; shows no draft of an account that was removed to the next account; and tells the answer to a request of a removed account to nobody, with the request held at the proxy. `glossary::` reads, adds, changes, moves and removes glossary terms against the server binary, with a change from an older list refused while the event stream of the window is held; keeps the height of a row whose form a change of another person closed; sends nothing from a form whose permission was taken; takes changes again after another person undid the change of the person; moves no row when the permission is given or taken; and shows a change of another person that came before a slow answer of the change of the person. The Windows job leaves out `transcripts::`, as it leaves out `recordings::`: the recording server of the tests stores the files of a recording, which the server does on Unix systems only (ADR 0912).
- The Windows job also leaves out the administration tests that save a direct-conversation export or a channel export with files and recordings. Their server fixture needs the same Unix file operations; the channel export also uses the recording worker fixture. The archived-channel warning and recordings-list tests remain in the Windows job because they need no stored media files. Testing these file-dependent flows from a Windows window requires a server on a supported system.
