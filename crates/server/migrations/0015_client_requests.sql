-- Lines of direct-message calls in the conversation (ADR 0518).

-- What the line of the end of a direct-message call reads: the account whose entry made
-- the first ring, the time from which both accounts were inside the call, the time from
-- which one of them is alone again, and whether the call owes no missed call at its end,
-- because a ring of it ended with a line of its own or the other account was inside.
alter table calls add column direct_caller uuid references accounts (id);
alter table calls add column direct_answered_at timestamptz;
alter table calls add column direct_alone_since timestamptz;
alter table calls add column direct_ring_settled boolean not null default false;
alter table calls add constraint calls_direct_caller_is_a_side
    check (direct_caller is null or direct_caller in (direct_low, direct_high));
alter table calls add constraint calls_answer_of_a_direct_call
    check ((direct_answered_at is null or direct_low is not null)
        and (direct_alone_since is null or direct_answered_at is not null)
        and (not direct_ring_settled or direct_low is not null));

-- The account for which a system line counts as unread: the callee of a call that was not
-- answered.
alter table messages add column notifies uuid references accounts (id);
alter table messages add constraint messages_notifies_is_a_system_line
    check (notifies is null or system is not null);
create index messages_notifies on messages (place_id, notifies, position)
    where notifies is not null and not deleted and thread_root is null;

alter table notifications drop constraint notifications_kind_check;
alter table notifications add constraint notifications_kind_check
    check (kind in ('message', 'mention', 'direct', 'thread', 'missed_call'));

-- One request marks every unread notice of an account (ADR 0412).
create index notifications_unread on notifications (account_id, created_at)
    where read_at is null;

-- The choice of an account to follow a thread (ADR 0412). `following` stays the value that
-- holds; `rule_following` is what the rule gives without a choice: the account wrote in the
-- thread or was named in it.
alter table thread_reads add column follow_setting boolean;
alter table thread_reads add column rule_following boolean not null default false;
update thread_reads set rule_following = following;
alter table thread_reads add constraint thread_reads_following_is_the_choice_or_the_rule
    check (following = coalesce(follow_setting, rule_following));
drop index thread_writers;
create index thread_notified on thread_reads (root_id, account_id)
    where coalesce(follow_setting, writer);
create index thread_reads_followed
    on thread_reads (account_id, place_id, latest_id desc, root_id desc)
    where following and latest_id is not null;

-- The authors of the replies of a thread, each with the position of their first reply and
-- the number of their replies that are not deleted (ADR 0413).
create table thread_reply_authors (
    root_id uuid not null references messages (id) on delete cascade,
    account_id uuid not null references accounts (id),
    first_position bigint not null,
    replies bigint not null check (replies > 0),
    primary key (root_id, account_id)
);
create index thread_reply_authors_order
    on thread_reply_authors (root_id, first_position, account_id);
insert into thread_reply_authors (root_id, account_id, first_position, replies)
select thread_root, author_id, min(position), count(*)
from messages
where thread_root is not null and not deleted and author_id is not null
group by thread_root, author_id;

create or replace function update_thread_summary() returns trigger language plpgsql as $$
begin
    if old.thread_root is null and new.thread_root is null then return null; end if;
    if tg_op <> 'INSERT' and old.thread_root is not null and not old.deleted then
        update messages set reply_count=reply_count-1 where id=old.thread_root;
        if old.author_id is not null then
            delete from thread_reply_authors
                where root_id=old.thread_root and account_id=old.author_id and replies=1;
            if not found then
                update thread_reply_authors set replies=replies-1
                    where root_id=old.thread_root and account_id=old.author_id;
            end if;
        end if;
    end if;
    if tg_op <> 'DELETE' and new.thread_root is not null and not new.deleted then
        update messages set reply_count=reply_count+1 where id=new.thread_root;
        if new.author_id is not null then
            insert into thread_reply_authors(root_id,account_id,first_position,replies)
                values(new.thread_root,new.author_id,new.position,1)
                on conflict(root_id,account_id)
                do update set replies=thread_reply_authors.replies+1;
        end if;
        insert into thread_reads(root_id,account_id,writer,following,rule_following)
            select distinct new.thread_root,author_id,true,true,true from messages
            where id in(new.thread_root,new.id) and author_id is not null
            on conflict(root_id,account_id) do update set writer=true,rule_following=true,
                following=coalesce(thread_reads.follow_setting,true)
            where not thread_reads.writer;
    end if;
    update messages root set last_reply=(
        select id from messages where thread_root=root.id and not deleted
        order by position desc,id desc limit 1)
        where root.id in(old.thread_root,new.thread_root);
    update thread_reads r set latest=coalesce(reply.position,0),latest_id=m.last_reply
        from messages m left join messages reply on reply.id=m.last_reply
        where r.root_id=m.id and r.following and m.id in(old.thread_root,new.thread_root);
    return null;
end;
$$;

-- The daily quiet time names a time zone (ADR 0412). A stored distance from UTC becomes
-- the same window on the clock of UTC, which is quiet at the same instants.
-- stored offsets: begin
update notification_settings set settings = jsonb_set(settings, '{daily_quiet}',
    jsonb_build_object(
        'start_minute', ((((settings->'daily_quiet'->>'start_minute')::int
            - coalesce((settings->'daily_quiet'->>'utc_offset_minutes')::int, 0)) % 1440) + 1440) % 1440,
        'end_minute', ((((settings->'daily_quiet'->>'end_minute')::int
            - coalesce((settings->'daily_quiet'->>'utc_offset_minutes')::int, 0)) % 1440) + 1440) % 1440,
        'time_zone', 'UTC'))
where jsonb_typeof(settings->'daily_quiet') = 'object'
  and not (settings->'daily_quiet' ? 'time_zone');
-- stored offsets: end

-- Whether an account is in its quiet time: do-not-disturb is on, or the clock of the zone
-- of its daily quiet time is inside the window. A zone that the database does not know
-- gives no daily quiet time; the statement that asks does not fail.
create or replace function notification_quiet(subject uuid, at_time timestamptz) returns boolean
language plpgsql stable as $$
declare
    own jsonb;
    daily jsonb;
    starts int;
    ends int;
    local_time timestamp;
    minute int;
begin
    select s.settings into own from notification_settings s
        where s.account_id = subject and s.scope_id = '00000000-0000-0000-0000-000000000000';
    if own is null then return false; end if;
    if coalesce((own->>'do_not_disturb')::boolean, false) then return true; end if;
    daily := own->'daily_quiet';
    if daily is null or jsonb_typeof(daily) <> 'object' or daily->>'time_zone' is null then
        return false;
    end if;
    begin
        local_time := at_time at time zone (daily->>'time_zone');
    exception when invalid_parameter_value then
        return false;
    end;
    starts := (daily->>'start_minute')::int;
    ends := (daily->>'end_minute')::int;
    if starts is null or ends is null then return false; end if;
    minute := extract(hour from local_time)::int * 60 + extract(minute from local_time)::int;
    if starts < ends then
        return minute >= starts and minute < ends;
    end if;
    return minute >= starts or minute < ends;
end;
$$;
