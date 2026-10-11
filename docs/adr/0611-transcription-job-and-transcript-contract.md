# 0611. Transcription job and transcript contract

## Status

Accepted

## Context

The worker runs without network access (ADR 0610), so the server and the worker exchange files. A recording has one audio file per track and one microphone can have several files after a reconnect; each file is a segment with a start time, and the transcript is built on one timeline (ADR 0602). A recording with a failed segment keeps its other segments (ADR 0601), so one audio file that cannot be read must not lose the transcript of the others. An audio file can also be readable only in part: the recorder can stop while it writes, pages of a file can be lost, and a job can reach a limit. A transcript that covers a part of a file must say so. Lines from a microphone carry the member, lines from shared sound carry a speaker label, and a rename applies to one speaker label in one recording (ADR 0603). The server and the worker are released together, but they are separate programs in different languages. A path that is checked and then opened by name can point to another file at the moment it is opened. The regular-expression function of Python lets `$` match before a final line break, which the JSON Schema dialect does not.

## Decision

Files:

- The input is one job file and the output is one transcript file. Both are JSON and carry `format_version`, an integer. The first version is 1.
- Each format has a JSON Schema in `workers/transcription/src/junto_transcribe/schemas/`. The schemas are the reference for both programs.
- The worker validates the job against the job schema before any work and validates the transcript against the transcript schema before it writes it. A transcript that does not match is not written.
- Both schemas refuse unknown fields. Every pattern is written so that it must match the whole string in both regular-expression dialects.

Job:

- The job names the recording, the language (`auto` or a Whisper language code), the glossary in order of priority, whether speakers in shared audio are separated, and the segments.
- A segment has an identifier, a file path, a kind (`microphone` or `shared`), a participant (opaque identifier and display name) and its start time on the timeline in milliseconds.
- No string and no key of a job may contain a control character or a lone surrogate. The worker checks this before the schema and before any other use, so every accepted value can be written as UTF-8.
- A path is relative to an audio root, which is the directory of the job file or a directory given on the command line. An absolute path, an empty part, or a `.` or `..` part refuses the job.
- When the worker reads the job file, before it loads a model, it opens the directory of the job file without following a link and keeps the descriptor. It reads the job file from that descriptor; the job file may not be a link. The audio root is that descriptor, or the descriptor of the named audio root, opened at the same moment in the same way. The directory of the transcript is opened at the same moment; when it is the directory of the job file, the one descriptor is used.
- The worker opens each audio file by walking its path from the descriptor of the audio root one part at a time, and it refuses a part that is a link. It checks the opened descriptor: the file must be a regular file. It decodes from that descriptor.
- The transcript is written as a new file with a random name through the descriptor of its directory and renamed there.
- A path that changes after the job file was read, also while the models load, can therefore not change what is read or where the transcript is written.
- An audio file must contain exactly one audio stream. A file with none or with several fails its segment, because the transcript could not say which stream it covers.
- A glossary term may not contain the written form of a control token of Whisper, and may not be tokenised into one (ADR 0613).
- A job that is not valid is refused as a whole, with exit code 2 and no transcript. The refusal names a location in the job and a fixed code, and no value (ADR 0610).

Transcript:

- Lines are in timeline order. A line has start and end in milliseconds on the timeline, the segment, the source, the text, the language and word times when the model gives them.
- The source of a microphone line is the participant. The source of a shared-audio line is the participant and a label `Speaker N`, or no label.
- A header reports: every participant, a result per segment, the glossary terms that were used and those that were cut, every speaker label with its participant, the version of the worker, every model with its version, licence and checksum, every library version, the settings in use, and timing figures.
- All values that differ between two runs of the same job are in the `timing` object.
- The transcript is written to a temporary file in the target directory and renamed.

Segment results:

- The status of a segment is `ok`, `empty`, `damaged` or `failed`. `ok` and `empty` mean that the whole file was processed. `damaged` means that only a part of the audio was processed; the lines of that part are in the transcript, and the result states how much audio was decoded. `failed` means that nothing was processed.
- A segment is damaged when a packet cannot be read or decoded, when a gap between packet timestamps is over its limit, when a packet timestamp is more than 50 ms earlier than the end of the packet before it, or when the segment reaches a limit on audio length or processing time. The worker does not skip a packet that it cannot decode, because the audio after it would have a wrong time, and it does not choose between two pieces of audio that claim the same time.
- A segment result has two lengths. `audio_ms` is the audio that was decoded, with inserted silence; decoding stops at a limit. `processed_ms` is how far the lines reach. The two differ only when the processing time limit stopped the segment.
- A gap between packet timestamps up to the limit becomes silence of the same length, so the lines after the gap keep their time. The first decoded sample is the start of the file.
- `damaged` and `failed` carry a reason: a code from a fixed list and a fixed message.
- Speaker separation has its own result per segment: `not_applicable`, `not_requested`, `separated`, `unassigned` or `failed`, with its own reason for the last two. `separated` means that every line of the segment has a label (ADR 0614). Speaker separation that the time limit stopped is reported with the reason `time_limit`.
- A damaged or failed segment does not fail the job. The exit code is 0 whenever a transcript was written, and the server reads the result of each segment from the transcript.

Versioning:

- A change that a reader or writer of the current version cannot process increases `format_version`: a removed or renamed field, a changed meaning, a new required field in the job.
- The worker reads exactly the job versions it knows and refuses every other version. It writes one transcript version.
- A new optional field does not increase the version. Because the schemas refuse unknown fields, the schema, the worker and the server change in the same release.

## Consequences

- The server can tell a transcript of a whole file from a transcript of a part of a file from the segment results, without reading logs. A transcript with a damaged or failed segment is `partial` (ADR 0607).
- A recording file that simply ends early, without a packet that fails, is decoded to its end and reported as `ok`. The worker cannot know that more audio was meant to follow; the server knows the expected length of a segment and can compare it with `audio_ms`.
- A link anywhere in an audio path is refused, also a link that points inside the audio root. The directory of the job file, a named audio root and the output directory may not be links themselves; the directories above them can be. The server must store audio files as regular files under the audio root.
- A recorder file with several audio streams must be split before it becomes a segment.
- The server needs the display names only once, in the header; lines refer to participants by identifier.
- A transcript states which models and settings produced it, so a later change of a model can be told apart from a change of the audio.
- A worker and a server of different releases do not work together when one of them uses a field that the other does not know. They are deployed together (ADR 0003).
- The worker writes no partial transcript. A reader that finds the file finds a complete and valid one.
- The transcript holds the original machine text only. Corrections and renames (ADR 0603) are stored by the server and are not part of this format.
- How the server starts the worker and passes the files is in ADR 0607.
