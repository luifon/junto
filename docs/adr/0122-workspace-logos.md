# 0122. A logo for each workspace

## Status

Accepted

## Context

A server holds several workspaces (ADR 0115), and the server book of the client lists the workspaces of each server under the server (ADR 0703). Each workspace row drew a generated picture from the identifier of the workspace beside its name, so two workspaces of one server differed only by their names and by patterns that nobody chose. The server has one logo (ADR 0101), and an account has an avatar (ADR 0119); a workspace had no picture.

## Decision

- A workspace has at most one logo: a still PNG, JPEG or WebP under the byte, dimension, format and animation checks of an avatar (ADR 0119). Migration 0058 adds the table `workspace_logos`, one row for each workspace with a logo, with its bytes, hash and dimensions; the row is removed with its workspace.
- `PUT /api/v1/workspaces/{workspace_id}/logo` sets it and `DELETE` removes it. The caller holds the server permission "administrator" with a session, the permission that renames and deletes a workspace (ADR 0102); no workspace permission manages a workspace. Access tokens cannot change it, as for the logo of the server. A change shares the profile-change limit of the account. An unknown or deleted workspace is `not_found`.
- `GET /api/v1/workspaces/{workspace_id}/logo` returns the image to a person who sees the workspace (`ViewWorkspace`), with its detected media type, a SHA-256 ETag, `nosniff` and `private, no-cache`; a matching `If-None-Match` returns 304 without the bytes. A person who does not see the workspace, and a workspace without a logo, get `not_found`. Unlike the logo of the server, the route is not public: a workspace and its name are visible only to the people who see it.
- The type `Workspace` carries the metadata of the logo (`logo`: hash, media type, bytes, width, height), and no image bytes, in the list of the workspaces, in the snapshot and in the event `workspace_updated`. A change of the logo publishes `workspace_updated` with the workspace and its new logo to the people who see the workspace, and records the audit action `workspace_logo_changed` with the workspace and the metadata, or null after a removal. An identical upload and an already absent removal record nothing.
- The client keeps the images of the workspace logos in the cache of the logos of the servers (`state::logo::Logos`), keyed by the server and the workspace: the logo that the state of the server states for each workspace is the truth, the image is read with the session of the account when its hash changes, decoded away from the window, and released from the GPU when it leaves the cache (ADR 0774). A read that gets no answer is tried again as a server logo is.
- The workspace list of the server book, and the workspace list of the menu of a server, draw the logo in the square of the generated picture, 15 by 15 design pixels with a radius of 2. A logo of other proportions stays inside its square in two ways: the client decodes it as it decodes an avatar, cut to its middle square (`state::people::decode`), and the element draws the image covering its square (`ObjectFit::Cover`), which gpui paints within the bounds of the element. The screens `logo-book-workspaces` (100 %) and `logo-book-workspaces-menu` (200 %, the hover menu of the collapsed server book) draw a logo three times as wide as high whose outer bands have colours that no theme uses, and fail when one pixel of those bands is drawn; with the cut and the cover both removed they find 460 and 1032 such pixels.
- The page of the workspaces in the administration shows the logo in the row of each workspace, and the actions "Logo" and "Remove logo" for an administrator. "Logo" asks for a file and opens the cut of a square picture of ADR 0119 (`PicturePurpose::WorkspaceLogo`), as the logo of the server does; the file is sent only while the page of the same owner is open (ADR 0746). "Remove logo" asks first.

## Tests

- Server: `workspace_logos::an_administrator_sets_and_removes_the_logo_of_a_workspace_that_its_members_read` (the authorization, the stored metadata in the list and the snapshot, the events, the headers of the image, an identical upload, the audit entry, a workspace that the person does not see, the removal) and `workspace_logos::a_workspace_logo_uses_the_limits_of_an_avatar`.
- Window: `logo::an_administrator_sets_and_removes_the_logo_of_a_workspace_and_the_workspace_lists_follow` (the file, the cut, the stored square, the workspace list of the owner and of a member, the question before the removal, the generated picture after it).

## Consequences

- A workspace has a picture that its administrators choose, in the place where the person chooses the workspace.
- The logo of a workspace is visible only to the people who see the workspace; a person before joining sees the logo of the server only.
