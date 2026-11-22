-- The comment of a forward is stored apart from the forwarded content (ADR 0408) and is
-- searched with it.
alter table messages add column comment jsonb
    constraint messages_comment_bytes check (comment is null or octet_length(comment::text) <= 32768);
drop index messages_search;
alter table messages drop column search_vector;
alter table messages add column search_vector tsvector generated always as
    (to_tsvector('simple',
        coalesce(message_search_text(document),'') || ' ' || coalesce(message_search_text(comment),''))) stored;
create index messages_search on messages using gin(search_vector);
