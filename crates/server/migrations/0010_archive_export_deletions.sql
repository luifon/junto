-- Archive (ADR 0410), exports (ADR 0411), the deletion of roles, categories and workspaces
-- (ADR 0308) and the removal of the content of deleted channels (ADR 0017).

-- An archived channel is read-only and is in no channel list. `deletion_notice_at` is the
-- time of the notice that precedes its automatic deletion by 7 days.
alter table channels
    add column archived_at timestamptz,
    add column deletion_notice_at timestamptz,
    add constraint channels_notice_of_an_archived_channel
        check (deletion_notice_at is null or archived_at is not null);

create index channels_archived on channels (workspace_id, archived_at, id)
    where archived_at is not null and deleted_at is null;
create index channels_archive_notice on channels (workspace_id, archived_at, id)
    where archived_at is not null and deleted_at is null and permanent_message_count = 0
        and deletion_notice_at is null;
create index channels_archive_deletion on channels (deletion_notice_at, id)
    where archived_at is not null and deleted_at is null and permanent_message_count = 0
        and deletion_notice_at is not null;
create index channels_category on channels (category_id) where category_id is not null;

-- The time after which an archived channel of the workspace is deleted. No row: 30 days.
-- A row without days: never.
create table workspace_archive_periods (
    workspace_id uuid primary key references workspaces (id),
    days integer check (days between 7 and 36500)
);

-- A deleted workspace stays as a row, because audit entries refer to it (ADR 0305). It has
-- no member, no role and no category, and its channels are deleted.
alter table workspaces add column deleted_at timestamptz;

-- The deleted channels and workspaces whose content the server still has to remove. A row
-- is written in the transaction of the deletion and deleted when nothing is left. A
-- subject that has to wait, for the end of a call or of a recording, is taken again at
-- `retry_at`.
create table purges (
    id uuid primary key,
    kind text not null check (kind in ('channel', 'workspace')),
    requested_at timestamptz not null,
    retry_at timestamptz
);

create index purges_order on purges (requested_at, id);

-- The channels that were deleted before this migration kept their recordings and the files
-- of them. The purge removes what they still hold.
insert into purges (id, kind, requested_at)
select id, 'channel', deleted_at from channels where deleted_at is not null;

-- The accounts that one event is for, when the rule that selected them no longer holds
-- after the change: the holders of a deleted role, the members of a deleted workspace.
create table event_receivers (
    event_id uuid not null references events (id) on delete cascade,
    account_id uuid not null references accounts (id),
    primary key (event_id, account_id)
);

-- One export of a channel or of a direct-message conversation for the account that asked
-- for it. It holds the items of its place whose identifier is not greater than its own
-- identifier. One row is `running` for one server process, which renews `heartbeat_at`. A
-- row with `removed_at` is shown by no route; it stays until its file is deleted.
create table exports (
    id uuid primary key,
    account_id uuid not null references accounts (id),
    place_id uuid not null references message_places (id),
    state text not null check (state in ('waiting', 'running', 'ready', 'failed')),
    failure text check (failure in ('storage_low', 'access_ended', 'interrupted', 'failed')),
    requested_at timestamptz not null,
    attempts integer not null default 0 check (attempts >= 0),
    retry_at timestamptz,
    claimed_by uuid,
    heartbeat_at timestamptz,
    finished_at timestamptz,
    expires_at timestamptz,
    bytes bigint check (bytes >= 0),
    removed_at timestamptz,
    -- The call that ran when the file got a recording of it. While that call runs in a
    -- channel that hides who is inside, only an account inside downloads the file.
    held_call_id uuid,
    check ((failure is not null) = (state = 'failed')),
    check ((claimed_by is not null) = (state = 'running')),
    check ((claimed_by is null) = (heartbeat_at is null)),
    check ((finished_at is not null) = (state in ('ready', 'failed'))),
    check ((finished_at is null) = (expires_at is null)),
    check ((bytes is not null) = (state = 'ready'))
);

create index exports_of_account on exports (account_id, id) where removed_at is null;
create index exports_of_place on exports (place_id, id) where removed_at is null;
create index exports_open on exports (id)
    where state in ('waiting', 'running') and removed_at is null;
create index exports_expiry on exports (expires_at, id)
    where expires_at is not null and removed_at is null;
create index exports_removal on exports (removed_at, id) where removed_at is not null;

alter table storage_reservations drop constraint storage_reservations_kind_check;
alter table storage_reservations add constraint storage_reservations_kind_check
    check (kind in ('recording', 'attachment', 'export'));

-- The rows that refer to a deleted row are found through an index. Without these, each
-- deleted message read the messages of its place, each removed recording read every
-- message, and each deleted role read every override.
create index messages_reply_to on messages (reply_to) where reply_to is not null;
create index messages_recording on messages (recording_id) where recording_id is not null;
create index messages_replies_of_place on messages (place_id, id) where thread_root is not null;
create index messages_answers_of_place on messages (place_id, id) where reply_to is not null;
create index call_notes_recording on call_notes (recording_id) where recording_id is not null;
create index attachments_live_of_place on attachments (place_id, id) where removed_at is null;
create index recordings_of_channel on recordings (channel_id, id);
create index notification_settings_scope on notification_settings (scope_id, account_id);
create index channel_overrides_role on channel_overrides (role_id);
create index account_server_roles_role on account_server_roles (role_id);
create index thread_forks_channel on thread_forks (channel_id);

-- The private lines of a call are stored by call. The channel of the call is stored with
-- them, so that the rows of a deleted channel are found.
alter table call_message_privacy add column channel_id uuid;
update call_message_privacy p set channel_id = c.channel_id
    from calls c where c.id = p.call_id;
update call_message_privacy p set channel_id = n.channel_id
    from call_notes n where n.call_id = p.call_id and p.channel_id is null;
create index call_message_privacy_channel on call_message_privacy (channel_id, call_id)
    where channel_id is not null;
create or replace function refresh_call_message_privacy(subject uuid) returns void language sql as $$
    insert into call_message_members
    select cm.call_id,cm.account_id from call_members cm
    join calls c on c.id=cm.call_id join channels ch on ch.id=c.channel_id
    where cm.call_id=subject and ch.hides_occupants
    on conflict do nothing;
    insert into call_message_privacy(call_id,hidden,readers,channel_id)
    select c.id,ch.hides_occupants,
        array(select account_id from call_members where call_id=subject order by account_id),
        c.channel_id
    from calls c join channels ch on ch.id=c.channel_id where c.id=subject
    on conflict(call_id) do update
        set hidden=excluded.hidden,readers=excluded.readers,channel_id=excluded.channel_id;
$$;

-- A backup that copies the database and the files of the server (ADR 0018). While a row
-- has not expired, the server deletes no file of an attachment and of a recording, so that
-- every file that the copied database refers to is still there when the files are copied.
create table backups (
    id uuid primary key,
    started_at timestamptz not null,
    expires_at timestamptz not null,
    check (expires_at > started_at)
);
