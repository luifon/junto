# 0212. Setup tool: file format, matching of entries and commands

## Status

Accepted

## Context

ADR 0204 decides the commands `export`, `plan` and `apply`, a file with stable identifiers and a format version, explicit deletions, a plan that records its revision, and a JSON Schema of the file format. ADR 0211 decides the structure document, its revision and the route that applies changes. This record decides the fields of the file, how an entry of a file is matched to an object of a server, what `plan` computes, and how the tool gets its credential. A file that was exported from one server is also applied to another server, whose default workspace and "everyone" roles exist already under other identifiers.

## Decision

Crate:

- The tool is the crate `junto-setup` with the binary `junto-setup`. It depends on `junto-protocol` for the types of the file and of the two routes, and not on the server. Its HTTP client is the one of the client library (ADR 0714): TLS with the trust store of the operating system.
- The function that computes a plan reads two documents and contacts nothing.

File format 4 (formats 1 to 3 remain readable):

- A file is one JSON object with `format`, `server_roles`, `workspaces` and `delete`. A workspace holds `roles`, `categories`, `channels` and `glossary`; a category holds `overrides`; a channel holds `synced` and `overrides`. The types are in `junto-protocol`, and the JSON Schema is generated into `docs/api/structure-file.json` and compared by a test (ADR 0202).
- An object of a file with a field that the format does not name is refused, so that a misspelled field is not ignored.
- An entry has `id` and `name`; a channel also has `kind`. Every other field is optional. An absent field keeps the value of an existing object and takes the default of the server for a new one. `category: null` and `user_limit: null` remove the value.
- An entry that a file does not hold is kept. An override that a category or channel entry does not state is kept; an override with two empty sets removes the override. The exception is a channel that applying the file detaches from its category, below.
- `delete` lists the identifiers of the entries to delete, per kind: `channels`, `categories`, `roles` (workspace roles), `server_roles` and `workspaces`. Nothing else is deleted. A listed entry that the server does not have is no change. The channels of a deleted category stay; a deleted workspace is deleted with everything that it holds (ADR 0308).
- An exported file states every field, and no deletion.
- Format 2 adds optional role `position` keys expressing relative order, with larger keys higher and everyone at zero. Numeric gaps and shifts do not request changes. Only explicitly positioned roles exchange their existing slots; other roles keep their places. A new unpositioned role stays at the bottom (ADR 0309).
- Format 3 adds the optional `glossary` of a workspace: the list of its terms in their order of priority (ADR 0603). The list replaces the whole glossary, and `[]` removes every term. As for every optional field, an absent `glossary` keeps the glossary of an existing workspace, and a new workspace has an empty glossary.
- Format 4 adds the role overrides of a category and the sync state of a channel (ADR 0304). A category entry states `overrides` as a channel entry does: the allow set and the deny set of each role, which the synced channels of the category take. A channel entry in a category states `synced`. `true`: the channel takes the overrides of its category, and the entry states no overrides. `false`: the channel uses its own overrides, which the entry states. A channel without a category has no `synced`.
- An absent `synced` keeps the state of an existing channel, and a new channel in a category is synced. An override that such an entry states is compared with the overrides that the channel uses now, which for a synced channel are those of its category. The first override that changes what a synced channel uses detaches it, as the route of the channel's rules does. A file of format 1 to 3 is applied this way.
- The fields are read in a file of any format number; a file states the lowest format whose fields it uses.
- A file states no rule of a person. A rule of a person names an account, and accounts differ between servers, so a file of one server applies to another. Applying a file keeps the rules of people of the server: of the server, of workspaces, of categories and of channels. A channel that stops the sync with its category keeps the rules of people of the category as its own. A channel that the file syncs with its category again takes all rules of the category, and its own rules, also those of people, are removed, as the sync route does (ADR 0304); its plan line states this.

Matching:

- An entry is matched by its identifier (ADR 0211). A matched entry with another name is a rename.
- A workspace entry with `default: true` whose identifier the server does not know stands for the default workspace of the server. A role entry with `everyone: true` stands for the "everyone" role of its workspace, whatever its identifier is.
- The tool refuses a file before any request that changes the server when: its format is not one of 1 to 4; two entries of one kind share an identifier; an entry is in another workspace than its object; the kind of a channel differs; a channel names a category or a role that is neither in the file nor on the server; an override of a category or a channel has a permission in both sets, or names a role that is neither in the file nor on the server, or that is listed for deletion; a channel entry with `synced: true` states overrides or has no category; a text channel has a setting of voice channels; an entry of any kind is also listed for deletion, or is listed twice; a channel names a category or a role that is listed for deletion; a second workspace is marked as default; two entries stand for one role, or two overrides of a category or a channel for one role. It states the problems of the file alone together; for a file without such a problem it states the problems against the server together.

