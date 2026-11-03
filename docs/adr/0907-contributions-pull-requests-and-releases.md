# 0907. Contributions, pull requests and releases

## Status

Accepted

CLA assistant is not yet linked to the repository, and no contributor has signed. A lawyer has not reviewed the two agreements.

## Context

The repository is public under Apache-2.0 (ADR 0801), and the first contribution of another person arrived as a pull request. The branch `main` of the GitHub repository has a protection rule that requires a pull request; the rule does not apply to administrators. CI runs on three machines of the project and on GitHub (ADR 0905); its result is valid for the one commit that it ran on (`docs/release.md`). A release needs the steps of `docs/release.md`: CI on the release commit on Linux, macOS and Windows, the real-system checks of ADR 0902, client packages built on the three systems, and a test of a first install and of an update on every system.

Without another agreement, a contribution to an Apache-2.0 project is licensed to the project under Apache-2.0 (section 5 of the licence).

Database migrations go forward only, and the way back from a migration is the copy from before it or a backup (ADR 0005, ADR 0021). The client supports the current and the previous version of the API (ADR 0201).

## Decision

### Changes to `main`

- Changes reach `main` through GitHub pull requests, which the maintainer of the project merges. A push to `main` is allowed when the maintainer approves it.
- CI runs once on the head commit of each pull request, on Linux, macOS and Windows; a failed job runs again.
- Each pull request gets one independent review.
- A pull request is merged as one merge commit.
- The maintainer decides every merge and every publication.

Reasons: the pull request holds the CI result, the review and the merge decision of one change in one place, and the protection rule of `main` requires it. The protection rule does not apply to administrators (`enforce_admins` stays off), so that a push that the maintainer approves is possible. A CI result is valid only for the commit that it ran on, so the run is on the head commit; a run on each earlier commit of the pull request would use the machines of ADR 0905 for commits that are not merged. A merge commit keeps the commits of the pull request with their authors and hashes in the history of `main`, so the head commit that CI tested is a commit of `main`; a squash or a rebase makes new commits.

`CONTRIBUTING.md` at the root describes these steps for a contributor.

### Contributor licence agreement

- A contribution requires a signed contributor licence agreement (CLA) of each author of its commits.
- The agreements are adaptations of the Harmony Agreements templates "Harmony Individual Contributor License Agreement" and "Harmony Entity Contributor License Agreement", version 1.0 of 2011-07-04, from <https://www.harmonyagreements.org/agreements.html> (the files `docs/ha-cla-i-v1.odt`, SHA-256 `319226a66a26ee8e5a2d126bb5a738d51adfb788291d628cbb2657dceaea5085`, and `docs/ha-cla-e-v1.odt`, SHA-256 `df669346ddb37c60623c93c455f701a29c9cfe32fa5b004a0f349cd2a69b736d`, read on 2026-11-03), under the Creative Commons Attribution 3.0 Unported License. They are `docs/legal/cla-individual.md` and `docs/legal/cla-entity.md`; each one states at its top the template, its licence and the changes.
- The outbound licence is Option Five of Section 2.3 of the templates: the project may license a contribution under any licence, and also licenses it under the licence of the project on the date of the contribution (Apache-2.0 today). Options One to Four limit the outbound licences to the licence of the project, to a list, to licences that the Open Source Initiative approves, or to the copyleft licences that the Free Software Foundation recommends; none of them permits every licence.
- The optional licences for Media of the templates are left out: Option Five already covers the whole contribution, Media included.
- The party "We" or "Us" is the company LF CONSALTER FONCATTI SOLUCOES EM TECNOLOGIA DA INFORMACAO LTDA (trade name ASTROCODE SOLUCOES EM TI), CNPJ 41.276.590/0001-67. The governing law of Section 6.1 is the law of the Federative Republic of Brazil.
- The signature blocks of the templates are replaced by the signature through CLA assistant on a pull request.

Reasons: the agreement lets the project also license contributions under other licences than Apache-2.0. Under Apache-2.0 alone the project holds only the rights of Apache-2.0 to a contribution. Option Five gives the other rights, and its condition keeps every contribution available under the licence of the project. The Harmony templates are published for reuse with these options, and the individual and the entity texts differ only where an entity needs it (Affiliates, and no employer clause).

### Signing

