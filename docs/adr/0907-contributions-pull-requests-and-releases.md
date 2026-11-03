# 0907. Contributions, pull requests and releases

## Status

Accepted

The workflow `.github/workflows/cla.yml` has not run yet: it runs only from the default branch of the GitHub repository. A lawyer has not reviewed the two agreements, and two of their values are not filled in (see Consequences).

## Context

The repository is public under Apache-2.0 (ADR 0801), and the first contribution of another person arrived as a pull request. The branch `main` of the GitHub repository has a protection rule that requires a pull request. CI runs on three machines of the project and on GitHub (ADR 0905); its result is valid for the one commit that it ran on (`docs/release.md`). A release needs the steps of `docs/release.md`: CI on the release commit on Linux, macOS and Windows, the real-system checks of ADR 0902, client packages built on the three systems, and a test of a first install and of an update on every system.

Without another agreement, a contribution to an Apache-2.0 project is licensed to the project under Apache-2.0 (section 5 of the licence). The maintainer plans paid hosting and paid features, and may use contributed code under other licences than Apache-2.0.

Database migrations go forward only, and the way back from a migration is the copy from before it or a backup (ADR 0005, ADR 0021). The client supports the current and the previous version of the API (ADR 0201).

## Decision

### Changes to `main`

- Every change to `main`, also a change of a maintainer, goes through a GitHub pull request. The maintainer of the project merges it.
- CI runs once on the head commit of each pull request, on Linux, macOS and Windows; a failed job runs again.
- Each pull request gets one independent review.
- A pull request is merged as one merge commit.
- The maintainer decides every merge and every publication.

Reasons: the pull request holds the CI result, the review and the merge decision of one change in one place, and the protection rule of `main` can require it. A CI result is valid only for the commit that it ran on, so the run is on the head commit; a run on each earlier commit of the pull request would use the machines of ADR 0905 for commits that are not merged. A merge commit keeps the commits of the pull request with their authors and hashes in the history of `main`, so the head commit that CI tested is a commit of `main`; a squash or a rebase makes new commits.

`CONTRIBUTING.md` at the root describes these steps for a contributor.

### Contributor licence agreement

- A contribution requires a signed contributor licence agreement (CLA) of each author of its commits.
- The agreements are adaptations of the Harmony Agreements templates "Harmony Individual Contributor License Agreement" and "Harmony Entity Contributor License Agreement", version 1.0 of 2011-07-04, from <https://www.harmonyagreements.org/agreements.html> (the files `docs/ha-cla-i-v1.odt`, SHA-256 `319226a66a26ee8e5a2d126bb5a738d51adfb788291d628cbb2657dceaea5085`, and `docs/ha-cla-e-v1.odt`, SHA-256 `df669346ddb37c60623c93c455f701a29c9cfe32fa5b004a0f349cd2a69b736d`, read on 2026-11-03), under the Creative Commons Attribution 3.0 Unported License. They are `docs/legal/cla-individual.md` and `docs/legal/cla-entity.md`; each one states at its top the template, its licence and the changes.
- The outbound licence is Option Five of Section 2.3 of the templates: the project may license a contribution under any licence, and also licenses it under the licence of the project on the date of the contribution (Apache-2.0 today). Options One to Four limit the outbound licences to the licence of the project, to a list, to licences that the Open Source Initiative approves, or to the copyleft licences that the Free Software Foundation recommends; none of them permits a commercial licence.
- The optional licences for Media of the templates are left out: Option Five already covers the whole contribution, Media included.
- The signature blocks of the templates are replaced by a comment on a pull request. A person who contributes work that an employer or another legal entity owns states the full legal name of the entity and the authority to sign for it in the pull request, and the comment then signs the entity agreement.

Reasons: under Apache-2.0 alone the project holds only the rights of Apache-2.0 to a contribution, and cannot license it under other terms. Option Five gives those rights, and its condition keeps every contribution available under the licence of the project. The Harmony templates are published for reuse with these options, and the individual and the entity texts differ only where an entity needs it (Affiliates, and no employer clause).

### Signing

