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

What it does not decide:

- A direct message that exists stays closed to everyone except its two participants (ADR 0302). This record decides only whether the two may exchange something now.

## Consequences

- Two accounts in different workspaces cannot write to each other or call each other, unless one of them administers the server.
- An administrator reaches every active account, and every active account reaches an administrator. An operator of a server can therefore answer a member who has no common workspace with the operator. The administrator permission already passes every check of a channel (ADR 0301); it gives no access to a direct message of two other accounts.
- A moderator with "ban members" and without the administrator permission sees a profile and cannot call the account.
- When the last common workspace of two accounts ends, or the administrator permission of one of them is removed, their direct-message call ends at the next admission check, and neither account receives a further event that names the other. Each gets `call_access_ended` (ADR 0510).
- A delivery of an event of a direct-message call reads the state, the memberships and the server roles of the other account in the transaction of its batch.
- Whether an account can still read an old direct message after the rule stopped holding is decided with the text unit.
