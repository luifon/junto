create table conversations (
    id uuid primary key,
    low_account_id uuid not null references accounts(id),
    high_account_id uuid not null references accounts(id),
    check (low_account_id < high_account_id),
    unique (low_account_id, high_account_id)
);
create table message_places (
    id uuid primary key,
    channel_id uuid unique references channels(id),
    conversation_id uuid unique references conversations(id),
    head bigint not null default 0 check (head >= 0),
    check (num_nonnulls(channel_id, conversation_id) = 1)
);
create table messages (
    id uuid primary key,
    place_id uuid not null references message_places(id),
    position bigint not null check (position >= 0),
    readers uuid[],
    author_id uuid references accounts(id),
    created_at timestamptz not null,
    edited_at timestamptz,
    document jsonb,
    system jsonb,
    reply_to uuid references messages(id) on delete set null,
    thread_root uuid references messages(id),
    deleted boolean not null default false,
    permanent boolean not null default false,
    call_id uuid,
    recording_id uuid references recordings(id) on delete set null,
    check (readers is not null or position>0),
    check ((not deleted and num_nonnulls(document, system) = 1) or
           (deleted and document is null and system is null and author_id is null and not permanent)),
    check ((document is null) = (author_id is null)),
    check (document is null or octet_length(document::text) <= 32768)
);
create index messages_history on messages(place_id,position,id);
create index messages_unread on messages(place_id,position) where author_id is not null and not deleted and thread_root is null;
create table message_reads (
    place_id uuid not null references message_places(id),
    account_id uuid not null references accounts(id),
    position bigint not null check (position >= 0),
    primary key (place_id, account_id)
);
create table message_reactions (
    message_id uuid not null references messages(id) on delete cascade,
    account_id uuid not null references accounts(id),
    emoji text not null check (octet_length(emoji) between 1 and 64),
    primary key (message_id, account_id, emoji)
);
create table message_mentions (
    place_id uuid not null references message_places(id),
    position bigint not null,
    message_id uuid not null references messages(id) on delete cascade,
    account_id uuid references accounts(id),
    role_id uuid,
    everyone boolean not null default false,
    recipients uuid[] not null default '{}',
    check (num_nonnulls(account_id, role_id) + everyone::integer = 1)
);
create index message_mentions_message on message_mentions(message_id);
create table message_mention_accounts (
    place_id uuid not null references message_places(id),
    account_id uuid not null references accounts(id),
    position bigint not null,
    message_id uuid not null references messages(id) on delete cascade,
    primary key(place_id,account_id,position)
);
create index message_mention_accounts_message on message_mention_accounts(message_id);
create table attachments (
    id uuid primary key,
    place_id uuid not null references message_places(id),
    owner_id uuid not null references accounts(id),
    message_id uuid references messages(id) on delete set null,
    name text not null check (char_length(name) between 1 and 200),
    bytes bigint not null check (bytes between 1 and 1073741824),
    received bigint not null default 0 check (received >= 0 and received <= bytes),
    sha256 text not null check (sha256 ~ '^[0-9a-f]{64}$'),
    complete boolean not null default false,
    failed boolean not null default false,
    media_type text not null default 'application/octet-stream',
    width integer, height integer,
    expires_at timestamptz not null,
    touched_at timestamptz not null,
    removed_at timestamptz,
    attached boolean not null default false,
    staging_cleaned boolean not null default false,
    check ((width is null) = (height is null)),
    check (not complete or received = bytes)
);
create index attachments_message on attachments(message_id);
create table attachment_chunks (
    attachment_id uuid not null references attachments(id) on delete cascade,
    offset_bytes bigint not null check (offset_bytes >= 0),
    bytes integer not null check (bytes between 1 and 1048576),
    sha256 text not null check (sha256 ~ '^[0-9a-f]{64}$'),
    stored boolean not null default false,
    primary key (attachment_id, offset_bytes)
);
alter table storage_reservations drop constraint storage_reservations_kind_check;
alter table storage_reservations add constraint storage_reservations_kind_check check(kind in ('recording','attachment'));

