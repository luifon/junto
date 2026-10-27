-- A picture that a person sets, an avatar and the logo of the server, has at most 2 MiB, the
-- largest request body; the setting JUNTO_PICTURE_BYTES can lower it (ADR 0119).
alter table account_avatars
    drop constraint account_avatars_content_check,
    add constraint account_avatars_content_check
        check (octet_length(content) between 1 and 2097152);
alter table server_logo
    drop constraint server_logo_content_check,
    add constraint server_logo_content_check
        check (octet_length(content) between 1 and 2097152);
