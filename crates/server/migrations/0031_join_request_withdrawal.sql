-- A device withdraws its waiting join request before it discards its key, and a request
-- ends when its key gets an account in another way (ADR 0111).

alter table join_requests drop constraint join_requests_state_check;
alter table join_requests add constraint join_requests_state_check
    check (state in ('pending', 'approved', 'denied', 'expired', 'withdrawn'));

do $$
declare
    found text;
begin
    for found in
        select conname from pg_constraint
        where conrelid = 'join_requests'::regclass
          and contype = 'c'
          and pg_get_constraintdef(oid) like '%decided_by IS NULL%'
    loop
        execute format('alter table join_requests drop constraint %I', found);
    end loop;
end
$$;
alter table join_requests add constraint join_requests_decided_by
    check ((state in ('approved', 'denied')) = (decided_by is not null));

alter table challenges drop constraint if exists challenges_operation_check;
alter table challenges add constraint challenges_operation_check check (
    operation in ('login', 'claim_owner', 'join', 'request_to_join',
                  'join_request_status', 'link_device', 'moderation_notices',
                  'withdraw_join_request')
);
