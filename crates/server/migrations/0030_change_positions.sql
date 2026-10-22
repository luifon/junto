-- A position belongs to a change: every event that one transaction writes gets the same
-- position, and its ordinal gives the order inside the change (ADR 0010, ADR 0011).

-- The rows that existed only to keep the number of positions of a change the same.
delete from events where audience->>'to' = 'suppressed';
drop index events_deliverable;

alter table events add column ordinal integer default 0 check (ordinal >= 0);
alter table events alter column ordinal drop default;
alter table events add constraint events_ordinal_with_position
    check ((position is null) = (ordinal is null));
alter table events drop constraint events_position_unique;
alter table events add constraint events_position_unique unique (position, ordinal);

create or replace function events_assign_position() returns trigger
language plpgsql as $$
declare
    assigned bigint := nullif(current_setting('junto.event_change', true), '')::bigint;
    following integer;
begin
    -- The first event of the change takes the lock and the position; the others of the
    -- same transaction take the next ordinal. Both settings end with the transaction.
    if assigned is null then
        perform pg_advisory_xact_lock(5356273455836692482);
        assigned := nextval('event_positions');
        following := 0;
        perform set_config('junto.event_change', assigned::text, true);
    else
        following := current_setting('junto.event_ordinal')::integer + 1;
    end if;
    perform set_config('junto.event_ordinal', following::text, true);
    update events set position = assigned, ordinal = following where id = new.id;
    perform set_config('junto.event_cursor', assigned::text, false);
    return null;
end
$$;

create or replace function events_are_append_only() returns trigger
language plpgsql as $$
begin
    if old.position is null and new.position is not null
        and old.ordinal is null and new.ordinal is not null
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

-- The log keeps a number of changes (ADR 0207). A message and its notice are one change, so
-- the link that kept them together as one unit of the trimming is not needed. The first
-- event of each change marks the change for the trimming.
drop trigger events_retention_pair_at_commit on events;
drop function events_check_retention_pair();
drop index events_retention_units;
alter table events drop constraint events_notice_parent_kind;
alter table events drop column retention_parent;
create index events_changes on events (position) where ordinal = 0;

-- The cursors of clients count positions of events. The positions of changes are another
-- history, so every cursor of the earlier one leads to a snapshot (ADR 0010).
update event_stream set epoch = gen_random_uuid();
