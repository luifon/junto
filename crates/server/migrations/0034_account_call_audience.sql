alter table events drop constraint events_addressed_to_an_account;
alter table events add constraint events_addressed_to_an_account check (
    ((audience->>'to' in ('account','account_seeing')) or
     (audience->>'to' in ('mention','call') and audience->>'account_id' is not null))
    = (audience_account_id is not null)
);
