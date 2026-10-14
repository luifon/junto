-- The last seat of each account that ended, with the reason, for the answer to a request
-- that renews it (ADR 0520).
create table call_seat_ends (
    account_id uuid primary key references accounts (id),
    identity text not null check (identity ~ '^[0-9a-f-]{36}:[0-9a-f]{32}$'),
    reason text not null check (reason in ('left', 'disconnected', 'moved', 'switched',
                                           'connection_lost', 'never_connected',
                                           'access_ended')),
    ended_at timestamptz not null,
    check (starts_with(identity, account_id::text || ':'))
);

-- A microphone block of the member ended as a grant of meeting mode, and the device has not
-- reported an open microphone since (ADR 0521).
alter table call_members add column meeting_grant boolean not null default false;

-- A member with a microphone block holds no grant.
create function call_microphone_block_withdraws_grant() returns trigger
language plpgsql as $$
begin
    update call_members set meeting_grant = false
    where call_id = new.call_id and account_id = new.account_id and meeting_grant;
    return null;
end
$$;

create trigger call_microphone_block_withdraws_grant
    after insert on call_microphone_blocks
    for each row execute function call_microphone_block_withdraws_grant();
