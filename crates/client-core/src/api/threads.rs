//! The routes of threads, forks and pins (ADR 0409).

use junto_protocol::ids::{ChannelId, MessageId};
use junto_protocol::message::{
    FollowedThreads, ForkThread, MarkRead, Message, SendMessage, SetThreadFollow, ThreadPage,
    ThreadState, UnreadThreads,
};
use junto_protocol::structure::{Channel, ChannelKind};

use super::client::read_json;
use super::error::ApiError;
use super::messages::{PageQuery, all_bounded, bounded};
use super::session::ServerApi;
use super::transport::Method;

fn thread_of(root_id: MessageId) -> String {
    format!("/messages/{root_id}/thread")
}

impl ServerApi {
    // Threads.

    /// The root of a thread, one page of its replies in the order of their positions, and
    /// the channels that the thread was forked into which the caller can read.
    pub async fn thread(
        &self,
        root_id: MessageId,
        page: &PageQuery,
    ) -> Result<ThreadPage, ApiError> {
        let response = self
            .call(Method::Get, thread_of(root_id), page.pairs(), None)
            .await?;
        let page: ThreadPage = read_json(&response)?;
        all_bounded(std::slice::from_ref(&page.root))?;
        all_bounded(&page.replies.messages)?;
        Ok(page)
    }

    /// Sends a reply into a thread. As a send into a place, the call does not tell after a
    /// lost answer whether the reply exists (ADR 0721).
    pub async fn reply_in_thread(
        &self,
        root_id: MessageId,
        message: &SendMessage,
    ) -> Result<Message, ApiError> {
        bounded(self.send(Method::Post, thread_of(root_id), message).await?)
    }

    /// Copies a thread into a new channel of the workspace of its channel. The caller
    /// needs "manage channel" in the channel and in the workspace. A thread has one fork:
    /// a second request answers with a conflict (ADR 0409).
    pub async fn fork_thread(
        &self,
        root_id: MessageId,
        name: &str,
        kind: ChannelKind,
    ) -> Result<Channel, ApiError> {
        let body = ForkThread {
            name: name.to_owned(),
            kind,
        };
        self.send(Method::Post, format!("/messages/{root_id}/fork"), &body)
            .await
    }

    /// The read position of the caller in a thread. The server holds one only for an
    /// account that wrote in the thread or was mentioned in it; every other account gets
    /// `not_found`.
    pub async fn thread_state(&self, root_id: MessageId) -> Result<ThreadState, ApiError> {
        self.get(format!("{}/read", thread_of(root_id))).await
    }

    /// Moves the read position of the caller in a thread forward.
    pub async fn mark_thread_read(
        &self,
        root_id: MessageId,
        position: u64,
    ) -> Result<ThreadState, ApiError> {
        let path = format!("{}/read", thread_of(root_id));
        self.send(Method::Put, path, &MarkRead { position }).await
    }

    /// One page of the threads that hold something unread for the caller, the newest
    /// first. `before` is the continuation of the page before.
    pub async fn unread_threads(
        &self,
        before: Option<&str>,
        limit: Option<u32>,
    ) -> Result<UnreadThreads, ApiError> {
        let mut query = Vec::new();
        if let Some(before) = before {
            query.push(("before", before.to_owned()));
        }
        if let Some(limit) = limit {
            query.push(("limit", limit.to_string()));
        }
        let response = self
            .call(Method::Get, "/threads/unread".to_owned(), query, None)
            .await?;
        read_json(&response)
    }

    /// Stores the choice of the caller to follow a thread or not. The replies of a thread
    /// that is not followed do not count as unread and do not notify; a mention still
    /// does (ADR 0412).
    pub async fn set_thread_follow(
        &self,
        root_id: MessageId,
        following: bool,
    ) -> Result<ThreadState, ApiError> {
        let path = format!("{}/follow", thread_of(root_id));
        self.send(Method::Put, path, &SetThreadFollow { following })
            .await
    }

    /// Removes the choice of the caller for a thread. The rule decides again: an account
    /// that wrote in the thread or was named in it follows it.
    pub async fn clear_thread_follow(&self, root_id: MessageId) -> Result<(), ApiError> {
        self.act(Method::Delete, format!("{}/follow", thread_of(root_id)))
            .await
    }

    /// One page of the threads of a channel that the caller follows and that have a
    /// reply, the one with the newest reply first, read threads included. `before` is the
    /// continuation of the page before (ADR 0413).
    pub async fn followed_threads(
        &self,
        channel_id: ChannelId,
        before: Option<&str>,
        limit: Option<u32>,
    ) -> Result<FollowedThreads, ApiError> {
        let mut query = Vec::new();
        if let Some(before) = before {
            query.push(("before", before.to_owned()));
        }
        if let Some(limit) = limit {
            query.push(("limit", limit.to_string()));
        }
        let path = format!("/channels/{channel_id}/threads");
        let response = self.call(Method::Get, path, query, None).await?;
        read_json(&response)
    }

    // Pins.

    /// The pinned messages of a channel as they are now, in the order in which they were
    /// pinned.
    pub async fn pins(&self, channel_id: ChannelId) -> Result<Vec<Message>, ApiError> {
        let pins: Vec<Message> = self.get(format!("/channels/{channel_id}/pins")).await?;
        all_bounded(&pins)?;
        Ok(pins)
    }

    /// Pins a message of a channel. The caller needs "manage messages". Pinning a pinned
    /// message changes nothing.
    pub async fn pin_message(&self, message_id: MessageId) -> Result<(), ApiError> {
        self.act(Method::Put, format!("/messages/{message_id}/pin"))
            .await
    }

    pub async fn unpin_message(&self, message_id: MessageId) -> Result<(), ApiError> {
        self.act(Method::Delete, format!("/messages/{message_id}/pin"))
            .await
    }
}
