-- A share can show every window of one application (ADR 0737). The column keeps the
-- values `screen` and `window` of the shares before it.
alter table call_shares drop constraint call_shares_kind_check;
alter table call_shares
    add constraint call_shares_kind_check check (kind in ('screen', 'window', 'application'));
