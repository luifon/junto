alter table server_roles
    add column color integer check (color between 0 and 16777215),
    add column hoist boolean not null default false;

alter table workspace_roles
    add column color integer check (color between 0 and 16777215),
    add column hoist boolean not null default false,
    add constraint workspace_everyone_not_hoisted check (not is_everyone or not hoist);

create function account_role_appearance(person uuid) returns jsonb
language sql stable as $$
    select case when a.status <> 'active' then '{}'::jsonb else jsonb_build_object(
        'color', (select '#' || upper(lpad(to_hex(r.color), 6, '0'))
                  from account_server_roles h join server_roles r on r.id = h.role_id
                  where h.account_id = person and r.color is not null
                  order by r.position desc, r.id limit 1),
        'group', (select jsonb_build_object('id', r.id, 'name', r.name,
                      'position', r.position,
                      'color', case when r.color is null then null
                              else '#' || upper(lpad(to_hex(r.color), 6, '0')) end)
                  from account_server_roles h join server_roles r on r.id = h.role_id
                  where h.account_id = person and r.hoist
                  order by r.position desc, r.id limit 1)
    ) end from accounts a where a.id = person
$$;
