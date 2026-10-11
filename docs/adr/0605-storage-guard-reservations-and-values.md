# 0605. Storage guard: reservations, values and the stop at the reserve

## Status

Accepted

## Context

ADR 0006 decides that a recording reserves disk space when it starts, that the number of recordings at one time is limited, that the server keeps a reserve of free space, and that a recording that reaches the reserve is stopped in order. It fixes no values and does not state how a reservation is stored, how free space is read, or how the administrators are warned. The final size of a recording is not known when it starts. Step 0 measured a composite of 1080p at about 20 MB per minute, which exists twice until its pieces are deleted, and an audio file at about 1 MB per minute (ADR 0602). One recording used 2.5 to 3.9 CPU cores of the recorder host (ADR 0602). Two requests that start recordings at the same moment must not both use the same free space. The workspace forbids `unsafe` code.

## Decision

Reservations:

- A reservation is a row with its subject, its kind and its bytes. Recordings are the first kind; exports add theirs.
- A recording reserves 8 GiB when it starts. This is about 3 hours of a composite with five audio files, with the composite counted twice.
- A start is refused with `storage_low` (503) unless the free space is at least the reserve, plus every reservation that exists, plus the new one.
- The check and the new row are in the transaction of the start (ADR 0009), so two starts at the same time cannot both pass on the same space.
- A reservation counts in full for as long as its row exists. The bytes that its recording has written are not subtracted.
- The reservation of a recording ends when the recording fails without a file. For every other recording it stays until the processing of its files has ended (ADR 0606).

Reserve:

- The reserve is 5 GiB of free space on the file system of the recordings.
- A start is refused with `storage_low` while the free space is at or below the reserve, and when the free space cannot be read.
- The controller of recordings (ADR 0604) reads the free space at each pass. At or below the reserve it asks for the stop of the recording, with the reason `storage_reserve`. The recorder ends every job, so the files are complete.

Limit:

- At most 2 recordings are starting or run at one time. A further start gets `recordings_full` (429) with `Retry-After: 30`.
- A recording whose stop is asked for counts until its jobs have ended and have left the room (ADR 0604).

Reading the free space:

- The free space is the space that a process without special rights can write, read with `statvfs` for the directory of the recordings (`JUNTO_RECORDINGS_DIR`).
- The server uses the crate `rustix` for this call. The standard library has no function for it, and the workspace forbids `unsafe`, which the call through `libc` needs.
- On a system that is not Unix the free space cannot be read, and no recording starts.

Warning:

- The server writes a log record of the level `warn` when the free space reaches the reserve, and one of the level `info` when it is above it again.
- The health route (ADR 0012) states `storage_low`: true at or below the reserve, false above it, and `null` on a server without a recorder or when the free space cannot be read.

Values:

- The reserve, the reservation and the limit are limits in the code of the server, as the other limits are. They are not environment variables.

## Consequences

- With the default values a server needs 13 GiB of free space for the first recording and 21 GiB for two.
- A recording that is longer than its reservation is not stopped by the reservation. It is stopped when the free space reaches the reserve.
- The reservations are an upper estimate: space that a recording has already written is counted as used by the file system and as reserved by its row. A server refuses a recording earlier than the free space alone would require.
- An administrator sees the state of the storage in the log and on the health route. The client shows nothing; an administrator who starts a recording gets `storage_low`.
- Uploads (ADR 0408) and exports (ADR 0411) use the same reserve and the same table of reservations. An export reserves the bytes that its file was counted to need, read from the free space of the directory of the exports.
- A change of a value needs a new build of the server until the values become settings.
- What the recorder does when the disk is full was not tested (ADR 0006). The stop at the reserve is meant to act before that point; a recording with five audio files writes about 1.6 GiB per hour, so one pass per second sees the reserve in time.
