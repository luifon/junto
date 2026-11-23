-- The longest continuous stretch of sound that the composite of a recording lacks, on the
-- timeline of the recording (ADR 0606).
alter table recording_segments
    add column missing_sound_from timestamptz,
    add column missing_sound_until timestamptz,
    add check ((missing_sound_from is null) = (missing_sound_until is null));

-- The participant of the job has been seen in the room of the call: the job of a composite
-- then has the start limit of every job (ADR 0604).
alter table recording_segments
    add column job_joined boolean not null default false;
