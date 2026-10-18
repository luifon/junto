# 0910. Recording test of the deployment

## Status

Accepted

Open: the test runs the local form on one machine. It was run on Docker Desktop on arm64 and on no other system, and never on a CI runner.

## Context

ADR 0902 decides that a test participant without a user interface publishes a known tone and a known test picture into a call, that a test records the call and checks the file, and that tests cover a recorder that stops during a recording. Until this record every test of the recording path replaced one part of it. The tests of the server used a recorder in memory and files that ffmpeg generated (ADR 0604, ADR 0606). The tests of the layout page used a browser without a recorder (ADR 0609). The tests of the hand-over used a stand-in worker (ADR 0016). Step 0 ran the recorder with the page and the scripts of its own technical test. No test joined the server, the recorder, the built layout page as the server serves it (ADR 0615), the files on the volume that the containers share, and the worker container.

The local form runs every component on one machine (ADR 0015). A login needs a signature of a device key (ADR 0108), and the command-line client has no command for a call or a recording. The ffmpeg of the server image writes no raw picture (ADR 0014). The storage guard starts a recording only with 13 GiB of free space (ADR 0605). The composite of 1080p used 1.5 to 2.9 CPU cores of the recorder on arm64 (`spikes/recorder/RESULT.md`).

## Decision

The test:

- `deploy/tests/recording.sh` starts the local form with new secrets and small memory limits, runs `deploy/tests/recording_driver.py`, and removes the containers, the volumes and the network. The sum of the memory limits is under 4 GB.
- The driver creates the accounts through the API as a client does: the owner with the owner code of the last start, read from the standard output of the server container alone (`deploy/tests/server_output.py`, ADR 0008), three members with invites. `deploy/tests/junto_api.py` holds the device keys of the run and computes the Ed25519 signature with the Python standard library, so that the test needs no package and no build of a client.
- The participants are processes of `spikes/test-participant` on the machine of the test. Each enters the voice channel through the API and connects with the join token and the address of the answer. Two publish a microphone tone and a camera picture, the third a screen picture with its sound, and that person reports the screen share through the API, so that the server names the main share (ADR 0602).
- The recording counts as running when every segment records and the composite has written its first piece. The first piece is written after the layout page has signalled its start.

Checks of the first recording:

- The recording is `ready` with four segments that ended in order, each with a file that passed the check of the server (ADR 0606).
- The volume holds `<recording>/<segment>.mp4`, one `<recording>/<segment>.ogg` per audio track and no other file of the recorder; the pieces are deleted.
- The video is H.264 with 1920x1080 at 30 frames per second with AAC sound, its length lies between the time from its first piece to the stop and the time from the request to the stop, and it differs from the length that the recorder reported by at most 3 seconds.
- Picture, at 4 seconds and 4 seconds before the end: the main area has the colour of the screen picture in at least 3 of 4 fields; each camera tile shows the eight bars of the camera picture in at least 7 places and the band colour of its participant; the stage beside the tiles is dark; each camera tile has a dark label box at its lower left, and the box of the long display name is at least 2.5 times as wide as the box of the short one; the main tile has light text of a label at its lower left. Over the whole video, the share and each of the two cameras have, at least every 2 seconds, a frame in which a pixel differs from the frame before by more than half of the range; the stage beside the tiles and a name label have no such frame, which shows that the noise of the encoder does not count as a change.
- Sound: the composite holds the three tones of the call, each at least 25 dB above the bands between them; each audio file holds its own tone and neither of the others. This is measured in the first seconds and, in a run without speech, in the last seconds. The composite has sound in its middle.
- Downloads (ADR 0608): 401 without a session; the video with its type, length and the bytes of the file on the volume; a range, a range from the end and a range outside the file; an audio file for a member; 404 for an unknown segment and for a member whose role is denied `view_channel`; 403 for a removal without `manage_recordings`.
- Storage guard (ADR 0605): a reservation of 8 GiB exists while the recording runs and is gone at the final state; the health route states that the storage is not low; the recording states the bytes of its files.
- Removal: the directory of the recording is gone, no file is left below the recordings, the temporary directory of the recorder and the spool of the transcription jobs are empty.

The picture is measured with the ffmpeg of the server container: `crop` with `signalstats` gives the average colour of a part of a frame, `crop` with `bbox` gives the columns of the label box, and `tblend` with `signalstats` gives the largest difference between two frames. The places follow the rules of the layout page (ADR 0609) for a share and two cameras.

Transcript:

