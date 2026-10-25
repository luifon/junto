-- The lines of a person who joins a running call and of a person who leaves it, in the text
-- chat of the voice channel (ADR 0408).
alter table call_notes
    drop constraint call_notes_kind_check,
    add constraint call_notes_kind_check check (
        kind in (
            'call_started', 'call_ended', 'screen_share_started', 'entry_requested',
            'joined_by_invitation', 'meeting_started', 'meeting_ended', 'recording_started',
            'recording_stopped', 'recording_failed', 'recording_ready', 'person_joined',
            'person_left'
        )
    );
