use crate::{error::ApiResult, events};
use junto_protocol::{
    events::Event,
    ids::{AccountId, ConversationId},
    message::{MessagePlace, MessageState},
};
use sqlx::PgConnection;
use time::OffsetDateTime;

pub async fn list(
    connection: &mut PgConnection,
    account: AccountId,
    states: &[MessageState],
) -> ApiResult<Vec<ConversationId>> {
    let visible: Vec<_> = states
        .iter()
        .filter_map(|state| match state.place {
            MessagePlace::Direct {
                conversation_id, ..
            }
            | MessagePlace::Group { conversation_id } => Some(conversation_id),
            _ => None,
        })
        .collect();
    Ok(sqlx::query_scalar("select conversation_id from hidden_conversations where account_id=$1 and conversation_id=any($2) order by conversation_id")
        .bind(account).bind(visible).fetch_all(connection).await?)
}

async fn tell(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    account: AccountId,
    place: MessagePlace,
    hidden: bool,
) -> ApiResult<()> {
    let (MessagePlace::Direct {
        conversation_id, ..
    }
    | MessagePlace::Group { conversation_id }) = place
    else {
        return Ok(());
    };
    let event = Event::ConversationVisibilityChanged {
        conversation_id,
        hidden,
    };
    if matches!(place, MessagePlace::Group { .. }) {
        events::tell_account_at(
            connection,
            now,
            account,
            &[junto_protocol::call::CallPlace::Group { conversation_id }],
            &event,
        )
        .await
    } else {
        events::tell_account(connection, now, account, &event).await
    }
}

pub async fn set(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    account: AccountId,
    place: MessagePlace,
    hidden: bool,
) -> ApiResult<()> {
    let (MessagePlace::Direct {
        conversation_id, ..
    }
    | MessagePlace::Group { conversation_id }) = place
    else {
        return Ok(());
    };
    let changed = if hidden {
        sqlx::query("insert into hidden_conversations(account_id,conversation_id) values($1,$2) on conflict do nothing")
            .bind(account).bind(conversation_id).execute(&mut *connection).await?.rows_affected()
    } else {
        sqlx::query("delete from hidden_conversations where account_id=$1 and conversation_id=$2")
            .bind(account)
            .bind(conversation_id)
            .execute(&mut *connection)
            .await?
            .rows_affected()
    };
    if changed > 0 {
        tell(connection, now, account, place, hidden).await?;
    }
    Ok(())
}

pub async fn restore_for_message(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    place: MessagePlace,
) -> ApiResult<()> {
    let accounts: Vec<AccountId> = match place {
        MessagePlace::Direct { conversation_id, low_account_id, high_account_id } => sqlx::query_scalar(
            "delete from hidden_conversations where conversation_id=$1 and account_id=any($2) returning account_id")
            .bind(conversation_id).bind([low_account_id, high_account_id].as_slice()).fetch_all(&mut *connection).await?,
        MessagePlace::Group { conversation_id } => sqlx::query_scalar(
            "delete from hidden_conversations h using conversation_members m where h.conversation_id=$1 and m.conversation_id=h.conversation_id and m.account_id=h.account_id returning h.account_id")
            .bind(conversation_id).fetch_all(&mut *connection).await?,
        _ => return Ok(()),
    };
    for account in accounts {
        tell(&mut *connection, now, account, place, false).await?;
    }
    Ok(())
}
