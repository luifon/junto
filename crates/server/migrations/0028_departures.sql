-- An account can leave its last workspace and become inactive, and its person can delete it
-- (ADR 0121). Its row stays in both states, for the audit log and for its content.
alter table accounts drop constraint accounts_status_check;
alter table accounts add constraint accounts_status_check
    check (status in ('active', 'banned', 'removed', 'inactive', 'deleted'));

alter table ended_sessions drop constraint ended_sessions_reason_check;
alter table ended_sessions add constraint ended_sessions_reason_check
    check (reason in (
        'logged_out', 'device_removed', 'banned', 'removed', 'recovered', 'inactive', 'deleted'
    ));
