create index messages_group_call_arrival on messages
    (call_id, (system->>'actor_id'), created_at desc, id desc)
    where system->>'action' in
        ('call_started','joined_by_invitation','person_joined','person_left','person_out');
