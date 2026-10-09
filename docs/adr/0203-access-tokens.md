# 0203. Administrator access tokens

## Status

Accepted

## Context

A device logs in with a device key and a signed challenge (ADR 0103), which a script or another program cannot use conveniently. Automation of administration needs a credential that a program can hold. A credential that keeps its access after the person who created it loses a permission, or leaves the server, is a way around the permission system.

## Decision

- An administrator creates an access token with an expiry date and a stated set of actions.
- Each token can be revoked alone.
- Each request made with a token is checked against both the token's set of actions and the current permissions of the administrator who created it.

## Consequences

- A token loses access when its creator loses access. Removing or banning the creator, or removing a permission from the creator, takes effect on the token at the next request.
- A token never allows more than its stated set of actions, whatever its creator is allowed to do.
- A token stops working at its expiry date and must be created again.
- Bot accounts are not in the first version (`docs/scope.md`); a token acts with the permissions of its creator.
