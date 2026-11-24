-- The default spoken language of the recordings of the server (ADR 0604): a language code
-- of two or three lower-case letters, or 'auto' for the language of each sentence. English
-- on a new server and after the upgrade.
alter table server_state
    add column recording_language text not null default 'en'
        check (recording_language = 'auto' or recording_language ~ '^[a-z]{2,3}$');
