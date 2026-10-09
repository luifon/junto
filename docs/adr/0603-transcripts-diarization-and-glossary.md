# 0603. Transcripts, diarization and glossary

## Status

Accepted, pending step 0

## Context

A recording has one audio file per track (ADR 0602), so the speaker of a microphone file is known. A microphone file is mostly silence, and Whisper invents text for long silence. A shared-sound file can contain several voices that belong to no member. Whisper accepts a list of names and terms before it starts, but it reads only a short list, about 220 tokens, and the list influences spelling without guaranteeing it. The transcription worker runs without outbound network access (ADR 0007).

## Decision

Transcription:

- A voice detector finds the parts of each audio file that contain speech. Only those parts go to Whisper.
- Whisper detects the language of each recording. The person who starts a recording can set the language beforehand.
- Lines from a microphone carry the name of the member.
- Lines from shared sound are labelled as shared audio from that member's screen.

Diarization:

- An optional diarization component separates the voices inside shared sound into "Speaker 1", "Speaker 2" and so on.
- Step 0 chooses the component (`docs/step-0/README.md`, test 7).
- The component must run without network access, and its licence terms must allow distribution with the stated attribution.

Corrections:

- Anyone who can see a recording can correct a line or rename a speaker.
- A rename applies to one speaker label in one recording and can be repeated.
- Each change shows who made it, and the original text stays viewable.

Glossary:

- Each workspace has a glossary of names and terms that Whisper receives before it starts.
- The glossary has a size limit and an order of priority.
- Every member of the workspace can see the glossary, so it must not hold confidential terms.
- Editing the glossary needs "manage glossary".

## Consequences

- Speaker names for microphone lines come from the track and need no diarization.
- Voices inside shared sound have numbered labels until a person renames them. A rename does not carry over to another recording. Stored voice profiles that name recurring speakers automatically are in the later list of `docs/scope.md`.
- The glossary influences the spelling of its terms and does not guarantee it. The glossary cannot grow beyond its size limit, and the order of priority decides which terms come first in the list that Whisper receives.
- A transcript can be changed by any person who can see the recording. The changes are attributed and the original text is kept.
- Transcripts are searchable (ADR 0405) and are posted with the video (ADR 0602).
- If no diarization component passes step 0, the design changes or speaker separation inside shared sound is removed from the first version.
- The Whisper model, and what a server without a GPU does, are not fixed by this record.
