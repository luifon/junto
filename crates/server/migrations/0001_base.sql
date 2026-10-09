create table workspaces (
    id uuid primary key,
    name text not null check (char_length(name) between 1 and 64),
    created_at timestamptz not null
);

create table accounts (
    id uuid primary key,
    name text not null check (char_length(name) between 1 and 64),
    status text not null check (status in ('active', 'banned', 'removed')),
    created_at timestamptz not null
);

create table server_state (
    singleton boolean primary key default true check (singleton),
    server_id uuid not null,
    name text not null check (char_length(name) between 1 and 64),
    logo_path text,
    default_workspace_id uuid not null references workspaces (id),
    owner_account_id uuid references accounts (id),
    owner_code_hash bytea check (octet_length(owner_code_hash) = 32),
    owner_code_expires_at timestamptz,
    created_at timestamptz not null,
    check (owner_code_expires_at is null or owner_code_hash is not null)
);

create table devices (
    id uuid primary key,
    account_id uuid not null references accounts (id),
    public_key bytea not null check (octet_length(public_key) = 32),
    name text not null check (char_length(name) <= 64),
    origin text not null
        check (origin in ('owner_code', 'invite', 'join_request', 'link', 'recovery')),
    created_at timestamptz not null,
    last_login_at timestamptz,
    constraint devices_public_key_unique unique (public_key)
);

create index devices_account on devices (account_id);

create table sessions (
    id uuid primary key,
    device_id uuid not null references devices (id) on delete cascade,
    token_hash bytea not null unique check (octet_length(token_hash) = 32),
    created_at timestamptz not null,
    expires_at timestamptz not null
);

create index sessions_device on sessions (device_id);
create index sessions_expiry on sessions (expires_at);

create table challenges (
    id uuid primary key,
    public_key bytea not null check (octet_length(public_key) = 32),
    operation text not null check (
        operation in (
            'login', 'claim_owner', 'join', 'request_to_join', 'join_request_status', 'link_device'
        )
    ),
    account_id uuid references accounts (id),
    nonce bytea not null check (octet_length(nonce) = 32),
    expires_at timestamptz not null,
    consumed_at timestamptz,
    constraint challenges_one_per_key_and_operation unique (public_key, operation)
);

create index challenges_expiry on challenges (expires_at);

create table invites (
    id uuid primary key,
    code_hash bytea not null unique check (octet_length(code_hash) = 32),
    created_by uuid not null references accounts (id),
    created_at timestamptz not null,
    expires_at timestamptz not null,
    max_uses integer not null check (max_uses >= 1),
    uses integer not null default 0,
    revoked_at timestamptz,
    revocation text check (revocation in ('revoked', 'creator_banned')),
    constraint invites_uses_within_maximum check (uses >= 0 and uses <= max_uses),
    check ((revoked_at is null) = (revocation is null))
);

create index invites_creator on invites (created_by);

create table invite_workspaces (
    invite_id uuid not null references invites (id) on delete cascade,
    workspace_id uuid not null references workspaces (id),
    primary key (invite_id, workspace_id)
);

create table join_requests (
    id uuid primary key,
    public_key bytea not null check (octet_length(public_key) = 32),
    name text not null check (char_length(name) between 1 and 64),
    state text not null check (state in ('pending', 'approved', 'denied', 'expired')),
    created_at timestamptz not null,
    expires_at timestamptz not null,
    decided_at timestamptz,
    decided_by uuid references accounts (id),
    account_id uuid references accounts (id),
    check ((state in ('pending', 'expired')) = (decided_at is null)),
    check ((state in ('pending', 'expired')) = (decided_by is null)),
    check ((state = 'approved') = (account_id is not null))
);

create unique index join_requests_one_pending_per_key
    on join_requests (public_key) where state = 'pending';
create index join_requests_by_key on join_requests (public_key, created_at desc);

create table device_links (
    id uuid primary key,
    code_hash bytea not null unique check (octet_length(code_hash) = 32),
    public_key bytea not null check (octet_length(public_key) = 32),
    device_name text not null check (char_length(device_name) <= 64),
    created_at timestamptz not null,
    expires_at timestamptz not null,
    constraint device_links_one_per_key unique (public_key)
);

create table recoveries (
    id uuid primary key,
    account_id uuid not null references accounts (id),
    device_id uuid references devices (id) on delete set null,
    performed_by uuid references accounts (id),
    performed_at timestamptz not null
);

