# 0603. Transcripts, diarization and glossary

## Status

Accepted

## Context

A recording has one audio file per track (ADR 0602), so the speaker of a microphone file is known. A microphone file is mostly silence, and Whisper invents text for long silence. A shared-sound file can contain several voices that belong to no member. Whisper accepts a list of names and terms before it starts, but it reads only a short list, and the list influences spelling without guaranteeing it. The transcription worker runs without outbound network access (ADR 0007).

Step 0 tested the components and measured how they must be used (`docs/step-0/README.md`, test 7). All test speech was synthetic.

- Without voice detection, Whisper invented 19 to 220 lines for 10 minutes of noise that contained two sentences. With voice detection and clips that the worker cuts itself, the transcript was exactly the two sentences. The built-in voice filter of the library joins the speech before Whisper reads it and lost one of the two sentences.
- With one language detection per piece of joined speech, the word error rate on mixed English and Portuguese was 49 to 52 %, because every sentence of one language was left out. With one detection per clip it was 0.9 to 1.4 %. For English and Portuguese alone it was 0.3 to 1.5 %.
- The library reads at most 223 tokens of a glossary and cuts a longer list without a message. With a glossary of 12 terms, the terms that were spelled right rose from 3 of 23 to 21 of 23 in English and from 7 of 23 to 19 of 23 in Portuguese.
- Speaker embeddings with clustering found the right number of speakers on 8 of 8 files and put 96 to 97 % of the speech time under the right speaker. The diarization pipelines whose model files need an account could not be used.

## Decision

Components:

- Voice detection: Silero VAD v6.
- Transcription: faster-whisper 1.2.1 on CTranslate2 4.8.2, with the Whisper model `large-v3-turbo`.
- Speaker separation: speaker embeddings from sherpa-onnx 1.13.8 with the TitaNet-Large model, and agglomerative clustering.
- All code is MIT or Apache-2.0. The models are MIT and CC-BY-4.0, and every model file is a plain HTTPS download that needs no account. The versions, the process and its isolation are in ADR 0610, and the model files in ADR 0612.

Transcription:

- A voice detector finds the parts of each audio file that contain speech. Only those parts go to Whisper.
- The worker builds the speech clips itself and does not use the voice filter of the library (ADR 0613).
- Whisper detects the language of each clip. The person who starts a recording can set the language beforehand; then every clip is transcribed in that language.
- Lines from a microphone carry the name of the member.
- Lines from shared sound are labelled as shared audio from that member's screen.

Diarization:

- An optional diarization component separates the voices inside shared sound into "Speaker 1", "Speaker 2" and so on.
- The component is the speaker separation named above. Its method and settings are in ADR 0614.
- The component runs without network access, and its licence terms allow distribution with the stated attribution: the code is Apache-2.0, and the model is CC-BY-4.0 with an attribution to its publisher.

Corrections:

- Anyone who can see a recording can correct a line or rename a speaker.
- A rename applies to one speaker label in one recording and can be repeated.
- Each change shows who made it, and the original text stays viewable.

Glossary:

- Each workspace has a glossary of names and terms that Whisper receives before it starts.
- The glossary has a size limit and an order of priority. The limit is 223 tokens, counted with the tokenizer of the Whisper model. The worker keeps the terms in order of priority up to the limit and reports which terms were used and which were cut (ADR 0613).
- Every member of the workspace can see the glossary, so it must not hold confidential terms.
- Editing the glossary needs "manage glossary".

## Consequences

- Speaker names for microphone lines come from the track and need no diarization.
- Voices inside shared sound have numbered labels until a person renames them. A rename does not carry over to another recording. Stored voice profiles that name recurring speakers automatically are in the later list of `docs/scope.md`.
- The glossary influences the spelling of its terms and does not guarantee it. The glossary cannot grow beyond its size limit, and the order of priority decides which terms are in the list that Whisper receives. A term of an invented name uses about 5 tokens.
- Mixed languages are transcribed when the speaker pauses between them. A change of language inside one clip is transcribed in one language.
- The results of speaker separation come from synthetic voices only: seven voices, at most four speakers, files of at most 10 minutes. The result fails abruptly when the threshold is too high: two voices became one speaker, and the error rate rose from 13 % to 36 % or more. A check on real recordings is required before a release (ADR 0614). A wrong label is corrected with the rename function.
- Overlapping speech gets one speaker, and the words of the second voice can be missing.
- Speed and memory, measured on a host with 8 CPU cores and a GPU with 8 GB, with a language per clip: 19 to 25 times real time for dense speech with the GPU, with 2.2 GB of memory and 3.3 GB of video memory. Without the GPU, 0.2 times real time on 8 threads while other work loaded the host, so that figure is a lower bound. Speaker separation ran on the CPU at 17 to 25 times real time. A track that is mostly silence costs the time of the voice detector only, which ran at about 400 times real time.
- A server without a GPU runs the same chain on the CPU (ADR 0610). One hour of dense speech then takes several hours.
- The worker runs with no network in its container. The telemetry client of ONNX Runtime is absent from the pinned version and is turned off (ADR 0007, ADR 0610).
- A transcript can be changed by any person who can see the recording. The changes are attributed and the original text is kept.
- Transcripts are searchable (ADR 0405) and are posted with the video (ADR 0602).
