alter table messages drop constraint messages_document_check;
alter table messages add constraint messages_document_bytes
    check (document is null or octet_length(document::text) <= 524288);
