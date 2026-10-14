create table moderation_notices (
    id uuid primary key,
    account_id uuid not null references accounts(id),
    kind text not null check (kind in ('workspace_removed', 'server_removed', 'banned')),
    workspace_id uuid references workspaces(id),
    name text not null,
    message text,
    occurred_at timestamptz not null,
    check ((kind = 'workspace_removed') = (workspace_id is not null))
);
create index moderation_notices_account on moderation_notices(account_id, id desc);

create index moderation_notices_age on moderation_notices(occurred_at);

create table moderation_notice_devices (
    account_id uuid not null references accounts(id),
    public_key bytea not null check (octet_length(public_key) = 32),
    notice_id uuid not null references moderation_notices(id) on delete cascade,
    primary key (public_key, notice_id)
);

create index moderation_notice_devices_notice on moderation_notice_devices(notice_id);

alter table challenges drop constraint if exists challenges_operation_check;
alter table challenges add constraint challenges_operation_check check (
    operation in ('login', 'claim_owner', 'join', 'request_to_join',
                  'join_request_status', 'link_device', 'moderation_notices')
);
