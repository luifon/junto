-- The recipients of new notices are queued for the cap of 500 notices per account once for
-- each statement, from the inserted notices, in place of once for each notice (ADR 0409).
drop trigger queue_notification_trim on notifications;
create or replace function queue_notification_trim() returns trigger language plpgsql as $$
begin
    insert into notification_trim_accounts select distinct account_id from inserted
        on conflict do nothing;
    return null;
end;
$$;
create trigger queue_notification_trim after insert on notifications
    referencing new table as inserted
    for each statement execute function queue_notification_trim();
