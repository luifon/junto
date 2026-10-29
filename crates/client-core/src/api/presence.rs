use junto_protocol::presence::{PresencePage, ReadPresence, SetPresence};

use super::{ApiError, Method, ServerApi};

impl ServerApi {
    pub async fn presence(&self, body: &ReadPresence) -> Result<PresencePage, ApiError> {
        self.send(Method::Post, "/presence".to_owned(), body).await
    }

    pub async fn set_presence(&self, body: &SetPresence) -> Result<SetPresence, ApiError> {
        self.send(Method::Put, "/account/presence".to_owned(), body)
            .await
    }
}
