# 0606. Files of a finished recording: search, recovery, join, check and final state

## Status

Accepted

## Context

ADR 0604 leaves a recording in the state `processing` when every job of the recorder has ended. ADR 0602 decides that the server joins the pieces of the composite into one MP4 file, places every file on one timeline, checks every finished file and recovers the files that a lost recorder left. It does not state which programs do this, how the server finds a file, what a file of a lost job is, which thresholds the check uses, or how the final state follows from the files. Step 0 measured the check with `ffprobe` and `ffmpeg` (`docs/step-0/README.md`, test 3) and stated its thresholds in `spikes/recorder-two-hosts/RESULT.md` (X3, X6, X8).

The recorder writes into the directory of the recordings and reports a file name for each job. The recorder loads a web page and decodes media that people send, so the server does not treat what it writes as its own data. A muted microphone gives an audio file of digital silence (ADR 0601). A server can stop in the middle of the work, and two server processes can run on one database for a short time during a restart.

## Decision

Programs:

- The server runs `ffprobe` and `ffmpeg` as child processes. `JUNTO_FFPROBE` and `JUNTO_FFMPEG` name them; without the settings the server takes them from its path.
- A server with a recorder checks at its start that both programs run, and does not start without them.
- A child process gets `PATH` and no other variable of the environment of the server. One run has a time limit of one hour.
- A program reads a file as its standard input, from the descriptor that the server opened, and gets no path of an input. The server states the form of the file (`ogg` for an audio track, `mp4` for a video, `mpegts` for the pieces), so a file of another form, for example a playlist that names further files, is refused and not read by its content.

Paths:

- The recorder writes the file of an audio track to `<recording>/<segment>.ogg` and the pieces of a composite to `<recording>/<segment>/part_<number>.ts` (ADR 0604). The server writes the video of a composite to `<recording>/<segment>.mp4`.
- The server builds every path from the identifiers that it generated. It does not use the file name that the recorder reports.
- The server opens a file one path part at a time from the directory of the recordings, follows no link, and accepts a regular file only. This holds for the check, for the join and for a download (ADR 0608).
- The directory of the recordings itself, and the directories above it, are trusted: the administrator names them, and the recorder cannot replace them.
- Every change that the server makes below the directory of the recordings works from a descriptor of the directory that holds the file, reached in the same way: a new directory, a new file, a new name, a move and a deletion. A link above the place is an error, and a link at the place is deleted and not followed.

Recovery after a lost recorder:

- The start script of the recorder saves the files of a lost recorder to `rescued/<job>/` below the directory of the recordings (ADR 0602). For an audio track without a file at its path, the server takes `rescued/<job>/<segment>.ogg` and moves it to the path of the segment. A job identifier that is not one safe path part names no directory.
- The pieces of a composite are at their path, because the recorder copies each finished piece there. The server joins the pieces that are on disk.
- A file that is found for a failed segment is marked as recovered. The segment stays failed.
- When the processing of a recording has ended, the directory `rescued/<job>/` of a lost job is deleted (ADR 0608): of an audio track when the server has taken its file, and of a composite always. Saved audio that the server could not take stays.

Join of the composite:

- The server does not read the playlist of the recorder. It lists the pieces `part_<number>.ts` in the directory, orders them by their numbers, and gives their bytes one after the other to `ffmpeg -f mpegts -i pipe:0 -c copy -movflags +faststart`. This is what the playlist reader of ffmpeg does with a closed playlist, so a playlist without an end marker needs no repair, and no text that the recorder wrote decides which file is read.
- ffmpeg writes the video to a path, under a new name with a random part, because it reads its own output again to move the index to the start. The server then gives the video its name with a hard link, which never replaces a file. A video that exists is not joined again.
- The process that holds an attempt (below) deletes the unfinished videos that it finds in the directory of the recording: an attempt that is over left them.
- Each composite segment becomes its own video. A recording whose recorder was lost has two.
- The pieces are deleted when the video passed the check. They stay otherwise.

Timeline:

- An audio file is placed at its reported start minus 0.047 seconds, and a composite at its reported end, minus its media length, minus 0.135 seconds (ADR 0602).
- A composite without a reported end is placed by its sound: the server decodes up to 60 seconds of the composite and of the longest microphone file that passed its check and has sound, from the same moment by their reported starts, computes the loudness of both in steps of 2 milliseconds, and searches the shift of at most 6 seconds at which the two agree best. Less than 4 seconds of common sound, or an agreement below 0.5 of the highest possible value, places nothing.
- A file without a place has no start time. The start of the timeline of a recording is the earliest start of its files.

