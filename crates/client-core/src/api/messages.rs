//! The message routes (ADR 0408): history, writes, reactions, read positions, direct
//! conversations, and the files of messages.

use junto_protocol::ids::{AccountId, AttachmentId, MessageId};
use junto_protocol::message::{
    AttachmentLimits, Conversation, Document, EditMessage, MarkRead, Message, MessagePage,
    MessageState, OpenConversation, SendMessage, SetReaction, StartUpload, Upload,
};

use super::client::read_json;
use super::error::ApiError;
use super::session::ServerApi;
use super::transport::{BodyStream, Method, RequestBody};
use crate::model::PlaceId;
use crate::secret::SecretBytes;

/// The bytes of one part of an upload. The last part of a file can be shorter.
pub const CHUNK_BYTES: usize = 1024 * 1024;

/// Which page of the history of a place a read asks for. Without a field the page is the
/// latest one. The server accepts one of the four cursors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PageQuery {
    /// Messages with a lower position.
    pub before: Option<u64>,
    /// Messages with a higher position.
    pub after: Option<u64>,
    /// Messages before this one, also those that share its position.
    pub before_message: Option<MessageId>,
    /// Messages after this one, also those that share its position.
    pub after_message: Option<MessageId>,
    /// From 1 to 200. The server uses 50 without it.
    pub limit: Option<u32>,
}

impl PageQuery {
    pub(super) fn pairs(&self) -> Vec<(&'static str, String)> {
        let mut pairs = Vec::new();
        if let Some(before) = self.before {
            pairs.push(("before", before.to_string()));
        }
        if let Some(after) = self.after {
            pairs.push(("after", after.to_string()));
        }
        if let Some(before) = self.before_message {
            pairs.push(("before_message", before.to_string()));
        }
        if let Some(after) = self.after_message {
            pairs.push(("after_message", after.to_string()));
        }
        if let Some(limit) = self.limit {
            pairs.push(("limit", limit.to_string()));
        }
        pairs
    }
}

/// The body of a file as it arrives.
pub struct FileBody {
    /// True when the body starts at the byte that was asked for. False when it is the
    /// whole file.
    pub partial: bool,
    /// The first byte of the range that the answer states, when it states one.
    pub starts_at: Option<u64>,
    pub body: BodyStream,
}

/// An answer that holds a message above the bounds of the client (ADR 0747) is not an
/// answer that the client reads: nothing of it is used.
fn above_bounds() -> ApiError {
    ApiError::InvalidResponse {
        status: 200,
        reason: "a message above the bounds of a message",
    }
}

pub(super) fn bounded(message: Message) -> Result<Message, ApiError> {
    if crate::model::fits(&message) {
        Ok(message)
    } else {
        Err(above_bounds())
    }
}

pub(super) fn all_bounded(messages: &[Message]) -> Result<(), ApiError> {
    if messages.iter().all(crate::model::fits) {
        Ok(())
    } else {
        Err(above_bounds())
    }
}

fn messages_of(place: PlaceId) -> String {
    match place {
        PlaceId::Channel(channel_id) => format!("/channels/{channel_id}/messages"),
        PlaceId::Direct(conversation_id) => format!("/conversations/{conversation_id}/messages"),
        PlaceId::Thread { root_id, .. } => format!("/messages/{root_id}/thread"),
    }
}

/// An emoji as one segment of a path. Every byte is written as an escape, because an
/// emoji can contain `#` and `*`, and its other bytes are not ASCII.
fn emoji_segment(emoji: &str) -> String {
    emoji.bytes().map(|byte| format!("%{byte:02X}")).collect()
}

impl ServerApi {
    // History and writes.

    /// One page of the messages of a place, in the order of their positions. For a
    /// thread these are its replies; [`ServerApi::thread`] also gives its root.
    pub async fn messages(
        &self,
        place: PlaceId,
        page: &PageQuery,
    ) -> Result<MessagePage, ApiError> {
        if let PlaceId::Thread { root_id, .. } = place {
            return Ok(self.thread(root_id, page).await?.replies);
        }
        let response = self
            .call(Method::Get, messages_of(place), page.pairs(), None)
            .await?;
        let page: MessagePage = read_json(&response)?;
        all_bounded(&page.messages)?;
        Ok(page)
    }

    /// Sends a message. The server has no key that names one attempt: after a lost answer
    /// the caller cannot tell from this call whether the message exists (ADR 0721).
    pub async fn send_message(
        &self,
        place: PlaceId,
        message: &SendMessage,
    ) -> Result<Message, ApiError> {
        bounded(self.send(Method::Post, messages_of(place), message).await?)
    }

    pub async fn message(&self, message_id: MessageId) -> Result<Message, ApiError> {
        bounded(self.get(format!("/messages/{message_id}")).await?)
    }

    /// Replaces the text of a message of the caller. No earlier text is kept (ADR 0401).
    pub async fn edit_message(
        &self,
        message_id: MessageId,
        document: Document,
    ) -> Result<Message, ApiError> {
        let path = format!("/messages/{message_id}");
        bounded(
            self.send(Method::Put, path, &EditMessage { document })
                .await?,
        )
    }

    pub async fn delete_message(&self, message_id: MessageId) -> Result<(), ApiError> {
        self.act(Method::Delete, format!("/messages/{message_id}"))
            .await
    }

    /// Adds the reaction of the caller with one standard emoji. Adding it twice changes
    /// nothing.
    pub async fn add_reaction(&self, message_id: MessageId, emoji: &str) -> Result<(), ApiError> {
        let body = super::client::encode(&SetReaction {
            emoji: emoji.to_owned(),
        })?;
        let path = format!("/messages/{message_id}/reaction");
        self.call(Method::Put, path, Vec::new(), Some(body))
            .await
            .map(|_| ())
    }

