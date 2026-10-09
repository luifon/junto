# 0901. Repository rules and the process per feature

## Status

Accepted

## Context

The repository is public. A tracked file that contains a name, a hostname, an address or an account identifier publishes it permanently, because the history of a public repository is copied by others. A design record that mixes decisions with other content is harder to use as a reference for the code. A feature that is committed without tests, without a check in the running application and without a second reading can contain errors that later features build on.

## Decision

Repository rules:

- No personal information of any kind is in a tracked file: no names, hostnames, addresses or account identifiers.
- Comments are kept to a minimum. The code should explain itself.
- ADRs state engineering decisions and their technical reasons only.

Process per feature, in this order:

1. Tests, and a check in the running application.
2. An independent adversarial review.
3. Fixes.
4. An ADR.
5. A local commit.
6. A push, after a review by a project maintainer.

## Consequences

- Examples, test data and documentation use invented values.
- Every feature adds or changes an ADR, so `docs/adr/` describes the current design.
- A feature is not pushed before it has passed tests, a check in the running application, an adversarial review and a maintainer review.
