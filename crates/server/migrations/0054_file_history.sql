alter table attachments add column message_created_at timestamptz;
update attachments a set message_created_at=m.created_at from messages m where m.id=a.message_id;
create function attachment_message_time() returns trigger language plpgsql as $$
begin
    new.message_created_at := (select created_at from messages where id=new.message_id);
    return new;
end;
$$;
create trigger attachment_message_time before insert or update of message_id on attachments
    for each row execute function attachment_message_time();
alter table attachments add constraint attachment_message_time_present
    check (message_id is null or message_created_at is not null);
create index attachments_file_history on attachments(place_id,message_created_at desc,message_id desc)
    where attached and complete and removed_at is null and message_id is not null;
