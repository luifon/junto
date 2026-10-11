-- Access tokens of administrators (ADR 0203, ADR 0210). The server stores the hash of a
-- token. A row is deleted when the token is revoked, when it has expired, and when the
-- access of its creator ends.
create table access_tokens (
    id uuid primary key,
    token_hash bytea not null unique check (octet_length(token_hash) = 32),
    created_by uuid not null references accounts (id),
    name text not null check (char_length(name) between 1 and 64),
    -- One bit per action. A new action widens this check.
    actions bigint not null check (actions between 1 and 15),
    created_at timestamptz not null,
    expires_at timestamptz not null,
    check (expires_at > created_at)
);

create index access_tokens_creator on access_tokens (created_by, id desc);
create index access_tokens_expiry on access_tokens (expires_at);

-- The access token of the request that wrote the entry. It is no reference, because the
-- entry outlives the token.
alter table audit_entries add column access_token_id uuid;

-- The identifier under which a structure file created an object, when it is not the
-- identifier of the object (ADR 0211). The identifier of a deleted channel stays in use.
create table structure_identifiers (
    kind text not null
        check (kind in ('server_role', 'workspace', 'workspace_role', 'category', 'channel')),
    identifier uuid not null,
    object_id uuid not null,
    primary key (kind, identifier),
    unique (kind, object_id)
);
