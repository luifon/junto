alter table message_reads add column read_revision bigint not null default 0;
alter table thread_reads add column read_revision bigint not null default 0;
alter table message_reads add column manual_unread boolean not null default false;
alter table thread_reads add column manual_unread boolean not null default false;
