# 0502. Screen share with sound

## Status

Accepted, pending step 0

## Context

Recording a shared window with its sound is the main use of screen sharing in Junto. Each operating system offers different ways to capture sound: the sound of one application, or all system sound. A change from one application's sound to all system sound without the sharer's knowledge could send an unrelated private call to the participants. Microphone processing, noise suppression and echo cancellation in particular, damages audio that contains several voices. The LiveKit Rust SDK does not capture system sound or application sound.

## Decision

Requirement:

- Screen share with sound is required on macOS, Windows and Linux in the first version.

Choice of sound:

- The sharer chooses the sound of a share: this application, all system sound, or none.
- A choice that the operating system cannot provide is not offered.
- Nothing is chosen automatically.
- The sharer and the viewers see which choice is active for as long as the share runs.
- The sound of the Junto application itself is always excluded, so a call does not hear itself.

Several shares:

- Several people can share at once.
- Each viewer picks which share to watch and hears the sound of that share.
- A viewer can mute the sound of a share, or change its volume, and keep the picture.

Track:

- Shared sound is its own audio track.
- It is sent at a higher bitrate, without noise suppression and without echo cancellation.

Capture:

- Capture of system sound and of application sound is the project's own code, with one implementation per operating system (ADR 0701).
- Step 0 tests, on each of the three systems, capture of one application alone, capture of all system sound, and exclusion of the application's own sound in both. The result decides which choices each system offers (`docs/step-0/README.md`, test 1).

## Consequences

- The set of sound choices can differ between macOS, Windows and Linux.
- A sharer who starts a share makes one explicit choice of sound each time, including the choice "none".
- The project maintains three implementations of sound capture.
- A shared-sound track uses more bandwidth than a microphone track, because of its higher bitrate.
- The recorder writes shared sound as a separate audio file (ADR 0602), and the transcript labels its lines as shared audio (ADR 0603).
- A failed test in step 0 changes this design or removes a choice or the feature on that system from the first version.