Plan:

- `plan` reads the structure document and prints one line for each change, with the fields that differ. It sends only the fields that differ.
- The changes are ordered so that each change names only entries that exist before it: server roles, then for each workspace its roles, its categories each with its overrides, and its channels each with its sync state and then its overrides, and then its glossary, then the deletions: channels, categories, workspace roles, workspaces, server roles; atomic role reorders come last.
- A glossary of a file that differs from the glossary of the server, in a term or in the order, is one change `replace_glossary` with the whole list (ADR 0211). Its line states the added terms, the removed terms and the new list in its order of priority. The tool removes the space around each term, as the server does, so that a file that was applied plans no change. The tool does not check the terms in another way: the server checks them with the rules of the glossary route and refuses the request with the number of the change.
- The tool refuses, against the server, a file that lists an "everyone" role or the default workspace for deletion: the server deletes neither.
- The changes of format 4 are `set_category_override` for an override of a category and `set_channel_sync` for the sync state of a channel (ADR 0211). The plan computes what each channel uses after its creation or update, as the server decides it (ADR 0304): a new channel in a category is synced; a synced channel that moves takes the overrides of its new category, or keeps those of its former category when it leaves for none; a channel with its own overrides keeps them when it moves. A channel that the file detaches keeps the overrides of its category as its own, and the plan removes those of roles that the entry does not state, so that the channel has the overrides of the entry. The export does not tell whether a channel without a category and without overrides takes the overrides of a category it moves into; for such a channel the plan sends every override that the entry states, and the server does not change an override that it holds already.
- `plan --out` writes the plan: the format, the address of the server, the revision and the changes.
- A plan with more than 200 changes is not sent.

Commands:

- `export [--out <file>]`, `plan <file> [--out <plan file>]`, `apply <file>`, `apply --plan <plan file>`. `apply <file>` computes the plan and sends it with its revision in one command.
- `apply --plan` refuses a plan that names another server address.
- The access token comes from the variable `JUNTO_ACCESS_TOKEN` or from the first line of the file of `--token-file`. It is never an argument. The address comes from `--server` or `JUNTO_SERVER`.
- The tool sends the token over `https`, or over `http` to a loopback address, and to no other address. It follows no redirect.
- The tool sends a request again, up to three times, after `busy` and `timeout` (ADR 0205).
- The exit code is 0 after success, 1 after a failure and 2 after wrong arguments. After an error answer of the API to `apply`, the tool states the code and the message of the server and that none of the changes was applied. When no answer arrived, or an answer that is no answer of the API, it states that it cannot tell, and that `plan` shows it.
- `apply --plan` sends a plan without changes as well, so that the server compares its revision.

## Consequences

- `plan` changes nothing: it sends one reading request. A token with `read_structure` alone can export and plan. The export holds the glossary of each workspace, which every member of the workspace reads (ADR 0603).
- A second `apply` of the same file plans no change and sends no request that changes the server.
- The export of one server, applied to an empty server, renames the default workspace of that server, changes its "everyone" role, and adds the other entries. The two first channels of the empty server stay, because the file does not list them for deletion.
- A person who writes a new entry by hand gives it a new UUID.
- The tool reads file formats 1 to 4 and writes format 4. Saved plans use format 4; older saved plans must be recomputed. A later format has another number.
- A tool that reads format 3 at most refuses an exported file of format 4, because a category entry holds `overrides` and a channel entry holds `synced`.
- The protocol crate states the format that is written and the oldest format that is read as two constants. The description of the generated schema and the refusal of a file of another format are written from them, so they name the current formats.
- A structure with more than 200 differences is applied in several files, each with a part of the entries.
- The plan file contains no credential.
- A file applied to a server with other accounts gives the same role overrides and sync states, and the people of that server keep their own rules.
- A tool that reads format 3 at most refuses an exported file of format 4, because each category entry holds `overrides`.

Role order and its action-specific exemptions are decided in ADR 0309.
