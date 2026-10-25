# 0111. Invite and join request rules, and the limits on attempts

## Status

Accepted

## Context

ADR 0105 states that an invite code has an expiry date, a maximum number of uses and the workspaces it grants, that a join request waits for an approval, and that the limit on join attempts uses the client address that the entry host forwards. It leaves open the defaults and bounds, how simultaneous uses are counted, how long a request waits, and the numbers of the limit.

## Decision

Invites:

- An invite without an expiry date expires 7 days after its creation. An expiry date must be in the future and at most 365 days ahead.
- An invite without a maximum number of uses has 1 use. The maximum is from 1 to 10 000.
- The workspaces of an invite are fixed when it is created. An invite that names none stores the default workspace.
- A join counts its use with one conditional update that succeeds only while the invite is not cancelled, not expired and has uses left. The use, the account, the device, the memberships and the audit entry are one transaction.
- A join that fails after the count does not use up the invite.
- An account reads an invite when it holds the "create invites" permission and either created the invite or may see every workspace that the invite names. The list of invites returns the invites that the caller reads, the events of an invite go to the accounts that read it (ADR 0208), and an invite is cancelled by an account that reads it. For every other caller with the permission the cancel route answers `not_found`, as for an invite that does not exist (ADR 0205).
- Reason: an invite names its workspaces, and a holder of "create invites" who is not a member of one of them gets `not_found` for that workspace on every other route. The permission function decides the rule in one place for the list, the cancellation and the events.
- A workspace that was deleted (ADR 0308) is not named and does not count for this rule.
- A cancelled invite stays in the list with the reason: cancelled, or creator banned (ADR 0106).
- An invite is created only by an account that is active in the transaction that stores the invite (ADR 0009). A ban that overlaps the creation either cancels the invite or prevents it.
- A logged-in account can use an invite to enter further workspaces (ADR 0115).

Join requests:

- A join request stores the public key and the name. The list for approvers returns the identifier, the name and the two dates.
- A request has one of five states: waiting, approved, denied, expired, withdrawn. The state `withdrawn` is stored only: the status route and the events of API version 1 do not carry it (ADR 0206).
- A waiting request expires after 7 days. An expired request is not in the list for approvers, cannot be approved, and does not count as waiting.
- One device key has at most one waiting request. A second request of a key whose request waits creates nothing and returns the waiting request with the status 200, with its own name and dates; a new request gets 201. Reason: a person who presses the button again, or a client that sends again after a lost answer, must not create a second request, and the client needs no error path for it. Requests of one key at the same time are serialized by the serializable transaction (ADR 0009) and the partial unique index on the waiting request of a key: an insert that meets the request of a concurrent transaction runs the transaction again, and the next run returns that request.
- The device withdraws its waiting request with a signed `withdraw_join_request` challenge at `POST /api/v1/join-requests/withdrawal`. The answer is 204 also when no request of the key waits. A key that belongs to an active or banned account gets `key_already_registered`: its request was approved, and the device must keep its key. A withdrawn request is not in the list for approvers and cannot be approved or denied. The approvers get the event `join_request_withdrawn`, and the audit log records `join_request_withdrawn` without an acting account.
- When a key gets a device of an account in another way (an invite code, the owner code, a device link, a recovery) or an inactive account returns with an invite, its waiting request is withdrawn in the same transaction, with the same event and audit entry. An approval of that request is then refused as `conflict` and not with `key_already_registered`.
- Reason for the withdrawal: a device that deletes its key while its request waits leaves a request that an approval turns into an account that no device holds, and the same person asking again with a new key makes a second request. The server cannot tell two keys of one device apart, so the device ends its request before it deletes the key.
- A join request states `accounts_with_name`: the number of active, inactive and banned accounts other than the returning account whose name is the name of the request, apart from case and surrounding spaces, at the moment the server gives the request (the list, the creation, the event). The window marks such a request (ADR 0761).
- A key whose request expired, was denied or was withdrawn can ask again; the earlier request stays stored. A denial is not a ban: the person can ask again, and each new request waits for a new decision. The limit of attempts per client address and the ceiling of waiting requests bound repeated requests.
- The server holds at most 200 waiting requests and refuses further ones with the error `join_requests_full`. The count and the new request are one serializable transaction (ADR 0009), so concurrent requests cannot pass the ceiling.
- Denied, expired and withdrawn requests stay stored as history for 30 days after their expiry time. The maintenance task deletes them then (ADR 0009). Approved requests are kept.
- The waiting device asks for the result with a signed `join_request_status` challenge (ADR 0108). The result is the state of the newest request of that key that was not withdrawn, and `not_found` when there is none. A request of another device does not change it.
- An approval creates the account and its device from the stored key. The device then logs in.
- A key that already belongs to an account cannot join or ask to join.

