# 0901. Repository rules and the process per feature

## Status

Accepted

## Context

The repository is public. A tracked file that contains a name, a hostname, an address or an account identifier publishes it permanently, because the history of a public repository is copied by others. A design record that mixes decisions with other content is harder to use as a reference for the code. A feature that is committed without tests, without a check in the running application and without a second reading can contain errors that later features build on.

## Decision

Repository rules:

- No personal information of any kind is in a tracked file: no names, hostnames, addresses or account identifiers. The exceptions:
  - The address of the public repository, `https://github.com/luifon/junto`, may appear in tracked files.
  - The contributor licence agreement (`docs/legal/`) names the company that receives the contributions: its legal name, its trade name and its CNPJ (ADR 0907).
  - Release notes may credit contributors by their GitHub user name. The signature records of the contributor licence agreement are held by CLA assistant (cla-assistant.io), not in the repository (ADR 0907).
- Comments are kept to a minimum. The code should explain itself.
- ADRs state engineering decisions and their technical reasons only.

Process per feature, in this order:

1. Tests, and a check in the running application.
2. An independent adversarial review.
3. Fixes.
4. An ADR.
5. A local commit.
6. A pull request to `main`, which the maintainer merges after CI on its head commit and one independent review, or a push to `main` that the maintainer approves (ADR 0907).

## Consequences

- Examples, test data and documentation use invented values.
- Every feature adds or changes an ADR, so `docs/adr/` describes the current design.
- A feature is not merged before it has passed tests, a check in the running application, an adversarial review and the merge decision of the maintainer.
- `AGENTS.md` at the root of the repository names where each part of the repository is, how to build and test it, and the rules of this record, each with its path. A change of a path, a command or a rule that it names also changes `AGENTS.md`.
