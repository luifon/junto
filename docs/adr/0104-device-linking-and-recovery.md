# 0104. Device linking and recovery

## Status

Accepted

## Context

An account has no password (ADR 0103), so a new device gets access only when an existing key or a privileged person approves it. A client holds a separate device key for each server, so a person with several servers would have to repeat a linking procedure once per server. Giving a new device access to an existing account gives it access to that account's direct messages, which no administrator can otherwise read (ADR 0302).

## Decision

Devices:

- A second device is added with a short-lived one-time code.
- The approval names the public key of the new device.
- One flow links all the servers of the first device: the new device creates a device key per server, and the first device approves each one.
- A user can list the devices of the account and remove one. Removing a device ends the sessions of that device.

Recovery:

- Only the server owner can give a new device access to an existing account.
- Each recovery is written to the audit log (ADR 0303).
- The other devices of the account receive a notice of the recovery.
- The user sees the date of the recovery at the next login.
- When the server owner has lost access, the server command line assigns a new server owner (ADR 0201).

## Consequences

- A person who has lost every device of an account depends on the server owner to return to it.
- The server owner can give a device access to any account, including its direct messages. Each use is recorded and is shown to the account.
- An administrator who is not the server owner cannot perform a recovery (ADR 0301).
- Linking a device needs the first device at hand, and the first device approves one device key per server.
- A removed device cannot log in again without a new link or a recovery.
