-- The lines of a call and of a recording that the text chat of a voice channel shows beside
-- those of 0032: the end of a share, the end of meeting mode with its reason, the lock of the
-- room, the end of a transcript, and the note without a line of a person whom a moderator
-- removed or moved out (ADR 0408).
alter table call_notes
    drop constraint call_notes_kind_check,
    add constraint call_notes_kind_check check (
        kind in (
            'call_started', 'call_ended', 'screen_share_started', 'entry_requested',
            'joined_by_invitation', 'meeting_started', 'meeting_ended', 'recording_started',
            'recording_stopped', 'recording_failed', 'recording_ready', 'person_joined',
            'person_left', 'screen_share_ended', 'meeting_turned_off',
            'meeting_ended_share_stopped', 'meeting_ended_presenter_left', 'person_out',
            'call_locked', 'call_unlocked', 'transcript_ready', 'transcript_partial',
            'transcript_failed'
        )
    );
