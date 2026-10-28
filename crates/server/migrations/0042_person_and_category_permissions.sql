alter table workspace_roles drop constraint workspace_roles_permissions_check;
alter table channel_overrides drop constraint channel_overrides_allow_check;
alter table channel_overrides drop constraint channel_overrides_deny_check;
alter table workspace_roles add constraint workspace_roles_permissions_check
    check (permissions between 0 and 2147483647);
alter table channel_overrides
    add constraint channel_overrides_allow_check check (allow between 0 and 2147483647),
    add constraint channel_overrides_deny_check check (deny between 0 and 2147483647);

-- Channel use previously followed visibility. Changing the limit required manage channel.
update workspace_roles set permissions = permissions
    | case when is_everyone then 1610481664 else 0 end
    | ((permissions & 4) << 27);
update channel_overrides set allow = allow | ((allow & 4) << 27),
    deny = deny | ((deny & 4) << 27);

alter table channels add column permissions_synced boolean not null default true;
update channels c set permissions_synced = false
    where exists (select 1 from channel_overrides o where o.channel_id = c.id);

create table server_person_overrides (
    account_id uuid primary key references accounts(id) on delete cascade,
    allow bigint not null check (allow between 0 and 63),
    deny bigint not null check (deny between 0 and 63),
    check (allow & deny = 0),
    check (allow | deny <> 0)
);

create table workspace_person_overrides (
    workspace_id uuid not null,
    account_id uuid not null,
    allow bigint not null check (allow between 0 and 2147483647),
    deny bigint not null check (deny between 0 and 2147483647),
    primary key (workspace_id, account_id),
    foreign key (workspace_id, account_id)
        references workspace_members(workspace_id, account_id) on delete cascade,
    check (allow & deny = 0),
    check (allow | deny <> 0)
);

create table category_overrides (
    category_id uuid not null,
    role_id uuid not null,
    workspace_id uuid not null,
    allow bigint not null check (allow between 0 and 2147483647),
    deny bigint not null check (deny between 0 and 2147483647),
    primary key (category_id, role_id),
    foreign key (category_id, workspace_id)
        references categories(id, workspace_id) on delete cascade,
    foreign key (role_id, workspace_id)
        references workspace_roles(id, workspace_id) on delete cascade,
    check (allow & deny = 0),
    check (allow | deny <> 0)
);
create index category_overrides_role on category_overrides(role_id);

create table category_person_overrides (
    category_id uuid not null,
    account_id uuid not null,
    workspace_id uuid not null,
    allow bigint not null check (allow between 0 and 2147483647),
    deny bigint not null check (deny between 0 and 2147483647),
    primary key (category_id, account_id),
    foreign key (category_id, workspace_id)
        references categories(id, workspace_id) on delete cascade,
    foreign key (workspace_id, account_id)
        references workspace_members(workspace_id, account_id) on delete cascade,
    check (allow & deny = 0),
    check (allow | deny <> 0)
);
create index category_person_overrides_member
    on category_person_overrides(workspace_id, account_id);

create table channel_person_overrides (
    channel_id uuid not null,
    account_id uuid not null,
    workspace_id uuid not null,
    allow bigint not null check (allow between 0 and 2147483647),
    deny bigint not null check (deny between 0 and 2147483647),
    primary key (channel_id, account_id),
    foreign key (channel_id, workspace_id)
        references channels(id, workspace_id) on delete cascade,
    foreign key (workspace_id, account_id)
        references workspace_members(workspace_id, account_id) on delete cascade,
    check (allow & deny = 0),
    check (allow | deny <> 0)
);
create index channel_person_overrides_member
    on channel_person_overrides(workspace_id, account_id);

create view effective_channel_overrides as
    select c.id as channel_id, o.role_id, c.workspace_id, o.allow, o.deny
    from channels c join category_overrides o on o.category_id = c.category_id
    where c.permissions_synced
    union all
    select o.channel_id, o.role_id, o.workspace_id, o.allow, o.deny
    from channel_overrides o join channels c on c.id = o.channel_id
    where not c.permissions_synced or c.category_id is null;

create view effective_channel_person_overrides as
    select c.id as channel_id, o.account_id, c.workspace_id, o.allow, o.deny
    from channels c join category_person_overrides o on o.category_id = c.category_id
    where c.permissions_synced
    union all
    select o.channel_id, o.account_id, o.workspace_id, o.allow, o.deny
    from channel_person_overrides o join channels c on c.id = o.channel_id
    where not c.permissions_synced or c.category_id is null;

update event_stream set epoch = gen_random_uuid();
