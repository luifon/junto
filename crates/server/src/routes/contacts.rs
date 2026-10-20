//! The accounts that the caller may write to and call (ADR 0307), found by a part of their
//! name. A person chooses there an account that is in no member list that the person can
//! open: an administrator reaches every active account, and every active account reaches
//! an administrator.

use axum::{Json, extract::State};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use junto_authz::{Contact, Request, ServerPermission, ServerPermissions};
use junto_protocol::ids::AccountId;
use junto_protocol::message::{ContactAccount, ContactPage};
use serde::Deserialize;

use crate::access::Access;
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiQuery, Bearer};
use crate::state::AppState;
use crate::{db, read, store};

/// The accounts of one page when the request names no limit, and the most that it can name.
pub const DEFAULT_LIMIT: u32 = 50;
pub const MOST_LIMIT: u32 = 100;
/// The longest part of a name that is searched: the length of a name (ADR 0119).
const LONGEST_QUERY: usize = 64;

#[derive(Deserialize, utoipa::IntoParams)]
pub struct Query {
    /// A part of the name, compared without regard to case. Absent or empty: every name.
    pub q: Option<String>,
    /// The `next_after` of the page before.
    pub after: Option<String>,
    /// 1 to 100; 50 when absent.
    pub limit: Option<u32>,
}

