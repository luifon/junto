# 0704. Client updates and code signing

## Status

Accepted, pending step 0

Open: the packaged install by the steps of the install instructions was not run (`docs/step-0/README.md`, test 8). After an update only the permission status was read; a microphone or screen capture after an update was not run. The identity of a signed build across updates is tested (test 4).

## Context

The client is installed on desktop systems and must be updated without a package store. An update mechanism installs code on the user's machine, so a false or old build offered to it is a way to attack the user. The project does not buy operating-system code signing (`docs/scope.md`), so Windows and macOS treat the application as coming from an unknown publisher.

macOS stores each permission of an application together with the designated requirement of its code signature. Step 0 installed three builds of one application one after the other (`docs/step-0/README.md`, test 4):

- With an ad-hoc signature, the designated requirement is the code hash of the build. It changed with every build, and the microphone and screen-recording permissions were lost at every update.
- With a signature from one self-signed code-signing certificate, the designated requirement is the bundle identifier and the hash of the certificate. It was the same for all three builds, and both permissions remained without a prompt.
- The requirement does not refer to a trust chain. The permissions remained with the certificate absent from the machine at every step.
- The signing tool refused a certificate whose validity period had ended.
- A downloaded copy was blocked at the first launch with both kinds of signature.
- No kind of signature kept an item of the system key store readable after an update (ADR 0103).

## Decision

Updates:

- The project's release page is the only source of client builds.
- The client checks for a new version at start and periodically, shows a notice, and updates when the user clicks.
- The download and its version information are verified against the project's signing keys.
- The client trusts a set of keys, so that a key can be replaced.
- The client does not install a version older than the one it runs.
- The update replaces the application in one step and keeps the previous version for a rollback.
- When a server requires a newer client, only that server is unavailable until the update.
- An administrator sees a warning when the server is behind.
- The server is never updated automatically.

Code signing:

- The project uses no paid operating-system code signing.
- Every macOS build is signed with the project's own self-signed code-signing certificate. No build is released with an ad-hoc signature.
- The bundle identifier and the certificate never change. A new certificate, also one for the same key, is a different application for macOS, and so is a different bundle identifier.
- The certificate has a long validity period, because a build cannot be signed after the period has ended. The certificate of the test was valid for ten years.
- The private key of the certificate is kept with the same care as the update signing keys. A program that is signed with it and has the same bundle identifier receives the permissions that a user granted to Junto.
- The certificate is not installed on the user's machine and is not marked as trusted there.
- The install instructions state the consequences:
  - On Windows the application runs only with Smart App Control off, and Windows shows a warning at the first launch.
  - On macOS the first launch of a downloaded copy is blocked. The dialog has no button that opens the application. The user opens System Settings, Privacy & Security, and chooses "Open Anyway".

## Consequences

- The integrity of an update depends on the project's own signing keys, not on an operating-system certificate. The set of trusted keys allows a key to be replaced without losing the ability to update.
- A user cannot be moved to an older version by the update mechanism.
- A failed update can be reverted to the previous version.
- The update check is the only connection that the client makes outside its server book (ADR 0007).
- A Windows system with Smart App Control on cannot run the application.
- On macOS a user grants the microphone and the screen-recording permission once. They remain after each update, for as long as the bundle identifier and the certificate stay the same.
- The certificate cannot be replaced in the way an update signing key can. A change of the certificate makes macOS ask every user again for both permissions.
- The self-signed certificate does not change the first launch: macOS blocks a downloaded copy with it as it does without it.
- The device key on macOS is in a key file, because the system key store does not stay readable after an update (ADR 0103).
- Not tested: an installed build after its certificate has expired, a build with the hardened runtime, a standard account without administrator rights, the confirmation that follows "Open Anyway", and macOS versions other than 26.
- An administrator updates the server by hand (ADR 0005).
