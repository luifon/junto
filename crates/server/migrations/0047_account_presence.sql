alter table accounts add column presence_mode text not null default 'online'
    check (presence_mode in ('online', 'idle', 'invisible'));
