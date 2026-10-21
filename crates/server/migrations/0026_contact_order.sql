-- The active accounts in the order of the list of contacts of an administrator (ADR 0307):
-- their name in lower case and their identifier. A page reads its rows in this order.
create index accounts_contact_order on accounts ((lower(name) collate "C"), id)
    where status = 'active';
