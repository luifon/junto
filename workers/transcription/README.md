# Junto transcription worker

A program that turns the audio files of one recording into one transcript. It runs voice detection (Silero VAD), transcription (Whisper `large-v3-turbo` on CTranslate2 through faster-whisper) and speaker separation for shared audio (TitaNet embeddings through sherpa-onnx, with clustering). The choice of components and their settings come from the technical test in `spikes/transcription/RESULT.md`.

It is the only component of Junto that is not written in Rust (ADR 0610). It runs in a container without network access, reads one job file, writes one transcript file and exits.

- Design records: ADR 0610 to 0614 in `docs/adr/`.
- Licences of every model and package: [`THIRD-PARTY.md`](THIRD-PARTY.md).

## Commands

```
junto-transcribe run <job.json> --out <transcript.json> [--audio-root <dir>]
junto-transcribe models fetch --dir <path> [--whisper <name>]
junto-transcribe models verify --dir <path> [--whisper <name>]
junto-transcribe models list
junto-transcribe self-test
```

`run` and `self-test` take `--models-dir` (default `$JUNTO_TRANSCRIBE_MODELS`, then `/models`), `--whisper-model` (default `$JUNTO_TRANSCRIBE_WHISPER_MODEL`, then `large-v3-turbo`), `--device auto|cpu|cuda` (default `$JUNTO_TRANSCRIBE_DEVICE`, then `auto`), `--compute-type`, `--threads` and `--batch-size`. `run` also takes the limit options of the section "Limits".

| Command | What it does |
|---|---|
| `run` | Transcribes one job. One process runs one job. |
| `models fetch` | Downloads the model files over HTTPS and keeps a file only when its size and SHA-256 checksum match the manifest. This is the only command that uses the network. |
| `models verify` | Checks that every model file is present with the right size and checksum, and that a model directory holds no other file. |
| `models list` | Prints the manifest: models, licences, attributions, checksums. |
| `self-test` | Verifies and loads the models, runs each of them once on generated noise, and runs one whole job (a microphone file, a shared file and a missing file). It uses no speech recording, so it checks that the models load and run on this machine, not their accuracy. |

Exit codes of `run`:

| Code | Meaning | Transcript written |
|---|---|---|
| 0 | The job was processed. Single segments can be damaged or failed; the transcript says which. | yes |
| 1 | Unexpected error. | no |
| 2 | The job file or the command line is not valid, or the directory of the job file or the audio root cannot be used. | no |
| 3 | The worker refused to start: a model file is missing, unlisted or different, the worker can write to the model files, a model file changed while the models were loaded, a library has another version than the tested one, the requested device does not exist, or the output directory cannot be used. | no |
| 4 | A model cannot be loaded on the device, or failed in a way that makes the process unreliable (for example video memory ran out). | no |

The transcript is written to a temporary file and renamed, so a reader never sees a partial file.

### Logs

Log lines go to standard error. They contain positions, counts and fixed codes: `segment 3 of 8: status=failed clips=0 lines=0 speakers=not_applicable reason=file_missing`. They never contain a value of the job (identifiers, names, glossary terms, paths) or transcript text.

- A refused job is reported as a location in the job and a code, for example `/segments/3/path: path_not_inside` or `/glossary/17: schema.pattern`. A location contains array positions and the field names that the schema defines. Any other key of the job file appears as `<key N>`, its position in its object.
- A refused start is reported as a code, with names from the model manifest where a model file is meant: `model_file_missing: whisper large-v3-turbo model.bin`, `model_files_writable`, `output_directory_not_usable`. It contains no path of the host.
- A model that cannot be loaded, and a model failure on the device, are reported as a code and the type of the error. The message of a library is not passed on.
- An unexpected error is logged with its type and the place in the code, without its message.
- Tests scan the logs of refused jobs, refused starts, a whole run and failures for values of the job and for paths.

`models fetch` and `models verify` are commands of the administrator and do not read a job. `models fetch` names the directory that the administrator gave and the download address when it fails.

## The contract

The job file and the transcript file are the contract between the server and the worker. Both are JSON with a `format_version`. The schemas are the reference:

- [`src/junto_transcribe/schemas/job.schema.json`](src/junto_transcribe/schemas/job.schema.json)
- [`src/junto_transcribe/schemas/transcript.schema.json`](src/junto_transcribe/schemas/transcript.schema.json)

The worker validates the job against the job schema before it does anything, and validates its own transcript against the transcript schema before it writes it. Both schemas refuse unknown fields, and every pattern must match the whole string. The rules for changing the formats are in ADR 0611.

### Job, format version 1

