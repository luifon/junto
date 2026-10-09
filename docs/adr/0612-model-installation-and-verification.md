# 0612. Model installation and verification

## Status

Accepted

## Context

The worker cannot download a model at run time; the models are present before it starts (ADR 0007). The model files are large: 1.6 GB for Whisper `large-v3-turbo` and 101 MB for the speaker-embedding model. A model name of a library can be an alias that is redirected to another repository: the alias `large-v3-turbo` of faster-whisper answered with a redirect during the technical test (`spikes/transcription/RESULT.md`, T1). A changed model file changes every transcript and can change what the process does when it loads the file. Every file of the chosen combination is a plain HTTPS download that needs no account. The libraries load a model by its path: the worker cannot hand them the bytes that it checked. faster-whisper also reads optional files when they are in the model directory, for example `preprocessor_config.json`, which changes how audio is prepared. A download that writes to a file name that others can guess can be led to another file through a link.

## Decision

Manifest:

- The worker contains a manifest of every model it accepts: role, name, version, licence, attribution, source, and for every file its name, size, SHA-256 checksum and download address.
- The manifest lists every file that a loader reads. A model directory may contain the listed files and nothing else: an unlisted file, a link or a subdirectory is an error.
- A Whisper download address names the repository and a commit, never an alias or a branch.
- The manifest has Whisper `large-v3-turbo` (default), Silero VAD v6, TitaNet-Large for speaker embeddings, and Whisper `small` for the development test tier.
- The voice detector is a file inside the faster-whisper package. The manifest lists it with its checksum, and it is verified like the other files.

Installation:

- `junto-transcribe models fetch --dir <path>` downloads the files. It is the only code path of the worker that uses the network, and an administrator runs it as a user that is not the worker's user.
- It accepts HTTPS addresses only and refuses a redirect to an address that is not HTTPS.
- It writes only into a directory that is a real directory, belongs to the user who runs the command and cannot be written by other users. It refuses a directory or a destination file that is a link.
- It locks the model directory for the whole fetch. A second fetch on the same directory waits. What an interrupted fetch left is removed only by the fetch that holds the lock.
- It downloads into a new file that it creates exclusively, with a random name, inside the checked directory, and renames it only when the size and the checksum match the manifest. It stops when a file of that name exists. A file that is already correct is not downloaded again.
- It leaves every file and every directory without write permission.
- An administrator without Internet access on the server copies the same files into the directory.
- `junto-transcribe models verify --dir <path>` checks the directory without network access.
- The model files are not in the images and not in the repository.

Verification at start:

- `run` and `self-test` refuse to start when the worker can write to the model directory or to a model file.
- They compute the SHA-256 checksum of every model file before a model is loaded, and again after all models are loaded.
- When a file is missing, unlisted, or differs in size or checksum at either check, the worker does not start. It exits with code 3 and names the file.
- Models are loaded by directory path, with the option that forbids a download.
- The transcript header names each model with its version and the checksum of its weights file.

Trust boundary:

- The worker trusts the administrator of the host. Whoever can write to the model directory on the host can change a model.
- The worker detects a model file that differs from the manifest at its first check or at its second check. It does not detect a file that was changed after the first check and restored before the second.

## Consequences

- A model cannot change without a new release of the worker: a new model or a new revision is a change of the manifest.
- An upgrade of a loader library needs a check of the files that the new version reads.
- Every start reads the model files twice to compute the checksums. One pass took 1.2 s for 1.7 GB on the local disk of the test machine.
- An administrator runs one command with network access before the first transcript, or copies the files. Files that were copied by hand need their write permission removed, or a read-only mount.
- A model directory that is shared with other software cannot hold other files next to the model files.
- The worker depends on the download addresses only while `models fetch` runs. If an address stops working, the files can come from any copy, because the checksum decides.
- A distribution that includes the TitaNet file must keep the attribution that CC-BY-4.0 requires. It is in the manifest, in `THIRD-PARTY.md` and in the output of `models list`.
- Whisper `small` is in the manifest for the fast test tier. Its accuracy for product use was not measured.
- Loading a model from bytes that the worker holds would bind the check to the loaded bytes. faster-whisper accepts model files from memory; the voice detector and the speaker model are loaded by path through their libraries. This was not done.
