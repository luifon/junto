# Contributing to Junto

Every change to `main` goes through a pull request on GitHub, also a change of a maintainer. The maintainer of the project merges each pull request. [ADR 0907](docs/adr/0907-contributions-pull-requests-and-releases.md) records these rules and their reasons.

## Steps

1. Fork the repository, or create a branch if you can push to it.
2. Make the change on a branch of its own. Follow the rules for a change below.
3. Open a pull request against `main`.
4. Sign the contributor licence agreement in the pull request, once (see below).
5. CI runs once on the head commit of the pull request, on Linux, macOS and Windows. A failed job runs again.
6. The change gets one independent review. Answer the review with new commits on the same branch.
7. The maintainer decides the merge and merges the pull request as one merge commit.

## Contributor licence agreement

Junto accepts a contribution only after each author of its commits has signed the contributor licence agreement. The maintainer plans paid hosting and paid features, and the agreement lets the project licence a contribution under other licences than Apache-2.0, as well as under Apache-2.0.

- [`docs/legal/cla-individual.md`](docs/legal/cla-individual.md) applies when you own the work that you contribute.
- [`docs/legal/cla-entity.md`](docs/legal/cla-entity.md) applies when your employer or another legal entity owns the work. Before you sign, state in the pull request the full legal name of the entity and that you may sign for it. Your signature then signs the entity agreement for that entity.

A lawyer has not reviewed the two texts yet.

To sign, post a comment on your pull request with exactly this sentence:

```
I have read the Junto contributor licence agreement and I sign it.
```

The workflow `.github/workflows/cla.yml` records the signature in `.github/cla/signatures.json` of the branch `cla-signatures`: your GitHub user name, your GitHub user ID, the ID and the time of the comment, and the number of the pull request. One signature covers your later pull requests. Comment `recheck` to run the check again.

The check reads the author of each commit. Use an email address that is linked to your GitHub account for your commits; a commit whose author is not a GitHub user cannot be signed for.

## Work that you did not write

Name in the pull request each part of the change that you did not write, with its source, its author and its licence. Junto is under Apache-2.0, and [ADR 0801](docs/adr/0801-licence-and-reference-code.md) states how the project uses code of others. The maintainer decides whether such a part can be accepted.

## Rules for a change

[`AGENTS.md`](AGENTS.md) is the guide to the repository: where each part is, how to build and test it, and the checks that must pass before a commit. The rules of [ADR 0901](docs/adr/0901-repository-rules-and-feature-process.md) apply to every contribution:

- No personal information in a tracked file: no names, host names, addresses or account identifiers. Examples and test data use invented values.
- Comments are kept to a minimum.
- Each decision adds or changes an ADR in [`docs/adr/`](docs/adr/README.md), with its engineering reasons only.
- A change has tests, and a check in the running application where it changes the application.
- After a change of a route, a type or an event, regenerate `docs/api/` with `cargo run -p junto-server --bin junto-openapi` and commit the files.

Write commit messages in plain English. The first line states what the change does.

## Credit

Release notes may credit contributors by their GitHub user name.
