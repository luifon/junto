# 0118. A recovery notice stays until its device acknowledges it

## Status

Accepted

## Context

A recovery gives a new device access to an account, and each device of the account must show it to the person (ADR 0104). ADR 0112 removed the notice of a device when that device logged in. A client logs in without an action of the person, and it can stop between the login and the display of the notice. The notice was then gone and the person never saw the recovery. The stream closes a connection with the reason `recovered` and the recovery (ADR 0208), and the notice list could no longer contain that recovery.

## Decision

- A login does not remove a notice. The response to a login carries every recovery that the device has not acknowledged.
- `GET /api/v1/account/recovery-notices` returns the same list for the calling device, oldest first.
- `DELETE /api/v1/account/recovery-notices/{recovery_id}` acknowledges one recovery for the calling device. The client calls it after it has shown the recovery to the person.
- An acknowledgement removes the notice of the calling device only. The other devices of the account keep theirs.
- An acknowledgement of a recovery that the device has no notice of gets `not_found`.
- The recovery in the `ended` message of the stream is one of the recoveries in the notice list of that device, until the device acknowledges it.

## Consequences

- A device that stops before it has shown a recovery shows it at its next start.
- A device that never acknowledges keeps its notices. Their number is the number of recoveries of the account.
- The list of all recoveries of the account (`GET /api/v1/account/recoveries`) is unchanged and does not depend on acknowledgements.
- A notice row is deleted with its device, as before.
