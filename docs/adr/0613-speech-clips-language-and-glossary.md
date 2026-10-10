# 0613. Speech clips, language per clip and the glossary limit

## Status

Accepted

## Context

Only the parts of an audio file that contain speech go to Whisper, Whisper detects the language unless the person who starts the recording sets it, and a workspace glossary influences the spelling of names and terms (ADR 0603). The technical test of step 0 measured how the chosen libraries must be used (`spikes/transcription/RESULT.md`, T2, T3, T4):

- Without voice detection Whisper invented 19 to 220 lines for ten minutes of noise.
- The built-in filter of faster-whisper joins all detected speech before Whisper reads it. With one English and one Portuguese sentence it lost one of the two sentences, and on mixed speech the word error rate was 49 to 52 % in every built-in mode. With one clip per detected speech span and a language per clip it was 0.9 to 1.4 %.
- With 200 ms of padding around a span the first word of a sentence was lost; with 400 ms it was not.
- faster-whisper has two ways to give Whisper a text before a clip: `initial_prompt` and `hotwords`. Both are cut by the library without a message at 223 tokens. `initial_prompt` keeps the last 223 tokens and `hotwords` keeps the first 223.
- With one clip per span and a glossary under the limit, both ways gave the same result: 21 of 23 terms right in English and 19 of 23 in Portuguese, against 3 and 7 without a glossary.
- With a glossary over the limit and the important terms first, `hotwords` gave 12 and 16 of 23 and `initial_prompt` gave 3 and 7 of 23, the same as no glossary, because the library cut the important terms.
- The test recommends `hotwords` with at most 200 tokens.

ADR 0603 states that Whisper detects the language of each recording and that the list has about 220 tokens. The measured need is a language per clip, and the limit that the technical test recommends is 200 tokens.

## Decision

Clips:

- The worker computes the speech probability of each 512-sample frame (32 ms at 16 kHz) with Silero VAD v6 and builds the clips itself. It does not use the voice filter of faster-whisper.
- Speech starts at a frame with a probability of at least 0.5. It ends at the first frame below 0.35 that is followed by 300 ms without a frame of at least 0.5. A shorter pause does not split a clip.
- Each clip gets 400 ms of padding on each side. Where two clips are closer than 800 ms, they meet in the middle of the pause. Padding stops at the start and the end of the file.
- A clip is at most 29 s long with its padding. Longer speech is split at its last pause of more than 98 ms, or at the limit when it has none.
- These are the rules and values of the library function that the technical test measured. A unit test compares the worker's clips with that function on generated probability sequences, and a model test compares them on speech.
- Each clip is one element of a batched Whisper call, with its own start and end time. One transcript line is one clip.

Blocks:

- An audio file is read in blocks (ADR 0610). The clips do not depend on the block length.
- The worker runs the voice detector as one stream per file: the recurrent state of the model and the last 64 samples before each frame go from one block to the next, as the library does inside one call for a whole file.
- Speech that has not ended at the end of a block continues in the next block. A span gets its padding when the padding can no longer change: when the next span has ended, when the audio after it has been silent for 800 ms, or at the end of the file.
- The worker keeps the audio that a later clip can need, which is the audio since 400 ms before the earliest span without final padding. This is at most about one minute.
- A unit test compares the clips, the lines and the speaker labels of the same audio for several block lengths with a detector whose output depends on earlier frames. A model test requires the same transcript.

Language:

- With `auto`, Whisper detects the language of each clip, and the line carries that language.
- With a language code in the job, every clip is transcribed in that language and no detection runs.
- A language code that the model does not know refuses the job.

Decoding:

