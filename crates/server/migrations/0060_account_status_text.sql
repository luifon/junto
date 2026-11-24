alter table accounts
    add column status_text text,
    add column status_text_until timestamptz,
    add constraint accounts_status_text_shape check (
        (status_text is null and status_text_until is null)
        or (status_text is not null and char_length(status_text) between 1 and 100)
    );

create index accounts_status_text_until on accounts (status_text_until)
    where status_text_until is not null;
