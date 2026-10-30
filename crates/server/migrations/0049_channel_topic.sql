alter table channels add column topic text not null default '' check (char_length(topic) <= 1024);