- Contributors sign through the hosted service CLA assistant (<https://cla-assistant.io>), which SAP provides without charge. It is linked to the repository through its GitHub application. On each pull request it posts a comment with a link to the agreement and sets the status `license/cla`, which stays pending until each author of the commits of the pull request has signed. A contributor opens the link, logs in with GitHub, fills in the form and accepts. The page `https://cla-assistant.io/luifon/junto` also takes a signature without a pull request.
- The text that CLA assistant shows is one file of a GitHub gist: `docs/legal/cla-assistant-preface.md`, then `docs/legal/cla-individual.md`, then `docs/legal/cla-entity.md`, in that order. The preface states that the choice in the form decides which of the two agreements the person signs. CLA assistant shows only the first file of the gist whose name is not `metadata`, so both agreements are in one file.
- The form is the file `metadata` of the gist, a copy of `docs/legal/cla-assistant-fields.json`, in the format of the custom fields of CLA assistant (its schema `custom-fields-schema.json`): for whom the person signs (for oneself, or on behalf of a legal entity), and the full legal name of the entity and the role of the signer in it. The schema has no condition between fields, so the name and the role are not required fields; their descriptions state that they are filled in for an entity.
- CLA assistant keeps the signature records in its own database, not in the repository: the GitHub user name and user ID, the address and the version of the gist, the time and the fields of the form. The maintainer can export them as a CSV file.
- A change of the gist is a new version of the agreement: CLA assistant asks each contributor to sign again on the next pull request. A change of one of the three files of `docs/legal/` that the gist holds is copied into the gist.
- The account `luifon` of the maintainer and the bot accounts are exempt, through the list of exempt users of CLA assistant: `luifon,*[bot]`. A GitHub user name has only letters, digits and hyphens, so `*[bot]` matches only accounts of applications.

Choice of the tool, checked on 2026-11-03:

- CLA Assistant Lite (`contributor-assistant/github-action`, a GitHub Action): the repository is archived and read-only, and its README states that it is no longer maintained. Its last release, v2.6.1 of 2024-09-26, fails on its first run when the signature file does not exist (open issue 155).
- The CLA signature bot of Roblox (`Roblox/cla-signature-bot`): the repository is archived.
- EasyCLA serves projects of the Linux Foundation, and the CLA bot of the Python Software Foundation serves its projects.
- CLA assistant (`cla-assistant/cla-assistant`, Apache-2.0): the repository is not archived, and its last commit is of 2026-10-13. The commit before it is of 2023-10-16.

Reasons: CLA assistant is the one tool of the list that is maintained and open to any repository. It needs no workflow, no token and no branch in the repository, and it keeps the records outside the repository, so no tracked file holds them (ADR 0901). It shows one document and asks for the fields of the form before the signature, so one link serves the individual and the entity agreement.

### Releases and versions

- The release cycle is two weeks or longer. A weekly cycle only when enough contributors take part.
- Regular versions are the default. The patch number changes for fixes and small changes without a server migration. The minor number changes for new features and for any change that needs a server migration.
- A big feature may go through test builds. A test build has the tag `v<version>-test.<n>`, of the version that it will become, and the title "Junto <version> test <n>". Every test build is tested with the maintainer's friends who help the project. The files of the test build that passes are released unchanged as `v<version>`.
- The files of a test build state the version that it will become. Testers install each test build by hand. The rule of `docs/release.md` that a version whose files were published is not used again applies to releases, not to test builds.
- Release notes may credit contributors by their GitHub user name.

Reasons: the steps of `docs/release.md` are the same work for a release with few changes as for one with many. A server migration cannot be undone by the image of the earlier version (ADR 0005), so the minor number tells the person who runs a server that the upgrade changes the database and that the way back is the copy or a backup; a patch version has no migration, so the database keeps the schema of the earlier version. A test build is a pre-release, which the latest address of the update source does not serve (ADR 0724), so installed clients are not offered it. Releasing the files of the test build unchanged makes the released files the tested files, so the files of each test build already state the version of the release. Two test builds of one version then state the same version, and the update check of a client offers only a newer version (ADR 0780), so a client of one test build is not offered the next one.

### Version 1.0

- 1.0 is the compatibility promise: any 1.x server accepts any 1.x client, an upgrade within 1.x needs no manual step, and the API documents of `docs/api/` are frozen within 1.x.
- 1.0 comes when the first version of `docs/scope.md` is complete and in use, several release cycles pass without a high defect found in live use, a review of all unsafe code and a security review are done, and backup, restore and upgrade are tested on a real server.

Reasons: today a client supports only the current and the previous API version (ADR 0201). A promise for every 1.x combination is kept only when the API documents do not change within 1.x. The conditions are the evidence that the first version works in use and that the promise can be kept.

## Consequences

- A contributor signs once; the signature covers later pull requests of the same GitHub account while the gist does not change. A commit whose author has no GitHub account cannot be signed for, and the status of its pull request stays pending.
- A lawyer has not reviewed the agreements.
- The first contribution (pull request 1, in `main` as commit `e1878d60`) was merged before the agreement existed; its author signs afterwards on `https://cla-assistant.io/luifon/junto`.
- The signature records exist only in the hosted service of SAP, which can export them as a CSV file.
- The link of CLA assistant, the gist, the exempt accounts, the required status `license/cla`, the protection rule and the allowed merge methods are settings of GitHub and of CLA assistant, not files of this repository.
- `docs/release.md`, ADR 0724, ADR 0727 and `packaging/release_assets.py` publish a test release as a pre-release under the tag `v<version>` and edit it into the release; the test builds of 0.1.0 used the tags `v0.1.0-test.1` to `v0.1.0-test.4`, as this record decides. The release tool and those documents do not follow this record yet.
- A cycle of more than 21 days needs the freshness renewal of ADR 0724 between two releases.
