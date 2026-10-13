alter table events add column retention_parent uuid unique references events(id) on delete cascade;
alter table events add constraint events_notice_parent_kind check (retention_parent is null or kind='notification_changed');
create index events_retention_units on events(position desc) where retention_parent is null;

create function events_check_retention_pair() returns trigger language plpgsql as $$
begin
    if not exists (
        select 1 from events parent join events notice on notice.retention_parent=parent.id
        where notice.id=new.id and parent.kind='message_changed' and parent.retention_parent is null
          and parent.payload->>'message_id'=notice.payload->>'message_id'
          and notice.position=parent.position+1
    ) then
        raise exception 'a notice retention pair needs its immediately preceding message event';
    end if;
    return null;
end
$$;
create constraint trigger events_retention_pair_at_commit after insert on events
    deferrable initially deferred for each row when (new.retention_parent is not null)
    execute function events_check_retention_pair();

create or replace function events_are_append_only() returns trigger
language plpgsql as $$
begin
    if old.position is null and new.position is not null
        and new.retention_parent is not distinct from old.retention_parent
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

alter table notification_receivers alter constraint notification_receivers_event_id_fkey deferrable initially deferred;
