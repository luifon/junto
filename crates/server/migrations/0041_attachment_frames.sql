-- The frames of an animated picture of a message (ADR 0408). A still picture and every
-- other file have none.
alter table attachments add column frames integer;
alter table attachments add constraint attachments_frames_of_a_picture check (
    frames is null or (frames >= 2 and width is not null)
);
