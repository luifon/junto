-- The place of an attached file among the files of its message: its index in the send
-- request (ADR 0408). Files attached before this migration keep the order that the server
-- returned for them, the order of their ids.
alter table attachments add column position integer check (position >= 0);
update attachments a set position = ordered.position
    from (select id, row_number() over (partition by message_id order by id) - 1 as position
          from attachments where attached and message_id is not null) ordered
    where a.id = ordered.id;
alter table attachments add constraint attachments_attached_have_a_position
    check (not attached or message_id is null or position is not null);
