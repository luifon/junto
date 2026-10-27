-- The screen shares of the members of a call: up to three per member, each with the slot
-- that the sharing device gave it (ADR 0502, ADR 0510). They replace the one share of a
-- member in `call_members`. `share_id` is the identifier that the device gave the share
-- when it started it; a share copied from `call_members` and a share of a client that
-- states none have no identifier.
create table call_shares (
    call_id uuid not null,
    account_id uuid not null,
    slot smallint not null check (slot between 0 and 2),
    share_id uuid,
    kind text check (kind in ('screen', 'window')),
    audio text not null check (audio in ('application', 'system', 'none')),
    started_at timestamptz not null,
    seen_at timestamptz,
    missing_since timestamptz,
    primary key (call_id, account_id, slot),
    foreign key (call_id, account_id)
        references call_members (call_id, account_id) on delete cascade
);

insert into call_shares
    (call_id, account_id, slot, audio, started_at, seen_at, missing_since)
select call_id, account_id, 0, share_audio, share_started_at, share_seen_at,
       share_missing_since
from call_members
where share_audio is not null;

alter table call_members
    drop column share_audio,
    drop column share_started_at,
    drop column share_seen_at,
    drop column share_missing_since;
