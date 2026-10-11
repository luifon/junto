alter table messages drop constraint messages_thread_root_fkey;
alter table messages add constraint thread_root_same_place foreign key(thread_root,place_id) references messages(id,place_id);
create index messages_thread on messages(thread_root,position,id) where thread_root is not null;
create table thread_forks (
    root_id uuid not null references messages(id) on delete cascade,
    channel_id uuid not null references channels(id),
    primary key(root_id)
);
create table message_pins (
    message_id uuid primary key references messages(id) on delete cascade,
    channel_id uuid not null references channels(id),
    pinned_by uuid not null references accounts(id),
    pinned_at timestamptz not null,
    foreign key(message_id,channel_id) references messages(id,place_id)
);
create index message_pins_channel on message_pins(channel_id,pinned_at,message_id);
alter table attachments add column storage_id uuid;
update attachments set storage_id=id;
alter table attachments alter column storage_id set not null;
create index attachments_storage on attachments(storage_id);

create table notification_settings (
    account_id uuid not null references accounts(id),
    scope_id uuid not null,
    settings jsonb not null,
    primary key(account_id,scope_id)
);
create table workspace_notification_defaults (
    workspace_id uuid primary key references workspaces(id),
    level text not null check(level in ('all','mentions','nothing'))
);
create table notifications (
    id uuid not null,
    account_id uuid not null references accounts(id),
    message_id uuid not null references messages(id) on delete cascade,
    kind text not null check(kind in ('message','mention','direct','thread')),
    created_at timestamptz not null,
    read_at timestamptz,
    primary key(account_id,id),
    unique(account_id,message_id)
);
create index notifications_account on notifications(account_id,created_at desc,id desc);
create index notifications_message on notifications(message_id);
create index notifications_age on notifications(created_at,account_id,id);
create table workspace_file_retention (
    workspace_id uuid primary key references workspaces(id),
    days integer check(days between 1 and 36500)
);
create table direct_message_retention (
    singleton boolean primary key default true check(singleton),
    days integer check(days between 1 and 36500)
);
insert into direct_message_retention(singleton) values(true);
create index messages_retention on messages(place_id,created_at,id) where not deleted;

