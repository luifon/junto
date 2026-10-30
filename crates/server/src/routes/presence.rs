use std::collections::{HashMap, HashSet};

use axum::{Json, extract::State};
use junto_authz::{Acquaintance, Actor, Position, Request};
use junto_protocol::{
    events::Event,
    ids::AccountId,
    presence::{
        AccountPresence, PRESENCE_PAGE, Presence, PresenceChoice, PresencePage, ReadPresence,
        SetPresence,
    },
};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::{
    access::Access,
    db,
    error::{ApiError, ApiResult},
    events,
    extract::{ApiJson, Bearer},
    proof, read,
    state::AppState,
    store, transact,
};

struct Preference {
    account_id: AccountId,
    choice: PresenceChoice,
    administrator: bool,
}

type PreferenceRow = (AccountId, String, bool, bool, i64, i64, i64);

async fn preferences(
    connection: &mut PgConnection,
    accounts: &[AccountId],
) -> ApiResult<Vec<Preference>> {
    let rows: Vec<PreferenceRow> = sqlx::query_as(
        "select a.id, a.presence_mode,
                coalesce((n.settings->>'do_not_disturb')::boolean, false),
                coalesce(a.id = s.owner_account_id, false),
                coalesce((select bit_or(r.permissions) from account_server_roles h
                          join server_roles r on r.id=h.role_id where h.account_id=a.id), 0),
                coalesce(o.allow, 0), coalesce(o.deny, 0)
         from accounts a cross join server_state s
         left join notification_settings n on n.account_id=a.id and n.scope_id=$2
         left join server_person_overrides o on o.account_id=a.id
         where a.id=any($1) and a.status='active'",
    )
    .bind(accounts)
    .bind(Uuid::nil())
    .fetch_all(connection)
    .await?;
    rows.into_iter()
        .map(|(account_id, mode, dnd, owner, roles, allow, deny)| {
            let choice = match mode.as_str() {
                "invisible" => PresenceChoice::Invisible,
                _ if dnd => PresenceChoice::DoNotDisturb,
                "online" => PresenceChoice::Online,
                "idle" => PresenceChoice::Idle,
                _ => return Err(ApiError::internal(&"invalid stored presence mode")),
            };
            let actor = Actor {
                position: if owner {
                    Position::Owner
                } else {
                    Position::Member
                },
                server_roles: vec![db::server_permissions(roles)?],
                server_override: crate::access::permissions::server_rule(allow, deny)?,
            };
            Ok(Preference {
                account_id,
                choice,
                administrator: actor.is_administrator(),
            })
        })
        .collect()
}

#[utoipa::path(post,path="/api/v1/presence",tag="account",request_body=ReadPresence,security(("session"=[])),responses((status=200,body=PresencePage),ApiError))]
pub async fn read_presence(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiJson(body): ApiJson<ReadPresence>,
) -> ApiResult<Json<PresencePage>> {
    if body.accounts.len() > PRESENCE_PAGE {
        return Err(ApiError::invalid("read at most 100 presence identities"));
    }
    let mut counted = false;
    let mut change_counted = false;
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, app.now()).await?;
        access.require(Request::OwnAccount)?;
        if !counted {
            proof::limit(&app.presence_reads, access.account_id, app.now())?;
            counted = true;
        }
        if app
            .streams
            .idle_differs(access.account_id, access.session_id, body.idle)
        {
            if !change_counted {
                proof::limit(&app.presence_changes, access.account_id, app.now())?;
                change_counted = true;
            }
            app.streams
                .set_idle(access.account_id, access.session_id, body.idle);
        }
        page(&app, connection, &access, &body.accounts).await
    })
    .map(Json)
}