```json
{
  "format_version": 1,
  "recording_id": "0198c3f2-recording",
  "language": "auto",
  "glossary": ["Ysolde Varkonen", "Quorvex", "Lumitrax"],
  "separate_speakers": true,
  "segments": [
    {
      "id": "seg-1",
      "path": "audio/seg-1.ogg",
      "kind": "microphone",
      "participant": {"id": "member-17", "name": "Ysolde Varkonen"},
      "start_ms": 0
    },
    {
      "id": "seg-2",
      "path": "audio/seg-2.ogg",
      "kind": "shared",
      "participant": {"id": "member-17", "name": "Ysolde Varkonen"},
      "start_ms": 75200
    }
  ]
}
```

| Field | Meaning |
|---|---|
| `recording_id` | Opaque identifier, copied to the transcript. |
| `language` | `auto`: Whisper detects the language of each speech clip. A Whisper language code (`en`, `pt`, …): every clip is transcribed in that language. A code the model does not know refuses the job. |
| `glossary` | Names and terms, most important first. No duplicates, no leading or trailing space. A term may not contain `<|` or `|>`, and may not be tokenised into a control token of the Whisper model: both refuse the job with `glossary_control_token`. |
| `separate_speakers` | Whether voices in `shared` segments get speaker labels. |
| `segments[].id` | Opaque identifier, unique in the job. The transcript reports each segment under it. |
| `segments[].path` | The audio file, relative to the audio root, with `/` separators. An absolute path, an empty part, or a `.` or `..` part refuses the job. The audio root is the directory of the job file, or `--audio-root`. |
| `segments[].kind` | `microphone`: one participant's microphone. `shared`: the sound of that participant's screen share. |
| `segments[].participant` | Opaque `id` and display `name`. The same `id` must have the same `name` in every segment. |
| `segments[].start_ms` | Start of the file on the timeline of the recording. A participant who reconnects has several segments with their own start times. |

Rules for the whole job file:

- It is a regular file of at most 4 MiB (`--max-job-bytes`). A larger file, a device or a pipe is refused before it is parsed.
- No string, and no key, contains a control character (U+0000 to U+001F, U+007F to U+009F) or a lone surrogate. This is checked before anything else, so every value can be written to the transcript.
- It has at most 2000 segments (`--max-segments`).
- Validation stops after the first five errors.

How the files are reached:

- When the job file is read, before any model is loaded, the worker opens the directory of the job file and keeps its descriptor. A directory that is a link is refused (`job_directory_not_usable`). The job file is read from that descriptor and may not be a link.
- The audio root is that same descriptor, or the descriptor of `--audio-root`, opened at the same moment under the same rule (`audio_root_not_usable`).
- The directory of `--out` is opened at the same moment. When it is the directory of the job file, the one descriptor is used. The transcript is written as a new file with a random name inside that descriptor and renamed.
- The worker walks each audio path from the descriptor of the audio root one part at a time and refuses a part that is a link. It accepts the file from its open descriptor only when it is a regular file, and decodes from that descriptor.
- A directory or file that is replaced after the job file was read, also while the models load, cannot change what is read or where the transcript is written. A segment whose path became a link fails with `not_a_regular_file`.

Audio formats: Ogg with Opus (what the recorder writes), Ogg with Vorbis, WAV, FLAC and Matroska/WebM with those codecs. A file must contain exactly one audio stream; it is mixed to mono 16 kHz. The first decoded sample is the start of the file.

### Transcript, format version 1

```json
{
  "format_version": 1,
  "recording_id": "0198c3f2-recording",
  "language": "auto",
  "participants": [{"id": "member-17", "name": "Ysolde Varkonen"}],
  "lines": [
    {
      "start_ms": 1180,
      "end_ms": 3240,
      "segment_id": "seg-1",
      "source": {"kind": "microphone", "participant_id": "member-17"},
      "text": "Can everyone see my screen now?",
      "language": "en",
      "words": [{"start_ms": 1180, "end_ms": 1400, "text": " Can"}]
    },
    {
      "start_ms": 76020,
      "end_ms": 80110,
      "segment_id": "seg-2",
      "source": {"kind": "shared", "participant_id": "member-17", "speaker": "Speaker 1"},
      "text": "Thanks for joining.",
      "language": "en",
      "words": null
    }
  ],
  "segments": [
    {"id": "seg-1", "status": "ok", "reason": null, "audio_ms": 60000, "processed_ms": 60000, "speech_ms": 2100, "clips": 1, "lines": 1, "speakers": "not_applicable", "speakers_reason": null},
    {"id": "seg-2", "status": "damaged", "reason": {"code": "decode_incomplete", "message": "decoding stopped before the end of the file; the lines cover the audio before that point"}, "audio_ms": 30000, "processed_ms": 30000, "speech_ms": 4000, "clips": 1, "lines": 1, "speakers": "separated", "speakers_reason": null}
  ],
  "glossary": {"token_limit": 200, "token_count": 19, "used": ["Ysolde Varkonen", "Quorvex", "Lumitrax"], "cut": []},
  "speaker_separation": {"requested": true, "speakers": [{"label": "Speaker 1", "participant_id": "member-17"}]},
  "engine": {"worker": "0.1.0", "device": "cuda", "compute_type": "float16", "threads": 6, "models": [], "libraries": {}, "settings": {}},
  "timing": {"total_ms": 9100, "audio_ms": 90000, "speech_ms": 6100, "stages_ms": {}, "peak_memory_mb": 2100}
}
```