/// The contacts of an account that administers the server: every active account, read in
/// the order of the index `accounts_contact_order`. Every account is visible to it, so the
/// rows that the statement reads beyond a page tell it nothing that it may not know.
/// `$1` the caller, `$2` the part of the name, `$3` and `$4` the cursor, `$5` the rows.
pub const CONTACTS_OF_ADMINISTRATOR: &str = "select a.id, a.name, lower(a.name)
     from accounts a
     where a.status = 'active'
       and a.id <> $1
       and strpos(lower(a.name), $2) > 0
       and ($3::text is null
            or (lower(a.name) collate \"C\", a.id) > ($3 collate \"C\", $4::uuid))
     order by lower(a.name) collate \"C\", a.id
     limit $5";

/// The contacts of every other account: it starts from the candidates, the members of the
/// workspaces of the caller, the server owner and the holders of a server role with the
/// administrator permission, so the rows that it reads follow those and not the accounts
/// that the caller may not see. Each lookup is a lateral subquery with `offset 0`, which the
/// planner keeps as a lookup by key for each row before it: a join could become a merge or
/// a hash join that reads every member, holder or account (ADR 0119 states the same of the
/// member list). `$1` the caller, `$2` the part of the name, `$3` and `$4` the cursor, `$5`
/// the bit of the administrator permission, `$6` the rows.
pub const CONTACTS_OF_MEMBER: &str = "with candidates as (
         select theirs.account_id as id
         from workspace_members mine
         cross join lateral (
             select account_id from workspace_members
             where workspace_id = mine.workspace_id offset 0
         ) theirs
         where mine.account_id = $1
         union
         select owner_account_id from server_state where owner_account_id is not null
         union
         select held.account_id
         from server_roles r
         cross join lateral (
             select account_id from account_server_roles
             where role_id = r.id offset 0
         ) held
         where (r.permissions & $5) <> 0
     )
     select a.id, a.name, lower(a.name)
     from candidates c
     cross join lateral (
         select id, name, status from accounts where id = c.id offset 0
     ) a
     where a.status = 'active'
       and a.id <> $1
       and strpos(lower(a.name), $2) > 0
       and ($3::text is null
            or (lower(a.name) collate \"C\", a.id) > ($3 collate \"C\", $4::uuid))
     order by lower(a.name) collate \"C\", a.id
     limit $6";

/// The place after the last account of a page: its name in lower case and its identifier,
/// which order the accounts. The cursor holds both, so a renamed account does not move a
/// later page.
fn cursor(name: &str, id: AccountId) -> String {
    URL_SAFE_NO_PAD.encode(format!("{id}:{name}"))
}

fn read_cursor(text: &str) -> ApiResult<(String, AccountId)> {
    let invalid = || ApiError::invalid("invalid contact cursor");
    let bytes = URL_SAFE_NO_PAD.decode(text).map_err(|_| invalid())?;
    let text = String::from_utf8(bytes).map_err(|_| invalid())?;
    let (id, name) = text.split_once(':').ok_or_else(invalid)?;
    Ok((name.to_owned(), id.parse().map_err(|_| invalid())?))
}

/// The active accounts other than the caller that the caller may contact, whose name holds
/// `q`, in the order of their names in lower case. For an administrator these are all
/// active accounts; for every other account those that share a workspace with it and those
/// that administer the server. Each account is decided by the permission function, as a
/// direct message is.
#[utoipa::path(
    get,
    path = "/api/v1/direct-messages/contacts",
    tag = "messages",
    params(Query),
    security(("session" = [])),
    responses((status = 200, body = ContactPage), ApiError)
)]
pub async fn list_contacts(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiQuery(query): ApiQuery<Query>,
) -> ApiResult<Json<ContactPage>> {
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT);
    let part = query.q.as_deref().unwrap_or_default().trim().to_lowercase();
    if !(1..=MOST_LIMIT).contains(&limit)
        || part.chars().count() > LONGEST_QUERY
        || part.chars().any(char::is_control)
    {
        return Err(ApiError::invalid(
            "use a name part of at most 64 characters without control characters and a \
             limit of 1 to 100",
        ));
    }
    let after = query.after.as_deref().map(read_cursor).transpose()?;
    let administrator = db::server_bits(ServerPermissions::from(ServerPermission::Administrator));
    let now = app.now();
    let page = read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::OwnAccount)?;
        // The caller reaches an account without a common workspace only when it
        // administers the server itself.
        let reaches_everyone = access.allows(Request::Contact(Contact::Unrelated));
        let after_name = after.as_ref().map(|(name, _)| name.as_str());
        let after_id = after.as_ref().map(|(_, id)| *id);
        let fetched = i64::from(limit) + 1;
        let rows: Vec<(AccountId, String, String)> = if reaches_everyone {
            sqlx::query_as(CONTACTS_OF_ADMINISTRATOR)
                .bind(access.account_id)
                .bind(&part)
                .bind(after_name)
                .bind(after_id)
                .bind(fetched)
                .fetch_all(&mut *connection)
                .await?
        } else {
            sqlx::query_as(CONTACTS_OF_MEMBER)
                .bind(access.account_id)
                .bind(&part)
                .bind(after_name)
                .bind(after_id)
                .bind(administrator)
                .bind(fetched)
                .fetch_all(&mut *connection)
                .await?
        };
        let more = rows.len() > limit as usize;
        let rows = &rows[..rows.len().min(limit as usize)];
        let ids: Vec<AccountId> = rows.iter().map(|(id, _, _)| *id).collect();
        let standing = store::contacts(connection, &access, &ids).await?;
        let accounts = rows
            .iter()
            .filter(|(id, _, _)| {
                standing
                    .get(id)
                    .is_some_and(|contact| access.allows(Request::Contact(*contact)))
            })
            .map(|(id, name, _)| ContactAccount {
                id: *id,
                name: name.clone(),
            })
            .collect();
        let next_after = rows
            .last()
            .filter(|_| more)
            .map(|(id, _, lower)| cursor(lower, *id));
        Ok(ContactPage {
            accounts,
            next_after,
        })
    })?;
    Ok(Json(page))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cursor_holds_the_name_and_the_identifier_of_the_last_account() {
        let id = AccountId::generate();
        let text = cursor("bruno: the second", id);
        assert_eq!(
            read_cursor(&text).unwrap(),
            ("bruno: the second".to_owned(), id)
        );
        for invalid in ["", "%%", &URL_SAFE_NO_PAD.encode("no identifier")] {
            assert!(read_cursor(invalid).is_err(), "{invalid}");
        }
    }
}
