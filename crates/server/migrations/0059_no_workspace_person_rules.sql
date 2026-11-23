-- A workspace has no permission rule for one member any more (ADR 0301). The start turned the
-- rule of each current member into a role of that member and removed every rule before this
-- migration (`person_rules::convert`); the table is empty here.
drop table workspace_person_overrides;

-- The roles of the members changed without events: every client loads a snapshot.
update event_stream set epoch = gen_random_uuid();