    pub async fn remove_reaction(
        &self,
        message_id: MessageId,
        emoji: &str,
    ) -> Result<(), ApiError> {
        let path = format!("/messages/{message_id}/reaction/{}", emoji_segment(emoji));
        self.act(Method::Delete, path).await
    }

    // Read positions.

    /// The read position of the account in a channel or a direct conversation, with the
    /// unread counts. A thread has [`ServerApi::thread_state`].
    pub async fn message_state(&self, place: PlaceId) -> Result<MessageState, ApiError> {
        self.get(format!("{}/read", messages_of(place))).await
    }

    /// Moves the read position of the account in a channel or a direct conversation
    /// forward. A lower position changes nothing. A thread has
    /// [`ServerApi::mark_thread_read`].
    pub async fn mark_read(&self, place: PlaceId, position: u64) -> Result<MessageState, ApiError> {
        let path = format!("{}/read", messages_of(place));
        self.send(Method::Put, path, &MarkRead { position }).await
    }

    // Direct conversations.

    /// The direct conversations of the account that the contact rule allows now
    /// (ADR 0307).
    pub async fn conversations(&self) -> Result<Vec<Conversation>, ApiError> {
        self.get("/conversations".to_owned()).await
    }

    /// The direct conversation with an account. It is created when there is none.
    pub async fn open_conversation(&self, account_id: AccountId) -> Result<Conversation, ApiError> {
        let path = "/conversations".to_owned();
        self.send(Method::Post, path, &OpenConversation { account_id })
            .await
    }

    // The files of messages.

    pub async fn attachment_limits(&self) -> Result<AttachmentLimits, ApiError> {
        self.get("/attachment-limits".to_owned()).await
    }

    /// Replaces the limits of the server. Only an administrator can do this.
    pub async fn set_attachment_limits(
        &self,
        limits: &AttachmentLimits,
    ) -> Result<AttachmentLimits, ApiError> {
        self.send(Method::Put, "/attachment-limits".to_owned(), limits)
            .await
    }

    /// Declares a file for a message of one place. The server reserves its size.
    pub async fn start_upload(&self, upload: &StartUpload) -> Result<Upload, ApiError> {
        self.send(Method::Post, "/attachments".to_owned(), upload)
            .await
    }

    /// How many bytes of an upload the server holds.
    pub async fn upload_status(&self, attachment_id: AttachmentId) -> Result<Upload, ApiError> {
        self.get(format!("/attachments/{attachment_id}/upload"))
            .await
    }

    /// Sends the part of the file that starts at `offset`: [`CHUNK_BYTES`] bytes, or the
    /// rest of the file. The server accepts the next offset only, and the same bytes at an
    /// offset that it holds.
    pub async fn put_chunk(
        &self,
        attachment_id: AttachmentId,
        offset: u64,
        bytes: Vec<u8>,
    ) -> Result<Upload, ApiError> {
        let path = format!("/attachments/{attachment_id}/chunks/{offset}");
        let body = RequestBody::bytes(SecretBytes::from_vec(bytes));
        read_json(&self.call_part(path, body).await?)
    }

    /// Asks the server to check the file against its declared hash. The answer states
    /// whether the upload is complete.
    pub async fn complete_upload(&self, attachment_id: AttachmentId) -> Result<Upload, ApiError> {
        let path = format!("/attachments/{attachment_id}/complete");
        read_json(&self.call(Method::Post, path, Vec::new(), None).await?)
    }

    pub async fn cancel_upload(&self, attachment_id: AttachmentId) -> Result<(), ApiError> {
        self.act(
            Method::Delete,
            format!("/attachments/{attachment_id}/upload"),
        )
        .await
    }

    /// The file of an attachment from the byte `from` on. The file comes from another
    /// account: the caller treats it as a file of an unknown sender (ADR 0408).
    pub async fn attachment_file(
        &self,
        attachment_id: AttachmentId,
        from: u64,
    ) -> Result<FileBody, ApiError> {
        let opened = self
            .open(format!("/attachments/{attachment_id}/file"), from)
            .await?;
        Ok(FileBody {
            partial: opened.status == 206,
            starts_at: opened.range_start,
            body: opened.body,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_emoji_is_one_escaped_segment_of_a_path() {
        assert_eq!(emoji_segment("#\u{fe0f}\u{20e3}"), "%23%EF%B8%8F%E2%83%A3");
        assert_eq!(emoji_segment("\u{1f44d}"), "%F0%9F%91%8D");
        let address = crate::address::ServerAddress::parse("chat.example.test").unwrap();
        let path = format!("/api/v1/messages/m/reaction/{}", emoji_segment("*\u{20e3}"));
        let url = address.endpoint(&path);
        assert_eq!(url.path(), "/api/v1/messages/m/reaction/%2A%E2%83%A3");
        assert_eq!(url.fragment(), None);
    }

    #[test]
    fn a_page_query_names_the_fields_that_are_set() {
        assert!(PageQuery::default().pairs().is_empty());
        let message_id = MessageId::generate();
        let query = PageQuery {
            after_message: Some(message_id),
            limit: Some(200),
            ..PageQuery::default()
        };
        assert_eq!(
            query.pairs(),
            [
                ("after_message", message_id.to_string()),
                ("limit", "200".to_owned())
            ]
        );
        let query = PageQuery {
            before: Some(7),
            ..PageQuery::default()
        };
        assert_eq!(query.pairs(), [("before", "7".to_owned())]);
    }
}
