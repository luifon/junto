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
- A request has one of four states: waiting, approved, denied, expired.
- A waiting request expires after 7 days. An expired request is not in the list for approvers, cannot be approved, and does not count as waiting.
- One device key has at most one waiting request. A key whose request expired or was denied can ask again; the earlier request stays stored.
- The server holds at most 200 waiting requests and refuses further ones with the error `join_requests_full`. The count and the new request are one serializable transaction (ADR 0009), so concurrent requests cannot pass the ceiling.
- Denied and expired requests stay stored as history for 30 days after their expiry time. The maintenance task deletes them then (ADR 0009). Approved requests are kept.
- The waiting device asks for the result with a signed `join_request_status` challenge (ADR 0108). The result is the state of the newest request of that key. A request of another device does not change it.
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

## Consequences

- Two simultaneous joins with a one-use code create one account; the other gets the error `invite_exhausted`.
- A change of the default workspace does not change invites that exist.
- A person whose request expired or was denied can ask again with the same key, and until then the device still gets `expired` or `denied` as its result.
- 200 waiting requests stop further requests until an approver answers some or they expire.
- People behind one address share the limit of 20 attempts per 10 minutes.
- A restart of the server clears the counts of the address limits, the count of wrong signatures included.
- Without a configured proxy address the server ignores `X-Forwarded-For`, so a client cannot choose the address that is counted.
- The memory of the limits has a fixed upper bound. A party with more than 100 000 addresses in one window makes the limits forget the oldest addresses early; the ceilings for the whole server (ADR 0108, ADR 0112 and the 200 waiting join requests) still hold.
