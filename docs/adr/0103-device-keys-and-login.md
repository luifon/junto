# 0103. Accounts, device keys and login

## Status

Accepted, pending step 0

Open: the key file on Windows is not tested, because no Windows test of step 0 has run. The storage on macOS is decided (`docs/step-0/README.md`, test 4).

## Context

An account is used from several devices and a person has accounts on several independent servers. Two server operators must not be able to recognise the same person by comparing credentials. A device must prove its identity to the server without sending a secret that another party could use again.

On macOS an item in the system key store is tied to the binary that created it: each item records the code hash of that build. An updated build has another code hash, so the read is refused and macOS asks the user for the password of the login keychain. Step 0 observed this for a build with an ad-hoc signature and for a build signed with the project's own self-signed certificate, with the certificate present on the machine and absent. The second key store of macOS, the Data Protection keychain, refused both builds with the error for a missing entitlement. The builds have no entitlements, because the project uses no paid operating-system code signing (ADR 0704). A key file with mode 0600 was readable by every build.

## Decision

Accounts and device keys:

- An account has its own identifier and a list of device keys.
- The client generates a separate device key for each server, so two server operators cannot recognise the same person by key.
- There are no passwords.

Login:

- The server sends a challenge and the device signs it with its device key.
- A challenge is used once and expires quickly.
- A challenge names the server, the account, the device and the operation it is for.
- The number of failed attempts is limited.

Where a device key is stored:

- On every system the device key is in a key file that only the user can read, as SSH uses. On macOS and Linux the file has mode 0600 and is in a directory with mode 0700 under the user's application data directory. On Windows the file and its directory have an access list that names only the user.
- The first version has this one store on all three systems, behind one interface. Reason: one code path is tested and behaves the same everywhere. A system key store ties an item to the build that created it on macOS, and a secret service is not present on every Linux desktop.
- A system secret store can be added later behind the same interface. On Linux, a running secret service stored, read and deleted a secret in 0.115 seconds without a prompt (`docs/step-0/README.md`, test 2).

## Consequences

- There is no password to reset. A person who loses a device adds another device from an existing one or asks the server owner for a recovery (ADR 0104).
- A signed challenge cannot be used a second time, on another server, for another account, by another device or for another operation.
- A person's accounts on two servers have no key in common.
- On macOS the device key stays readable after an update, and no prompt appears.
- A key file is protected by its file permissions, and by the disk encryption while the machine is off. Another user account cannot read it. Any program that runs as the same user can read it: the file is not tied to the application, and no prompt appears. It is part of a backup of the user's files unless it is excluded.
- The values of the limits on failed attempts are not fixed by this record.
