# 0204. Setup from a file with a command-line tool

## Status

Accepted

## Context

The structure of a server consists of workspaces, categories, channels, roles and permission overrides. An administrator who builds or changes a large structure in the client makes many separate changes, and the structure cannot be kept in version control or copied to another server. Applying a file to a server can delete entries by mistake, can stop after part of the changes, and can overwrite a change that another administrator made after the file was prepared.

## Decision

- A command-line tool has three commands:
  - `export` writes the server structure (workspaces, categories, channels, roles, permission overrides) to a file.
  - `plan` shows what applying a file would change.
  - `apply` makes the changes.
- Entries in the file carry stable identifiers.
- A deletion must be written explicitly in the file.
- The file states its format version.
- A plan records the revision of the structure it was computed from.
- `apply` refuses a plan whose revision is no longer current.
- `apply` uses one endpoint of the public API (ADR 0201). That endpoint applies all changes or none.
- The file format has a JSON Schema.

## Consequences

- An entry that is absent from a file is not deleted. Renaming an entry does not create a new one, because the entry is matched by its identifier.
- A structure that changed after `plan` requires a new `plan` before `apply`.
- A failed `apply` leaves the structure as it was.
- The server keeps a revision of its structure and exposes the endpoint that applies a set of changes in one transaction.
- The tool has no access that the public API does not give.
- The file contains only the structure listed above.
