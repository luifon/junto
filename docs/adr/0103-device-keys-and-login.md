# 0103. Accounts, device keys and login

## Status

Accepted, pending step 0

## Context

An account is used from several devices and a person has accounts on several independent servers. Two server operators must not be able to recognise the same person by comparing credentials. A device must prove its identity to the server without sending a secret that another party could use again. On macOS the system key store ties access to a stored key to the identity of the application, and an unsigned application gets a new identity with each update, which would make the key unreadable after an update.

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

Where a device key is stored (decided by step 0):

- macOS: the system key store, if step 0 shows that signing the builds with the project's own self-signed certificate keeps the application identity stable across updates. The fallback is a key file that only the user can read, as SSH uses.
- Linux: the desktop's secret service where one exists, and the same kind of key file otherwise.
- Windows: the Windows credential store.

## Consequences

- There is no password to reset. A person who loses a device adds another device from an existing one or asks the server owner for a recovery (ADR 0104).
- A signed challenge cannot be used a second time, on another server, for another account, by another device or for another operation.
- A person's accounts on two servers have no key in common.
- The storage location on each system is final only after step 0 (`docs/step-0/README.md`, test 4). If the key in the macOS key store is not readable after an update, macOS uses the key file.
- The values of the limits on failed attempts are not fixed by this record.
