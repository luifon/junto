-- The limit of pending uploads counts the uploads of one account that are not attached:
-- the ones that wait for a message, and the removed ones whose reservation still exists.
create index attachments_unattached_of_owner on attachments (owner_id) where not attached;

-- The accounts that have a direct-message conversation with one account are read from both
-- sides. The unique constraint serves the lower account only.
create index conversations_high_account on conversations (high_account_id);

-- An event about an invite is for the accounts that read the invite (ADR 0111). The stored
-- events of invites were recorded for every holder of `create_invites`; they get the
-- audience of their invite. An event whose invite no longer exists is kept for the
-- administrators, who see every workspace.
alter table events disable trigger events_no_change;
update events e
set audience = jsonb_build_object(
        'to', 'invite',
        'created_by', i.created_by,
        'workspace_ids', coalesce((
            select jsonb_agg(w.workspace_id order by w.workspace_id)
            from invite_workspaces w where w.invite_id = i.id), '[]'::jsonb))
from invites i
where e.kind in ('invite_created', 'invite_used', 'invite_revoked')
  and e.audience ->> 'to' = 'server_permission'
  and coalesce(e.payload -> 'invite' ->> 'id', e.payload ->> 'invite_id')
      ~ '^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$'
  and i.id = coalesce(e.payload -> 'invite' ->> 'id', e.payload ->> 'invite_id')::uuid;
update events
set audience = jsonb_build_object('to', 'server_permission', 'permission', 'administrator')
where kind in ('invite_created', 'invite_used', 'invite_revoked')
  and audience ->> 'to' = 'server_permission';
alter table events enable trigger events_no_change;
