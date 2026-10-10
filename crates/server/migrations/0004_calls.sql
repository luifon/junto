-- Calls (ADR 0501 to ADR 0514). The rows of these tables are the server's own record of who
-- is allowed in each room of the media server.

alter table channels
    add column user_limit integer check (user_limit between 1 and 99),
    add column hides_occupants boolean not null default false,
    add constraint channels_call_settings_of_voice
        check (kind = 'voice' or (user_limit is null and not hides_occupants));

-- One row per call in progress: at most one per voice channel and one per pair of accounts.
-- The room of a call has a random name, so that a token of an earlier call names no room of
-- a later one (ADR 0508).
create table calls (
    id uuid primary key,
    room_name text not null check (room_name ~ '^jr-[0-9a-f]{12}-[0-9a-f]{32}$'),
    workspace_id uuid,
    channel_id uuid,
    direct_low uuid references accounts (id),
    direct_high uuid references accounts (id),
    created_at timestamptz not null,
    started_at timestamptz,
    revision bigint not null default 0,
    locked_by uuid references accounts (id),
    locked_at timestamptz,
    meeting_presenter uuid references accounts (id),
    meeting_started_at timestamptz,
    meeting_presenter_gone_since timestamptz,
    constraint calls_room_name_unique unique (room_name),
    constraint calls_one_per_channel unique (channel_id),
    constraint calls_one_per_pair unique (direct_low, direct_high),
    foreign key (channel_id, workspace_id) references channels (id, workspace_id),
    check ((channel_id is null) = (workspace_id is null)),
    check ((channel_id is null) <> (direct_low is null)),
    check ((direct_low is null) = (direct_high is null)),
    check (direct_low < direct_high),
    check ((locked_by is null) = (locked_at is null)),
    check (locked_by is null or channel_id is not null),
    check ((meeting_presenter is null) = (meeting_started_at is null)),
    check (meeting_presenter is not null or meeting_presenter_gone_since is null),
    check (meeting_presenter is null or channel_id is not null)
);

-- The accounts that are allowed in the room of a call, each with the one identity that it
-- may use there. An account is in one call at a time.
create table call_members (
    call_id uuid not null references calls (id) on delete cascade,
    account_id uuid not null references accounts (id),
    device_id uuid not null,
    session_id uuid not null,
    identity text not null check (identity ~ '^[0-9a-f-]{36}:[0-9a-f]{32}$'),
    admitted_at timestamptz not null,
    usable_until timestamptz not null,
    connected_at timestamptz,
    absent_since timestamptz,
    self_muted boolean not null default false,
    self_deafened boolean not null default false,
    hand_raised_at timestamptz,
    share_audio text check (share_audio in ('application', 'system', 'none')),
    share_started_at timestamptz,
    share_seen_at timestamptz,
    share_missing_since timestamptz,
    primary key (call_id, account_id),
    constraint call_members_one_call_per_account unique (account_id),
    constraint call_members_identity_unique unique (identity),
    check (starts_with(identity, account_id::text || ':')),
    check ((share_audio is null) = (share_started_at is null)),
    check (share_audio is not null or (share_seen_at is null and share_missing_since is null)),
    check (connected_at is not null or absent_since is null)
);

create function call_members_fit_their_call() returns trigger
language plpgsql as $$
declare
    low uuid;
    high uuid;
begin
    select direct_low, direct_high into low, high from calls where id = new.call_id;
    if low is not null and new.account_id <> low and new.account_id <> high then
        raise exception 'the account is not a participant of the direct message'
            using errcode = '23514';
    end if;
    return new;
end
$$;

create trigger call_members_of_a_direct_call
    before insert or update on call_members
    for each row execute function call_members_fit_their_call();

-- A block belongs to the call and to the person, not to one stay of the person in the call:
-- leaving and joining again does not remove it.
create table call_microphone_blocks (
    call_id uuid not null references calls (id) on delete cascade,
    account_id uuid not null references accounts (id),
    blocked_by uuid not null references accounts (id),
    reason text not null check (reason in ('moderator', 'meeting')),
    blocked_at timestamptz not null,
    primary key (call_id, account_id)
);

