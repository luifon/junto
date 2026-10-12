# 0110. Format and storage of owner, invite and link codes

## Status

Accepted

## Context

Three codes are typed or read by a person: the owner code of the first start (ADR 0201), an invite code (ADR 0105) and the code of a device link (ADR 0104). The records do not fix their alphabet, their length or how the server stores them. The server command line prints the owner code once, and the server must not keep a secret that a copy of the database would reveal.

## Decision

Format:

- A code uses 32 symbols: the digits and the upper-case letters without `I`, `L`, `O` and `U`. Each symbol carries 5 random bits from the random source of the operating system.
- A code is written in groups of 4 symbols joined by hyphens.
- Before a code is compared, hyphens and spaces are removed, letters are made upper case, `O` is read as `0`, and `I` and `L` are read as `1`.
- The owner code has 20 symbols (100 bits). An invite code has 16 symbols (80 bits). A link code has 8 symbols (40 bits).

Storage:

- The server stores the SHA-256 hash of the normalized code with a prefix that states the kind of code. It does not store the code.
- An invite and a link are found by the hash. The owner code is compared with the stored hash in constant time.
- A code is sent in a request body, never in a path or a query.

Owner code:

- Each start of a server that has no owner generates a new owner code, replaces the stored hash and prints the code. The code of the start before stops working.
- The claim checks the code and sets the owner in one transaction that locks the server row. Two simultaneous claims produce one owner.
- After the claim the stored hash is removed.
- The host command `issue-owner-code` stores a code of the same form on a server that has an owner. That code lives 60 minutes and gives access to the account of the owner (ADR 0114).

Invite code:

- The response that creates an invite is the only place where its code appears.

## Consequences

- A code can be read aloud and typed without regard to case, and look-alike letters do not cause an error.
- A copy of the database does not contain a usable owner, invite or link code.
- A lost invite code cannot be shown again; a new invite is created.
- An owner code of a server without an owner that was printed and not used is valid until the next start of the server.
- A link code has 40 bits because it is not a secret that gives access: the approval of a logged-in device names the public key (ADR 0112). An account tries at most 10 link codes in 10 minutes, for the lookup and the approval together (ADR 0111), so a code is not found by trying.
