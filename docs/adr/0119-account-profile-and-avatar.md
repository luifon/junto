# 0119. Account profile: display name and avatar

## Status

Accepted

## Context

A new account starts from the name and the avatar that the person chose at installation, and the person can edit both per server (ADR 0105). The server base stored the name and had no route to change it and no avatar. An avatar is a file that a person uploads and that other people's clients decode, so its size and its format need bounds. A profile shows that an account exists, so it needs a rule for who may read it.

## Decision

Profile:

- The profile of an account is its display name and its avatar.
- `PATCH /api/v1/account` sets the name of the caller's account. A name has 1 to 64 characters, as every name of the server base.
- `PUT /api/v1/account/avatar` stores the avatar, and `DELETE` on the same path removes it.
- `GET /api/v1/accounts/{account_id}/profile` returns the name and the description of the avatar. `GET /api/v1/accounts/{account_id}/avatar` returns the image.

Who may see a profile:

- An account may see its own profile, the profile of an account that is a member of a workspace that it is a member of, the profile of an active account that is the server owner or holds the administrator permission, and every profile when it holds `ban_members`, which also lists the accounts.
- Reason for the administrators: every active account may write to them (ADR 0307), and a person who writes to an account needs its name and its picture. An account may therefore see the profile of every account that it may contact: one that administers the server sees every profile through `ban_members`, which the administrator permission includes (ADR 0301).
- The permission function decides this, as every other check (ADR 0301).
- A profile that the caller may not see is answered as an account that does not exist (ADR 0205).
- The avatar route needs a session. There is no public address of an avatar.

Avatar:

- The body of the upload is the image itself.
- An avatar has at most 262 144 bytes (256 KiB). A larger one gets `image_too_large` (413).
- The server accepts a still PNG, JPEG or WebP image. It reads the format from the bytes of the file and ignores the `Content-Type` of the request.
- The image is at most 1024 by 1024 pixels, as its header states. An animated PNG and an animated WebP are refused.
- Another file gets `image_unsupported` (415).
- The server does not decode and does not change the image. It checks the signature of the format and the size in the header.
- The avatar route answers with the stored bytes, the media type that the server found, the header `X-Content-Type-Options: nosniff`, and an `ETag` with the SHA-256 hash of the image.
- The description of an avatar carries that hash, the media type, the number of bytes, the width and the height. A client keeps the image for as long as the hash stays the same.

Storage:

- The avatar is stored in PostgreSQL, in one row per account, with a constraint for each bound.
- Reason: the change of an avatar and its event are one transaction (ADR 0009, ADR 0011). A file on disk would be an effect outside the transaction, which can run several times.

Events:

- A change of the name or of the avatar records `account_profile_updated` with the whole profile.
- The event goes to the accounts that may see the profile. The decision is made when the event is delivered (ADR 0011), with how the recipient stands to the account at that moment: a common workspace, the account administers the server, or neither. The routes and the delivery give the same fact to the same request of the permission function.
- A request that changes nothing records no event. The same holds for the name of a device: `PATCH /api/v1/account/devices/{device_id}` with the name that the device has records no `device_renamed`.

Limit:

- `PATCH /api/v1/account`, `PUT` and `DELETE /api/v1/account/avatar` and `PATCH /api/v1/account/devices/{device_id}` share one limit: 30 requests per account per minute. A further request gets `rate_limited` with a `Retry-After` header.
- Reason: each of these requests can record an event, and an event uses one position of the log, which keeps a fixed number of positions (ADR 0207). A change of the profile is also delivered to every fellow member.

Member and participant descriptions:

- `WorkspaceMember.avatar` in the member list and `workspace_member_added`, and `CallParticipant.avatar` in call reads, snapshots and participant events, carry the same optional metadata as the profile. Older payloads without the field read it as absent. The image still requires the avatar route and its authorization.
- These fields have exactly the audience of the containing member or participant; a hidden call exposes no additional participant. The database reads the avatar metadata of each listed member by its account primary key, once for each member, without reading image bytes or sending a request per profile. The statement reads it in a subquery of the member, not in a join: for a join the planner may choose a merge join or a hash join, which reads the avatars of every account whose identifier sorts before the last member, or of every account, so the rows read follow the size of the server instead of the length of the list. The member list reads the account name in the same way. Test: `profiles::profile_metadata_reads_follow_the_members` in `crates/server`, which explains both statements under each join method, with ten thousand other accounts whose identifiers sort before the members. Profile change events remain the notification of a later image change.

## Consequences

- A backup of the database contains the avatars.
- The avatars add at most 256 KiB per account to the database.
- A client must treat an avatar as a file of an unknown sender: the server checked the header, not the image data.
- A person who leaves every common workspace of another person no longer sees that person's profile and receives no further event about it, unless that person administers the server.
- The profile of an administrator goes to every account of the server, and a change of it is an event for every connected account. When an account loses the administrator permission, or is banned or removed, the accounts without a common workspace no longer see its profile. Test: `contacts::the_profile_of_an_administrator_is_seen_by_the_accounts_that_may_write_to_it` in `crates/server`.
- The name of a removed account stays readable for accounts with `ban_members` only, until messages refer to it.
- An avatar larger than the largest request body (ADR 0013) gets `invalid_request`, not `image_too_large`.

Avatar metadata in a member list or call participant has that containing resource’s audience, including a token with `read_structure` and a direct-call participant without a common workspace. This includes the hash; it grants no right to fetch image bytes. Clients must handle a forbidden or absent avatar despite receiving metadata. Invite creator names have the distinct invite audience of ADR 0111.