- With `JUNTO_RECORDING_TEST_MODELS` and `JUNTO_RECORDING_TEST_SPEECH` the participants switch from their tones to speech files after 9 seconds of the recording. `deploy/tests/recording-prepare.sh` makes both: the generated speech of the worker tests (English, Brazilian Portuguese, and a talk of three voices for the screen share) and the model files for Whisper `small`, the development model of ADR 0612, with the speaker model. The test requires a transcript in the state `ready`; for each track an agreement of 0.8 between the words of its lines and the words of its speech file in their order, and under 0.3 with each other file; the language of each track; no speaker label on a microphone line; and for the shared sound a label on every line, three labels numbered from 1 in the order of their first line, each listed with the person who shared, and one label of its own for each of the three voices of the file on at least 8 of 10 of the lines of that voice (ADR 0614).
- The test sets the batch size of the speech model to 1 (ADR 0016), because the model needs 2.1 GB with its own batch size and the memory limits of the test leave the worker 1.4 GB.
- Without the two settings the worker container has no model file. The test requires that the hand-over reaches the worker, that the worker refuses to start, and that the transcript fails with `worker_not_ready` while the recording stays `ready`.

Failures, each followed by a removal after which no file is left:

- Everybody leaves: two people leave through the API and the third loses the connection. The server ends the call and stops the recording with the reason `call_ended`, and the recording is `ready` with four checked files.
- The recorder is lost: every process of the recorder container is killed during a recording. The container ends, its restart rule starts it again, which the test reads from the restart count of the container, and its start script saves the partial audio files. The four segments fail with `recorder_lost`, four new segments record, and the recording is `partly_failed` with a recovered file for each lost segment (ADR 0601, ADR 0606). The directories below `rescued/` are deleted after the processing.
- The storage guard refuses: a second deployment has a volume of the recordings of 64 MB (`deploy/tests/recording-low-storage.yaml`). A start is answered with `storage_low`, the health route states it, and no recording, no reservation and no file exists.

At its end the first part checks that the recorder reaches the server, the media server and Redis and not PostgreSQL, that the logs of the recorder, the server, the media server and the worker hold no signed token and no address with a token, and it prints the STUN servers that the log of the recorder names (ADR 0020).

When it runs:

- The test is run by hand: before a release, and after a change of the recorder version, the media server version, the layout page, the Compose files, the recorder client of the server or the controller of recordings. It is not part of CI (ADR 0905): a hosted runner has too little free disk space for the storage guard and too few CPU cores for the recorder beside the other parts.
- It needs Docker with Compose, `curl`, `python3`, a build of the test participant, 13 GiB of free disk space for Docker and four free CPU cores.

## Consequences

- The recording path is tested from a published signal to the content of the output file, as ADR 0902 states, with every part of the product in it.
- A video that is black, of one colour, frozen in one of its three pictures, silent, or of another page fails the picture and sound checks. A wrong display name of the same width as the right one passes: the test measures the width of a label and does not read its text. A picture that is wrong only between the two measured frames, and outside the measured places, passes.
- The test does not cover a recorder that stays stopped, a removal while a transcript runs, and a restart of the server during a recording.
- A change of the sizes or places of the layout page needs the same change in the driver, which holds the rules of the page a second time.
- The test found, at its first runs: a manifest file of the recorder beside each audio file; a recording that ended as `partly_failed` when the media server closed the room in the second in which the server ended the call; requests of the recorder to public STUN servers in the local form; rights of the recorder container that it does not need (ADR 0604, ADR 0020).
- Measured in three runs in a row on an arm64 machine with 8 cores, with Docker Desktop and other work on the machine: the composite of 63 seconds had 23.5 to 24.6 MB and each audio file 0.8 to 1.0 MB. The recorder container used 833 to 838 MiB without the cache of the files that it wrote, and 936 to 952 MiB at its peak, under a limit of 1280 MiB. Every source recorded 8 to 10 seconds after the request, and the recording was `ready` 23 to 26 seconds after the stop. The transcript of 96 seconds of speech was ready after 81 to 85 seconds. After the kill the recorder container ran again in under 1 second, the server declared its jobs lost after 23 seconds, and new jobs recorded every source after 30 seconds. One run takes 8 to 9 minutes.
- The speech is synthetic and short, and the model is the development model. The test shows that the audio of the recorder reaches the worker and that the transcript is stored with its labels. It does not measure accuracy.
- The transcript part needs downloads: four test voices (250 MB) and the model files (585 MB). They are not in the repository and not in an image.
- The picture and the sound are checked for a recording of about one minute. A long recording, a recording with many tiles, and real screen content are not covered.
- A machine with other work fails the test without a defect when the recorder gets too little CPU: the file then has holes in its sound (ADR 0602), and the check of the server reports them.
