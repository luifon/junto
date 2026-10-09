# Step 0: technical tests before product code

Step 0 is a set of eight technical tests. Product code starts after the result of every test is recorded in this file and in the affected ADRs ([ADR 0903](../adr/0903-step-0-and-build-order.md)).

Rules:

- Each test has a stated result that passes or fails.
- A failure changes the design or removes a feature from the first version.
- The results can remove a feature from the first version ([`../scope.md`](../scope.md)). They do not add to it.

How to record a result: tick the box of each check that passed, fill the result table of the test, and set the line "Result" to "passed" or "failed" with the date. When a result changes a decision, change the ADR in the same commit.

## Summary

| Test | Subject | Decides or tests | Result |
|---|---|---|---|
| 1 | Sound capture | Which sound choices each system offers (ADR 0502) | Not recorded |
| 2 | gpui | The dependency revisions; how much a screen reader can use (ADR 0701, ADR 0702) | Not recorded |
| 3 | Recorder | The layout page, the separate audio files, recovery of the job state (ADR 0003, ADR 0601, ADR 0602) | Not recorded |
| 4 | macOS update | Where device keys are stored on macOS; permissions after an update (ADR 0103, ADR 0704) | Not recorded |
| 5 | Network | Calls over TURN on port 443 only; behaviour when the link between the hosts drops (ADR 0004, ADR 0506) | Not recorded |
| 6 | Capacity | The capacity of the media host (ADR 0003) | Not recorded |
| 7 | Diarization component | The diarization component (ADR 0603) | Not recorded |
| 8 | Packaged install | Installation on a clean system of each kind (ADR 0705) | Not recorded |

## Test 1: Sound capture

Subject: capture of sound for a screen share on each of the three systems.

Checks, on each system:

- One application alone is captured.
- All system sound is captured.
- The sound of the Junto application itself is excluded in both.

| System | One application alone | All system sound | Own sound excluded (one application) | Own sound excluded (all system sound) |
|---|---|---|---|---|
| macOS | [ ] | [ ] | [ ] | [ ] |
| Windows 11 | [ ] | [ ] | [ ] | [ ] |
| Linux with PipeWire | [ ] | [ ] | [ ] | [ ] |

Gate:

- A sound choice passes on a system when the capture contains the intended source and does not contain the application's own sound.
- The result decides which choices each system offers. A choice that fails on a system is not offered on that system.
- Screen share with sound is required on all three systems in the first version (ADR 0502). A system on which no choice passes fails the test, and the failure changes the design or removes the feature from the first version.

Result: not recorded.

Notes:

## Test 2: gpui

Subject: the functions that the client needs from gpui, on each system, with one fixed set of dependency revisions.

Checks, on each system:

| Check | macOS | Windows 11 | Linux X11 | Linux Wayland |
|---|---|---|---|---|
| Text input, including an input method | [ ] | [ ] | [ ] | [ ] |
| Text selection | [ ] | [ ] | [ ] | [ ] |
| File drop | [ ] | [ ] | [ ] | [ ] |
| A second window | [ ] | [ ] | [ ] | [ ] |
| Operating-system notifications | [ ] | [ ] | [ ] | [ ] |
| Interface scaling | [ ] | [ ] | [ ] | [ ] |
| Several video tiles | [ ] | [ ] | [ ] | [ ] |
| What a screen reader reads (recorded below) | [ ] | [ ] | [ ] | [ ] |

Dependency revisions:

- [ ] One set of revisions of gpui and of the component library works together on all systems.

| Dependency | Revision |
|---|---|
| gpui | |
| Component library | |

Gate:

- Each check other than the screen reader passes when the function works on that system with the fixed set of revisions.
- The screen reader check has no pass condition. Its result is a description of what a screen reader reads on each system, and it decides how much a screen reader can use in the first version (ADR 0702).
- The dependency check passes when one set of revisions is fixed. If no set works, the result is the decision to fork the component library (ADR 0701).
- A function that fails on a system changes the design or removes the feature that depends on it from the first version.

