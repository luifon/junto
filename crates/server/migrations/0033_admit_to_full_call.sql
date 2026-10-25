-- The workspace permission "admit to full call" (bit 16), and the accounts that answer a
-- request to enter a full call: null for everyone inside (ADR 0514).
do $$
declare
    found record;
begin
    for found in
        select conrelid::regclass::text as tab, conname from pg_constraint
        where conrelid in ('workspace_roles'::regclass, 'channel_overrides'::regclass)
          and contype = 'c'
          and pg_get_constraintdef(oid) like '%65535%'
    loop
        execute format('alter table %s drop constraint %I', found.tab, found.conname);
    end loop;
end
$$;
alter table workspace_roles
    add constraint workspace_roles_permissions_check check (permissions between 0 and 131071);
alter table channel_overrides
    add constraint channel_overrides_allow_check check (allow between 0 and 131071),
    add constraint channel_overrides_deny_check check (deny between 0 and 131071);
alter table call_entry_requests add column answerers uuid[];
