# 0403. Threads and forks

## Status

Accepted

## Context

A discussion about one message can fill a channel and hide other messages. A discussion that grows can need its own channel, or a call, with the same audience as the channel where it started.

## Decision

- A thread is a set of replies attached to one message. The client shows it in a side panel.
- A thread can be forked into a text channel or into a voice channel.
- The new channel inherits the category and the permissions of the thread's channel.
- The new channel receives a copy of the thread's messages.
- The thread gets a link to the new channel.
- A reply in a thread notifies the people who wrote in that thread (ADR 0404).

## Consequences

- A fork does not give access to any person who could not read the thread, because the new channel has the permissions of the thread's channel.
- After a fork the messages exist twice, in the thread and in the new channel. An edit or a deletion in one place does not change the copy in the other.
- A thread reply appears in the thread only. The option "also send to the channel" is in the later list of `docs/scope.md`.