| Field | Meaning |
|---|---|
| `lines` | In timeline order: by `start_ms`, then `end_ms`, then the order of the segments in the job. One line is one speech clip. |
| `lines[].start_ms`, `end_ms` | On the timeline of the recording: the segment's `start_ms` plus the position in the file. |
| `lines[].source` | For a microphone: `kind` and `participant_id`. For shared audio: also `speaker`, a label `Speaker N`, or `null` when the line has no label. |
| `lines[].language` | The language Whisper used for this line: detected for the clip (`auto`) or the job's language. |
| `lines[].words` | Word times on the same timeline, or `null` when the model gave none. A word's `text` keeps its leading space: the texts of the words of a line, joined without a separator, are the text of the line. |
| `segments[]` | One result per segment of the job, in job order. |
| `segments[].status` | `ok`: the whole file was processed and has at least one line. `empty`: the whole file was processed and has no line. `damaged`: only a part of the audio was processed; the lines of that part are in the transcript. `failed`: nothing was processed. A segment that was not processed whole is never `ok` or `empty`. |
| `segments[].reason` | `null` for `ok` and `empty`; `code` and `message` for `damaged` and `failed`. The message is a fixed text for the code. |
| `segments[].audio_ms` | Length of the audio that was decoded, with the silence that filled timestamp gaps. Decoding stops at a limit, so this is at most the limit. |
| `segments[].processed_ms` | The lines cover the audio from the start of the file to this point. It equals `audio_ms`, except when the processing time limit stopped the segment: then audio was decoded that has no lines. |
| `segments[].speakers` | `not_applicable` (microphone), `not_requested`, `separated` (every line has a label), `unassigned` (at least one line has no label) or `failed` (no line has a label). |
| `segments[].speakers_reason` | `null`, or `code` and `message` for `unassigned` and `failed`. |
| `glossary` | The terms that were given to Whisper (`used`) and those that did not fit in the token limit (`cut`). `used` followed by `cut` is the job's glossary. |
| `speaker_separation.speakers` | Every speaker label of the transcript with the participant whose shared audio it belongs to. |
| `engine` | Worker version, device, every model (role, name, version, licence, checksum of the weights), every library version, and the settings in use. |
| `timing` | Total time, audio and speech length, time per stage, peak memory of the process. This is the only part that differs between two runs of the same job. |

Reason codes of `segments[].reason`:

| Code | Status | Meaning |
|---|---|---|
| `file_missing` | failed | The audio file does not exist. |
| `not_a_regular_file` | failed | The path is not a regular file inside the audio root, or a part of it is a link. |
| `decode_failed` | failed | Nothing could be decoded. |
| `not_one_audio_stream` | failed | The file has no audio stream or more than one. |
| `processing_failed` | failed | A processing step failed. The message adds the type of the error. |
| `decode_incomplete` | damaged | A packet could not be read or decoded. The lines cover the audio before it. |
| `timestamp_gap` | damaged | A gap between packet timestamps is over the limit. The lines cover the audio before the gap. |
| `timestamp_backward` | damaged | A packet timestamp is more than 50 ms earlier than the end of the packet before it. The lines cover the audio before it. |
| `segment_duration_limit` | damaged | The audio is longer than the limit for one segment. The lines cover the audio up to the limit. |
| `job_duration_limit` | damaged or failed | The audio of the job reached the limit for one job. The segment that reached it is damaged; later segments are failed. |
| `time_limit` | damaged or failed | The processing time limit was reached. The segment in work is damaged; later segments are failed. |

Reason codes of `segments[].speakers_reason`: `no_voice_window` (a line has no speech that is long enough for a voice vector), `speaker_window_limit` (the limit of voice windows was reached; later lines have no label), `time_limit` (the processing time limit was reached before the speakers were separated), `embedding_failed`, `clustering_failed`, `segment_failed`.

