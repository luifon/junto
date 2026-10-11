-- The processing of recordings (ADR 0606), transcripts and the glossary (ADR 0607), and the
-- directories that wait for their deletion (ADR 0608).

-- A recording that no file was found of fails with `files_missing`. Each run of the
-- processing counts as an attempt when it starts. One server process holds an attempt and
-- renews `processing_heartbeat_at`; an attempt whose heartbeat is old has lost its process.
-- `timeline_started_at` is the earliest first sample of the files of the recording;
-- transcript times count from it.
alter table recordings
    drop constraint recordings_failure_check,
    add constraint recordings_failure_check check (
        failure in ('recorder_unavailable', 'nothing_recorded', 'files_missing')
    ),
    add column processing_attempts integer not null default 0 check (processing_attempts >= 0),
    add column processing_retry_at timestamptz,
    add column processing_by uuid,
    add column processing_heartbeat_at timestamptz,
    add column processed_at timestamptz,
    add column timeline_started_at timestamptz,
    add constraint recordings_attempt_has_a_heartbeat check (
        (processing_by is null) = (processing_heartbeat_at is null)
    ),
    add constraint recordings_attempt_while_processing check (
        processing_by is null or state = 'processing'
    ),
    add constraint recordings_processed_is_final check (
        (processed_at is not null) = (state in ('ready', 'partly_failed'))
        or state = 'failed'
    );

create index recordings_processing on recordings (id) where state = 'processing';

-- What the server found of the file of a segment. `file_bytes` is absent while the
-- recording is not processed and when no file was found. `file_starts_at` is the first
-- sample of the file on the timeline.
alter table recording_segments
    add column file_bytes bigint check (file_bytes >= 0),
    add column media_ms bigint check (media_ms >= 0),
    add column file_starts_at timestamptz,
    add column width integer check (width > 0),
    add column height integer check (height > 0),
    add column file_check text check (file_check in ('passed', 'failed')),
    add column file_problems text[] not null default '{}',
    add column file_silent boolean not null default false,
    add column file_recovered boolean not null default false,
    add constraint recording_segments_file_is_checked check (
        (file_bytes is null) = (file_check is null)
    ),
    add constraint recording_segments_file_has_length check (
        (file_bytes is null) = (media_ms is null)
    ),
    add constraint recording_segments_problems_fail check (
        (file_check = 'failed') = (file_problems <> '{}')
    ),
    add constraint recording_segments_picture_size check ((width is null) = (height is null));

-- The transcript of a recording and its job for the transcription worker. A recording
-- without a row has no transcript. One row is `running` for one server process, which
-- renews `heartbeat_at`; a row whose heartbeat is old can be claimed again.
create table transcripts (
    recording_id uuid primary key references recordings (id) on delete cascade,
    state text not null check (state in ('waiting', 'running', 'ready', 'partial', 'failed')),
    failure text check (
        failure in (
            'worker_failed', 'job_refused', 'worker_not_ready', 'time_limit',
            'transcript_invalid', 'interrupted'
        )
    ),
    attempts integer not null default 0 check (attempts >= 0),
    requested_at timestamptz not null,
    retry_at timestamptz,
    claimed_by uuid,
    claimed_at timestamptz,
    heartbeat_at timestamptz,
    finished_at timestamptz,
    lines integer not null default 0 check (lines >= 0),
    glossary_used text[] not null default '{}',
    glossary_cut text[] not null default '{}',
    check ((failure is not null) = (state = 'failed')),
    check ((claimed_by is not null) = (state = 'running')),
    check ((claimed_by is null) = (claimed_at is null)),
    check ((claimed_by is null) = (heartbeat_at is null)),
    check ((finished_at is not null) = (state in ('ready', 'partial', 'failed'))),
    check (state in ('ready', 'partial') or lines = 0)
);

create index transcripts_open on transcripts (requested_at, recording_id)
    where state in ('waiting', 'running');

