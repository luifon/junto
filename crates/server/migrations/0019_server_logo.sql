create table server_logo (
    singleton boolean primary key default true check(singleton),
    media_type text not null check(media_type in ('image/png','image/jpeg','image/webp')),
    content bytea not null check(octet_length(content) between 1 and 262144),
    hash bytea not null check(octet_length(hash)=32),
    width integer not null check(width between 1 and 1024),
    height integer not null check(height between 1 and 1024)
);
