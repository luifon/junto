create sequence event_positions as bigint;

create table event_stream (
    singleton boolean primary key default true check (singleton),
    epoch uuid not null,
    trimmed_through bigint not null default 0 check (trimmed_through >= 0)
);

insert into event_stream (epoch) values (gen_random_uuid());

create table events (
    id uuid primary key,
    position bigint check (position > 0),
    recorded_at timestamptz not null,
    class text not null
        check (class in ('control', 'general', 'direct_message', 'mention', 'call_invite')),
    kind text not null,
    audience jsonb not null,
    audience_account_id uuid,
    revalidates_account_id uuid,
    payload jsonb not null,
    constraint events_position_unique unique (position),
    constraint events_payload_size check (octet_length(payload::text) <= 16384),
    constraint events_audience_size check (octet_length(audience::text) <= 65536),
    check (payload ->> 'type' = kind),
    check (jsonb_typeof(audience) = 'object' and audience ? 'to'),
    check ((audience ->> 'to' = 'account') = (audience_account_id is not null)),
    check (audience_account_id is null or audience ->> 'account_id' = audience_account_id::text)
);

-- A position is assigned when the transaction commits, under a lock that the transaction
-- holds until its commit is visible. A later position therefore never becomes visible before
-- an earlier one (ADR 0010).
create table ended_sessions (
    session_id uuid primary key,
    account_id uuid not null,
    reason text not null
        check (reason in ('logged_out', 'device_removed', 'banned', 'removed', 'recovered')),
    recovery_id uuid references recoveries (id),
    ended_at timestamptz not null,
    check ((reason = 'recovered') = (recovery_id is not null))
);

create index ended_sessions_age on ended_sessions (ended_at);

create function events_assign_position() returns trigger
language plpgsql as $$
begin
    perform pg_advisory_xact_lock(5356273455836692482);
    update events set position = nextval('event_positions') where id = new.id;
    return null;
end
$$;

create constraint trigger events_position_at_commit
    after insert on events
    deferrable initially deferred
    for each row execute function events_assign_position();

create function events_are_append_only() returns trigger
language plpgsql as $$
begin
    if old.position is null and new.position is not null
        and (new.id, new.recorded_at, new.class, new.kind, new.audience,
             new.audience_account_id, new.revalidates_account_id, new.payload)
            is not distinct from
            (old.id, old.recorded_at, old.class, old.kind, old.audience,
             old.audience_account_id, old.revalidates_account_id, old.payload)
    then
        return new;
    end if;
    raise exception 'events are append-only';
end
$$;

create trigger events_no_change
    before update on events
    for each row execute function events_are_append_only();