-- The result of the transcript for each audio segment of the recording. A segment that was
-- not part of the job has the status `not_transcribed`.
create table transcript_segments (
    recording_id uuid not null references transcripts (recording_id) on delete cascade,
    segment_id uuid not null references recording_segments (id) on delete cascade,
    status text not null check (
        status in ('ok', 'empty', 'damaged', 'failed', 'not_transcribed')
    ),
    reason text check (char_length(reason) between 1 and 64),
    audio_ms bigint check (audio_ms >= 0),
    processed_ms bigint check (processed_ms >= 0),
    lines integer not null default 0 check (lines >= 0),
    speakers text not null check (
        speakers in ('not_applicable', 'not_requested', 'separated', 'unassigned', 'failed')
    ),
    speakers_reason text check (char_length(speakers_reason) between 1 and 64),
    primary key (recording_id, segment_id),
    check ((reason is not null) = (status in ('damaged', 'failed', 'not_transcribed')))
);

-- The lines of a transcript as the worker wrote them. `speaker` is the number of the label
-- `Speaker N` of a line of shared sound.
create table transcript_lines (
    recording_id uuid not null references transcripts (recording_id) on delete cascade,
    line integer not null check (line >= 0),
    start_ms bigint not null check (start_ms >= 0),
    end_ms bigint not null,
    segment_id uuid not null references recording_segments (id) on delete cascade,
    source text not null check (source in ('microphone', 'shared')),
    account_id uuid not null references accounts (id),
    speaker integer check (speaker >= 1),
    text text not null check (char_length(text) between 1 and 4000),
    language text not null check (language ~ '^[a-z]{2,3}$'),
    primary key (recording_id, line),
    check (end_ms >= start_ms),
    check (source = 'shared' or speaker is null)
);

-- The corrections of a line, each with its author. The newest is the text that is shown;
-- the text of the line itself is never changed (ADR 0603).
create table transcript_corrections (
    id uuid primary key,
    recording_id uuid not null,
    line integer not null,
    text text not null check (char_length(text) between 1 and 2000),
    corrected_by uuid not null references accounts (id),
    corrected_at timestamptz not null,
    foreign key (recording_id, line) references transcript_lines (recording_id, line)
        on delete cascade
);

create index transcript_corrections_of_line on transcript_corrections (recording_id, line, id);

-- The speaker labels of a transcript, each with the account whose shared sound it belongs
-- to, and the names that people gave to a label. The newest name is shown.
create table transcript_speakers (
    recording_id uuid not null references transcripts (recording_id) on delete cascade,
    speaker integer not null check (speaker >= 1),
    account_id uuid not null references accounts (id),
    primary key (recording_id, speaker)
);

create table transcript_speaker_names (
    id uuid primary key,
    recording_id uuid not null,
    speaker integer not null,
    name text not null check (char_length(name) between 1 and 64),
    renamed_by uuid not null references accounts (id),
    renamed_at timestamptz not null,
    foreign key (recording_id, speaker) references transcript_speakers (recording_id, speaker)
        on delete cascade
);

create index transcript_speaker_names_of_speaker
    on transcript_speaker_names (recording_id, speaker, id);

-- The glossary of a workspace in its order of priority (ADR 0603).
create table glossary_terms (
    workspace_id uuid not null references workspaces (id) on delete cascade,
    position integer not null check (position >= 0),
    term text not null check (char_length(term) between 1 and 200),
    primary key (workspace_id, position),
    constraint glossary_terms_unique unique (workspace_id, term)
);

-- Directories of removed recordings that the server still has to delete. `path` is relative
-- to the directory of the recordings. A row is written in the transaction that removes the
-- recording and deleted after the directory is gone. A deletion that failed is tried again
-- at `retry_at`.
create table file_removals (
    id uuid primary key,
    path text not null check (path ~ '^[A-Za-z0-9_-]{1,128}(/[A-Za-z0-9_-]{1,128})?$'),
    requested_at timestamptz not null,
    retry_at timestamptz
);

-- The note for the post of a finished recording in the text chat of its voice channel
-- (ADR 0602). It is removed with the recording.
alter table call_notes
    drop constraint call_notes_kind_check,
    add constraint call_notes_kind_check check (
        kind in (
            'call_started', 'call_ended', 'screen_share_started', 'entry_requested',
            'joined_by_invitation', 'meeting_started', 'meeting_ended', 'recording_started',
            'recording_stopped', 'recording_failed', 'recording_ready'
        )
    );