A damaged or failed segment does not fail the job: the other segments are transcribed and `run` exits with 0. A job in which every segment failed still gives a transcript with no lines.

Speaker labels: the voices in all `shared` segments of one participant are clustered together, so one voice has one label in every segment of that participant's shared audio. Labels are numbered over the whole recording in the order in which the speakers first appear, so a label names one speaker in the recording. Microphone segments are never clustered.

## Processing rules

1. **Voice detection first.** The worker computes the speech probability of each 32 ms frame and builds the speech clips itself: speech ends after 300 ms without speech, each clip gets 400 ms of padding on both sides (two clips that are closer meet in the middle), and a clip is at most 29 s long. Only clips go to Whisper (ADR 0613).
2. **Language.** With `auto`, each clip gets its own language detection. With a fixed language, every clip uses it.
3. **Glossary.** The terms are joined into one text, tokenised with the tokenizer of the Whisper model, and cut to the longest prefix that fits in 200 tokens. The text is given to Whisper with every clip, through the `hotwords` option of faster-whisper (ADR 0613).
4. **Speaker separation**, for `shared` segments when the job asks for it: 2.5 s windows every 1.25 s over the detected speech, one TitaNet embedding per window, average-linkage clustering on cosine distance with the threshold 0.6. A line takes the speaker with the most time inside it; a line that overlaps no voice window gets no label (ADR 0614).
5. **Long files.** A file is read in blocks of 30 minutes. The state of the voice detector, the speech that has not ended, and the audio that a later clip needs for its padding go from one block to the next. Clips wait until 64 of them exist and go to Whisper as one group. The clips, the groups and the voice windows are therefore the same for every block length, and so is the transcript.
6. **Timestamp gaps.** When the timestamp of a packet is more than 50 ms later than the end of the audio before it, the worker inserts silence of that length, so the audio after the gap keeps its time. A single gap over one hour (`--max-gap-seconds`) ends the segment as `damaged`.
7. **Timestamps that go back.** A packet whose timestamp is more than 50 ms earlier than the end of the packet before it ends the segment as `damaged` (`timestamp_backward`). The worker does not guess which audio is right.
8. **Damaged audio.** A packet that cannot be read or decoded ends the segment as `damaged`. The worker does not skip the packet, because the audio after it would have a wrong time.

### Determinism

The same job with the same model files, library versions, device, compute type, thread count and batch size gives the same transcript, except for `timing`. This was observed for repeated runs on the GPU and on the CPU (see Tests). Decoding uses beam search with temperature 0 and no sampling; clustering has a fixed order; no state of one Whisper call reaches the next call. The length of the blocks in which a file is read does not change the transcript.

It does not hold across devices or compute types: the GPU (`float16`) and the CPU (`int8`) can give different punctuation or words. It is not guaranteed across library versions, thread counts or batch sizes. CTranslate2 and ONNX Runtime do not document bit-exact results for GPU kernels; no difference between two runs was observed. A transcript that reached the processing time limit depends on the speed of the machine.

## Limits

Every limit has an option of `run` and an environment variable. A value must be at least 1.

| Limit | Option | Environment variable | Default | When it is reached |
|---|---|---|---|---|
| Size of the job file | `--max-job-bytes` | `JUNTO_TRANSCRIBE_MAX_JOB_BYTES` | 4 194 304 (4 MiB) | The job is refused (exit code 2). |
| Segments in a job | `--max-segments` | `JUNTO_TRANSCRIBE_MAX_SEGMENTS` | 2000 | The job is refused (exit code 2). |
| Decoded audio of one segment | `--max-segment-seconds` | `JUNTO_TRANSCRIBE_MAX_SEGMENT_SECONDS` | 21 600 (6 hours) | Decoding stops. The segment is `damaged` (`segment_duration_limit`) with the lines up to the limit. |
| Decoded audio of the job | `--max-job-seconds` | `JUNTO_TRANSCRIBE_MAX_JOB_SECONDS` | 86 400 (24 hours) | The segment that reaches it is `damaged`, later segments are `failed` (`job_duration_limit`). |
| Processing time, counted after the models are loaded | `--max-processing-seconds` | `JUNTO_TRANSCRIBE_MAX_PROCESSING_SECONDS` | 43 200 (12 hours) | The segment in work is `damaged`, later segments are `failed` (`time_limit`), and speaker separation that was not done is reported (`speakers_reason` `time_limit`). The limit is checked before each packet that is decoded, before each group of clips for Whisper, before each speaker embedding and before the clustering of each participant's shared audio. One Whisper call for at most 64 clips, or one clustering of at most 4000 windows, can run past it. |
| One timestamp gap | `--max-gap-seconds` | `JUNTO_TRANSCRIBE_MAX_GAP_SECONDS` | 3600 (1 hour) | The segment is `damaged` (`timestamp_gap`). Inserted silence counts as decoded audio. |
| Voice windows with a speaker embedding, per job | `--max-speaker-windows` | `JUNTO_TRANSCRIBE_MAX_SPEAKER_WINDOWS` | 60 000 (about 20 hours of speech in shared audio) | Later lines of shared audio get no label; their segments are `unassigned` (`speaker_window_limit`). |

