-- Corrections of recordings and transcripts after the audit of the server.

-- An event of a transcript carries no text and no name (ADR 0607). The log of events is
-- append-only, so the stored events of corrections and of renames lose here what such an
-- event no longer carries.
alter table events disable trigger events_no_change;
update events
set payload = payload - 'text' - 'corrected_by' - 'corrected_at'
where kind = 'transcript_line_corrected';
update events
set payload = jsonb_set(payload, '{speaker}', payload -> 'speaker' -> 'number')
where kind = 'transcript_speaker_renamed'
  and jsonb_typeof(payload -> 'speaker') = 'object';
alter table events enable trigger events_no_change;

-- A job that is declared lost before the recorder took a stop for it is owed that stop
-- (ADR 0604). The record keeps this, so that a server process that starts after the
-- declaration sends the stop.
alter table recording_segments
    add column stop_owed boolean not null default false,
    add constraint recording_segments_stop_owed check (
        not stop_owed
        or (failure = 'recorder_lost' and job_id is not null and stop_delivered_at is null)
    );
