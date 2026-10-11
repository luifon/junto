//! Access and ordered positions in a channel or private two-account conversation.
use crate::access::Access;
use crate::error::{ApiError, ApiResult};
use crate::store;
use junto_authz::{
    Authorship, ChannelScope, MessagePlace as AuthPlace, Participation, Permanence, Request,
    WorkspacePermission,
};
use junto_protocol::{
    ids::{AccountId, ChannelId, ConversationId},
    message::{Message, MessagePlace},
};
use sqlx::PgConnection;
use uuid::Uuid;

pub struct Place {
    pub id: Uuid,
    pub value: MessagePlace,
    pub scope: Option<ChannelScope>,
}
impl Place {
    pub async fn channel(
        connection: &mut PgConnection,
        access: &Access,
        id: ChannelId,
    ) -> ApiResult<Self> {
        let (record, scope) = access.channel(connection, id).await?;
        access.require(Request::Channel {
            scope: &scope,
            permission: WorkspacePermission::ViewChannel,
        })?;
        Ok(Self {
            id: id.as_uuid(),
            value: MessagePlace::Channel {
                workspace_id: record.channel.workspace_id,
                channel_id: id,
            },
            scope: Some(scope),
        })
    }
    pub async fn direct(
        connection: &mut PgConnection,
        access: &Access,
        id: ConversationId,
    ) -> ApiResult<Self> {
        let row: Option<(AccountId, AccountId)> = sqlx::query_as(
            "select low_account_id, high_account_id from conversations where id = $1",
        )
        .bind(id)
        .fetch_optional(&mut *connection)
        .await?;
        let (low, high) = row.ok_or_else(ApiError::not_found)?;
        let participant = access.account_id == low || access.account_id == high;
        if !access.allows(Request::DirectMessage(if participant {
            Participation::Participant
        } else {
            Participation::Outsider
        })) {
            return Err(ApiError::not_found());
        }
        let other = if access.account_id == low { high } else { low };
        if !store::may_reach(connection, access, other).await? {
            return Err(ApiError::not_found());
        }
        Ok(Self {
            id: id.as_uuid(),
            value: MessagePlace::Direct {
                conversation_id: id,
                low_account_id: low,
                high_account_id: high,
            },
            scope: None,
        })
    }
    pub async fn conversations(
        connection: &mut PgConnection,
        access: &Access,
    ) -> ApiResult<Vec<junto_protocol::message::Conversation>> {
        let rows: Vec<(ConversationId, AccountId, AccountId)> = sqlx::query_as(
            "select id,low_account_id,high_account_id from conversations
             where low_account_id=$1 or high_account_id=$1 order by id",
        )
        .bind(access.account_id)
        .fetch_all(&mut *connection)
        .await?;
        let peers: Vec<_> = rows
            .iter()
            .map(|(_, low, high)| {
                if *low == access.account_id {
                    *high
                } else {
                    *low
                }
            })
            .collect();
        let contacts = store::contacts(connection, access, &peers).await?;
        Ok(rows
            .into_iter()
            .zip(peers)
            .filter_map(|((id, low, high), other)| {
                contacts
                    .get(&other)
                    .filter(|contact| access.allows(Request::Contact(**contact)))
                    .map(|_| junto_protocol::message::Conversation {
                        id,
                        low_account_id: low,
                        high_account_id: high,
                    })
            })
            .collect())
    }
    pub async fn load(connection: &mut PgConnection, access: &Access, id: Uuid) -> ApiResult<Self> {
        let row: Option<(Option<ChannelId>, Option<ConversationId>)> =
            sqlx::query_as("select channel_id, conversation_id from message_places where id = $1")
                .bind(id)
                .fetch_optional(&mut *connection)
                .await?;
        match row {
            Some((Some(id), None)) => Self::channel(connection, access, id).await,
            Some((None, Some(id))) => Self::direct(connection, access, id).await,
            _ => Err(ApiError::not_found()),
        }
    }
    pub fn edit(&self, access: &Access, message: &Message) -> ApiResult<()> {
        if message.deleted {
            return Err(ApiError::not_found());
        }
        access.require(Request::EditMessage {
            place: self.scope.as_ref().map_or(
                AuthPlace::DirectMessage(Participation::Participant),
                AuthPlace::Channel,
            ),
            permanence: if message.permanent {
                Permanence::Permanent
            } else {
                Permanence::Editable
            },
            authorship: if message.author_id == Some(access.account_id) {
                Authorship::Author
            } else {
                Authorship::Other
            },
        })
    }
    pub async fn ensure(&self, connection: &mut PgConnection) -> ApiResult<()> {
        let (channel, direct) = match self.value {
            MessagePlace::Channel { channel_id, .. } => (Some(channel_id), None),
            MessagePlace::Direct {
                conversation_id, ..
            } => (None, Some(conversation_id)),
        };
        sqlx::query(
            "insert into message_places (id, channel_id, conversation_id)
             values ($1,$2,$3)
             on conflict do nothing",
        )
        .bind(self.id)
        .bind(channel)
        .bind(direct)
        .execute(connection)
        .await?;
        Ok(())
    }
    pub async fn next(&self, connection: &mut PgConnection) -> ApiResult<i64> {
        self.ensure(connection).await?;
        Ok(sqlx::query_scalar(
            "update message_places set head = head + 1 where id = $1 returning head",
        )
        .bind(self.id)
        .fetch_one(connection)
        .await?)
    }
}