The limits bound the work, not only the result. The decoder stops when it has given out the allowed samples: it decodes at most one frame beyond them to learn that more audio follows, and it inserts silence for a timestamp gap only up to the limit. A job of many segments that name one long file decodes no more than the limit for the job.

Limits of the format, in the schema: 10 000 segments (the highest value that `--max-segments` can have an effect for), 2000 glossary terms of at most 200 characters, 1000 characters for a path, 200 for an identifier or a name.

## Resources

- One process runs one job, one segment after the other. The server must not start two workers on one GPU unless the video memory allows it.
- GPU when one is present (`--device auto`), CPU otherwise. Speaker embeddings always run on the CPU.
- Measured needs with `large-v3-turbo` are in the table under Tests. Plan for 4 GB of memory and 4 GB of video memory per worker. `docker/compose.yaml` limits a worker to 6 GB of memory, 4 CPUs and 512 processes (`JUNTO_WORKER_MEMORY`, `JUNTO_WORKER_CPUS`).
- Without a GPU, `large-v3-turbo` transcribes dense speech at 0.2 to 0.4 times real time (6 threads of a loaded 8-core machine). Silence costs almost nothing, because only detected speech reaches Whisper.
- Memory does not grow with the length of an audio file: a file is read in 30-minute blocks (115 MB of samples per block), at most about one minute of earlier audio is kept for clips that are not final yet, and at most 64 clips (each at most 29 s, 122 MB together in the worst case) wait for their Whisper call.
- Memory for speaker separation grows with the speech in shared audio and is bounded by the limit of voice windows: 60 000 vectors are 46 MB. Clustering needs one more copy of the vectors of one participant, a square matrix of at most 4000 windows (128 MB), and batches of 4096 vectors. Above 4000 windows of one participant's shared audio (about 80 minutes of speech), an evenly spaced subset is clustered and the other windows take the nearest speaker.
- The transcript is built in memory. Its size is bounded by the limits on audio length.

## Models

The model files are not in the image and not in the repository. An administrator installs them once, as a user that is not the worker's user:

```sh
mkdir -p /srv/junto/models
docker run --rm --user "$(id -u):$(id -g)" -v /srv/junto/models:/models junto-transcription-worker:cpu models fetch --dir /models
docker run --rm --network none -v /srv/junto/models:/models:ro junto-transcription-worker:cpu models verify --dir /models
```

`models fetch` needs network access. It downloads from the addresses in the manifest (`src/junto_transcribe/manifest.json`): Hugging Face at a fixed revision for Whisper, a GitHub release for the speaker model. No account or token is needed. A server without Internet access gets the same files copied by hand; `models verify` checks them.

What `models fetch` does to be safe on a shared host:

- It locks the model directory for the whole fetch. A second `models fetch` on the same directory waits. Files that an interrupted fetch left are removed only by the fetch that holds the lock.
- It writes only into directories that are real directories (not links), belong to the user who runs it, and cannot be written by other users. It refuses any other directory.
- It downloads into a new file that it creates exclusively, with a random name, inside that directory, and renames it only when size and checksum match. It never writes through an existing file or link, and it stops when a file of that name exists. It refuses a destination that is a link.
- It leaves every file with mode `0444` and every directory with mode `0555`.

### What the worker checks, and what it cannot check

- The manifest lists every file that a loader reads. A model directory may contain the listed files and nothing else, because faster-whisper also reads optional files that it finds there (`preprocessor_config.json`). An extra file, a link or a subdirectory refuses the start.
- The worker refuses to start when it can write to the model directory or to a model file. Mount the directory read-only (`:ro`), and do not run `models fetch` as the worker's user.
- `run` and `self-test` compute the SHA-256 checksum of every model file before the models are loaded, including the voice detector inside the faster-whisper package, and again after they are loaded. A difference at either check refuses the start (exit code 3).

