# Contributing to Junto

Changes reach `main` through pull requests on GitHub, which the maintainer of the project merges. [ADR 0907](docs/adr/0907-contributions-pull-requests-and-releases.md) records these rules and their reasons.

## Issues

Open an issue with one of the two forms of the repository: a problem report or a feature request. The problem report asks for the versions of Junto and of the server, the operating system and how Junto was installed. Remove from every field what you do not want to be public, such as server addresses and names.

## Steps

1. Fork the repository, or create a branch if you can push to it.
2. Make the change on a branch of its own. Follow the rules for a change below.
3. Open a pull request against `main`.
4. Sign the contributor licence agreement through the link that CLA assistant posts on the pull request (see below). `license/cla` is a required check of `main`: a pull request cannot be merged until every author of its commits has signed.
5. CI runs once on the head commit of the pull request, on Linux, macOS and Windows. A failed job runs again.
6. The change gets one independent review. Answer the review with new commits on the same branch.
7. The maintainer decides the merge and merges the pull request as one merge commit.

## Contributor licence agreement

Junto accepts a contribution only after each author of its commits has signed the contributor licence agreement. The agreement lets the project also license a contribution under other licences than Apache-2.0.

- [`docs/legal/cla-individual.md`](docs/legal/cla-individual.md) applies when you sign for yourself.
- [`docs/legal/cla-entity.md`](docs/legal/cla-entity.md) applies when you sign on behalf of your employer or another legal entity that owns the work.

A lawyer has not reviewed the two texts yet.

You sign through CLA assistant (https://cla-assistant.io). On a new pull request it posts a comment with a link to the agreement, and the check `license/cla` of the pull request stays pending until each author of its commits has signed. Open the link, log in with your GitHub account, and fill in the form:

- For whom you sign: for yourself, or on behalf of a legal entity.
- When you sign on behalf of a legal entity: its full legal name, and your role in it. The role must let you sign for the entity.

Then accept the agreement. The page of the link shows both agreements; the choice in the form decides which one you sign. CLA assistant keeps the record of the signature: your GitHub user name and user ID, the version of the agreement, the time and the fields of the form. The repository holds no signature records. One signature covers your later pull requests while the text of the agreement stays the same; after a change of the text, CLA assistant asks you to sign again on your next pull request.

CLA assistant reads the author of each commit. Use an email address that is linked to your GitHub account for your commits; a commit whose author is not a GitHub user cannot be signed for. The comment of CLA assistant has a link to check the signatures again.

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
