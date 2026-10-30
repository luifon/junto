alter table conversations alter column low_account_id drop not null;
alter table conversations alter column high_account_id drop not null;
alter table conversations add column name text;
alter table conversations add constraint conversation_kind check (
    (name is null and low_account_id is not null and high_account_id is not null)
    or (name is not null and char_length(name) between 1 and 80
        and low_account_id is null and high_account_id is null)
);
create table conversation_members (
    conversation_id uuid not null references conversations(id),
    account_id uuid not null references accounts(id),
    primary key(conversation_id, account_id)
);
create index conversation_members_account on conversation_members(account_id, conversation_id);

create table conversation_former_members (
    conversation_id uuid not null references conversations(id),
    account_id uuid not null references accounts(id),
    primary key(conversation_id, account_id)
);
create index conversation_former_members_account on conversation_former_members(account_id, conversation_id);

alter table calls add column conversation_id uuid unique references conversations(id);
do $$ declare rule record; begin
    for rule in select conname from pg_constraint where conrelid='calls'::regclass
        and contype='c' and pg_get_constraintdef(oid) like '%<>%'
    loop execute format('alter table calls drop constraint %I', rule.conname); end loop;
end $$;
alter table calls add constraint calls_one_place check (num_nonnulls(channel_id,direct_low,conversation_id)=1);
