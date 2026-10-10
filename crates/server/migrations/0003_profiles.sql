-- The avatar of an account is stored with the account, so that a change of it and its event
-- are one transaction (ADR 0119).
create table account_avatars (
    account_id uuid primary key references accounts (id),
    media_type text not null check (media_type in ('image/png', 'image/jpeg', 'image/webp')),
    content bytea not null check (octet_length(content) between 1 and 262144),
    hash bytea not null check (octet_length(hash) = 32),
    width integer not null check (width between 1 and 1024),
    height integer not null check (height between 1 and 1024),
    updated_at timestamptz not null
);

alter table account_avatars alter column content set storage external;

-- An audience can name the account whose viewers receive the event.
alter table events add constraint events_account_viewers_named
    check (audience ->> 'to' <> 'account_viewers' or audience ? 'account_id');
