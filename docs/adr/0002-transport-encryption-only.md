# 0002. Transport encryption only

## Status

Accepted

## Context

The server provides message history, search, export and an API over message content. Each of these functions requires the server to read the messages. End-to-end encryption would prevent the server from reading them. Call recording requires the recorder to receive the audio and video of a call.

## Decision

- Junto has no end-to-end encryption.
- Every connection between a client and the server uses TLS.
- LiveKit encrypts media between each client and the media server.

## Consequences

- The server reads and stores message content, so history, search, export and the API operate on that content.
- The person who operates a server has technical access to all of its data, including direct messages. The join screen and the documentation state this (ADR 0302).
- The privacy of direct messages is enforced by the server's permission checks, not by cryptography.
- Media is protected between each client and the media server, not between one client and another.