Check of a file:

- Timestamps, from the packets, without decoding: the sound exists, and the picture of a composite exists; each stream begins at most 1 second after the file and ends at most 1 second before it; no gap between two packets is longer than 0.25 seconds for sound and 1 second for the picture.
- Length: the media length differs from the length that the recorder reported by at most 3 seconds plus one thousandth of that length. The rule applies to a segment that ended in order.
- Picture: at most 5 % of the key frames are dark (average brightness below 24 of 255), and at most 5 % are of one colour (brightness range below 16). The two shares are counted apart, as step 0 measured them. A video without a key frame fails.
- Sound of the composite: the composite is silent (below -70 dB for at least 1 second) for at most 1 second in total inside the span of a microphone file while that file is not silent. Only microphone files that passed their own check, have sound and have a place are compared. The sound of a screen share is not compared, because the composite carries the sound of one share only (ADR 0602).
- Digital silence of a whole file (highest level below -60 dB) is stated with the file and is no problem: a microphone that was muted for the whole recording gives such a file. A composite of a call in which nobody spoke is silent as well, and passes.
- A file that `ffprobe` or `ffmpeg` cannot read has the problem `unreadable`.

Attempts:

- One task processes one recording at a time.
- One server process holds an attempt. It takes the attempt in one transaction, states every 15 seconds that it still holds it, and gives it back when the attempt ends. Another process takes no attempt of that recording while that statement is younger than 90 seconds. An attempt whose process is gone is over after that time.
- A process stores the results only while it holds the attempt.
- An attempt stores nothing until every file is examined. The results, the final state and the events are one transaction.
- A step that did not run to its end, because a program did not start, reached its time limit or the file system refused, makes the attempt incomplete. It is repeated after 30 seconds. An attempt counts when it starts, and a recording has three.
- After the third attempt, a file that exists and was not examined has the problem `not_checked`.
- A recording whose three attempts were used by a server that stopped during each is finished by the next process that takes it, with the files that are on disk, each `not_checked`. No program runs for it again.

Final state:

- `ready`: every segment ended in order, and each has a file that passed the check.
- `partly_failed`: at least one file exists, and a segment failed, a file is missing or a file has a problem.
- `failed` with the reason `files_missing`: no file exists.
- The state of a recording states what was saved of its files. The transcript has its own state (ADR 0607).
- When the state is final, the reservation of disk space ends (ADR 0605), the server keeps the note `recording_ready` for the post in the text chat when a file exists (ADR 0602), and `recording_updated` is recorded.

What a recording states about a file:

- The media type, the bytes, the media length, the start on the timeline, the size of the picture, whether the check passed, the problems, whether the sound is digital silence, and whether the file was recovered.
- The problems are an open set, as the reasons of ADR 0604 are.

## Consequences

- A server with a recorder needs `ffmpeg` and `ffprobe`. The tests of recordings need them with the encoders `libopus` and `libx264`, because the recorder in memory writes files that ffmpeg generates.
- A recording with a failed segment is `partly_failed` also when the file of that segment was recovered whole.
- A request to start a job that failed, followed by one that succeeded for the same track, leaves a failed segment. The recording is `partly_failed`.
- A recording in which one person was muted for the whole time is `ready`. A track that recorded nothing because of a defect of the recorder is not told apart from a muted microphone.
- The video of a composite whose pieces do not pass the check stays with its pieces until the recording is removed.
- Two server processes do not process one recording at the same moment. A process that could not state for 90 seconds that it holds its attempt, and that still runs, can meet a second one: both then join the video, one hard link wins, and only the process that holds the attempt stores results.
- A link that a faulty recorder puts in the place of a file, or above it, is not followed. The segment then has no file.
- The output of the join is the one path that a program gets. A recorder that replaces the directory of a recording by a link at the moment of the join makes ffmpeg write the video into the directory of the link, under a name that the recorder cannot predict. The server then finds no video, and the attempt is incomplete.
- The check of a video of one hour takes about 80 seconds of one CPU core (ADR 0602). The recordings of a server are processed one after the other.
- The join by bytes was tested with pieces that ffmpeg wrote, not with pieces of the recorder. ADR 0602 measured the join with the playlist of the recorder.
- The placement by sound was tested with generated tones, as in step 0, and not with speech.
- The thresholds come from the measured recordings of step 0. They treat only digital silence and dark or one-colour key frames as defects of the content.
