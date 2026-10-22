-- The workspaces that an account left by itself, and the time of its departure from the
-- server (ADR 0121). The members of those workspaces and the account's conversation peers
-- are told of the departure and see the profile of the departed account. The time stays
-- after a removal, so that a direct conversation with the account stays readable.
create table account_departures (
    account_id uuid not null references accounts (id),
    workspace_id uuid not null references workspaces (id),
    primary key (account_id, workspace_id)
);

create index account_departures_workspace on account_departures (workspace_id);

alter table accounts add column departed_at timestamptz;
alter table accounts add constraint accounts_departed_check
    check (status not in ('inactive', 'deleted') or departed_at is not null);
