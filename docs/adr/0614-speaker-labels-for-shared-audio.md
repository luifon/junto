# 0614. Speaker labels for shared audio

## Status

Accepted

## Context

Shared sound can contain several voices that belong to no member. An optional component separates them into "Speaker 1", "Speaker 2" and so on; a rename applies to one speaker label in one recording (ADR 0603). The speaker of a microphone file is known from its track. A shared-sound track of one participant can consist of several files, because a reconnect or a new share starts a new segment (ADR 0602). The technical test of step 0 chose speaker embeddings with clustering and measured its settings (`spikes/transcription/RESULT.md`, T5): with TitaNet-Large, 2.5 s windows and the threshold 0.6 it found the right number of speakers on 8 of 8 synthetic files and put 96 to 97 % of speech time under the right speaker. The result fails abruptly when the threshold is too high: at 0.7, two voices became one. The embedding model gives no vector for audio that is too short. The vectors of a participant's shared audio must be kept until all of that audio is read, so their memory grows with the amount of speech.

## Decision

Scope:

- Speakers are separated only in `shared` segments, and only when the job asks for it.
- A microphone line carries its participant. Microphone audio is never embedded or clustered.
- All `shared` segments of one participant in a job are clustered together. One voice therefore has one label in every segment of that participant's shared audio.
- The shared audio of different participants is clustered separately.

Method:

- The speech spans come from the same voice detection as the clips (ADR 0613), with 200 ms of padding.
- Each span is cut into windows of 2.5 s with a step of 1.25 s. A span of at most 2.5 s is one window. The last window ends at the end of the span.
- Each window gets one embedding from TitaNet-Large through sherpa-onnx, on the CPU.
- The embeddings are clustered with average linkage on cosine distance, cut at the distance 0.6. The threshold belongs to the model and is stored with it in the manifest (ADR 0612).
- A cluster with fewer than 3 windows joins the larger cluster with the nearest centroid.
- Inside a span, each moment belongs to the window with the nearest centre.
- A transcript line takes the speaker with the most time inside the line. A line has one label.
- A line that overlaps no window gets no label. The worker does not give it the label of the nearest other speech.
- The clustering is the worker's own code. A unit test compares its result with SciPy.

Labels:

- Labels are `Speaker N`. N counts over the whole recording, in the order in which the speakers first appear on the timeline.
- The transcript lists every label with the participant whose shared audio it belongs to.

Result per segment:

- `separated`: every line of the segment has a label.
- `unassigned`: at least one line has no label. The reason is `no_voice_window` when the speech of a line was too short for a vector, or `speaker_window_limit` when the limit of voice windows was reached.
- `failed`: no line has a label, because the embedding model or the clustering failed, the segment itself failed, or the processing time limit was reached before the voices of that participant were clustered (`time_limit`). The text of the lines stays in the transcript.
- The time limit is checked before each embedding and before the clustering of each participant's shared audio (ADR 0610).

Bounds:

- A job embeds at most 60 000 windows (a setting, ADR 0610). Lines after that point get no label. The vectors are kept as 32-bit numbers: 46 MB at the limit.
- Above 4000 windows in one participant's shared audio, every n-th window is clustered and each other window takes the speaker with the nearest centroid. The assignment runs in batches of 4096 windows.
- The distance matrix of the clustered windows is the largest allocation: 128 MB for 4000 windows.

## Consequences

- A label names one speaker in one recording, so a rename by label (ADR 0603) needs no participant.
- The same person who is heard in the shared audio of two participants gets two labels.
- Labels do not carry over to another recording. Stored voice profiles are in the later list of `docs/scope.md`.
- The server can tell a segment in which every line has a label from one in which labels are missing, and it knows the reason.
- Overlapping voices get one label for the line: the second voice of an overlap is not labelled, and its words can be missing from the text. The technical test observed this on two overlaps.
- A voice that is heard for less than three windows in the whole recording gets the label of the most similar other voice.
- The memory for speaker separation is bounded by the limit of voice windows and by the size of the clustered subset. It does not depend on the length of one file.
- The threshold and the window length were chosen on seven synthetic voices and at most four speakers. They are not verified on real voices, on more than four speakers or on recordings longer than ten minutes. A check with real recordings is needed before a release, and a wrong label is corrected by a person through the rename function.
- The speaker model is under CC-BY-4.0 and needs the attribution in `workers/transcription/THIRD-PARTY.md`.
