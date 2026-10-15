# 0763. Event receipts in the client

## Status

Accepted

## Context

The server states in the header `Junto-Event-Cursor` of the answer to a change the position of the last event of the change that the caller receives, `null` when the change made no such event, and nothing when it cannot state it (ADR 0214). An answer and the events of its change arrive in either order. Before this record the client had no common order for them: the answer to a send was put into the timeline when it arrived, so a deletion that the events told before a late answer was undone until the place was read again (ADR 0746); the window showed a change of notification settings until the state held equal settings, held the switches of a role until its list changed, took a follow choice by whether an event arrived during the request, and could not wait for the event of a change of the level of a workspace (ADR 0791, ADR 0796, ADR 0797).

## Decision

Reading the receipt:

- The transport reads the header of every answer into a receipt: a cursor, "no event" for `null`, or "not stated" for an absent header and for a value that is not a cursor.
- The change functions whose callers wait for the change return the value of the answer with its receipt: a send of a message, a change of notification settings, of the level of a workspace and of its removal, a follow choice and its removal, the marking of every notice, and the change of one permission of a role. The other functions return the value as before.
- The state of a server tells whether it holds the events of a change: yes when the receipt is "no event", or a cursor of the epoch of the state at or before the stream cursor; not yet for a later position of that epoch; unknown for "not stated" and for another epoch. The stream cursor moves by events and by checkpoints, so a state at or after the position holds the change also at the background level (ADR 0214). A receipt never moves the stream cursor.

The answer to a send:

- The stream task puts the message of the answer into the timeline only while the state does not hold the events of the send. When it holds them, the events decided: the read that the event of the message started puts the message in, and a later edit or deletion stays.
- An answer without a stated position is put in as before.

What the window shows until the state holds a change:

- A change that the server took is shown, and its control takes no further change, until the state holds its events. A change whose receipt the state cannot place asks for a snapshot after the answer (`LiveServer::refresh`); the change is held when that snapshot replaced the state, or when the state of the server is gone.
- This holds for the settings of a scope in the dialog of the notification settings, for "do not disturb" and the daily quiet time, for the level of a workspace in the dialog and on the page "Notifications", for "Mark all as read", and for each switch of a permission of a role.
- The follow choice of a thread takes the state of its answer when the state does not hold the events of the change, and keeps the state of the events when it does. Only an answer whose receipt the state cannot place, during which an event told the state, makes the window read the state of the thread again.

The level of a workspace:

- The dialog of the notification settings of a workspace shows the level that the state holds (`WorkspaceModel.notification_default`), and no longer reads it from its route. The dialog and the page "Notifications" share one change that waits: a change from one of them holds both.

## Consequences

- A message that was deleted before the answer to its send arrived stays deleted. A server that states no position keeps the behaviour of before for that answer.
- A change is shown from its answer until its event, without a gap in which the control shows the value of before and without a comparison of values that another person can make equal.
- A change on a server that states no receipt costs one snapshot request.
- The answer to a send can remove the waiting message while the read of the message that its event started is on its way: the list shows the message again when that read answers.
- Tests: `stream::tests::messages::the_answer_to_a_send_is_put_into_the_timeline_only_before_its_event`, `api::transport::tests::the_receipt_header_states_a_cursor_no_event_or_nothing` and `requests::an_account_leaves_a_thread_follows_it_again_and_reads_every_notice_in_one_request` (the receipt of a real answer) in `crates/client-core`; `state::admin::roles::tests::a_permission_takes_one_change_at_a_time_until_the_state_holds_its_event`, `state::followed::tests::the_receipt_of_a_change_orders_its_answer_with_the_events` and `state::admin::view::notifications::tests::a_change_that_waits_for_the_state_is_shown_and_takes_no_further_choice` in `crates/client`.