create table call_message_privacy (
    call_id uuid primary key,
    hidden boolean not null,
    readers uuid[] not null default '{}'
);
create table call_message_members (
    call_id uuid not null,
    account_id uuid not null references accounts(id),
    primary key(call_id, account_id)
);
insert into call_message_privacy(call_id,hidden)
select distinct n.call_id, ch.hides_occupants from call_notes n join channels ch on ch.id=n.channel_id;
insert into call_message_privacy(call_id,hidden)
select c.id,ch.hides_occupants from calls c join channels ch on ch.id=c.channel_id
on conflict do nothing;
create function refresh_call_message_privacy(subject uuid) returns void language sql as $$
    insert into call_message_members
    select cm.call_id,cm.account_id from call_members cm
    join calls c on c.id=cm.call_id join channels ch on ch.id=c.channel_id
    where cm.call_id=subject and ch.hides_occupants
    on conflict do nothing;
    insert into call_message_privacy(call_id,hidden,readers)
    select c.id,ch.hides_occupants,
        array(select account_id from call_members where call_id=subject order by account_id)
    from calls c join channels ch on ch.id=c.channel_id where c.id=subject
    on conflict(call_id) do update set hidden=excluded.hidden,readers=excluded.readers;
$$;
create function remember_call_message_member() returns trigger language plpgsql as $$
declare
    subject uuid := coalesce(new.call_id,old.call_id);
begin
    if tg_op='DELETE' then
        insert into call_message_members
        select old.call_id,old.account_id from calls c join channels ch on ch.id=c.channel_id
        where c.id=subject and ch.hides_occupants on conflict do nothing;
    end if;
    perform refresh_call_message_privacy(subject);
    return null;
end;
$$;
create trigger remember_call_message_member after insert or delete on call_members
for each row execute function remember_call_message_member();
create function remember_hidden_call_members() returns trigger language plpgsql as $$
begin
    perform refresh_call_message_privacy(id) from calls where channel_id=new.id;
    return null;
end;
$$;
create trigger remember_hidden_call_members after update of hides_occupants on channels
for each row execute function remember_hidden_call_members();
create table message_mentions_notified (
    message_id uuid not null references messages(id) on delete cascade,
    account_id uuid not null references accounts(id),
    primary key(message_id,account_id)
);
create index attachments_expiry on attachments(expires_at,id) where not attached and removed_at is null;
create index attachments_removal on attachments(touched_at,id) where removed_at is not null;
create index attachments_staging on attachments(touched_at,id) where complete and not staging_cleaned and removed_at is null;

insert into message_places(id,channel_id) select id,id from channels;
insert into messages(id,place_id,position,created_at,system,call_id,recording_id)
select source.id,source.channel_id,row_number() over(partition by source.channel_id order by source.occurred_at,source.id),source.occurred_at,source.system,source.call_id,source.recording_id
from (
    select n.id,n.channel_id,n.occurred_at,
        jsonb_build_object('action',n.kind,'actor_id',n.account_id,'recording_id',n.recording_id) as system,
        n.call_id,n.recording_id
    from call_notes n
    union all
    select e.id,e.channel_id,e.occurred_at,
        jsonb_build_object('action',e.action,'actor_id',e.actor_id,'recording_id',null,'subject_id',case when e.action like 'call_%' then e.event->'account_id' else null end),
        case when e.action like 'call_%' or e.action like 'recording_%' or e.action like 'transcript_%' then coalesce(
            (select r.call_id from recordings r where r.id::text=e.event->>'recording_id'),
            (select n.call_id from call_notes n where n.channel_id=e.channel_id and n.kind='call_started'
             and (n.occurred_at,n.id)<=(e.occurred_at,e.id)
             and not exists(select 1 from call_notes ended where ended.call_id=n.call_id and ended.kind='call_ended' and (ended.occurred_at,ended.id)<(e.occurred_at,e.id))
             order by n.occurred_at desc,n.id desc limit 1),e.id) end,
        null::uuid
    from audit_entries e where e.channel_id is not null and e.action not in ('recording_started','recording_stopped')
) source join channels ch on ch.id=source.channel_id where ch.deleted_at is null;
-- Recover each line's setting and only participation supported at that instant.
update messages m set readers=array(
    select n.account_id from call_notes n where n.id=m.id and n.account_id is not null
    union
    select cm.account_id from call_members cm
        where cm.call_id=m.call_id and cm.admitted_at<=m.created_at
    union
    select admission.account_id from (
        select distinct on (e.audience_account_id) e.audience_account_id as account_id,e.payload
        from events e where e.kind='own_call_changed'
            and (e.recorded_at,e.id)<=(m.created_at,m.id)
        order by e.audience_account_id,e.recorded_at desc,e.id desc
    ) admission where admission.payload->'call'->>'call_id'=m.call_id::text
)
where m.call_id is not null and coalesce((
    select (e.event->>'hides_occupants')::boolean from audit_entries e
    where e.channel_id=m.place_id and e.action='channel_call_settings_updated'
        and (e.occurred_at,e.id)<=(m.created_at,m.id)
    order by e.occurred_at desc,e.id desc limit 1
),false);
-- Admission and setting events also prove participation between system lines.
with subjects as (
    select call_id,channel_id from call_notes union select id,channel_id from calls
)
insert into call_message_members
select distinct subjects.call_id,e.audience_account_id from events e
join subjects on subjects.call_id::text=e.payload->'call'->>'call_id'
where e.kind='own_call_changed' and e.audience_account_id is not null and coalesce((
    select (a.event->>'hides_occupants')::boolean from audit_entries a
    where a.channel_id=subjects.channel_id and a.action='channel_call_settings_updated'
        and (a.occurred_at,a.id)<=(e.recorded_at,e.id)
    order by a.occurred_at desc,a.id desc limit 1
),false)
on conflict do nothing;
with subjects as (
    select call_id,channel_id from call_notes union select id,channel_id from calls
)
insert into call_message_members
select distinct subjects.call_id,admission.account_id from audit_entries a
cross join lateral (
    select distinct on (e.audience_account_id) e.audience_account_id as account_id,e.payload
    from events e where e.kind='own_call_changed' and e.audience_account_id is not null
        and (e.recorded_at,e.id)<=(a.occurred_at,a.id)
    order by e.audience_account_id,e.recorded_at desc,e.id desc
) admission
join subjects on subjects.call_id::text=admission.payload->'call'->>'call_id'
    and subjects.channel_id=a.channel_id
