create index audit_entries_action on audit_entries(workspace_id, action, id desc);
create index audit_entries_actor on audit_entries(workspace_id, actor_id, id desc);
create index audit_entries_time on audit_entries(workspace_id, occurred_at desc, id desc);

create function audit_targets(event jsonb, workspace_id uuid, channel_id uuid, call_id uuid)
returns table(target_id uuid) language sql immutable as $$
    select distinct value::uuid from (
        select unnest(array[workspace_id, channel_id, call_id])::text as value
        union all
        select value #>> '{}' from jsonb_each(event)
        where (key like '%\_id' escape '\' or key in ('created_by', 'invite_created_by'))
          and jsonb_typeof(value) = 'string'
        union all
        select element #>> '{}' from jsonb_each(event)
        cross join lateral jsonb_array_elements(case when jsonb_typeof(value) = 'array' then value else '[]'::jsonb end) element
        where key like '%\_ids' escape '\'
    ) named where value ~* '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
$$;

create table audit_entry_targets (
    target_id uuid not null,
    entry_id uuid not null references audit_entries(id),
    primary key(target_id, entry_id)
);
insert into audit_entry_targets
select t.target_id, e.id from audit_entries e
cross join lateral audit_targets(e.event, e.workspace_id, e.channel_id, e.call_id) t;

create function index_audit_targets() returns trigger language plpgsql as $$
begin
    insert into audit_entry_targets select target_id, new.id
    from audit_targets(new.event, new.workspace_id, new.channel_id, new.call_id);
    return new;
end
$$;
create trigger audit_entries_index_targets after insert on audit_entries
    for each row execute function index_audit_targets();
create trigger audit_targets_no_row_change before update or delete on audit_entry_targets
    for each row execute function audit_entries_are_read_only();
create trigger audit_targets_no_truncate before truncate on audit_entry_targets
    for each statement execute function audit_entries_are_read_only();

create index audit_entries_action_actor on audit_entries(workspace_id, action, actor_id, id desc);
create index audit_entries_action_time on audit_entries(workspace_id, action, occurred_at desc, id desc);
create index audit_entries_actor_time on audit_entries(workspace_id, actor_id, occurred_at desc, id desc);
create index audit_entries_action_actor_time on audit_entries(workspace_id, action, actor_id, occurred_at desc, id desc);
