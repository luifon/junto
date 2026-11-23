//! The default spoken language of the recordings of a server (ADR 0604), as the page of
//! the server reads and sets it.

use std::collections::HashMap;

use gpui::Context;
use junto_client_core::protocol::ids::ServerId;
use junto_client_core::protocol::recording::RecordingSettings;

use super::AppState;

/// What the window knows of the default language of the recordings of each server.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecordingLanguages {
    /// The language as the server stated it last.
    pub language: HashMap<ServerId, String>,
    /// A change is on its way.
    pub sending: HashMap<ServerId, String>,
    /// Why the last read or change failed.
    pub failed: HashMap<ServerId, String>,
}

impl AppState {
    /// Reads the default language of the recordings of a server.
    pub fn read_recording_language(&mut self, server_id: ServerId, cx: &mut Context<Self>) {
        let Some(server) = self.client.server(server_id) else {
            return;
        };
        let api = server.api();
        cx.spawn(async move |this, cx| {
            let read = api.recording_settings().await;
            let _ = this.update(cx, |this, cx| {
                match read {
                    Ok(settings) => {
                        this.recording_languages.failed.remove(&server_id);
                        this.recording_languages
                            .language
                            .insert(server_id, settings.language);
                    }
                    Err(error) => {
                        this.recording_languages.failed.insert(
                            server_id,
                            crate::text::flow_error(&junto_client_core::flow::FlowError::from(
                                &error,
                            )),
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Sets the default language of the recordings of a server. The server decides whether
    /// the account may.
    pub fn set_recording_language(
        &mut self,
        server_id: ServerId,
        language: String,
        cx: &mut Context<Self>,
    ) {
        let Some(server) = self.client.server(server_id) else {
            return;
        };
        let api = server.api();
        self.recording_languages
            .sending
            .insert(server_id, language.clone());
        self.recording_languages.failed.remove(&server_id);
        cx.spawn(async move |this, cx| {
            let set = api
                .set_recording_settings(&RecordingSettings { language })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.recording_languages.sending.remove(&server_id);
                match set {
                    Ok(settings) => {
                        this.recording_languages
                            .language
                            .insert(server_id, settings.language);
                    }
                    Err(error) => {
                        this.recording_languages.failed.insert(
                            server_id,
                            crate::text::flow_error(&junto_client_core::flow::FlowError::from(
                                &error,
                            )),
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub fn recording_languages(&self) -> &RecordingLanguages {
        &self.recording_languages
    }
}
