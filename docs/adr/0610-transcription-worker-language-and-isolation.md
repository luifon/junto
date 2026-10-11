# 0610. Transcription worker: language, process and isolation

## Status

Accepted

## Context

The transcription worker runs voice detection, Whisper and an optional speaker separation (ADR 0001, ADR 0603). The technical test of step 0 chose faster-whisper on CTranslate2, Silero VAD and sherpa-onnx speaker embeddings (`spikes/transcription/RESULT.md`). faster-whisper is a Python library, and the test measured it through its Python interface. The worker reads a job file and audio files that another program wrote, and the libraries it loads are large third-party programs with native code. One job must not be able to use unlimited time or memory. The worker must run without outbound network access, and reporting that is built into a model library must be off (ADR 0007). `onnxruntime` 1.29 and later contains a telemetry client that is on by default. The binary package of PyAV, a dependency of faster-whisper, contains GPL libraries (ADR 0801). A transcript and a glossary are private data, and logs are often collected by other systems.

## Decision

Language:

- The worker is written in Python 3.12. It is the only component of Junto that is not written in Rust.
- Its code is in `workers/transcription/` and is not a member of the Cargo workspace.
- The code has type hints throughout and passes `mypy --strict` and `ruff`.
- The packages are locked with hashes in `uv.lock`. The libraries that the technical test measured are pinned to exact versions: faster-whisper 1.2.1, CTranslate2 4.8.2, onnxruntime 1.28.0, sherpa-onnx 1.13.8.

Process:

- One process runs one job and exits. The command is `junto-transcribe run <job.json> --out <transcript.json>`.
- The worker gets its work through files only: a job file, audio files and a model directory. It opens no socket and has no server part.
- A job runs one segment after the other. The worker uses a CUDA device when one is present and the CPU otherwise.
- When an NVIDIA device node exists and the user of the process cannot open it, the worker refuses to start. It uses the CPU on such a host only when the CPU is asked for.
- A model error that makes further calls unreliable, for example video memory that ran out, stops the process with its own exit code and no transcript.
- No state of one Whisper call is kept for the next call. The worker creates a new batched pipeline object for each call, because the library keeps the time of the last speech in that object and resets it only when a call ends without an error.

Limits:

- Every limit is a setting with a default, an option of `run` and an environment variable: the size of the job file (4 MiB), the number of segments (2000), the decoded audio of one segment (6 hours) and of one job (24 hours), the processing time after the models are loaded (12 hours), one gap between packet timestamps (1 hour), and the number of voice windows that get a speaker embedding (60 000).
- The job file is read up to its byte limit and must be a regular file. Validation stops after five errors.
- The limits bound the work and not only the result. The decoder stops when it has given out the allowed samples; it decodes at most one frame beyond them, and it inserts silence for a timestamp gap only up to the limit.
- The time limit is checked before each packet that is decoded, before each group of clips for Whisper, before each speaker embedding, and before the clustering of each participant's shared audio.
- A segment that reaches an audio limit or the time limit keeps the lines up to that point and is reported as damaged. Segments after it are reported as failed, and speaker separation that was not done is reported. The job still gives a transcript.
- An audio file is read in blocks of 30 minutes, and clips go to Whisper in groups of at most 64, so the memory for audio and for audio features does not grow with the length of a file.
- The length of the blocks does not change the result: the state of the voice detector, the speech that has not ended and the audio that a later clip needs go from one block to the next (ADR 0613).
- Speaker vectors are kept until all shared audio of a participant is read. Their number is bounded by the limit of voice windows (46 MB at the default). Clustering works on at most 4000 vectors and assigns the others in batches (ADR 0614).
- The Compose file limits a worker to 6 GB of memory, 4 CPUs and 512 processes by default.

Isolation:

- The worker runs in a container with `--network none`, as a user that is not root, with a read-only file system. The model directory is mounted read-only. The job directory is the only writable mount.
- There are two images from one definition: one for the CPU and one for NVIDIA GPUs. The worker image is the last stage of the definition, so a build without a target gives the worker image. The test tools are in a separate stage that is built only when it is named.
- Only the command `models fetch` uses the network (ADR 0612). Its code is in one module, and a test fails when another module imports a network module.
- The worker opens the directory of the job file, the audio root and the output directory when it reads the job file, before a model is loaded, and without following a link. It reaches the job file, every audio file and the transcript from those descriptors (ADR 0611).

