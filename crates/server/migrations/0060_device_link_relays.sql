create table device_link_relays (
    new_device_id uuid primary key references devices (id) on delete cascade,
    account_id uuid not null references accounts (id),
    old_device_id uuid not null references devices (id) on delete cascade,
    offer bytea not null check (octet_length(offer) between 1 and 65536),
    answer bytea check (octet_length(answer) between 1 and 65536),
    answers integer not null default 0 check (answers between 0 and 256),
    created_at timestamptz not null,
    expires_at timestamptz not null,
    check (old_device_id <> new_device_id)
);

create index device_link_relays_expiry on device_link_relays (expires_at);
create index device_link_relays_old_device on device_link_relays (old_device_id);
