use std::cell::RefCell;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use junto_protocol::events::{Cursor, EVENT_CURSOR_HEADER};
use sqlx::PgConnection;

use crate::error::ApiError;
use crate::state::AppState;
use junto_protocol::ids::AccountId;

struct Receipt {
    committed: bool,
    failed: bool,
    account: Option<AccountId>,
    clock: Arc<dyn crate::clock::Clock>,
    cursor: Option<Cursor>,
}

tokio::task_local! {
    static RECEIPT: RefCell<Receipt>;
}

pub fn requested() -> bool {
    RECEIPT.try_with(|_| ()).is_ok()
}

pub fn authorize_session(account: AccountId) {
    let _ = RECEIPT.try_with(|receipt| receipt.borrow_mut().account = Some(account));
}

pub async fn committed(connection: &mut PgConnection) {
    let who = RECEIPT
        .try_with(|receipt| {
            let mut receipt = receipt.borrow_mut();
            receipt.committed = true;
            if receipt.failed {
                None
            } else {
                receipt
                    .account
                    .map(|account| (account, receipt.clock.clone()))
            }
        })
        .ok()
        .flatten();
    let Some((account, clock)) = who else {
        return;
    };
    match read_cursor(connection, account, clock.as_ref()).await {
        Ok(Some(cursor)) => {
            let _ = RECEIPT.try_with(|receipt| receipt.borrow_mut().cursor = Some(cursor));
        }
        Ok(None) => {}
        Err(error) => {
            let _ = RECEIPT.try_with(|receipt| {
                let mut receipt = receipt.borrow_mut();
                receipt.failed = true;
                receipt.cursor = None;
            });
            tracing::warn!(%error, "request committed but its event receipt could not be read; responding without a cursor");
        }
    }
}

async fn read_cursor(
    connection: &mut PgConnection,
    account: AccountId,
    clock: &dyn crate::clock::Clock,
) -> Result<Option<Cursor>, sqlx::Error> {
    let (first, last): (Option<i64>, Option<i64>) = sqlx::query_as(
        "select nullif(current_setting('junto.event_first', true), '')::bigint,
                nullif(current_setting('junto.event_cursor', true), '')::bigint",
    )
    .fetch_one(&mut *connection)
    .await?;
    let Some(last) = last else {
        return Ok(None);
    };
    let first =
        first.ok_or_else(|| sqlx::Error::Protocol("event receipt has no first position".into()))?;
    let mut read =
        crate::database::begin_on_connection(connection, crate::database::Mode::ReadOnly).await?;
    let result =
        crate::events::delivery::last_receivable(&mut read, account, first, last, clock.now())
            .await;
    read.commit().await?;
    result.map_err(|error| {
        sqlx::Error::Protocol(format!("receipt delivery decision failed: {error:?}"))
    })
}

pub async fn middleware(State(app): State<AppState>, request: Request, next: Next) -> Response {
    let mutation = matches!(
        *request.method(),
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    );
    RECEIPT
        .scope(
            RefCell::new(Receipt {
                committed: false,
                failed: false,
                account: None,
                cursor: None,
                clock: app.clock.clone(),
            }),
            async move {
                let mut response = next.run(request).await;
                if !response.status().is_success() {
                    return response;
                }
                let value = RECEIPT.with(|receipt| {
                    let receipt = receipt.borrow();
                    (!receipt.failed
                        && receipt.account.is_some()
                        && (mutation || receipt.committed))
                        .then(|| serde_json::to_string(&receipt.cursor))
                });
                if let Some(value) = value {
                    let value = match value {
                        Ok(value) => value,
                        Err(error) => return ApiError::internal(&error).into_response(),
                    };
                    match HeaderValue::from_str(&value) {
                        Ok(value) => {
                            response.headers_mut().insert(EVENT_CURSOR_HEADER, value);
                        }
                        Err(error) => return ApiError::internal(&error).into_response(),
                    }
                }
                response
            },
        )
        .await
}
