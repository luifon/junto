//! The system lines of a direct-message call in the conversation of its two accounts
//! (ADR 0518).
use super::{Place, notifications};
use crate::access::Access;
use crate::error::ApiResult;
use crate::events;
use junto_protocol::{
    events::Event,
    ids::{AccountId, ConversationId, MessageId},
    message::SystemMessage,
};
use sqlx::{PgConnection, types::Json};
use time::OffsetDateTime;

/// What a line states about the call that `caller` made for `callee`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Line {
    /// The caller entered the call, and the callee was rung.
    Started,
    /// A ring ended and the callee had taken no place in the call, or the call ended and
    /// the callee was never inside it.
    NotAnswered,
    /// The callee declined the ring.
    Declined,
    /// The call ended after both accounts were in its room for `seconds`.
    Ended { seconds: u64 },
}

impl Line {
    const fn action(self) -> &'static str {
        match self {
            Self::Started => "direct_call_started",
            Self::NotAnswered => "direct_call_not_answered",
            Self::Declined => "direct_call_declined",
            Self::Ended { .. } => "direct_call_ended",
        }
    }
}

/// Writes one line into the conversation of the two accounts, which is created when the
/// two have none. The line is public in the conversation: both accounts read it while they
/// may read the conversation, whatever the notification settings of either are. A call
/// that was not answered counts as unread for the callee and gives the callee a notice by
/// the policy of the conversation, so the caller reads the same for a callee who muted the
/// conversation, who is in a quiet time, and who did not answer.
///
/// Nothing is written when the two accounts can no longer reach each other (ADR 0307), or
/// one of them is not active: the conversation is closed to both then, and a line would
/// wait in it unread.
pub async fn line(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    caller: AccountId,
    callee: AccountId,
    line: Line,
) -> ApiResult<()> {
    let Some(access) = Access::of_account(&mut *connection, caller).await? else {
        return Ok(());
    };
    if !crate::store::may_reach(&mut *connection, &access, callee).await? {
        return Ok(());
    }
    let (low, high) = (caller.min(callee), caller.max(callee));
    let (conversation, created) =
        Place::conversation_between(&mut *connection, low, high, ConversationId::generate())
            .await?;
    let place = Place::of_conversation(&conversation);
    if created {
        let opened = Event::ConversationOpened { conversation };
        events::publish(&mut *connection, now, &opened).await?;
    }
    let (actor, subject) = match line {
        Line::Declined => (callee, caller),
        Line::Started | Line::NotAnswered | Line::Ended { .. } => (caller, callee),
    };
    let notifies = (line == Line::NotAnswered).then_some(callee);
    let system = SystemMessage {
        action: line.action().to_owned(),
        subject_id: Some(subject),
        actor_id: Some(actor),
        recording_id: None,
        duration_seconds: match line {
            Line::Ended { seconds } => Some(seconds),
            _ => None,
        },
        name: None,
        previous_name: None,
    };
    let id = MessageId::generate();
    let position = place.next(&mut *connection).await?;
    sqlx::query(
        "insert into messages (id, place_id, position, created_at, system, notifies)
         values ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(place.id)
    .bind(position)
    .bind(now)
    .bind(Json(&system))
    .bind(notifies)
    .execute(&mut *connection)
    .await?;
    super::changed(
        &mut *connection,
        now,
        place.value,
        id,
        u64::try_from(position).unwrap_or_default(),
        false,
    )
    .await?;
    if let Some(account_id) = notifies {
        notifications::missed_call(connection, now, id, place.id, account_id).await?;
    }
    Ok(())
}
