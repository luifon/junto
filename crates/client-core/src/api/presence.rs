use junto_protocol::presence::{PresencePage, ReadPresence, SetPresence, SetStatusText};

use super::{ApiError, Method, ServerApi};

impl ServerApi {
    pub async fn presence(&self, body: &ReadPresence) -> Result<PresencePage, ApiError> {
        self.send(Method::Post, "/presence".to_owned(), body).await
    }

    pub async fn set_presence(&self, body: &SetPresence) -> Result<SetPresence, ApiError> {
        self.send(Method::Put, "/account/presence".to_owned(), body)
            .await
    }

    /// Sets or removes the status text of the account (ADR 0401).
    pub async fn set_status_text(&self, body: &SetStatusText) -> Result<SetStatusText, ApiError> {
        self.send(Method::Put, "/account/status-text".to_owned(), body)
            .await
    }
}
