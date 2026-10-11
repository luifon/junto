-- Recordings of calls (ADR 0601, ADR 0604) and the reservations of the storage guard
-- (ADR 0006, ADR 0605).

-- One row per recording. A recording outlives its call, so it refers to the call by its
-- identifier only. A voice channel has at most one recording that is starting or runs.
create table recordings (
    id uuid primary key,
    workspace_id uuid not null,
    channel_id uuid not null,
    call_id uuid not null,
    room_name text not null check (room_name ~ '^jr-[0-9a-f]{12}-[0-9a-f]{32}$'),
    state text not null check (
        state in ('starting', 'recording', 'processing', 'ready', 'partly_failed', 'failed')
    ),
    started_by uuid not null references accounts (id),
    language text not null check (language ~ '^(auto|[a-z]{2,3})$'),
    requested_at timestamptz not null,
    recording_since timestamptz,
    stop_reason text check (
        stop_reason in (
            'stopped', 'call_ended', 'storage_reserve', 'segment_limit', 'recorder_lost'
        )
    ),
    stopped_by uuid references accounts (id),
    stop_requested_at timestamptz,
    ended_at timestamptz,
    failure text check (failure in ('recorder_unavailable', 'nothing_recorded')),
    revision bigint not null default 0,
    foreign key (channel_id, workspace_id) references channels (id, workspace_id),
    check ((stop_reason is null) = (stop_requested_at is null)),
    check (stopped_by is null or stop_reason is not distinct from 'stopped'),
    check ((failure is not null) = (state = 'failed')),
    check ((ended_at is null) = (state in ('starting', 'recording'))),
    check (state <> 'starting' or recording_since is null)
);

create unique index recordings_one_active_per_channel on recordings (channel_id)
    where state in ('starting', 'recording');
create index recordings_of_workspace on recordings (workspace_id, id);
create index recordings_active on recordings (id) where state in ('starting', 'recording');

-- One row per file that the recorder is asked to write: the composite, or one audio track of
-- one person. The times and the file name are the ones that the recorder reports for the
-- file. `requested_path` is relative to the directory of the recordings.
create table recording_segments (
    id uuid primary key,
    recording_id uuid not null references recordings (id) on delete cascade,
    source text not null check (source in ('composite', 'microphone', 'screen_share_audio')),
    account_id uuid references accounts (id),
    track_id text check (char_length(track_id) between 1 and 128),
    state text not null check (state in ('starting', 'recording', 'ended', 'failed')),
    failure text check (
        failure in (
            'recorder_unavailable', 'start_refused', 'start_interrupted', 'recorder_lost',
            'recorder_error'
        )
    ),
    job_id text check (char_length(job_id) between 1 and 128),
    requested_path text not null,
    requested_at timestamptz not null,
    job_started_at timestamptz,
    stop_delivered_at timestamptz,
    file_name text,
    started_at timestamptz,
    ended_at timestamptz,
    duration_ms bigint check (duration_ms >= 0),
    constraint recording_segments_job_unique unique (job_id),
    check ((source = 'composite') = (account_id is null)),
    check ((source = 'composite') = (track_id is null)),
    check ((failure is not null) = (state = 'failed')),
    check ((job_id is null) = (job_started_at is null)),
    check (state in ('starting', 'failed') or job_id is not null),
    check (state <> 'starting' or job_id is null),
    check (stop_delivered_at is null or job_id is not null)
);

create index recording_segments_of_recording on recording_segments (recording_id, id);

-- The disk space that a job reserved when it started. The storage guard counts every row
-- against the free space. The subject is a recording; exports add their kind.
create table storage_reservations (
    subject_id uuid primary key,
    kind text not null check (kind in ('recording')),
    bytes bigint not null check (bytes > 0),
    reserved_at timestamptz not null
);

-- The start, the stop and the failure of a recording are kept for the system lines of the
-- text chat of the voice channel (ADR 0501).
alter table call_notes
    drop constraint call_notes_kind_check,
    add constraint call_notes_kind_check check (
        kind in (
            'call_started', 'call_ended', 'screen_share_started', 'entry_requested',
            'joined_by_invitation', 'meeting_started', 'meeting_ended', 'recording_started',
            'recording_stopped', 'recording_failed'
        )
    ),
    add column recording_id uuid references recordings (id) on delete set null;
