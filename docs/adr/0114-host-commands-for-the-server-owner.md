# 0114. Host commands for the server owner

## Status

Accepted

## Context

The server owner is the only position that can recover an account (ADR 0104). ADR 0201 gives the server a command line for the case that the owner has lost access, and ADR 0104 says that it assigns a new server owner. A restart prints an owner code only while the server has no owner (ADR 0110), so an owner who has lost every device had no way back, and no other person could become the owner without changing the database by hand.

## Decision

- The server binary has two commands besides serving the API. Both read `JUNTO_DATABASE_URL` and change the database directly, in one serializable transaction (ADR 0009). Neither exists as an API route.

`junto-server assign-owner <account-id>`:

- Makes an active account the server owner. A banned, removed or unknown account is refused.
- The owner before keeps its account, its devices and its roles, and loses the owner position.
- A stored owner code stops working.
- The audit log gets the entry `owner_assigned` with the new owner and the owner before. The entry has no acting account.

`junto-server issue-owner-code`:

- Stores a new one-time owner code in place of the stored one and prints it.
- The audit log gets the entry `owner_code_issued` in the same transaction, without an acting account. The entry states whether a stored code stopped working and when the new code expires. It never contains the code.
- On a server without an owner the code works as the code of the first start.
- On a server with an owner the code lives 60 minutes. The device that presents it to the claim route is added to the account of the owner, with the rules of a recovery (ADR 0112): every session of the account ends, each device gets a notice, and the audit log gets the entry `owner_access_restored` without an acting account. No account is created.
- A claim with a wrong or expired code gets `owner_code_invalid`. A claim on a server that has an owner and no stored code gets `owner_already_claimed`.

- Both commands refuse a database on which the server has not been started.

## Consequences

- A person with access to the host and the database can become the server owner or enter the account of the owner. ADR 0201 already states this for the first start.
- An owner who lost every device returns to the same account. Another person takes the position only through `assign-owner`.
- Each use of a command is in the audit log and visible to every reader of the server log, also when the issued code is never presented.
- A code that was issued and not used stops working after 60 minutes, at the next `issue-owner-code` and at an `assign-owner`.
- The commands need no running server and work while it runs.
