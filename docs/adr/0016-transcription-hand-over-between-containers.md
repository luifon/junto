# 0016. Hand-over of a transcription job between the server container and the worker container

## Status

Accepted

## Context

The server runs the transcription worker as a child process with three paths, reads its log from standard error and its result from the exit code, and ends it with a signal to its process group (ADR 0607). ADR 0607 leaves open how a deployment does this when the worker is a container. In the Compose deployment the worker is its own container without a network, with a read-only file system and another user than the server (ADR 0610, ADR 0003). A container that can start another container needs the Docker socket, which gives it the rights of root on the host. The mounts of a container are fixed when it starts, and the directory of a transcription run is created later, below the directory of its recording.

## Decision

- No container gets the Docker socket. The server container and the worker container share two volumes: the recordings and a spool directory.
- `JUNTO_TRANSCRIBE_COMMAND` is the script `junto-transcribe-handover` of the server image. The server calls it with the arguments of the worker.
- The process of the worker container is the script `junto-worker-loop`. It takes the requests of the spool in the order of their names and runs `junto-transcribe run` for one request at a time. One process still runs one job and exits (ADR 0610).
- The loop and a job have different credentials. The loop runs as root with the capabilities `SETUID`, `SETGID` and `KILL` and with the group of the spool. It runs each job as the user 10001 with the group 10001, without capabilities, without the group of the spool, and with the flag that forbids new privileges.

A request:

- The hand-over writes four kinds of files into the spool, all with a name that starts with the time and a random number: `.request` with the three paths, `.log` and `.result`, which it creates empty, and `.cancel`.
- The loop writes `taken` into the result when it starts the job and `exit <code>` when the job ended, and the standard error of the worker into the log.
- The hand-over passes new bytes of the log on to its standard error, at most 1 MiB per job, and ends with the exit code of the result. It reads the first 4096 bytes of the result only.
- A request leaves the spool only after its result states an exit code. A request that the hand-over gives up stays there with its cancel file, and a later hand-over removes it when its result states an exit code. Time alone removes no request: a paused worker container can continue a job later.
- A result that is larger than 4096 bytes ends the hand-over with the code 75 and a cancel file.
- When no loop wrote `taken` within 60 seconds, the hand-over writes the cancel file and ends with the code 75. The server counts a failed run (ADR 0607).
- A loop that finds a request with `taken` and without an exit code was ended during that job. It writes the exit code 70 and does not run the job again; the server starts another run.
- The variables `JUNTO_TRANSCRIBE_*` of the worker are settings of the worker container, not of the server.
- `JUNTO_TRANSCRIBE_BATCH_SIZE` is a setting of the loop. When it is set, the loop gives it to each job as the option `--batch-size` of the worker; a value that is no whole number from 1 to 64 stops the loop at its start. The worker reads its limits from the environment and has no variable for the batch size.

Stop:

- A stop signal to the hand-over writes the cancel file. A loop that takes a cancelled request does not run it and writes the exit code 143.
- A job runs in a process group of its own. For a cancel the loop sends the group the stop signal, and the kill signal after 8 seconds, which is less than the 10 seconds that the server waits (ADR 0607).
- After every job the loop sends the kill signal to the group of the job and to every other process of the user of the jobs, so that no process of one job is present during the next. A job cannot send a signal to the loop.
- A hand-over that was ended without a signal that it can handle leaves its request. One job runs at a time for all server processes (ADR 0607), so the next hand-over treats every other request as left behind: it cancels an unfinished one and removes a finished one.
- A stop of the worker container ends the job, and the result states its exit code.

Permissions:

- The spool belongs to the user of the server, with the group of the loop (10002) and the mode 0750. The loop reads it and cannot create, replace or remove a file in it. The log and the result are files that the hand-over creates and that the group of the loop may write. A job has no access to the spool: it cannot read a request and cannot write a result. The job writes its log through the file that the loop opened for it.
- The directory of the recordings belongs to the user of the server, with the group of the jobs (10001) and the mode 0750. Before each request the hand-over gives that group read access to the files of the recording that the server's user owns, without write access, and write access to the directory of the run (mode 2770). It follows no link.
- The server has both groups as additional groups, so it writes the requests and reads and deletes what a job wrote.
- The loop refuses a request whose paths are not inside the directory of the recordings.

## Consequences

- The worker container has two writable mounts, the recordings and the spool, where ADR 0610 states one job directory as the only writable mount. What a job can write is bounded by the file modes: the directory of the current run, and its log through the open file. A job cannot write to an audio file or to the spool.
- The first process of the worker container is root with three capabilities, where ADR 0610 states a user that is not root. The worker program, which reads the audio with native libraries, runs without root rights and cannot regain them. The loop reads only the requests of the server.
- The worker reads every recording on the volume, not only the recording of its job. Its container has no network.
- The worker can fill the volume of the recordings through the directory of a run, and can write up to the size of the spool volume into a log. The hand-over passes on 1 MiB of a log.
- A transcript file belongs to the user of the worker. The server reads it and stores its own copy (ADR 0607).
- The server cannot tell a worker container that is not running from a worker that is busy for less than 60 seconds; after that it gets the code 75.
- A restart of the worker container during a job costs one of the three runs of that job (ADR 0607).
- A removal of a recording while its job runs deletes the audio files at once (ADR 0608). The worker then refuses to go on with the exit code 2, within seconds and before the renewal of the server finds the job gone and ends the run; the hand-over reads that end and removes the request. The cancel file ends a job whose files are still there.
- Requests of a time in which no worker container ran stay in the spool, three small files each, until a loop states their end and a later hand-over removes them.
- On a host with a GPU the job gets the group of the device nodes through `JUNTO_WORKER_JOB_GROUPS` (`worker-gpu.yaml`). This was not run.
- A second server process on the same volumes would cancel the job of the first. A deployment runs one server process (ADR 0008).
- The two scripts are tested with two users and a stand-in worker in the server image (`deploy/tests/transcribe-handover.sh`). The recording test of the deployment (ADR 0910) runs the hand-over with jobs that the server started for recordings of the recorder: without model files the worker refuses the start with its code 3 and the transcript fails with `worker_not_ready`; with the model files the worker writes a transcript that the server stores.
- A deployment that runs the worker on the same machine without a container sets `JUNTO_TRANSCRIBE_COMMAND` to the worker and needs none of this. It sets `JUNTO_TRANSCRIBE_BATCH_SIZE` for the server, which gives the batch size to each job as the loop does (ADR 0607). The server container of this deployment does not get that variable: the hand-over script takes the arguments of one job only.
- The batch size decides the memory of a job. Whisper `small` on a CPU needed 2.1 GB with the batch size 8 of the model and 1.1 GB with 1, for 96 seconds of speech in three files, in 140 and 125 seconds. With a memory limit of 1.7 GB and the batch size 8 the job was ended by the limit, and so was the worker container; the server counted three failed runs (ADR 0607).