- The workflow `.github/workflows/cla.yml` runs the action CLA Assistant Lite (`contributor-assistant/github-action`) v2.6.1, named by its commit `ca4a40a7d1004f18d9960b404b97e5f30a505a08`, on `pull_request_target` (opened, reopened, synchronize, closed) and on `issue_comment` (created) of a pull request.
- A contributor signs with the comment `I have read the Junto contributor licence agreement and I sign it.`; the comment `recheck` runs the check again. The action reads the author of each commit of the pull request, and the check of the pull request fails until each author has signed. The account `luifon` of the maintainer and the accounts whose names contain `[bot]` are exempt (`allowlist: luifon,*[bot]`).
- The signatures are in `.github/cla/signatures.json` of the branch `cla-signatures`. Each record holds the GitHub user name, the GitHub user ID, the ID and the time of the comment, the ID of the repository and the number of the pull request. The branch holds only that file, and CI does not run on a push to it (`branches-ignore` of `.github/workflows/ci.yml`).
- The file is created with `{"signedContributors": []}` together with the branch, before the first run of the workflow.
- At the close of a pull request the action locks its conversation.
- The workflow checks out no code of the pull request, and its jobs run one at a time per pull request.

Reasons: a pull request from a fork gets a token without write rights on the event `pull_request`; `pull_request_target` runs the workflow of the base branch with a token that can write the comment and the signature file. No code of the pull request runs, so that token reaches no code of the contributor. The action writes the file through the contents API without a pull request, and the protection rule of `main` requires one, so the file is on a branch without protection. In v2.6.1 the code that creates a missing file on the first run compares the HTTP status with the text `"404"` (`setupClaCheck.ts`), so the first run fails with "Could not retrieve repository contents. Status: 404" (open issue 155 of the action); with the file present, that code is not reached. The action matches each entry of the allow list as a regular expression without anchors, so `*[bot]` matches a name that contains `[bot]`; a GitHub user name has only letters, digits and hyphens, so only an account of an application matches. According to the documentation of the action, the lock at the close keeps contributors from changing or removing their signature comments after the merge.

### Releases and versions

- The release cycle is two weeks or longer. A weekly cycle only when enough contributors take part.
- Regular versions are the default. The patch number changes for fixes and small changes without a server migration. The minor number changes for new features and for any change that needs a server migration.
- A big feature may go through test builds. A test build has the tag `v<version>-test.<n>`, of the version that it will become, and the title "Junto <version> test <n>". Every test build is tested with the maintainer's friends who help the project. The files of the test build that passes are released unchanged as `v<version>`.
- Release notes may credit contributors by their GitHub user name.

Reasons: the steps of `docs/release.md` are the same work for a release with few changes as for one with many. A server migration cannot be undone by the image of the earlier version (ADR 0005), so the minor number tells the person who runs a server that the upgrade changes the database and that the way back is the copy or a backup; a patch version has no migration, so the database keeps the schema of the earlier version. A test build is a pre-release, which the latest address of the update source does not serve (ADR 0724), so installed clients are not offered it. Releasing the files of the test build unchanged makes the released files the tested files.

### Version 1.0

- 1.0 is the compatibility promise: any 1.x server accepts any 1.x client, an upgrade within 1.x needs no manual step, and the API documents of `docs/api/` are frozen within 1.x.
- 1.0 comes when the first version of `docs/scope.md` is complete and in use, several release cycles pass without a high defect found in live use, a review of all unsafe code and a security review are done, and backup, restore and upgrade are tested on a real server.

Reasons: today a client supports only the current and the previous API version (ADR 0201). A promise for every 1.x combination is kept only when the API documents do not change within 1.x. The conditions are the evidence that the first version works in use and that the promise can be kept.

## Consequences

- A contributor signs once; the signature covers later pull requests of the same GitHub account. A commit whose author has no GitHub account cannot be signed for, and its pull request stays failed.
- The two agreements hold two values that are not filled in: `[OWNER_LEGAL_NAME]`, the legal name of the party "We" or "Us", and `[JURISDICTION]`, the governing law of Section 6.1. A lawyer has not reviewed them.
- The first contribution (pull request 1, in `main` as commit `e1878d60`) was merged before the agreement existed; its author signs afterwards with a comment on pull request 1.
- The repository of the action is archived and read-only; its README states that it is no longer maintained. A defect of the action, or a change of GitHub that it does not follow, is corrected by a fork or by another action, which is a change of the workflow and of this record. The action declares the runtime `node20`.
- The protection rule, the token permissions of Actions, a required check of the CLA and the allowed merge methods are settings of the GitHub repository, not files of this repository.
- `docs/release.md`, ADR 0724, ADR 0727 and `packaging/release_assets.py` publish a test release as a pre-release under the tag `v<version>` and edit it into the release; the test builds of 0.1.0 used the tags `v0.1.0-test.1` to `v0.1.0-test.4`, as this record decides. The release tool and those documents do not follow this record yet.
- The files of a test build state the version that it will become, as the files of the test builds of 0.1.0 do. Two test builds of one version state the same version, so the update check of a client of one test build does not offer the next one (ADR 0780): a tester installs each test build.
- A cycle of more than 21 days needs the freshness renewal of ADR 0724 between two releases.