where a.action='channel_call_settings_updated' and (a.event->>'hides_occupants')::boolean
on conflict do nothing;
insert into call_message_members
select distinct m.call_id,unnest(m.readers) from messages m where m.readers is not null
on conflict do nothing;
select refresh_call_message_privacy(id) from calls;
update messages m set readers=array(
    select account_id from call_message_members where call_id=m.call_id order by account_id
)
where m.readers is not null and exists(
    select 1 from call_notes n where n.call_id=m.call_id and n.kind='call_ended'
        and (n.occurred_at,n.id)<=(m.created_at,m.id)
);
with ordered as (
    select id,count(*) filter(where readers is null)
        over(partition by place_id order by created_at,id) as position
    from messages
)
update messages m set position=o.position from ordered o where o.id=m.id;
insert into call_message_privacy(call_id,hidden,readers)
select distinct on (call_id) call_id,readers is not null,'{}'::uuid[]
from messages where call_id is not null order by call_id,created_at desc,id desc
on conflict(call_id) do update set hidden=excluded.hidden,readers=excluded.readers;
select refresh_call_message_privacy(id) from calls;
update message_places p set head=(select coalesce(max(position),0) from messages m where m.place_id=p.id);

alter table message_places add constraint message_place_id_matches_subject check(id=coalesce(channel_id,conversation_id));
alter table messages add constraint messages_id_place_unique unique(id,place_id);
alter table messages drop constraint messages_reply_to_fkey;
alter table messages add constraint message_reply_same_place foreign key(reply_to,place_id) references messages(id,place_id) on delete set null (reply_to);
alter table attachments drop constraint attachments_message_id_fkey;
alter table attachments add constraint attachment_message_same_place foreign key(message_id,place_id) references messages(id,place_id) on delete set null (message_id);
create table attachment_limits (
    singleton boolean primary key default true check(singleton),
    bytes_per_file bigint not null check(bytes_per_file between 1 and 1073741824),
    files_per_message integer not null check(files_per_message between 1 and 100)
);

alter table events drop constraint events_addressed_to_an_account;
alter table events add constraint events_addressed_to_an_account check (
    ((audience->>'to' in ('account','account_seeing')) or
     (audience->>'to'='mention' and audience->>'account_id' is not null)) = (audience_account_id is not null)
);

create table message_mention_receivers (
    event_id uuid not null references events(id) on delete cascade,
    account_id uuid not null references accounts(id),
    primary key(event_id,account_id)
);

create unique index messages_public on messages(place_id,position) where readers is null and thread_root is null;
