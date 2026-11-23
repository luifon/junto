-- A group conversation may have no name: the client then shows the names of its members
-- (ADR 0302). A group keeps a name that is not null, which tells it apart from a pair.
alter table conversations drop constraint conversation_kind;
alter table conversations add constraint conversation_kind check (
    (name is null and low_account_id is not null and high_account_id is not null)
    or (name is not null and char_length(name) <= 80
        and low_account_id is null and high_account_id is null)
);