create table call_sound_blocks (
    call_id uuid not null references calls (id) on delete cascade,
    account_id uuid not null references accounts (id),
    blocked_by uuid not null references accounts (id),
    blocked_at timestamptz not null,
    primary key (call_id, account_id)
);

create table call_entry_requests (
    id uuid primary key,
    call_id uuid not null references calls (id) on delete cascade,
    account_id uuid not null references accounts (id),
    created_at timestamptz not null,
    expires_at timestamptz not null,
    accepted_by uuid references accounts (id),
    admits_until timestamptz,
    constraint call_entry_requests_one_per_account unique (call_id, account_id),
    check ((accepted_by is null) = (admits_until is null))
);

create table call_invitations (
    id uuid primary key,
    call_id uuid not null references calls (id) on delete cascade,
    account_id uuid not null references accounts (id),
    invited_by uuid not null references accounts (id),
    created_at timestamptz not null,
    expires_at timestamptz not null,
    constraint call_invitations_one_per_account unique (call_id, account_id),
    check (account_id <> invited_by)
);

create index call_invitations_account on call_invitations (account_id);

-- A move that waits for the join of the destination. Its identity is allowed in the room of
-- the destination while the account is still a member of the source (ADR 0511).
create table call_moves (
    id uuid primary key,
    account_id uuid not null,
    from_call_id uuid not null,
    to_call_id uuid not null references calls (id) on delete cascade,
    moved_by uuid not null references accounts (id),
    identity text not null check (identity ~ '^[0-9a-f-]{36}:[0-9a-f]{32}$'),
    created_at timestamptz not null,
    expires_at timestamptz not null,
    constraint call_moves_one_per_account unique (account_id),
    constraint call_moves_identity_unique unique (identity),
    check (from_call_id <> to_call_id),
    check (starts_with(identity, account_id::text || ':')),
    foreign key (from_call_id, account_id)
        references call_members (call_id, account_id) on delete cascade
);

create index call_moves_destination on call_moves (to_call_id);

-- What happened in the call of a voice channel, kept for the system lines of the channel's
-- text chat (ADR 0501). The event log of the stream is trimmed; these rows are not.
create table call_notes (
    id uuid primary key,
    occurred_at timestamptz not null,
    workspace_id uuid not null,
    channel_id uuid not null,
    call_id uuid not null,
    kind text not null check (
        kind in (
            'call_started', 'call_ended', 'screen_share_started', 'entry_requested',
            'joined_by_invitation', 'meeting_started', 'meeting_ended'
        )
    ),
    account_id uuid references accounts (id),
    foreign key (channel_id, workspace_id) references channels (id, workspace_id)
);

create index call_notes_channel on call_notes (channel_id, id);

-- The rings of direct-message calls: one row for each invitation that the call of one
-- account made for another account. The limit on rings and the pause after a declined ring
-- are counted here, so that they hold for every session of the caller (ADR 0515).
create table direct_call_rings (
    id uuid primary key,
    caller_id uuid not null references accounts (id),
    callee_id uuid not null references accounts (id),
    rung_at timestamptz not null,
    declined_at timestamptz,
    check (caller_id <> callee_id)
);

create index direct_call_rings_of_pair on direct_call_rings (caller_id, callee_id, rung_at);
create index direct_call_rings_age on direct_call_rings (rung_at);

-- The direct-message conversations that an account has muted. The account gets no call
-- invitation from the other account while the mute lasts (ADR 0404, ADR 0515).
create table direct_mutes (
    account_id uuid not null references accounts (id),
    other_account_id uuid not null references accounts (id),
    muted_at timestamptz not null,
    muted_until timestamptz,
    primary key (account_id, other_account_id),
    check (account_id <> other_account_id)
);

-- An event for one account that names voice channels is addressed to that account as well,
-- and is delivered only while the account sees those channels (ADR 0510).
alter table events
    drop constraint events_check1,
    add constraint events_addressed_to_an_account check (
        (audience ->> 'to' in ('account', 'account_seeing')) = (audience_account_id is not null)
    );
