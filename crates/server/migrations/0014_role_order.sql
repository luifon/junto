alter table server_roles add column position integer;
with ranked as (
    select id, row_number() over (order by (permissions & 17) <> 0, created_at, id)::integer as position
    from server_roles
)
update server_roles r set position = ranked.position from ranked where r.id = ranked.id;
alter table server_roles alter column position set not null;
alter table server_roles add constraint server_role_position_positive check (position > 0);
alter table server_roles add constraint server_role_position_unique unique (position) deferrable initially deferred;

alter table workspace_roles add column position integer;
with ranked as (
    select id, row_number() over (
        partition by workspace_id order by (permissions & 2) <> 0, created_at, id
    )::integer as position
    from workspace_roles where not is_everyone
)
update workspace_roles r set position = ranked.position from ranked where r.id = ranked.id;
update workspace_roles set position = 0 where is_everyone;
alter table workspace_roles alter column position set not null;
alter table workspace_roles alter column position set default 0;
alter table workspace_roles add constraint workspace_role_position_positive
    check ((is_everyone and position = 0) or (not is_everyone and position > 0));
alter table workspace_roles add constraint workspace_role_position_unique unique (workspace_id, position) deferrable initially deferred;

update event_stream set epoch = gen_random_uuid();

create index member_roles_account_workspace on member_roles (account_id, workspace_id);
