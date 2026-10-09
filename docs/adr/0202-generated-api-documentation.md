# 0202. Generated API documentation

## Status

Accepted

## Context

The public API is the only interface of the server (ADR 0201) and other programs depend on it. Documentation that is written by hand can differ from the behaviour of the server after a change. The WebSocket event stream is part of the API and needs a description as precise as the description of the REST routes.

## Decision

- An OpenAPI specification is generated from the route definitions and the type definitions of the server.
- A schema is generated for every event of the WebSocket event stream.
- The generated files are committed to the repository.
- CI regenerates the specification and the schemas and fails when they differ from the committed files.

## Consequences

- A change to a route, a type or an event that is committed without the regenerated files fails CI.
- Every change to the API is visible as a difference in the committed specification or schemas.
- Routes, types and events must be defined in a form that the generator can read.
- Other programs can generate their client code and validate events from the committed files.
