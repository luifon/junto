# 0717. Profile preset for a new account: the client sends the avatar once

## Status

Accepted

## Context

ADR 0105 states that a new server account starts from the profile preset of the client, and that the avatar is sent after the approval of a join request, because a join request carries a name only. The name travels with a join, a claim and a join request. The server stores an avatar through `PUT /api/v1/account/avatar` and refuses an image of more than 256 KiB, of more than 1024 by 1024 pixels, or of another format (ADR 0119). The client must know for which accounts the avatar still waits, must not treat a failed try as done, and must not send an image again that the server refuses every time.

## Decision

- The server book stores for each server the state of the profile preset: `not_needed`, `pending` with the number of failed tries, `applied` or `refused` (ADR 0710).
- The state becomes `pending` when this device creates an account and the preset has an avatar: after a join with a code, after a claim that created the owner account, and after the login that follows an approved join request.
- It does not become `pending` for an account that existed before: after a device link, a recovery, or a claim that returned the owner to an account.
- A claim creates the owner account only when its session tells no recovery. A claim that returned the owner to an existing account leaves a recovery notice for the new device (ADR 0114). This rule is for a claim only: the account of a join or of an approved join request is new also when its first login tells a recovery.
- An operation whose answer was lost is resolved with a login (ADR 0712). The account of that login counts as created when the operation was a join, or a claim by the rule above. A request for a link creates no account.
- When the server answers an operation with `key_already_registered`, the key had its account before. The account counts as created only when the book holds a join request of this device and the server states that this request was approved into the account of the login.
- The state belongs to the account of the membership in the book. When the device has another account or no account, the state becomes `not_needed`, also from `applied` and `refused`. The avatar is therefore never sent to an account that the device got later through a link or a recovery, and an account that the device creates after a removal gets the avatar again.
- After each action of the access flow that succeeded, and on request, the client applies a pending preset: it reads the avatar of the preset and stores it as the avatar of the account that the device has then, through the session of that account (ADR 0712).
- The client sends the avatar only. It does not send the name of the preset. The person can give another name for one server at the join, and a later change of the preset name must not replace the name of an existing account.
- A stored avatar sets the state to `applied`. A preset whose avatar was removed meanwhile sets it to `not_needed`.
- A refusal of the image itself sets the state to `refused`, and the image is not sent again: the server answered `image_too_large` or `image_unsupported`, or the stored image is not one that the client sends (ADR 0714), or the stored file is larger than an avatar can be. The same image gets the same answer at every try, and each try sends the whole image.
- Every other failure counts one failed try, and the state stays `pending`: a failed request, and an avatar file that cannot be read. The next successful action of the access flow tries again.
- The tries of one server run one after the other. A try reads the state after the try before it has recorded its result. The try after an action of the access flow does not start while another try runs.
- A try uses the session that the device has when the try starts. Its request is not sent again with a renewed session. Its result is recorded only when the device has not left the account since the try started, also when the device has returned to the same account.
- The preset accepts an avatar of at most 2 MiB that is a PNG, JPEG or WebP image by its first bytes (ADR 0710): the cut of a chosen file (ADR 0119), at most 1024 by 1024 pixels. A server with a lower `JUNTO_PICTURE_BYTES` can refuse it; the settings name that server.
- The state is written to the book with the same rule as the membership: a write that failed is tried again (ADR 0710).
- `forget account` sets the state to `not_needed` (ADR 0712).

## Consequences

- An account that this device creates has the avatar of the preset after the first login, without an action of the person.
- A try whose answer was lost is repeated at the next login. The server stores the same image again and records no second change (ADR 0119).
- An image with more than 1024 by 1024 pixels is accepted by the preset and refused by every server. Its state is `refused` on each server, and the interface can show that.
- A user who sets or changes the avatar of the preset after joining a server does not get it applied there, also when the state is `refused`. Editing the profile of one server is done with the operations of the account (ADR 0119).
- The record survives restarts, so a failed try is repeated after a restart.
- The rule is tested against the server in memory and against the real server.
