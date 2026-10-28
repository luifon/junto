-- A person's rules stay when the person leaves a workspace and apply again when the person
-- returns (ADR 0304). They end with the account, the workspace, the category or the channel.
do $$
declare
    foreign_key record;
begin
    for foreign_key in
        select conrelid::regclass as table_name, conname
        from pg_constraint
        where contype = 'f'
          and confrelid = 'workspace_members'::regclass
          and conrelid in ('workspace_person_overrides'::regclass,
                           'category_person_overrides'::regclass,
                           'channel_person_overrides'::regclass)
    loop
        execute format('alter table %s drop constraint %I',
                       foreign_key.table_name, foreign_key.conname);
    end loop;
end $$;

alter table workspace_person_overrides
    add foreign key (workspace_id) references workspaces(id) on delete cascade,
    add foreign key (account_id) references accounts(id) on delete cascade;
alter table category_person_overrides
    add foreign key (account_id) references accounts(id) on delete cascade;
alter table channel_person_overrides
    add foreign key (account_id) references accounts(id) on delete cascade;