- Beam search with 5 beams and temperature 0, batch size 8 for `large-v3-turbo`, word times on.
- Clips go to Whisper in groups of 64, in the order of the file; the last group of a file can be smaller. The groups do not depend on the block length, so the input of every Whisper call is the same for every block length.
- The audio of the clips of a group is put into one array with 3 s of silence between two clips. The silence never reaches the model: Whisper reads each clip by its position. The spacing makes every clip start after a pause for the library's correction of the time of the first word.
- The worker does not join neighbouring clips into longer pieces. The measured mode that does this dropped words, and a joining rule of the worker's own was not measured.

Glossary:

- The glossary is one text: the terms in order of priority, separated by a comma and a space, with a full stop at the end.
- The text goes to Whisper through the `hotwords` option of faster-whisper, with every clip.
- The worker counts the tokens of that text with the tokenizer of the loaded Whisper model, in the form in which the library tokenises `hotwords`. It keeps the longest prefix of the term list whose text has at most 200 tokens.
- A term that does not fit ends the list: a later, shorter term is not moved ahead of it.
- The transcript reports the token count, the terms that were used and the terms that were cut.
- A term that contains the written form of a control token (`<|` or `|>`), or that the tokenizer of the loaded model turns into a special token, refuses the job with the code `glossary_control_token`. The glossary text stands before the task and language tokens in the input of the model, and the library finds the language position by the first token with the number of the language token; a term must not be able to put a control token there.

Reasons for `hotwords` at 200 tokens:

- It is the recommendation of the technical test.
- When a text is over the library's limit, `hotwords` keeps the terms with the highest priority and `initial_prompt` drops them. The worker's own cut keeps the text under the library's limit, so this case is not expected; if the worker's count were ever wrong, `hotwords` loses the least important terms.
- For a text under the limit the two ways are equal. The worker measured both on the GPU with the same generated speech (one generation, before the test speech had a fixed seed): 18 of 23 terms right in English and 17 of 23 in Portuguese with either option, against 5 and 8 without a glossary; the word error rate of the two files was 6.3 % and 4.1 % with either option.
- The limits 200 and 223 gave the same result on a glossary of 232 terms: 17 of 23 terms right in both files with either limit and either option. 47 terms fit in 200 tokens and 53 in 223.
- 200 tokens leave 23 tokens between the worker's cut and the library's cut.

## Consequences

- Silence does not reach Whisper. A microphone file that is mostly silence costs the time of the voice detector only.
- Mixed English and Portuguese is transcribed in both languages when the speaker pauses between them. A change of language inside one clip is transcribed in one language.
- A short clip can be detected as a language that nobody spoke. The job's fixed language avoids this for a recording in one language.
- With `auto`, the word times of a clip are aligned with the language that was detected for the first clip of its group. This affects word times only, not the text or the language of the line.
- A clip is given to Whisper up to about 29 s after its speech ended, because a span waits for its padding and a group waits for 64 clips or the end of the file. The worker does not give out partial results, so this changes memory use only.
- Every clip is padded to a 30-second window for Whisper. On a CPU this makes dense speech slow: the worker measured 0.2 to 0.4 times real time for `large-v3-turbo` on 6 threads of a loaded 8-core machine.
- The server can show which glossary terms are in use from the transcript of any recording.
- The glossary influences the spelling and does not guarantee it (ADR 0603). The number of terms that are spelled right changes with the speech. The speech engine of the tests synthesises with random variation; ten generations of the same sentences (eight seeds and two runs without a seed) gave 18 to 20 of 23 terms right in English and 16 to 20 of 23 in Portuguese, against 2 to 5 and 5 to 9 without a glossary. One term was not spelled right in any generation of the Portuguese file.
- The test speech is generated with a fixed seed, and its checksums are recorded, so a result of the tests can be repeated. A test bound for the glossary is the lowest of the repeated measurements minus one term.
- A glossary that fills the limit helps less for each term than a short one: 17 of 23 with 47 terms against 18 of 23 with 12 terms in English.
- ADR 0603 names a limit of about 220 tokens. The limit in use is 200 tokens.
- The clip rules come from synthetic speech. They are not verified on real recordings.
