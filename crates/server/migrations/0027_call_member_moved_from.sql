-- The identity of the seat in the source call that a completed move replaced. The device
-- that was moved holds both seats until it reads the outcome, so a leave that names the
-- seat of the source ends the seat of the destination (ADR 0511).
alter table call_members add column moved_from text
    check (moved_from ~ '^[0-9a-f-]{36}:[0-9a-f]{32}$'
           and starts_with(moved_from, account_id::text || ':'));