Logs:

- Log lines and error messages contain positions, counts and fixed codes. They never contain a value of the job (identifiers, names, glossary terms, paths) or transcript text.
- A refused job is reported as a location in the job and a code. A location contains array positions and field names that the schema defines; any other key of the job file is shown as its position in its object. A segment is named by its position in the job.
- A refused start is a code, with names from the model manifest where a model file is meant. It contains no path of the host.
- A model that cannot be loaded, or that fails on the device, is reported as a code and the type of the error. The message of a library is not passed on, because it can contain a path or a value.
- An unexpected error is logged with its type and the place in the code, without its message.
- Tests scan the logs of refused jobs, of refused starts, of a whole run and of failures for values of the job and for paths.

Telemetry:

- `onnxruntime` is pinned to 1.28.0, the last version without the telemetry client. The worker refuses to start with another version of a pinned library.
- The images set `ORT_DISABLE_TELEMETRY=1`, `HF_HUB_DISABLE_TELEMETRY=1`, `HF_HUB_OFFLINE=1` and `DO_NOT_TRACK=1`. The worker sets the same variables itself when its package is imported, before it loads a library.
- A test reads the native libraries of onnxruntime, sherpa-onnx and CTranslate2 and fails when one contains the telemetry client.

Licences:

- The images do not install the binary package of PyAV. They compile PyAV from source against an FFmpeg that is built in the image under LGPL-2.1-or-later, with only the audio decoders and containers that the worker reads. The build fails when a GPL, version-3 or non-free option is on.
- `workers/transcription/THIRD-PARTY.md` lists every model, package and bundled native library with its licence, version and source.
- The project publishes the image definitions and no built image.

## Consequences

- A second toolchain exists in the repository. CI needs a Python job next to the Rust jobs, and the licence check of ADR 0801 needs a separate step for the Python packages.
- The worker cannot report progress while it runs, and the server cannot cancel a job except by stopping the container. ADR 0607 decides how jobs are handed over; this record fixes only the files.
- One GPU serves one job at a time per worker process. Several workers on one GPU are possible only when the video memory allows it.
- The memory of a job is bounded by the limits and does not depend on the length of one file. It does grow with the amount of speech in shared audio, up to the limit of voice windows, and with the number of lines, because the transcript is built in memory.
- One Whisper call for at most 64 clips, or one clustering of at most 4000 voice windows, can run past the time limit, because the worker cannot stop a library call. The server has its own time limit for the worker process (ADR 0607).
- A transcript that reached the time limit depends on the speed of the machine, so it is not the same on every run.
- An operator who reads the logs sees which field of a job was refused and why, and does not see the value. Finding the value needs the job file.
- An operator who reads the logs of a model that cannot be loaded sees the type of the error and not the library's text. Finding the cause can need a run of the library by hand.
- The commands `models fetch` and `models verify` read no job. `models fetch` names the directory that the administrator gave and the download address when it fails.
- An upgrade of a pinned library needs the telemetry check and all test tiers again.
- The images read Ogg with Opus or Vorbis, WAV, FLAC and Matroska only. Another audio format needs a change of the FFmpeg build.
- On a host that gives the GPU device nodes to a group, the container needs that group in addition to its user, because the worker does not run as root. This was observed on the test machine.
- The GPU image contains NVIDIA's cuBLAS and cuDNN, which are proprietary. An operator who builds the GPU image downloads them from PyPI. Whether a built GPU image may be redistributed is not verified.
- The CTranslate2 package contains Intel's Math Kernel Library, which is redistributable and not open source.
- Every Python package in an image is checked against its hash in the lock file. The tools that compile FFmpeg and PyAV are not: the compiler comes from the Debian packages of the base image, which is pinned by digest, and the Python build tools are pinned by version.
- A developer who installs the packages outside a container gets the binary PyAV package with the GPL libraries on the development machine. That package is not distributed by the project.