The trust boundary: the libraries load the models by path, so the worker cannot bind the bytes that were loaded to the bytes that were checked. A read-only mount stops the worker from changing the files; it does not stop the host. Whoever administers the host, or any process on the host that can write to the directory, can change a model. The worker detects a file that differs at its first check or at its second check. It does not detect a file that was changed after the first check and restored before the second. Keep the model directory writable only for the administrator.

## Container images

`docker/Dockerfile` builds both images; `docker/compose.yaml` defines them as two services with no network, a read-only file system, and memory, CPU and process limits.

```sh
docker build -f docker/Dockerfile --build-arg VARIANT=cpu -t junto-transcription-worker:cpu .
docker build -f docker/Dockerfile --build-arg VARIANT=gpu -t junto-transcription-worker:gpu .
```

The worker is the last stage of the definition, so these commands build the worker image. It contains no test tool and no development package. The stage `test` adds them and is built only with `--target test`.

Run with no network, a read-only file system, the models read-only and one directory for the job:

```sh
docker run --rm --network none --read-only --tmpfs /tmp --cap-drop ALL --security-opt no-new-privileges:true \
  --memory 6g --cpus 4 --pids-limit 512 \
  -v /srv/junto/models:/models:ro -v /srv/junto/jobs/0198c3f2:/jobs \
  junto-transcription-worker:cpu run /jobs/job.json --out /jobs/transcript.json

docker run --rm --network none --gpus all --read-only --tmpfs /tmp --cap-drop ALL --security-opt no-new-privileges:true \
  --memory 6g --cpus 4 --pids-limit 512 \
  -v /srv/junto/models:/models:ro -v /srv/junto/jobs/0198c3f2:/jobs \
  junto-transcription-worker:gpu run /jobs/job.json --out /jobs/transcript.json
```

- Both images run as the user `junto` (uid 10001), not as root. The job directory must be writable for that user.
- Sizes as built on x86-64: 715 MB for the CPU image and 4.5 GB for the GPU image, of which 2.2 GB are NVIDIA's libraries.
- **`--network none` is required.** The worker needs no network for `run`, `models verify` and `self-test`. Only `models fetch` is run with a network, by an administrator.
- Telemetry is off in two ways. By version: `onnxruntime` is pinned to 1.28.0, the last version without the telemetry client that 1.29 and later contain; the worker refuses to start with another version. By environment: the images set `ORT_DISABLE_TELEMETRY=1`, `HF_HUB_DISABLE_TELEMETRY=1`, `HF_HUB_OFFLINE=1` and `DO_NOT_TRACK=1`, and the worker sets the same four variables itself when its package is imported, before it loads a library.
- The GPU image needs the NVIDIA driver and the NVIDIA Container Toolkit on the host. It contains NVIDIA's cuBLAS and cuDNN, which are proprietary; see `THIRD-PARTY.md`.
- On a host where the GPU device nodes belong to a group (`ls -l /dev/nvidiactl` shows `crw-rw----`), the container's user needs that group: add `--group-add "$(stat -c %g /dev/nvidiactl)"`, or set `JUNTO_GPU_GROUP` for `docker/compose.yaml`. Without it the worker refuses to start and names the device; it does not fall back to the CPU without being asked (`--device cpu`).
- Every Python package comes from `uv.lock` with its hash. PyAV is compiled in the image against an FFmpeg build that is LGPL-only; see `THIRD-PARTY.md`.

## Development

```sh
uv sync                      # Python 3.12 and every locked package
uv run ruff check src tests
uv run ruff format --check src tests
uv run mypy
uv run pytest                # unit tests; the model tests are skipped without models
```

On a machine where other work compiles, run the heavy commands through `tools/serial.sh`.

The model tests refuse a model directory that the developer can write to, like the worker. After `models fetch` the files are read-only; a directory that was filled by hand needs `chmod -R a-w`.

Upgrading a pinned library: change the version in `pyproject.toml` and in `junto_transcribe/runtime.py`, run `uv lock`, and run all test tiers. `tests/test_runtime.py` fails when a native library of `onnxruntime`, `sherpa-onnx` or `ctranslate2` contains the telemetry client. Check again which files the new version of a loader reads, and list them in the manifest.

## Tests