create index recoveries_account on recoveries (account_id, performed_at);

create table recovery_notices (
    recovery_id uuid not null references recoveries (id) on delete cascade,
    device_id uuid not null references devices (id) on delete cascade,
    primary key (recovery_id, device_id)
);

create index recovery_notices_device on recovery_notices (device_id);

create table server_roles (
    id uuid primary key,
    name text not null check (char_length(name) between 1 and 64),
    permissions bigint not null check (permissions between 0 and 63),
    created_at timestamptz not null
);

create table account_server_roles (
    account_id uuid not null references accounts (id),
    role_id uuid not null references server_roles (id) on delete cascade,
    primary key (account_id, role_id)
);

create table workspace_members (
    workspace_id uuid not null references workspaces (id),
    account_id uuid not null references accounts (id),
    joined_at timestamptz not null,
    primary key (workspace_id, account_id)
);

create index workspace_members_account on workspace_members (account_id);

create table workspace_roles (
    id uuid primary key,
    workspace_id uuid not null references workspaces (id),
    name text not null check (char_length(name) between 1 and 64),
    permissions bigint not null check (permissions between 0 and 65535),
    is_everyone boolean not null default false,
    created_at timestamptz not null,
    unique (id, workspace_id),
    unique (id, workspace_id, is_everyone)
);

create unique index workspace_roles_one_everyone
    on workspace_roles (workspace_id) where is_everyone;
create index workspace_roles_workspace on workspace_roles (workspace_id);

create table member_roles (
    workspace_id uuid not null,
    account_id uuid not null,
    role_id uuid not null,
    is_everyone boolean not null default false check (not is_everyone),
    primary key (account_id, role_id),
    foreign key (workspace_id, account_id)
        references workspace_members (workspace_id, account_id) on delete cascade,
    foreign key (role_id, workspace_id, is_everyone)
        references workspace_roles (id, workspace_id, is_everyone) on delete cascade
);

create index member_roles_role on member_roles (role_id);

create table categories (
    id uuid primary key,
    workspace_id uuid not null references workspaces (id),
    name text not null check (char_length(name) between 1 and 64),
    position integer not null,
    created_at timestamptz not null,
    unique (id, workspace_id)
);

create index categories_workspace on categories (workspace_id);

create table channels (
    id uuid primary key,
    workspace_id uuid not null references workspaces (id),
    category_id uuid,
    kind text not null check (kind in ('text', 'voice')),
    name text not null check (char_length(name) between 1 and 64),
    position integer not null,
    permanent_messages boolean not null default false,
    permanent_message_count bigint not null default 0 check (permanent_message_count >= 0),
    created_at timestamptz not null,
    deleted_at timestamptz,
    unique (id, workspace_id),
    foreign key (category_id, workspace_id) references categories (id, workspace_id)
);

create index channels_workspace on channels (workspace_id) where deleted_at is null;

create table channel_overrides (
    channel_id uuid not null,
    role_id uuid not null,
    workspace_id uuid not null,
    allow bigint not null check (allow between 0 and 65535),
    deny bigint not null check (deny between 0 and 65535),
    primary key (channel_id, role_id),
    check (allow & deny = 0),
    check (allow | deny <> 0),
    foreign key (channel_id, workspace_id)
        references channels (id, workspace_id) on delete cascade,
    foreign key (role_id, workspace_id)
        references workspace_roles (id, workspace_id) on delete cascade
);

create table audit_entries (
    id uuid primary key,
    occurred_at timestamptz not null,
    actor_id uuid references accounts (id),
    workspace_id uuid references workspaces (id),
    channel_id uuid,
    action text not null,
    event jsonb not null,
    check (channel_id is null or workspace_id is not null),
    check (event ->> 'action' = action),
    foreign key (channel_id, workspace_id) references channels (id, workspace_id)
);

create index audit_entries_server on audit_entries (id desc) where workspace_id is null;
create index audit_entries_workspace on audit_entries (workspace_id, id desc);

create function audit_entries_are_read_only() returns trigger
language plpgsql as $$
begin
    raise exception 'audit entries are read-only';
end
$$;

create trigger audit_entries_no_row_change
    before update or delete on audit_entries
    for each row execute function audit_entries_are_read_only();

create trigger audit_entries_no_truncate
    before truncate on audit_entries
    for each statement execute function audit_entries_are_read_only();