create function message_search_text(doc jsonb) returns text language sql immutable strict as $$
    select coalesce(string_agg(value #>> '{}',' '),'')
    from jsonb_path_query(doc,'strict $.** ? (exists(@.text)).text') value;
$$;
alter table messages add column search_vector tsvector generated always as
    (to_tsvector('simple',coalesce(message_search_text(document),''))) stored;
create index messages_search on messages using gin(search_vector);
create function notification_policy(subject uuid,place uuid,at_time timestamptz)
returns table(level text,muted boolean,roles boolean,everyone boolean)
language sql stable as $$
    select case when cv.id is not null then 'all' else coalesce(ch.settings->>'level',ws.settings->>'level',d.level,a.settings->>'level','mentions') end,
        (coalesce((ch.settings->>'mute')::boolean,false) and
            (ch.settings->>'muted_until' is null or (ch.settings->>'muted_until')::timestamptz>at_time))
        or (dm.account_id is not null and (dm.muted_until is null or dm.muted_until>at_time)),
        coalesce((ch.settings->>'role_mentions')::boolean,true),
        coalesce((ch.settings->>'everyone_mentions')::boolean,true)
    from (select 1) seed
    left join channels c on c.id=place
    left join conversations cv on cv.id=place
    left join direct_mutes dm on dm.account_id=subject and dm.other_account_id=case when cv.low_account_id=subject then cv.high_account_id else cv.low_account_id end
    left join notification_settings ch on ch.account_id=subject and ch.scope_id=place
    left join notification_settings ws on ws.account_id=subject and ws.scope_id=c.workspace_id
    left join workspace_notification_defaults d on d.workspace_id=c.workspace_id
    left join notification_settings a on a.account_id=subject and a.scope_id='00000000-0000-0000-0000-000000000000';
$$;
create function notification_mentioned(subject uuid,message uuid,enabled_roles boolean,enabled_everyone boolean)
returns boolean language sql stable as $$
    select exists(select 1 from message_mentions mm where mm.message_id=message and (
        mm.account_id=subject or (mm.everyone and enabled_everyone) or
        exists(select 1 from workspace_roles r where r.id=mm.role_id and (
            (r.is_everyone and enabled_everyone and exists(select 1 from workspace_members w where w.workspace_id=r.workspace_id and w.account_id=subject)) or
            (not r.is_everyone and enabled_roles and exists(select 1 from member_roles mr where mr.role_id=r.id and mr.account_id=subject))
        ))));
$$;
alter table notification_settings add constraint notification_settings_object check (
    jsonb_typeof(settings)='object' and
    (settings->>'level' is null or settings->>'level' in ('all','mentions','nothing')) and
    (settings->>'mute' is null or jsonb_typeof(settings->'mute')='boolean') and
    (settings->>'role_mentions' is null or jsonb_typeof(settings->'role_mentions')='boolean') and
    (settings->>'everyone_mentions' is null or jsonb_typeof(settings->'everyone_mentions')='boolean')
);
create table notification_receivers (
    event_id uuid not null references events(id) on delete cascade,
    account_id uuid not null references accounts(id),
    background boolean not null,
    alert boolean not null default true,
    primary key(event_id,account_id)
);


alter table message_places add column read_floor bigint not null default 0;
alter table messages add column files_removed_by_retention boolean not null default false;
alter table messages add column reply_count bigint not null default 0 check(reply_count>=0);
alter table messages add column last_reply uuid;
create index messages_thread_live on messages(thread_root,position desc,id desc)
    where thread_root is not null and not deleted;
create table thread_reads (
    root_id uuid not null references messages(id) on delete cascade,
    account_id uuid not null references accounts(id),
    position bigint not null default 0 check(position>=0),
    writer boolean not null default false,
    following boolean not null default false,
    place_id uuid not null references channels(id),
    latest bigint not null default 0,
    latest_id uuid,
    primary key(root_id,account_id)
);
create index thread_reads_account on thread_reads(account_id,root_id);
create index thread_reads_unread on thread_reads(account_id,latest_id desc,root_id desc) include (place_id)
    where following and position<latest;
create function initialize_thread_read() returns trigger language plpgsql as $$
begin
    select m.place_id,coalesce(reply.position,0),m.last_reply
        into new.place_id,new.latest,new.latest_id
        from messages m left join messages reply on reply.id=m.last_reply where m.id=new.root_id;
    return new;
end;
$$;
create trigger initialize_thread_read before insert on thread_reads
    for each row execute function initialize_thread_read();
alter table message_mention_accounts add column thread_root uuid references messages(id) on delete cascade;

create function update_thread_summary() returns trigger language plpgsql as $$
begin
    if old.thread_root is null and new.thread_root is null then return null; end if;
    if tg_op <> 'INSERT' and old.thread_root is not null and not old.deleted then
        update messages set reply_count=reply_count-1 where id=old.thread_root;
    end if;
    if tg_op <> 'DELETE' and new.thread_root is not null and not new.deleted then
        update messages set reply_count=reply_count+1 where id=new.thread_root;
        insert into thread_reads(root_id,account_id,writer,following)
            select distinct new.thread_root,author_id,true,true from messages
            where id in(new.thread_root,new.id) and author_id is not null
            on conflict(root_id,account_id) do update set writer=true,following=true where not thread_reads.writer;
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
create trigger update_thread_summary after insert or delete or update of thread_root,deleted
    on messages for each row execute function update_thread_summary();

alter table attachments add column retention_at timestamptz;
update attachments a set retention_at=m.created_at from messages m
    where m.id=a.message_id and not m.permanent and a.removed_at is null;
create index attachments_retention on attachments(place_id,retention_at,id)
    where removed_at is null and retention_at is not null;
create table workspace_recording_retention (
    workspace_id uuid primary key references workspaces(id),
    days integer check(days between 1 and 36500)
);
create index recordings_retention on recordings(workspace_id,requested_at,id)
    where state in ('ready','partly_failed','failed');

alter table transcript_lines add column search_text text;
update transcript_lines l set search_text=coalesce((
    select text from transcript_corrections c
    where c.recording_id=l.recording_id and c.line=l.line order by id desc limit 1),l.text);
alter table transcript_lines alter column search_text set not null;
alter table transcript_lines add column search_vector tsvector generated always as
    (to_tsvector('simple',search_text)) stored;
create index transcript_lines_search on transcript_lines using gin(search_vector);
create function transcript_search_original() returns trigger language plpgsql as $$
begin
    new.search_text := new.text;
    return new;
end;
$$;
create trigger transcript_search_original before insert on transcript_lines
    for each row execute function transcript_search_original();
create function transcript_search_correction() returns trigger language plpgsql as $$
begin
    if tg_op='DELETE' and pg_trigger_depth()>1 then return null; end if;
    update transcript_lines l set search_text=coalesce((
        select text from transcript_corrections c where c.recording_id=l.recording_id
            and c.line=l.line order by id desc limit 1),l.text)
        where l.recording_id=coalesce(new.recording_id,old.recording_id)
            and l.line=coalesce(new.line,old.line);
    return null;
end;
$$;
create trigger transcript_search_correction after insert or update or delete on transcript_corrections
    for each row execute function transcript_search_correction();

create function notification_quiet(subject uuid,at_time timestamptz) returns boolean
language sql stable as $$
    select coalesce((settings->>'do_not_disturb')::boolean,false) or coalesce(
        case when (settings->'daily_quiet'->>'start_minute')::int < (settings->'daily_quiet'->>'end_minute')::int
        then minute >= (settings->'daily_quiet'->>'start_minute')::int and minute < (settings->'daily_quiet'->>'end_minute')::int
        else minute >= (settings->'daily_quiet'->>'start_minute')::int or minute < (settings->'daily_quiet'->>'end_minute')::int end,false)
    from (select coalesce((select settings from notification_settings where account_id=subject
        and scope_id='00000000-0000-0000-0000-000000000000'),'{}'::jsonb) as settings) s
    cross join lateral (select extract(hour from local_time)::int*60+extract(minute from local_time)::int as minute
        from (select (at_time at time zone 'UTC')+make_interval(mins=>coalesce((settings->'daily_quiet'->>'utc_offset_minutes')::int,0)) as local_time) t) clock;
$$;

alter table message_mention_accounts add column unread boolean not null default true;
alter table message_mention_accounts add column thread_head boolean not null default false;
create index message_mention_channel on message_mention_accounts(place_id,account_id,position)
    where thread_root is null;
create index message_mention_thread_root on message_mention_accounts(thread_root,account_id)
    where thread_root is not null;
create index message_mention_threads on message_mention_accounts(account_id,thread_root,position desc)
    where thread_root is not null and unread;
create index message_mention_unread_place on message_mention_accounts(place_id,account_id,position)
    where thread_root is not null and unread;
create unique index message_mention_head on message_mention_accounts(account_id,thread_root)
    where thread_head;
create index message_mention_inbox on message_mention_accounts(account_id,message_id desc,thread_root desc) include (place_id)
    where thread_head;
create function refresh_mention_head() returns trigger language plpgsql as $$
declare
    subject uuid := coalesce(new.account_id,old.account_id);
    root uuid := coalesce(new.thread_root,old.thread_root);
    newest uuid;
    newest_position bigint;
begin
    if root is null then return null; end if;
    select message_id,position into newest,newest_position from message_mention_accounts
        where account_id=subject and thread_root=root and unread order by position desc limit 1;
    update message_mention_accounts set thread_head=false
        where account_id=subject and thread_root=root and thread_head and message_id is distinct from newest;
    update message_mention_accounts set thread_head=true
        where account_id=subject and thread_root=root and unread and position=newest_position and not thread_head;
    return null;
end;
$$;
create trigger refresh_mention_head after insert or delete or update of unread on message_mention_accounts
    for each row execute function refresh_mention_head();

create table notification_trim_accounts (
    account_id uuid primary key references accounts(id) on delete cascade
);
create function queue_notification_trim() returns trigger language plpgsql as $$
begin
    insert into notification_trim_accounts values(new.account_id) on conflict do nothing;
    return null;
end;
$$;
create trigger queue_notification_trim after insert on notifications
    for each row execute function queue_notification_trim();

create index notification_all_subscribers on notification_settings(scope_id,account_id)
    where settings->>'level'='all';
create index thread_writers on thread_reads(root_id,account_id) where writer;