Result: not recorded.

Notes:

## Test 3: Recorder

Subject: the recorder on the data host, connected to the media server on the public host (the two-host form of ADR 0003).

Checks:

- [ ] The recorder loads the project's own layout page, and the video shows the main share with its sound.
- [ ] The separate audio files are correct across one reconnect of a participant.
- [ ] The separate audio files are correct across one mute of a participant.
- [ ] After the recorder restarts, the job state is recovered.

Gate:

- The layout check passes when the MP4 shows the most recently started share as the main picture and contains the sound of that share and the microphones (ADR 0602).
- The audio file checks pass when each audio track has its own file, a reconnect produces a new file with its own start time, and the files fit one timeline (ADR 0602).
- The recovery check passes when the server has the correct state of the recording after the recorder restarts (ADR 0601).
- A failure changes the recording design or removes a recording feature from the first version.

Result: not recorded.

Notes:

## Test 4: macOS update

Subject: an update of an application that is signed with the project's own self-signed certificate.

Checks, after the update:

- [ ] The device key in the system key store is readable.
- [ ] The microphone permission remains.
- [ ] The screen-recording permission remains.

Gate:

- The test passes when all three checks pass.
- If the device key is not readable, macOS stores the device key in a key file that only the user can read (ADR 0103).
- If a permission does not remain, macOS asks for it again after each update, and the install instructions state this (ADR 0704).

Result: not recorded.

Notes:

## Test 5: Network

Subject: the network paths for calls.

Checks:

- [ ] A call works when the network allows only TURN over TLS on port 443.
- [ ] The behaviour of calls when the link between the hosts drops is recorded below.
- [ ] The behaviour of admission checks when the link between the hosts drops, and when it returns, is recorded below.

Gate:

- The first check passes when a call works on a network that allows only TURN over TLS on port 443 (ADR 0004). A failure changes the design of the network paths.
- The second and third checks record behaviour and have no pass condition of their own. The recorded behaviour is compared with ADR 0506, which states that no admission check runs while the link is down and that the server compares the participants with the people allowed when its connection to LiveKit returns. A difference changes that ADR.

Result: not recorded.

Notes:

## Test 6: Capacity

Subject: the capacity of the media host, measured with test participants while a recording runs.

Checks:

- [ ] The measurement ran with test participants and a running recording.
- [ ] The measured limit is recorded below.

| Participants | Cameras | Screen shares | Recording running | Result |
|---|---|---|---|---|
| | | | | |

Gate:

- The test passes when a call of the size that the tested deployment is intended for, with one screen share, runs on the media host while a recording runs. The intended size and the measured limit are recorded with the result.
- A failure changes the deployment form of ADR 0003.

Result: not recorded.

Notes:

## Test 7: Diarization component

Subject: the optional component that separates the voices inside shared sound.

Checks, for the chosen component:

- [ ] It runs without network access.
- [ ] Its licence terms allow distribution with the stated attribution.
- [ ] Its result on a recording of several voices is usable.

| Component | Version | Licence | Runs without network | Result on several voices |
|---|---|---|---|---|
| | | | | |

Gate:

- The test passes when one component passes all three checks. That component is chosen (ADR 0603).
- If no component passes, the design changes or speaker separation inside shared sound is removed from the first version.

Result: not recorded.

Notes:

## Test 8: Packaged install

Subject: installation of the packaged application on a clean system of each kind.

Checks:

- [ ] macOS 26 on Apple Silicon
- [ ] Windows 11 on x86-64
- [ ] Linux on x86-64, tarball
- [ ] Linux on x86-64, AppImage

Gate:

- A check passes when the packaged application installs and starts on a clean system of that kind, by the steps of the install instructions (ADR 0704, ADR 0705).
- A failure changes the packaging or removes the system or the package from the first version.

Result: not recorded.

Notes:
