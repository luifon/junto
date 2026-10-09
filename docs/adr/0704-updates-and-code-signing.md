# 0704. Client updates and code signing

## Status

Accepted, pending step 0

## Context

The client is installed on desktop systems and must be updated without a package store. An update mechanism installs code on the user's machine, so a false or old build offered to it is a way to attack the user. The project does not buy operating-system code signing (`docs/scope.md`), so Windows and macOS treat the application as coming from an unknown publisher. On macOS, an application without a stable identity is asked again for microphone and screen-recording permission after each update, and loses access to its entry in the system key store (ADR 0103).

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
- The install instructions state the consequences:
  - On Windows the application runs only with Smart App Control off, and Windows shows a warning at the first launch.
  - On macOS the user allows the first launch in System Settings.
- Step 0 tests whether signing the builds with the project's own self-signed certificate keeps the macOS identity stable, so that the device key stays readable and the microphone and screen-recording permissions remain after an update (`docs/step-0/README.md`, test 4).

## Consequences

- The integrity of an update depends on the project's own signing keys, not on an operating-system certificate. The set of trusted keys allows a key to be replaced without losing the ability to update.
- A user cannot be moved to an older version by the update mechanism.
- A failed update can be reverted to the previous version.
- The update check is the only connection that the client makes outside its server book (ADR 0007).
- A Windows system with Smart App Control on cannot run the application.
- If the self-signed certificate does not keep the identity stable, macOS asks again for microphone and screen-recording permission after each update, and the device key is stored in a key file (ADR 0103).
- An administrator updates the server by hand (ADR 0005).
