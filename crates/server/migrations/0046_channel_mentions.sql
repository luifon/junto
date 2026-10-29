create or replace function notification_mentioned(subject uuid,message uuid,enabled_roles boolean,enabled_everyone boolean)
returns boolean language sql stable as $$
    select exists(select 1 from message_mentions mm where mm.message_id=message
        and subject=any(mm.recipients) and (
        mm.account_id=subject or (mm.everyone and enabled_everyone) or
        exists(select 1 from workspace_roles r where r.id=mm.role_id and (
            (r.is_everyone and enabled_everyone and exists(select 1 from workspace_members w where w.workspace_id=r.workspace_id and w.account_id=subject)) or
            (not r.is_everyone and enabled_roles and exists(select 1 from member_roles mr where mr.role_id=r.id and mr.account_id=subject))
        ))));
$$;
