create table notification_read_boundaries (
    account_id uuid primary key references accounts(id) on delete cascade,
    read_until timestamptz not null,
    read_at timestamptz not null
);

create index events_deliverable on events(position)
    where audience->>'to' is distinct from 'suppressed';