Workspaces that an invite or an approval can grant:

- A person without the administrator permission grants only workspaces that the person is a member of (ADR 0304).

Limits on attempts:

- Joining with a code, asking to join, claiming the server and asking for a device link share one limit: 20 attempts per 10 minutes per client address.
- Requests for a challenge have their own limit: 120 per minute per client address.
- Wrong signatures have their own limit: 10 per 10 minutes per client address (ADR 0108).
- A refused request gets the error `rate_limited` with a `Retry-After` header.
- The client address is the address of the connection. When that address is one of the configured proxy addresses, the server reads the `X-Forwarded-For` header from the right and takes the first address that is not a configured proxy. A header that cannot be read is ignored.
- A configured proxy is one address or a network in the form `192.0.2.0/24`: the entry container of a Compose network has no fixed address. An IPv4 address that is written as an IPv6 address is the IPv4 address. A network of IPv4 addresses is written in the IPv4 form.
- An IPv6 client is counted by its /64 prefix.
- Redeeming an invite as a logged-in account is limited to 10 attempts per 10 minutes per account and to 10 per 10 minutes per client address (ADR 0115).
- Looking up and approving a device link with its code share one limit: 10 attempts per 10 minutes per account (ADR 0110).
- The counts are held in the memory of the server process. The window of a key starts at its first attempt.
- Each limit holds at most 100 000 keys. The keys expire in the order in which their windows started, so each attempt drops the expired keys from the front of that order and scans nothing else. When a limit is full, the key whose window started first is dropped.

An invite includes `created_by_name`, the creator's current retained display name in the list and its name at creation in `invite_created`. It has exactly the readers of that invite, including after its creator is removed; it does not grant access to the creator's profile or avatar. Older payloads omit this optional field. The list joins the creator by the account primary key.

## Consequences

- Two simultaneous joins with a one-use code create one account; the other gets the error `invite_exhausted`.
- A change of the default workspace does not change invites that exist.
- A person whose request expired or was denied can ask again with the same key, and until then the device still gets `expired` or `denied` as its result.
- One device makes at most one account from its requests: one key has at most one waiting request and one account, and a device withdraws its request before it gives up its key. A device that deletes its key without the withdrawal (an older client, a deleted data directory, a second installation) still makes a second request; the window marks waiting requests that share a name (ADR 0761), and the approver decides.
- Tests: `joining::a_second_request_of_one_key_returns_the_first_one`, `joining::simultaneous_requests_of_one_key_make_one_request`, `joining::a_withdrawn_request_cannot_be_approved_and_the_key_can_ask_again`, `joining::a_key_that_joins_with_a_code_ends_its_waiting_request`, `joining::an_inactive_account_that_returns_with_a_code_ends_its_waiting_request`, `joining::a_key_that_becomes_a_linked_device_ends_its_waiting_request`, `joining::a_key_that_claims_the_server_ends_its_waiting_request`, `joining::an_approval_and_a_withdrawal_at_the_same_time_make_at_most_one_account`, `joining::a_request_states_how_many_accounts_have_its_name` and `maintenance::maintenance_removes_what_has_expired_and_keeps_the_rest` in `crates/server`. A recovery ends the request through the same device creation as the owner code and the device link and has no test of its own.
- 200 waiting requests stop further requests until an approver answers some or they expire.
- People behind one address share the limit of 20 attempts per 10 minutes.
- A restart of the server clears the counts of the address limits, the count of wrong signatures included.
- Without a configured proxy address the server ignores `X-Forwarded-For`, so a client cannot choose the address that is counted.
- The memory of the limits has a fixed upper bound. A party with more than 100 000 addresses in one window makes the limits forget the oldest addresses early; the ceilings for the whole server (ADR 0108, ADR 0112 and the 200 waiting join requests) still hold.

Removal from a workspace withdraws that workspace from every invite created by the removed account, in the same transaction. An invite that then names no workspace is revoked and records `invite_revoked` and its audit entry. A multi-workspace invite keeps the other workspaces and records `invite_changed` with its current description and the server audit action `invite_workspaces_changed` with its identifier, creator and removed workspace. The event has the invite's current audience. Each database page contains at most 100 invites; all pages commit with the membership removal.

There is no separate refusal for redeeming one's own invite: ordinary membership and validity rules apply. Another person's invite can readmit a removed member. Unlike server removal, a workspace has no ban, so removal must withdraw the removed person's own keys even when a new account would use them. Server removal deletes the account and its credentials and leaves its invites usable by new or existing accounts; the separate ban rule cancels its invites (ADR 0106).

The creator's display name is part of the invite description for every invite reader, even when that reader cannot read the creator's profile. This is an explicit audience of the name under this record, including the retained name after server removal; profile and avatar routes still apply ADR 0119 separately.
