# 0407. Deleted channels keep a marker row, and a channel counts its permanent messages

## Status

Accepted

## Context

Audit entries about a deleted channel stay visible to server administrators (ADR 0303), so an entry must still be able to refer to the channel. Only the server owner can delete a channel that contains permanent messages (ADR 0402), and automatic deletion skips such a channel (ADR 0406). Both rules ask whether a channel contains a permanent message. Channels exist from the first build step, before messages (ADR 0903).

## Decision

- Deleting a channel sets a deletion time on its row and removes its overrides. The row stays.
- A deleted channel is in no list and every request for it gets `not_found`.
- Audit entries keep their reference to the row.
- A channel row holds the channel setting "permanent messages" (ADR 0402) and the number of permanent messages that the channel contains.
- The feature that stores messages changes that number in the transaction that adds or deletes a permanent message.
- A deletion asks the permission function with the fact "contains permanent messages" taken from that number. When the number is above zero, only the server owner is allowed.
- The check and the deletion are one serializable transaction (ADR 0009). A change of the channel that commits in between makes the deletion run again from its start.
- A change of a deleted channel is refused with `not_found`.
- Channel names are not unique.

## Consequences

- The name of a deleted channel stays stored and is shown only in audit entries, to administrators.
- The content of a deleted channel is removed by the features that own it (ADR 0406); the marker row is not removed.
- The owner-only rule for deletion exists and is tested before messages exist.
- A permanent message cannot be added to a channel while that channel is being deleted without one of the two operations running again and seeing the result of the other.
- The count must be kept correct by every code path that adds or deletes a permanent message.