async fn page(
    app: &AppState,
    connection: &mut PgConnection,
    access: &Access,
    accounts: &[AccountId],
) -> ApiResult<PresencePage> {
    let mut asked = accounts.to_vec();
    asked.sort_unstable();
    asked.dedup();
    let fellows = store::fellow_members(connection, access.account_id, &asked).await?;
    let links = app.streams.presence_links(&asked);
    let sessions: Vec<_> = links.iter().map(|link| link.session_id).collect();
    let valid: HashSet<(Uuid, AccountId)> = sqlx::query_as(
        "select s.id, a.id from sessions s join devices d on d.id=s.device_id
         join accounts a on a.id=d.account_id
         where s.id=any($1) and s.expires_at>$2 and a.status='active'",
    )
    .bind(&sessions)
    .bind(app.now())
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .collect();
    let mut connected = HashMap::new();
    for link in links
        .iter()
        .filter(|link| valid.contains(&(link.session_id, link.account_id)))
    {
        let idle = connected.entry(link.account_id).or_insert(true);
        *idle &= link.idle;
    }
    let mut with_own = asked.clone();
    if !with_own.contains(&access.account_id) {
        with_own.push(access.account_id);
    }
    let preferences = preferences(connection, &with_own).await?;
    let own = preferences
        .iter()
        .find(|preference| preference.account_id == access.account_id)
        .map(|preference| preference.choice)
        .ok_or_else(ApiError::unauthenticated)?;
    let shown: HashMap<_, _> = preferences
        .into_iter()
        .filter_map(|preference| {
            may_see(access, &preference, &fellows).then(|| {
                let idle = connected.get(&preference.account_id);
                (
                    preference.account_id,
                    preference.choice.shown(
                        idle.is_some(),
                        idle.copied().unwrap_or(false),
                        preference.account_id == access.account_id,
                    ),
                )
            })
        })
        .collect();
    app.streams.watch_presence(
        access.account_id,
        access.session_id,
        &asked,
        &shown.keys().copied().collect(),
    );
    Ok(PresencePage {
        own,
        accounts: asked
            .into_iter()
            .map(|account_id| AccountPresence {
                account_id,
                presence: shown.get(&account_id).copied().unwrap_or(Presence::Offline),
            })
            .collect(),
    })
}

fn may_see(access: &Access, preference: &Preference, fellows: &HashSet<AccountId>) -> bool {
    let acquaintance = if preference.account_id == access.account_id {
        Acquaintance::Own
    } else if fellows.contains(&preference.account_id) {
        Acquaintance::SharedWorkspace
    } else if preference.administrator {
        Acquaintance::Administrator
    } else {
        Acquaintance::Unrelated
    };
    access.allows(Request::ViewAccount(acquaintance))
}

pub(crate) async fn may_receive_changes(
    app: &AppState,
    bearer: &Bearer,
    accounts: &[AccountId],
) -> ApiResult<bool> {
    read!(app, |connection| {
        let access = Access::open(connection, bearer, app.now()).await?;
        access.require(Request::OwnAccount)?;
        let fellows = store::fellow_members(connection, access.account_id, accounts).await?;
        Ok(preferences(connection, accounts)
            .await?
            .iter()
            .any(|preference| may_see(&access, preference, &fellows)))
    })
}

#[utoipa::path(put,path="/api/v1/account/presence",tag="account",request_body=SetPresence,security(("session"=[])),responses((status=200,body=SetPresence),ApiError))]
pub async fn set_presence(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiJson(body): ApiJson<SetPresence>,
) -> ApiResult<Json<SetPresence>> {
    let account_id = read!(app, |connection| {
        let access = Access::open(connection, &bearer, app.now()).await?;
        access.require(Request::OwnAccount)?;
        Ok(access.account_id)
    })?;
    let _visibility = app.streams.visibility_guard(account_id).await;
    let mut counted = false;
    let result = transact!(app, |connection| {
        let access = Access::open(connection, &bearer, app.now()).await?;
        access.require(Request::OwnAccount)?;
        super::account::limit_change(&app, &access, &mut counted)?;
        sqlx::query("update accounts set presence_mode=$2 where id=$1")
            .bind(access.account_id)
            .bind(body.choice.stored())
            .execute(&mut *connection)
            .await?;
        let dnd = body.choice == PresenceChoice::DoNotDisturb;
        let changed = sqlx::query(
            "insert into notification_settings(account_id,scope_id,settings) values($1,$2,jsonb_build_object('do_not_disturb',$3::boolean))
             on conflict(account_id,scope_id) do update set settings=jsonb_set(notification_settings.settings,'{do_not_disturb}',to_jsonb($3::boolean))
             where coalesce((notification_settings.settings->>'do_not_disturb')::boolean,false) is distinct from $3",
        ).bind(access.account_id).bind(Uuid::nil()).bind(dnd).execute(&mut *connection).await?.rows_affected();
        if changed > 0 {
            events::tell_account(
                connection,
                app.now(),
                access.account_id,
                &Event::NotificationSettingsChanged {},
            )
            .await?;
        }
        Ok((body, access.account_id))
    });
    let (body, account_id) = result?;
    app.streams
        .set_visible(account_id, body.choice != PresenceChoice::Invisible);
    app.streams.presence_changed_for(account_id);
    Ok(Json(body))
}
