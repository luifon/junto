alter table channels add column voice_status text not null default ''
    check (char_length(voice_status) <= 500 and (kind = 'voice' or voice_status = ''));
