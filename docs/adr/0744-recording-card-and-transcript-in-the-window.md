# 0744. The recording card and the transcript in the window

## Status

Accepted

## Context

ADR 0602 and ADR 0608 decide that the client has no player: it saves the video and gives the copy to the player of the system, and "Download" offers the video and, separately, the audio files. ADR 0723 gives the window a recording that a reader opens, with its segments, its transcript, the lines that were read, and the downloads of its files. The server posts a finished recording as the system message `recording_ready`, which names the recording; removing a recording needs "manage recordings" (ADR 0608).

## Decision

Card:

- In the chat of a voice channel the system message `recording_ready` is drawn as the card of its recording. Every other line of a recording stays a system line. The chat of the call of this device draws the line and no card: its side panel is the panel of the call (ADR 0741).
- The window holds open in the client core at most 24 recordings: the one whose transcript the side panel shows, and the ones that the loaded messages of the shown chat name: from the first row of its list on, at most three quarters of the count, then the ones before that row, the nearest first, for a list that moves towards them. A list at its end has the newest ones. A move of the list changes them.
- A recording whose channel is not the shown one is drawn as one that the server does not give (ADR 0714).
- The card states the channel and the day, the times of the start and of the end, the length, the person who started it, the size of the picture, the number of microphone files and of files of shared sound, and the sum of the sizes of its files. In place of a picture of the video it draws a dark field with the play mark and the length: the client decodes no video.
- The card states every state of the job that is not "ready": the recorder starts, the call is being recorded, the server checks the files, a part was not saved with the count of the failed files, and nothing was saved with its reason. A file that failed the check of the server is named with what the check found.
- The title line states the state of the transcript: none, waiting, being written, ready, covering a part, or failed with its reason.
- A recording that is being read, that could not be read, and that the server does not give to the account each have their statement in place of the content, the last two with "Try again": the server answers "not found" also for a recording that the account can read later (ADR 0604).

Files:

- "Open" saves the video to the download directory and gives the saved file to the application of the system for it, when the download ends, also when the window shows another place then. A video that was saved in this run of the application is opened without a second download while its path names a file of the saved length; a file that the person deleted or replaced is saved again.
- "Download" opens a list of the files: the video and each audio file with its person and its size, and the transcript as text. A choice saves that file. The window is told a change of the stage of a download and each further hundredth of the file. The card lists each file that is saved with its progress, "Saved" with the control that shows it in its folder, or the reason of a failure.
- A saved file is named by the channel, the day and the time of the start, what the file is, and its person.
- The opening of a file and the showing of a saved file in its folder are behind a setting of the application state, as the notifications of the system are (ADR 0742): only the binary opens a file with an application of the system or shows it in the file manager of the system. A test keeps the path. Reason: the test platform of gpui implements neither call and panics at both, so a test could not press "Open" or "Show in folder".

Transcript:

- The card shows the first 6 lines: the time from the start of the recording, the speaker, the source of the sound, and the text. A line of a microphone is named by its person. A line of shared sound is named by the name that a person gave its voice, with the label that the voice had, or by the label `Speaker N`, or by the person who shared when the line has no voice; under it stands whose screen the sound came from.
- "Show all" opens the transcript in the side panel, with every line that was read and "Show more lines" for the next page. The panel is a list that draws the rows in view and derives a line when it draws it: the state holds up to 20,000 lines (ADR 0723). The panel belongs to the shown voice channel, lies over the notices and the search, and under a thread and the pinned messages. Each row of the list is a flex column, whose text the list measures in the width of the panel, so a long line wraps inside the panel (ADR 0740 gives the rows of a message list the same width). Test: `transcripts::a_long_transcript_line_wraps_inside_the_panel` at window widths of 720, 1000 and 1600 pixels: the long line lies inside the panel and is higher than a line of one word.
- The transcript list reserves a scrolling error row after the loaded lines and before pagination, even without an error. The pagination button fills the available row width so its bounds also stay fixed when its label changes to a retry or a pending read (ADR 0730).
- A transcript result of a search (ADR 0743) shows the voice channel of its recording and opens the transcript in the side panel at its line. The lines are read as one page with that line in its middle in place of the loaded ones, when the state does not hold it, and again around it after a reload. The list goes to the line and marks it, once for each press on the result. "Show earlier lines" above the list reads the page before the first loaded line, and "Show more lines" the page after the last one.

Removal:

- The card has "Remove" for a person whose roles hold "manage recordings"; another person sees the control disabled with the reason. The roles are a prediction; the server decides (ADR 0739).
- A removal asks first. The card leaves the chat when the event of the server was followed. A refusal is stated in the card: no permission, a recording that the server still works on, and every other answer by the sentence of its error code. After the answer "not found" the recording is read again, and the card states that the server does not give it.

- A failed metadata refresh leaves the loaded transcript visible with an error and a retry that reloads the recording and transcript. The retry acquires its replacement reader before releasing the earlier one, keeping all loaded pages through failure and success. A failed page read retries the lines instead. This includes refreshes requested by a speaker rename.
- An archived channel offers no removal of its recordings. An open removal confirmation closes when the card loses that permission, and the removal handler checks that the current channel is active before sending a request. For an active channel the server decides permission and its refusal remains visible in the card.

## Consequences

- The prediction of "Remove" reads the roles of the workspace and not the overrides of the channel: a person who holds "manage recordings" only through an override of the voice channel sees the control disabled.
- The list of recordings of a workspace is described in ADR 0784 and uses the route of the client core (ADR 0723).
- The correction of a line, the name of a voice, the original text with who changed it, the details of a transcript, the new request for a transcript and the parts of a recording are in ADR 0616. The window follows a correction and a name that another client made.
- A person in a call reads the card of an earlier recording of that voice channel after the call.
- The state holds one range of lines without a hole: lines read around a line replace the lines from the start, and the person reads on in both directions. The bound of 20,000 lines drops lines at the end when lines are added before them.
- The card draws no frame of the video. Showing a video frame would require a thumbnail from the server.
- The window test against the real server makes a recording with the recorder in memory of the server, and checks the card with its files and its speakers, a saved audio file and the saved video against the files of the server, the transcript as text, the opening of the video, the transcript in the side panel, the jump from a search result, the refusal of a removal without the permission, and the removal with it.
