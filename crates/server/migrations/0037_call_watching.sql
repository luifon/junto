-- The share of another person that each member of a call watches, as the client of the member
-- reports it, so that the sharer hears and sees who watches (ADR 0502).
alter table call_members
    add column watching_account_id uuid references accounts (id),
    add column watching_slot smallint not null default 0
        check (watching_slot >= 0 and watching_slot < 16);
