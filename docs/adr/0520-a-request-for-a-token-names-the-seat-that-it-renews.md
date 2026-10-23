# 0520. A request for a token names the seat that it renews

## Status

Accepted

## Context

A device asks for a new token for its seat with the request that asks for a place: `POST /api/v1/channels/{channel_id}/call/join` or `POST /api/v1/direct-calls/{account_id}/join` (ADR 0510). The server gives a token for the seat that the account holds on that device and session, and otherwise a new seat. A request that the device sent before a moderator removed the person, and that the server reads after the removal, therefore gives the person a new seat, and the other people see that seat until the device gives it up (ADR 0785). When the account entered the call from another device in the meantime, the request takes the seat of that device, and the call ends on both. The server cannot tell a renewal from a new request to enter, because the request does not say which seat it is for.

## Decision

- Both routes take the optional query parameter `renews`, the identity of the seat that the request renews (`CallJoinQuery`). A request without it is a request for a place, as before.
- A request with `renews` gets a token for that seat or a refusal. It makes no seat, ends no seat and takes no seat of another device.
- The seat is renewed when the account holds it in the call of the place of the request, with the device and the session of the request, and it is usable: it has connected, or the time of its token has not ended (ADR 0517). The answer is the answer of a repeated request for the same seat: the same identity and a new token.
- Otherwise the answer is the error `seat_ended` (409). Its body has the optional field `error.left`, a `LeaveReason`, which states why the seat cannot be used:
  - `moved`: the seat ended by a move, and the account holds the seat of the move.
  - `switched`: the account holds another seat, on another device, in another session or in another call. That seat stays.
  - `never_connected`: the seat exists, and the time of its token ended without a connection. The admission check ends it.
  - the reason with which the seat ended, when it is the last seat of the account that ended: `disconnected` after a removal by a moderator, `connection_lost` and `never_connected` after the admission check, `left`, `access_ended`.
  - absent: the server does not know the seat.
- The server keeps the last seat that ended for each account, with its reason and time (`call_seat_ends`). `calls::store::take_out`, the one function that deletes a member row, writes it in the same transaction.
- The request reads the member row of the account with a row lock, as every request to enter does (ADR 0510). A removal and a renewal of the same seat run one after the other: a renewal that runs after the removal gets `seat_ended` with `disconnected`, and a removal that runs after the renewal ends the renewed seat.
- The checks of every request for a place come first: the session, the sight of the voice channel or the reach of the other account, and that the server reaches its media server. A renewal that fails one of them gets that error, as a request for a place does, and no seat.
- After `seat_ended` with `connection_lost` or `never_connected` a device that still wants to be in the call asks for a place without `renews`. With another reason it ends the call and does not ask again.

## Consequences

- A person whom a moderator removed while a renewal was on its way gets no seat, and nobody sees one. A renewal does not take the call from another device of the account.
- A client tells `seat_ended` apart from every other refusal by its code, and the reason by `error.left`. A client from before this ADR sends no `renews` and gets the answers of before.
- The error body has one more optional field. A reader ignores a field that it does not know (ADR 0206).
- `call_seat_ends` holds at most one row for each account.
- A renewal of a seat whose token time ended unused takes two requests now: the refusal and a request for a place. Before, one request ended the seat and gave a new one.
- Tests: `calls_renewal` in the API tests, among them `a_renewal_that_waits_behind_a_removal_gives_no_seat`, `a_renewal_does_not_take_the_seat_of_another_device` and `a_renewal_from_another_session_of_the_same_device_does_not_take_the_seat`.
