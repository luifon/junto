# 0747. Bounds on the messages and the moments that a server states

## Status

Accepted

## Context

ADR 0714 bounds one answer of a server at 4 MiB, and ADR 0721 bounds a timeline at 10,000 messages. Neither bounds what a timeline holds: a server that names 2,000 messages in small events and answers the read of each with 3 MiB of text makes the client hold 6 GiB. ADR 0408 bounds a message on the server, and the client did not check a message against those bounds, so the window built one element for each block, each reaction and each file of a message, however many a server stated. A server states moments in the form of RFC 3339, whose last year is 9999. The conversion of such a moment to the clock of another offset can leave the range of dates of the `time` library, and the conversion that the window used ends the program then.

## Decision

One message:

- The client takes a message from an answer only inside these bounds: at most 100 blocks, 1,000 inline nodes and 5,500 list items in its document, 100 files, 20 reactions, and a weight of at most 512 KiB. The numbers are those of ADR 0408, with the highest limit of files that a server can be set to; ADR 0408 states no number of list items, and its 16 KiB of text hold fewer than 5,500.
- The weight of a message is the bytes of its texts, and a fixed number for the message and for each block, list item, inline node, file and reaction. The memory that a message takes is at most a small multiple of its weight. A message inside the bounds of ADR 0408 weighs less than half of the bound.
- An answer that holds a message above the bounds is an answer that the client cannot read (ADR 0714). Nothing of it is used: a read of one message leaves the timeline marked as not current and is not sent again, a read of a page states that it failed, and a send stays `not confirmed`.
- The check is made in the functions of the API object that return messages, so the stream task, the window and the command-line tool get no message above the bounds.

What is held:

- A timeline holds at most 16 MiB of weight, in addition to its 10,000 messages. The messages that give way are marked as having more, so a reader reads them again when the person goes back to them. A message whose new copy weighs more than the copy before is bounded in the same way.
- Which messages give way depends on what the reader shows. The reader tells the stream task the position that it shows, or that it follows the end (ADR 0721), and the timeline holds that position. While the reader shows a position, messages give way one at a time from the end that holds more messages on its side of that position, so the messages that the reader shows stay, also when the timeline holds the end of the history: new messages of the place then are not added, and the reader reads them when the person goes to the end. While the reader follows the end, the end that was not added to gives way; when one message makes the timeline pass a bound and the timeline holds the end of the history, that end stays and the start gives way, so the new messages of the place still arrive; a timeline that does not hold the end keeps the half that the message is in.
- A pin list holds at most 8 MiB of weight: the oldest pins give way.
- The messages that the unread threads and the notices name hold at most 8 MiB of weight. A message that would pass it is not held, and its entry states that the read failed.
- The window holds at most 8 MiB of weight of the messages that replies name (ADR 0740). A message that would pass it is read once and not held, and the reply shows no text of it. A quoted message whose read ends with a refusal of the server, or with an answer that the client cannot read, is not held and not read again until an event names it or the state is loaded again. A read without an answer, with `busy`, with a limit on attempts, with a failure of the server, or one that was not sent because the device had no session at that moment, is sent again with a later change of the state.
- The memory of the messages of a server is so bounded by its open places and these four numbers, and not by what the server sent.

Counts and labels:

- The window takes an unread count and a mention count of a summary at 100 at most, the number that means "100 or more", and adds counts without passing 100. A server that states the largest number that the type holds shows 100.
- The window makes one index of the summaries of a server for each derivation and looks each place up in it. A search of the list for each row would cost the square of the number of summaries, which one answer of 4 MiB can make large.
- A name or a label that a server states (the name of an account, of a member and of a role, the author name of a message, the name of a file) is shown without control characters, without line breaks and without the characters that change the direction of the text after them (embeddings, overrides, isolates and the marks of direction), and cut at 200 characters. Right-to-left letters stay. The name under which a file is saved has its own rule (ADR 0721).

Moments:

- The window converts a moment of a server to the clock of an offset through one function that cannot fail: a moment that the clock of the offset cannot state is shown on the clock that the server wrote it with.
- The conversion of the `time` library that ends the program is a disallowed method of the two client crates (`clippy.toml` of each crate), so the check of the build refuses a new use.

## Consequences

- A message that a server of ADR 0408 stores is taken. A server that stores more in one message than ADR 0408 allows shows its places with a failed read.
- A moment in the year 9999 can be shown with the date of another zone than the zone of the device. A server of this repository states the time of its own clock and does not state such a moment.
- A later version of the server that raises a bound of ADR 0408 needs a client with the new numbers.
- The `clippy.toml` of a crate replaces the one of the workspace for that crate, so each of the two files repeats the settings of the workspace.
- A reader that shows the start of a long history and stays there does not get the new messages of the place in its timeline; they arrive when it goes to the end.
- Tests: a timeline that the reader shows at its start, in the middle and at its end in the model, and at its start in the stream client against played routes; counts of the largest number in both orders; one index of the summaries for a derivation; the labels of a member and of every character class that is left out.
- Tests: a message at the bounds of ADR 0408 and one part above each bound; the weight of a timeline, of a pin list and of the named messages; the stream client against played routes with small events and heavy answers, with a message above the bounds, and with an ordinary message afterwards; the quoted messages of the window; the last moment of the year 9999 through the message list, the audit log and the list of recordings.
