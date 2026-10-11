# 0212. Setup tool: file format, matching of entries and commands

## Status

Accepted

## Context

ADR 0204 decides the commands `export`, `plan` and `apply`, a file with stable identifiers and a format version, explicit deletions, a plan that records its revision, and a JSON Schema of the file format. ADR 0211 decides the structure document, its revision and the route that applies changes. This record decides the fields of the file, how an entry of a file is matched to an object of a server, what `plan` computes, and how the tool gets its credential. A file that was exported from one server is also applied to another server, whose default workspace and "everyone" roles exist already under other identifiers.

## Decision

Crate:

- The tool is the crate `junto-setup` with the binary `junto-setup`. It depends on `junto-protocol` for the types of the file and of the two routes, and not on the server. Its HTTP client is the one of the client library (ADR 0714): TLS with the trust store of the operating system.
- The function that computes a plan reads two documents and contacts nothing.

File format 1:

- A file is one JSON object with `format`, `server_roles`, `workspaces` and `delete`. A workspace holds `roles`, `categories` and `channels`; a channel holds `overrides`. The types are in `junto-protocol`, and the JSON Schema is generated into `docs/api/structure-file.json` and compared by a test (ADR 0202).
- An object of a file with a field that the format does not name is refused, so that a misspelled field is not ignored.
- An entry has `id` and `name`; a channel also has `kind`. Every other field is optional. An absent field keeps the value of an existing object and takes the default of the server for a new one. `category: null` and `user_limit: null` remove the value.
- An entry that a file does not hold is kept. An override that a channel entry does not state is kept; an override with two empty sets removes the override.
- `delete.channels` lists the identifiers of the channels to delete. Nothing else is deleted. A listed channel that the server does not have is no change.
- An exported file states every field, and no deletion.

Matching:

- An entry is matched by its identifier (ADR 0211). A matched entry with another name is a rename.
- A workspace entry with `default: true` whose identifier the server does not know stands for the default workspace of the server. A role entry with `everyone: true` stands for the "everyone" role of its workspace, whatever its identifier is.
- The tool refuses a file before any request that changes the server when: its format is not 1; two entries of one kind share an identifier; an entry is in another workspace than its object; the kind of a channel differs; a channel names a category or a role that is neither in the file nor on the server; an override has a permission in both sets; a text channel has a setting of voice channels; a channel is an entry and is listed for deletion; a second workspace is marked as default; two entries stand for one role, or two overrides of a channel for one role. It states the problems of the file alone together; for a file without such a problem it states the problems against the server together.

Plan:

- `plan` reads the structure document and prints one line for each change, with the fields that differ. It sends only the fields that differ.
- The changes are ordered so that each change names only entries that exist before it: server roles, then for each workspace its roles, categories and channels with their overrides, then the deletions.
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

- `plan` changes nothing: it sends one reading request. A token with `read_structure` alone can export and plan.
- A second `apply` of the same file plans no change and sends no request that changes the server.
- The export of one server, applied to an empty server, renames the default workspace of that server, changes its "everyone" role, and adds the other entries. The two first channels of the empty server stay, because the file does not list them for deletion.
- A person who writes a new entry by hand gives it a new UUID.
- The tool reads format 1 only. A later format has another number, and the tool states which format it reads.
- A structure with more than 200 differences is applied in several files, each with a part of the entries.
- The plan file contains no credential.
