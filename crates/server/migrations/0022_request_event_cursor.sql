create or replace function events_assign_position() returns trigger
language plpgsql as $$
declare
    assigned bigint;
begin
    perform pg_advisory_xact_lock(5356273455836692482);
    update events set position = nextval('event_positions') where id = new.id
        returning position into assigned;
    if nullif(current_setting('junto.event_first', true), '') is null then
        perform set_config('junto.event_first', assigned::text, false);
    end if;
    perform set_config('junto.event_cursor',
        assigned::text,
        false);
    return null;
end
$$;