| Tier | Marker | Needs | Content |
|---|---|---|---|
| Unit | none | nothing but the locked packages | Clip building from detector output (also compared with the library function that the technical test measured), spans and padding found piece by piece, equality of clips, lines and speaker labels for every block length, timeline arithmetic over segments with offsets and a reconnect gap, glossary cutting, control tokens in the glossary (stand-in tokenizer), clustering (compared with SciPy), speaker-label stability with synthetic embeddings, both schemas, failure isolation per segment, damaged and unassigned segments, timestamp gaps and timestamps that go back, files with several audio streams, directories pinned before the models load, every limit and the decoding work it allows, the deadline in speaker work, what a job file may contain, logs and start errors without values of the job or paths, model verification, read-only models, downloads with a stand-in server, the fetch lock, the image definition, telemetry switches and pins, audio decoding, exit codes. |
| Model | `models` | The model files and generated speech | Word error rate in English and Brazilian Portuguese, near-silent files, two sentences in ten minutes of noise, glossary effect, glossary over the limit, control tokens with the tokenizer of the model, mixed-language speech, three voices in two shared segments, the same job twice, a job read in blocks of three lengths (the transcript must be the same), the voice detector in pieces, a recorder file with lost pages, clips on speech compared with the library function, Ogg/Opus against WAV on the timeline, the command line with a failed segment and its output scanned for values of the job, `models verify`, `self-test`. |
| GPU | `gpu` | A CUDA device | The GPU is used when it is asked for. |
| Offline | `offline` | A container without network | The container has no network interface except loopback. Every model test also fails if Python code opens a connection. |

The model tests read `JUNTO_TRANSCRIBE_MODELS`, `JUNTO_TRANSCRIBE_TEST_AUDIO`, `JUNTO_TRANSCRIBE_WHISPER_MODEL`, `JUNTO_TRANSCRIBE_DEVICE` and `JUNTO_TRANSCRIBE_TEST_PROFILE` (`quick` uses the first sentences of each text, `full` uses all). They are skipped when the models or the speech are absent; with `JUNTO_TRANSCRIBE_TEST_REQUIRE=1` they fail instead.

The speech is generated, not recorded: `tests/speech/generate.py` synthesises invented sentences with Piper 1.2.0 in its own image and writes each file as WAV and as 48 kHz stereo Ogg/Opus, the format of the recorder. The tests read the Ogg files. No recording of a real person is used, and no audio is in the repository.

The speech is reproducible. Piper adds random variation to every sentence. The generator sets the random seed of ONNX Runtime from a seed number (default 1), the voice and the text before it loads the voice for each sentence, and ffmpeg writes without run-specific values. The same seed gave the same 54 files in three runs on the test machine; their checksums are recorded in `tests/speech/SHA256SUMS`, and `prepare.sh` compares the generated files with them. On another processor type the files can differ; the summary that the model tests write says whether the speech had the recorded checksums. `generate.py --seed N` makes another generation of the same sentences, and `--seed 0` sets no seed.

On a machine with Docker (and a GPU for the GPU tier), from `workers/transcription`:

```sh
sh tests/acceptance/prepare.sh /var/tmp/junto-worker-test cpu gpu   # images, voices, speech, models
JUNTO_TEST_PROFILE=full  sh tests/acceptance/run.sh /var/tmp/junto-worker-test gpu -m "models or gpu or offline"
JUNTO_TEST_PROFILE=quick sh tests/acceptance/run.sh /var/tmp/junto-worker-test cpu -m "models or offline"
```

`prepare.sh` runs the downloads and the speech generation as the calling user, so the worker's user can read the models and cannot change them. `run.sh` starts the test image with `--network none`, a read-only file system, and `--cpus 6 --memory 10g`.

### Results

Observed on 2026-10-09 with generated speech. Synthetic speech is clean: real recordings will have higher error rates. The numbers show that the worker reproduces the results of the technical test; they do not predict accuracy on real meetings.

Acceptance: `large-v3-turbo` on an NVIDIA GPU with 8 GB (`float16`, batch size 8), GPU image, `--network none`, `--cpus 6 --memory 10g`, full profile, speech with the recorded checksums. All 27 tests of the model, GPU and offline tiers passed.

| Check | Result |
|---|---|
| Word error rate, English (328 words) | 0.9 % with `auto` and with the fixed language |
| Word error rate, Brazilian Portuguese (343 words) | 0.9 % with `auto` and with the fixed language |
| Mixed English and Portuguese (18 sentences, 219 words) | 2.7 %; the language of 22 of 22 lines is the language of the sentence |
| Near-silent files (10 minutes of brown noise at −62 dBFS, 10 minutes of pink noise at −45 dBFS) | 0 lines |
| Two sentences in 10 minutes of noise | Exactly 2 lines, in English and Portuguese, no wrong word, within 0.7 s of the reference times |
| Glossary of 12 invented terms (67 tokens), terms spelled right | English 2 of 23 without and 20 of 23 with the glossary; Portuguese 7 of 23 and 19 of 23 |
| Glossary of 232 terms | 47 terms used (200 tokens), 185 cut and reported; 17 of 23 right |
| Control tokens | Each of the 1609 special tokens of the model is found in a glossary term |
| Three alternating voices in two shared segments of one participant | 3 labels; each voice has the same label in both segments; 21 of 21 lines carry the label of their voice |
| The same job twice | Same transcript except `timing` |
| A job read in blocks of 1 s, 7 s and 60 s | Same lines, segment results and speakers as the job read whole (32 lines) |
| An Ogg/Opus file with 30 kB of pages removed from its middle | Status `ok`; the decoded length equals the length of the whole file (119 114 ms); the last line ends within 0.1 s of its reference time |
| Speed, dense speech | 17 to 24 times real time with word times; 14 times with speaker separation |
| Speed, near-silent audio | 170 to 200 times real time |
| Peak memory of the process | 1.8 GB |
| Peak video memory of the process | 3 268 MiB |
| Model verification (each of the two passes) and load | 1.1 s and 1.8 s |

