-- Who is inside a call, and the setting that a call keeps from its start (ADR 0517).

-- A call keeps the setting "hides who is inside" that its voice channel had when the record
-- of the call was created. A change of the channel applies to the next call. The database
-- copies the value, so that no writer of a call can state another one.
alter table calls add column hides_occupants boolean not null default false;
update calls k set hides_occupants = ch.hides_occupants
    from channels ch where ch.id = k.channel_id;

create function calls_keep_the_setting_of_their_start() returns trigger
language plpgsql as $$
begin
    if tg_op = 'UPDATE' then
        new.hides_occupants := old.hides_occupants;
        return new;
    end if;
    new.hides_occupants := coalesce(
        (select hides_occupants from channels where id = new.channel_id), false);
    return new;
end
$$;

create trigger calls_keep_the_setting_of_their_start
    before insert or update of hides_occupants on calls
    for each row execute function calls_keep_the_setting_of_their_start();

-- An account is inside a call from the moment the admission check has recorded that the
-- media server lists its seat, until its member row is removed. Every statement, trigger
-- and route that asks who is inside uses this function.
create function is_inside_call(subject uuid, reader uuid) returns boolean
language sql stable as $$
    select exists (
        select 1 from call_members
        where call_id = subject and account_id = reader and connected_at is not null);
$$;

-- Whether an account may read what tells who is inside a call: the call does not hide who
-- is inside, or it has ended, or the account is inside.
create function reads_call(subject uuid, reader uuid) returns boolean
language sql stable as $$
    select not coalesce((select hides_occupants from calls where id = subject), false)
        or is_inside_call(subject, reader);
$$;

-- The private system lines of a call are for the accounts inside it, and the stored state
-- of a call is the one that the call started with.
create or replace function refresh_call_message_privacy(subject uuid) returns void
language sql as $$
    insert into call_message_members
    select cm.call_id, cm.account_id from call_members cm
    join calls c on c.id = cm.call_id
    where cm.call_id = subject and c.hides_occupants
      and is_inside_call(cm.call_id, cm.account_id)
    on conflict do nothing;
    insert into call_message_privacy (call_id, hidden, readers, channel_id)
    select c.id, c.hides_occupants,
        array(select account_id from call_members
              where call_id = subject and is_inside_call(call_id, account_id)
              order by account_id),
        c.channel_id
    from calls c where c.id = subject and c.channel_id is not null
    on conflict (call_id) do update
        set hidden = excluded.hidden, readers = excluded.readers,
            channel_id = excluded.channel_id;
$$;

create or replace function remember_call_message_member() returns trigger
language plpgsql as $$
declare
    subject uuid := coalesce(new.call_id, old.call_id);
begin
    -- The row is gone, so the function cannot read it: the condition of `is_inside_call`
    -- is stated on the old row.
    if tg_op = 'DELETE' and old.connected_at is not null then
        insert into call_message_members
        select old.call_id, old.account_id from calls c
        where c.id = subject and c.hides_occupants on conflict do nothing;
    end if;
    perform refresh_call_message_privacy(subject);
    return null;
end;
$$;

drop trigger remember_call_message_member on call_members;
create trigger remember_call_message_member
    after insert or delete or update of connected_at on call_members
    for each row execute function remember_call_message_member();

drop trigger remember_hidden_call_members on channels;
drop function remember_hidden_call_members();

-- The state of a call is stored when its record is created, before it has a member, so
-- that the delivery of an event finds it for every call.
create function remember_call_privacy() returns trigger
language plpgsql as $$
begin
    perform refresh_call_message_privacy(new.id);
    return null;
end;
$$;

create trigger remember_call_privacy
    after insert on calls
    for each row execute function remember_call_privacy();

select refresh_call_message_privacy(id) from calls;

-- Before this migration a seat counted as inside, so an account that asked for a place in
-- a call that hides who is inside and never connected became a reader of its private
-- lines. For the calls that run now, such an account loses that: it has a seat that the
-- admission check has not seen, and no stored event states that it entered this call
-- before. The lines of calls that have ended keep their readers: no record states who of
-- them was seen in the room.
create temporary table seats_never_seen on commit drop as
select m.call_id, m.account_id
from call_members m
join calls k on k.id = m.call_id
where k.hides_occupants and m.connected_at is null
  and not exists (
      select 1 from events e
      where e.kind = 'call_participant_joined'
        and e.payload ->> 'call_id' = m.call_id::text
        and e.payload -> 'participant' ->> 'account_id' = m.account_id::text);
delete from call_message_members c using seats_never_seen s
where c.call_id = s.call_id and c.account_id = s.account_id;
update messages m
set readers = array(
        select reader from unnest(m.readers) with ordinality as listed (reader, place)
        where not exists (
            select 1 from seats_never_seen s
            where s.call_id = m.call_id and s.account_id = listed.reader)
        order by listed.place)
where exists (
    select 1 from seats_never_seen s
    where s.call_id = m.call_id and m.readers @> array[s.account_id]);
-- The stored events of those lines name their readers in the audience.
alter table events disable trigger events_no_change;
update events e
set audience = jsonb_set(e.audience, '{readers}', coalesce((
        select jsonb_agg(listed.reader order by listed.place)
        from jsonb_array_elements_text(e.audience -> 'readers')
            with ordinality as listed (reader, place)
        where not exists (
            select 1 from seats_never_seen s
            where s.call_id::text = e.audience ->> 'call_id'
              and s.account_id::text = listed.reader)), '[]'::jsonb))
where e.audience ->> 'to' = 'system_message'
  and jsonb_typeof(e.audience -> 'readers') = 'array'
  and exists (
      select 1 from seats_never_seen s
      where s.call_id::text = e.audience ->> 'call_id'
        and e.audience -> 'readers' ? s.account_id::text);
alter table events enable trigger events_no_change;

-- The audience of an event about the inside of a call names the call. The stored events
-- get the call of their payload; an event whose call cannot be found gets the nil
-- identifier and is delivered to nobody.
alter table events disable trigger events_no_change;
update events e
set audience = e.audience || jsonb_build_object('call_id', coalesce(
        e.payload ->> 'call_id',
        e.payload -> 'request' ->> 'call_id',
        e.payload -> 'recording' ->> 'call_id',
        (select r.call_id::text from recordings r
         where e.payload ->> 'recording_id' ~ '^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$'
           and r.id = (e.payload ->> 'recording_id')::uuid),
        '00000000-0000-0000-0000-000000000000'))
where e.audience ->> 'to' = 'call' and not e.audience ? 'call_id';
alter table events enable trigger events_no_change;

-- A ring of a direct-message call for an account that muted the conversation or is in its
-- quiet time is stored and not shown to that account (ADR 0515).
alter table call_invitations add column silent boolean not null default false;

-- The snapshot reads the moves, the invitations and the requests of one account.
create index call_moves_mover on call_moves (moved_by);
create index call_invitations_inviter on call_invitations (invited_by);
create index call_entry_requests_account on call_entry_requests (account_id);

-- An audit entry that names a recording keeps the call of that recording. The audit log
-- returns such an entry by the rule of the recording (ADR 0604), also when the recording
-- was removed while its call runs.
alter table audit_entries add column call_id uuid;
alter table audit_entries disable trigger audit_entries_no_row_change;
update audit_entries e set call_id = r.call_id
from recordings r
where e.event ? 'recording_id'
  and e.event ->> 'recording_id' ~ '^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$'
  and r.id = (e.event ->> 'recording_id')::uuid;
alter table audit_entries enable trigger audit_entries_no_row_change;
