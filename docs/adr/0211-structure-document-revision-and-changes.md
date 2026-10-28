# 0211. Structure document, its revision, entry identifiers and the changes route

## Status

Accepted

## Context

ADR 0204 decides a setup tool that exports the structure of a server to a file, shows what applying a file would change, and applies the changes through one route of the public API that applies all of them or none. It requires stable identifiers of the entries, a revision of the structure, and a refusal of a plan whose revision is no longer current. The structure routes of ADR 0304 change one object each. An identifier of the API is a UUID that the server generates (ADR 0205), and several tables use the identifier of a channel or of a workspace as a key of other data: messages, read positions and notification settings. Every transaction is serializable (ADR 0009).

## Decision

Structure document:

- `GET /api/v1/structure` returns the structure of the server as one document, in the format of the structure file (ADR 0212), and its revision: the server roles, and each workspace that is not deleted with its roles, its categories with their role overrides, and its channels that are not deleted with their sync state and their own role overrides, and its glossary: its terms in their order of priority (ADR 0603). A channel that is synced with its category states no overrides, because it uses those of the category (ADR 0304). It names no account and no member, so it holds no rule of a person.
- The route needs the administrator permission, because the document holds every channel and every override. The document is read from one snapshot, with a number of statements that does not depend on the size of the structure.

Revision:

- The revision is the first 16 bytes of the SHA-256 hash of the document, written as 32 hexadecimal digits. Equal structures have equal revisions, and every change of the structure changes the revision, whichever statement made the change.
- The server stores no counter. One counter row would be written by every change of the structure, so that no two changes of the structure could run at the same time, and a change would wait for a long request of changes.

Changes route:

- `POST /api/v1/structure/changes` takes a revision and a list of at most 200 changes, and applies them in their order in one transaction.
- A change is one of: create, update or delete a server role; create, rename or delete a workspace; create, update or delete a workspace role; create, update or delete a category; create, update or delete a channel; update the call settings of a voice channel; set a channel override; set a category override; set the sync state of a channel; replace the glossary of a workspace.
- An override of a category runs the function of the category rule route, with the grant ceiling in the category and in each of its synced channels (ADR 0306). The sync state `true` runs the function of the sync route. The sync state `false` copies the rules of the category to the channel, which then uses them as its own; it changes no permission, so it has no grant ceiling, and it writes the audit entry and the event `channel_rules_synced` of a detach. A channel already in the requested state is not changed.
- Each change runs the function of its single route: the same permission checks with the grant rule (ADR 0304), the same limits (ADR 0301), the same ceilings (ADR 0306), the same audit entries and the same events. The single routes and this route share that function.
- The route needs the administrator permission, because the revision is the revision of the whole structure, which only an administrator reads.
- The transaction computes the revision of the structure before the first change. When it differs from the revision of the request, the answer is `conflict` and nothing is changed. A request without changes gets the same check.
- An error of one change ends the request with the code of that error and a message that starts with the number of the change, counted from 1, and its name. Nothing is changed. A change that names an entry that does not exist gets `invalid_request`, because every `not_found` response has one fixed body (ADR 0205).
- An error that is not about one change keeps its own form: `busy`, `timeout` and `internal` (ADR 0205).
- The answer carries the revision after the changes and, for each created object, the identifier of its entry and the identifier of the object.
- A new workspace gets the account of the request as its first member, as on the single route. The change names the identifier of the "everyone" role of the new workspace, so that later changes and files can name that role.

Glossary:

- The change `replace_glossary` names a workspace and the whole list of its terms, most important first. It runs the function of the glossary route `PUT /api/v1/workspaces/{workspace_id}/glossary`: the same checks of the terms and the same limit of their number (ADR 0603), the permission "manage glossary" in the workspace, the audit entry `glossary_replaced` and the event `glossary_replaced`. The list replaces the whole glossary; an empty list removes every term.
- The change has no `replaces` field (ADR 0616). The revision of the request does that check for the whole structure: the document holds every glossary, so a replacement of a glossary on its route after the plan makes the plan stale.

Entry identifiers:

- The identifier of an entry is the identifier of its object. An entry that a request creates carries an identifier that the author of the file chose. The server generates the identifier of the object as for every object, and stores the pair in the table `structure_identifiers`, per kind of object.
- The structure document writes the stored identifier of an object when one exists, and the identifier of the object otherwise. A change names an entry by either.
- A new entry cannot take an identifier that names an object of the same kind, as its entry identifier or as its own identifier. The identifier of a deleted channel and of a deleted workspace stays in use, because their rows stay (ADR 0407, ADR 0308). The identifier of a deleted role or category is free again.
- The server does not create an object with an identifier that a client chose. An identifier of a channel or a workspace is a key of other data, and a chosen identifier could equal a key of another kind, for example of a direct-message conversation. Two servers that apply the same file would also hold objects with equal identifiers.

## Consequences

- A plan is applied to the structure that it was computed from, or not at all. The transaction of a request reads the whole structure, and it is serializable (ADR 0009). A change of the structure that commits before the request has read is found by the comparison of the revisions. A change that commits while the request runs has one of two outcomes: PostgreSQL makes one of the two transactions run again, and the request then finds another revision and gets `conflict`; or both commit, and the result is the one of the request first and the other change after it. The second outcome occurs only for a change that read nothing that the request wrote.
- The server does not refuse a request because another change committed during it, when that change does not depend on what the request changed. The result equals the one of the other change arriving after the answer. The revision in the answer is the revision after the request alone.
- A change that is taken back gives the earlier revision again. A plan that was computed before both changes is then current again.
- The revision of a server changes when a new version of the server adds a field to the document. A plan from before the upgrade is then refused and computed again.
- Computing the revision reads the whole structure: once for an export, twice for a request with changes. The glossaries of all workspaces are read with one statement.
- A replacement of a glossary in the window or with `junto-cli` changes the revision, as a change of a channel does.
- The ceiling of 200 changes keeps a request far below the time limit of a request (ADR 0008). Measured on a development machine with a debug build, a change costs about 10 ms, and a request of 500 changes passed the limit of 30 seconds while the whole test suite ran beside it under CPU load. A larger file is applied in parts.
- A file that is applied to a second server creates objects with the identifiers of that server. The file and the export of the second server name them by the identifiers of the file, so the same file plans no change afterwards.
- An object that a file created has two identifiers: the one of its entry, in files and in the structure document, and its own, in every other route, in events and in the audit log. The answer of the changes route states both.
- A deletion of a role, a category or a workspace runs the function of its single route (ADR 0308), with the same checks: a deletion that the route refuses ends the request, and nothing is changed.
- A stale revision has the code `conflict`, like every state that makes a plan unusable. A program that gets `conflict` from this route computes a new plan.

Role order and its action-specific exemptions are decided in ADR 0309.

Role reorders are `reorder_server_roles` and `reorder_workspace_roles`, with resolved
absolute positions inside the atomic change request. The format-2 file keys express
relative order; the setup planner resolves them to these positions (ADR 0309).
