# 0307. Which two accounts may exchange direct messages and direct calls

## Status

Accepted

## Context

ADR 0302 decides that a direct message is a conversation between two accounts and is closed to everyone else. It does not decide which two accounts may start one. A server hosts several workspaces (ADR 0102), and two accounts on one server can have nothing in common.

The first version of ADR 0510 allowed a direct-message call when the caller could see the profile of the other account (ADR 0119). A profile is visible to an account with "ban members", so a moderator could call every account, and the rule for calls followed from a rule that was made for another purpose. The rule was checked when a call was started. The events of the call, their replay and the snapshot did not check it again after the two accounts had lost what connected them.

## Decision

The rule:

- Two accounts may exchange direct messages and direct-message calls only while both accounts are active and one of these holds:
  - both are members of at least one common workspace;
  - one of the two is the server owner or holds the administrator permission.
- No other permission allows it. "Ban members" lets an account see every profile (ADR 0119) and does not allow a direct message or a direct-message call.

Where it is decided:

- The permission function (ADR 0301) decides the rule as the request "contact". The server gives it one fact: how the actor stands to the other account. The values are "common workspace", "the other account administers the server" and "neither". The function allows the first two for every actor and the third only for an actor who administers the server.
- The server has one function that reads the fact and asks the permission function. Every decision about a direct-message call uses it:
  - the start of a direct-message call and the refusal of one;
  - the place of an account in a direct-message call, at every admission check (ADR 0509) and at every request of the account about its own call;
  - the delivery and the replay of every event of a direct-message call, and of every event for one account that names such a call, decided when the event is delivered (ADR 0011);
  - the direct-message calls and the invitations in the snapshot (ADR 0207).
- The text unit uses the same function for direct messages.

Who can be chosen:

- `GET /api/v1/direct-messages/contacts` lists the active accounts other than the caller that the caller may contact now, in the order of their names in lower case and then of their identifiers. For an account that administers the server these are all active accounts; for every other account those that share a workspace with it and those that administer the server. The statement reads only these candidates, and every listed account is decided by the permission function as a direct message is.
- `q` is a part of the name, at most 64 characters without control characters, compared without regard to case; absent or empty, it matches every name. `limit` is 1 to 100, 50 when absent. `next_after` is an opaque cursor that holds the name in lower case and the identifier of the last account, so a rename of that account does not move the next page; `after` takes it. A page that ends the list has no `next_after`.
- Reason: an account can be reached by the rule without being in any member list that the person can open: an administrator in no common workspace, or, for an administrator, an account of a workspace that the administrator is not a member of. A member cannot list the accounts of the server (ADR 0119), so without this list the person has no way to choose such an account. The list holds no account that the rule does not allow, so it shows a member nothing beyond the administrators and the accounts of its workspaces.
- The route needs a session of the account (`own_account`), as the direct messages do.

What it does not decide:

- A direct message that exists stays closed to everyone except its two participants (ADR 0302). This record decides only whether the two may exchange something now.

## Consequences

- Two accounts in different workspaces cannot write to each other or call each other, unless one of them administers the server.
- An administrator reaches every active account, and every active account reaches an administrator. An operator of a server can therefore answer a member who has no common workspace with the operator. The administrator permission already passes every check of a channel (ADR 0301); it gives no access to a direct message of two other accounts.
- A moderator with "ban members" and without the administrator permission sees a profile and cannot call the account.
- When the last common workspace of two accounts ends, or the administrator permission of one of them is removed, their direct-message call ends at the next admission check, and neither account receives a further event that names the other. Each gets `call_access_ended` (ADR 0510).
- A delivery of an event of a direct-message call reads the state, the memberships and the server roles of the other account in the transaction of its batch.
- ADR 0408 makes earlier direct-message history unavailable while this rule fails, and available again when it holds.
- Every account of the server can learn the names of the administrators from the list of contacts. ADR 0119 lets it see their profiles for the same reason.
- Test: `contacts::a_member_reaches_the_accounts_of_its_workspaces_and_the_administrators_only` and `contacts::the_contacts_are_found_by_a_part_of_the_name_in_pages_of_names` in `crates/server`: a member in a workspace of its own reaches the administrators only, a banned account and an account that lost the administrator permission leave the lists, each listed account takes a conversation, and the pages follow the names.
