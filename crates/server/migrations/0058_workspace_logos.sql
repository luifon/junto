-- The logo of a workspace (ADR 0122): a still PNG, JPEG or WebP within the limits of an avatar
-- (ADR 0119), set by a server administrator and shown in the workspace list of the client.
create table workspace_logos (
    workspace_id uuid primary key references workspaces(id) on delete cascade,
    media_type text not null check(media_type in ('image/png','image/jpeg','image/webp')),
    content bytea not null check(octet_length(content) between 1 and 2097152),
    hash bytea not null check(octet_length(hash)=32),
    width integer not null check(width between 1 and 1024),
    height integer not null check(height between 1 and 1024)
);
