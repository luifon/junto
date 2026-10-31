create table workspace_afk (
    workspace_id uuid primary key references workspaces (id),
    channel_id uuid not null references channels (id),
    enabled boolean not null default true,
    idle_minutes integer not null default 15 check (idle_minutes in (5, 15, 30, 60))
);

do $$
declare
    workspace record;
    channel uuid;
begin
    for workspace in select id from workspaces where deleted_at is null loop
        channel := gen_random_uuid();
        insert into channels (id, workspace_id, kind, name, position, created_at)
        values (channel, workspace.id, 'voice', 'AFK',
            (select coalesce(max(position), -1) + 1 from channels where workspace_id = workspace.id),
            now());
        insert into workspace_afk (workspace_id, channel_id) values (workspace.id, channel);
    end loop;
end $$;

alter table call_moves add column automatic_afk boolean not null default false;

alter table call_moves add column afk_idle_seconds integer
    check (afk_idle_seconds is null or afk_idle_seconds between 300 and 3600);
alter table call_moves add constraint call_move_afk_duration
    check (automatic_afk = (afk_idle_seconds is not null));
