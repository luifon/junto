# 0717. Profile preset for a new account: client side, waiting for the server operation

## Status

Accepted

## Context

ADR 0105 states that a new server account starts from the profile preset of the client, and that the avatar is sent after the approval of a join request, because a join request carries a name only. The name already travels with a join, a claim and a join request. The server has no operation yet that sets the name or the avatar of an account, so the avatar cannot be sent. The client must still know for which accounts the preset waits, so that nothing is lost when the operation arrives, and it must not treat a failed try as done.

## Decision

- The server book stores for each server the state of the profile preset: `not_needed`, `pending` with the number of failed tries, or `applied` (ADR 0710).
- The state becomes `pending` when this device creates an account and the preset has an avatar: after a join with a code, after a claim that created the owner account, and after the login that follows an approved join request.
- It does not become `pending` for an account that existed before: after a device link, a recovery, or a claim that returned the owner to an account.
- After each action of the access flow that succeeded, and on request, the client applies a pending preset: it reads the name and the avatar of the preset and gives them to the uploader, for the account that the device has then (ADR 0712).
- The uploader is an interface with one operation: send a profile through the session of an account. The uploader of the product sends nothing and answers that the server does not offer the operation. The state then stays `pending` and no try is counted.
- An uploader that fails for another reason counts one failed try, and the state stays `pending`. An uploader that succeeds sets the state to `applied`. A preset whose avatar was removed meanwhile sets it to `not_needed`.
- The state is written to the book with the same rule as the membership: a write that failed is tried again (ADR 0710).
- When the server gets the operation, the protocol crate gets its types, the session operations of the client get the call, and the uploader of the product uses it. The states that are `pending` then are applied at the next login of each server.

## Consequences

- No avatar is sent today. Every account that this device creates while the preset has an avatar is recorded as waiting.
- The record survives restarts, so the avatar reaches accounts that were created before the server operation existed.
- A user who sets an avatar after joining a server does not get it applied there by this rule. Editing the profile per server is the later feature of ADR 0105.
- The rule that decides `pending`, the counting of failed tries and the change to `applied` are tested with an uploader of the tests. The call to a server is not tested, because it does not exist.