#### Glossary result over several generations of the speech

The number of glossary terms that are spelled right depends on the speech. An earlier state of the generator set no seed, so every preparation made other audio; one preparation gave 20 of 23 in Portuguese and the next gave 17. Two runs without a seed gave different checksums; two runs with the same seed gave the same checksums.

Terms spelled right without and with the glossary, `large-v3-turbo` on the GPU, the two glossary files with 23 occurrences each:

| Generation | English | Portuguese |
|---|---|---|
| seed 1 (the test speech) | 2 → 20 | 7 → 19 |
| seed 2 | 5 → 20 | 9 → 19 |
| seed 3 | 5 → 18 | 7 → 19 |
| seed 4 | 3 → 19 | 5 → 19 |
| seed 5 | 3 → 18 | 6 → 16 |
| seed 6 | 2 → 19 | 7 → 20 |
| seed 7 | 3 → 18 | 7 → 19 |
| seed 8 | 2 → 19 | 5 → 18 |
| no seed, first run | 2 → 20 | 6 → 20 |
| no seed, second run | 3 → 20 | 8 → 19 |

With the glossary: 18 to 20 of 23 in English and 16 to 20 of 23 in Portuguese. "Quorvex" was not spelled right in any generation of the Portuguese file. The quick files (first five sentences, 8 occurrences) over seeds 1 to 8: 5 to 6 of 8 in English and 4 to 6 of 8 in Portuguese with the glossary, 0 to 2 and 2 to 4 without.

The bound of the test is the lowest result minus one term: 17 of 23 (English) and 15 of 23 (Portuguese), 4 of 8 and 3 of 8 for the quick files. The test also requires more terms right with the glossary than without. The bounds for the development model `small` are half of the occurrences and come from no repeated measurement.

The glossary was also measured through both options of faster-whisper on the same audio (one generation without a seed): `hotwords` and `initial_prompt` gave the same terms right and the same word error rate in every case, with the limit 200 and with the limit 223 (ADR 0613).

#### Other tiers

| Tier | Machine | Result |
|---|---|---|
| Unit | macOS, Apple silicon | 359 passed |
| Unit | GPU test image on the x86-64 test machine, `--network none` | 357 passed, 2 skipped (the image's FFmpeg has no muxer to write the two test files with a gap and with two streams) |
| Model, quick profile, Whisper `small`, CPU (`int8`) | macOS, Apple silicon | 24 passed, 3 skipped (GPU, offline, and one that needs the full profile) |
| Model, GPU and offline, full profile, `large-v3-turbo` | GPU image on the test machine, `--network none` | 27 passed |

With `small`, the first word of one test sentence is wrong ("and" for "Can") in the speech of seed 1. faster-whisper called directly on the whole file gives the same text, so the cause is the model and the speech. The test of that sentence requires no wrong word for `large-v3-turbo` only.

The CPU tier with `large-v3-turbo` in the CPU image (quick profile, 6 threads) was last run before the changes that followed the two reviews: 17 passed, 3 skipped, in 38 minutes; speed 0.2 to 0.4 times real time for dense speech; peak memory 3.2 GB. It was not repeated after those changes.

The GPU image was built with the documented command, without a target, and ran as the user `junto` with `--network none --read-only --cap-drop ALL --security-opt no-new-privileges:true`. `run` ended with exit code 0 and one failed segment for a missing file; with `--max-segment-seconds 20` it reported two damaged segments; a job directory that is a link and a glossary term with control tokens ended with exit code 2 and no transcript; an output directory elsewhere received the transcript. In the round before, a model directory that the worker could write to and a read-only model directory with one unlisted file refused the start, and the CPU image ran `models verify` and `self-test`. `models fetch` downloaded and verified the default models (1.7 GB) as another user than the worker's and left them read-only.
