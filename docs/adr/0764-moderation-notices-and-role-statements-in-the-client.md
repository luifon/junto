# 0764. Private moderation notices, the message to the person, and what the server states of roles and accounts in the client

## Status

Accepted

## Context

The server records a private notice for each removal from a workspace, removal from the server and ban, with an optional message that the moderator writes for the person and without the reason of the audit entry (ADR 0120). An active account reads its notices in its snapshot and from a route; a device without a session reads the notices of its key with a proof. The server lists with each account whether the caller may unban it, and with each server role and role of a workspace whether the caller may give, take and delete it (ADR 0311); it changes one permission of a role with one request. Before this record the window decided an unban and the actions on a role by role order alone (ADR 0791), which cannot see the roles that an account gets back in its workspaces or the overrides of a role in channels, and it sent the whole list of a role for a change of one permission.

## Decision

Notices:

- The snapshot answer is read with its notices; the state holds the newest 100, each name cut to 100 characters and each message to 1,000. No event changes them.
- A device whose account was removed from the server or banned reads the notices of its key with a proof, once each time that the server comes into that state.
- The main area shows above its content, for the account of a server: each removal from a workspace that the person has not put away, with the name of the workspace at the time, the day, and the message; and, for a removed or banned account, the newest notice of that action. When the main area shows a state panel, the notices are drawn under its actions (ADR 0732). Text of the notice is shown without control characters.
- "Close" puts a notice of a removal from a workspace away. The server book keeps the identifiers of the newest 100 notices that were put away, so a notice does not return after a restart. The book is at format version 3 (ADR 0710).

The message to the person:

- The questions of a ban, of a removal from the server and of a removal from a workspace have a second optional field, "Message to the person", under the reason, with the rules of the reason: at most 1,000 characters, no control character, sent without the space around it. A sentence states that the person reads it in the notice for 30 days and that the audit entry keeps it.
- A row of the audit log shows the message of its entry as a line of its own under the reason.
- `junto-cli` takes `--message` beside `--reason` and prints `message=` in an audit line.

What the server states of accounts and roles:

- The accounts, the server roles and the roles of a workspace are read with their flags. The roles of a workspace stay in the snapshot; their flags are a collection of their own that the page "Roles" and, for a holder of "manage roles", the page "Members" hold open.
- The flags describe the caller at the read. An event that can change them makes the collection stale and reads it again: the status of an account for the accounts; a change of a server role, of the server roles of an account and of the owner for every flag; a change of a role of the workspace, of the roles of a member and of an override of a channel of the workspace for the flags of that workspace. Until that read answers, the flags state nothing.
- An unban is offered when the server stated it; role order still explains a refusal of an account at or above the person. A banned account that the server keeps from the person states why in the row. While the statement is read the action waits; after a failed read the request decides.
- Giving, taking and deleting a role follow the statement of the server; role order and the grant rule only explain a refusal. A role that the statement does not name, and a statement on its way, hold the action with a sentence. Without an open collection, and after a failed read, the client decides by role order and the grant rule (ADR 0791).
- A switch of a permission sends one request that names that permission. Changes of different permissions of one role go at the same time; a switch whose change waits shows the asked state and takes no change until the state holds its events (ADR 0763).

## Consequences

- A person who was removed from a workspace learns it, and reads the message, also after a restart within the 30 days of the server. A removed or banned person reads the message beside the state of the server.
- An older client cannot read a server book of version 3, and does not drop the notices that were put away.
- The page "People" and the pages of the roles read their lists again after each change of the order of the roles; the lists are small next to a snapshot.
- Two administrators who change different permissions of one role at the same moment keep both changes.
- Tests: `model::admin_tests::the_flags_of_the_server_are_held_by_their_read_and_read_again_after_a_change_of_order` and `book::tests::the_moderation_notices_put_away_are_kept_and_a_book_of_version_2_has_none` in `crates/client-core`, and the removal and the ban with a message against the real server in `tests/e2e/remaining.rs`; `state::admin::view::roles::tests::the_statement_of_the_server_decides_giving_taking_and_deleting_a_role`, `state::admin::view::people::tests::an_account_at_or_above_the_person_takes_no_removal_no_ban_and_no_unban` and `state::admin::view::records::tests::a_ban_and_a_removal_state_their_reason_without_control_characters` in `crates/client`.
