//! The requests of the administration about the server itself: its logo (ADR 0101) and
//! the limits of the files of messages (ADR 0408).

use std::path::{Path, PathBuf};

use gpui::Context;
use junto_client_core::profile;
use junto_client_core::protocol::ids::ServerId;
use junto_client_core::protocol::message::AttachmentLimits;

use super::{Answered, AskKind};
use crate::state::AppState;
use crate::text::admin::server as txt;

/// Reads a file that the person chose as the logo. Only a regular file is read, and at
/// most one byte more than the largest logo, whatever the size of the file. A file that
/// no server stores has its reason, and is not sent.
fn read_logo(file: &Path) -> Result<Vec<u8>, &'static str> {
    let bytes = profile::read_avatar_file(file).map_err(|_| txt::LOGO_NOT_READ)?;
    match profile::check_avatar(&bytes) {
        Ok(_) => Ok(bytes),
        Err(invalid) => Err(txt::invalid_logo(invalid)),
    }
}

impl AppState {
    /// Sends the file at `file` as the logo of the server. The file is read on a
    /// background thread, and its size and its first bytes are checked before a request.
    /// The page shows the request as sending from the start of the read. The image
    /// itself arrives with the event that states the new logo.
    pub fn set_server_logo(&mut self, server_id: ServerId, file: PathBuf, cx: &mut Context<Self>) {
        let what = AskKind::SetLogo;
        let stamp = self.admin.begin(what);
        let reading = cx
            .background_executor()
            .spawn(async move { read_logo(&file) });
        cx.spawn(async move |this, cx| {
            let read = reading.await;
            let _ = this.update(cx, |this, cx| {
                // The person left the page or asked for something else meanwhile.
                if this.admin.answered(stamp) != Answered::Current {
                    return;
                }
                // The read ends here. The request that follows is counted by itself.
                let refused = read.as_ref().err().map(|reason| (*reason).to_owned());
                this.admin.settle(stamp, refused);
                if let Ok(image) = read {
                    this.admin_ask(
                        server_id,
                        what,
                        |api| async move { api.set_server_logo(image).await },
                        |_, _, _| {},
                        cx,
                    );
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Replaces the limits of the files of messages. The route takes both limits, so the
    /// limit that the person did not change is sent as the page shows it. The new limits
    /// arrive with their event.
    pub fn set_attachment_limits(
        &mut self,
        server_id: ServerId,
        limits: AttachmentLimits,
        cx: &mut Context<Self>,
    ) {
        self.admin_ask(
            server_id,
            AskKind::AttachmentLimits,
            |api| async move { api.set_attachment_limits(&limits).await },
            |_, _, _| {},
            cx,
        );
    }

    /// Removes the logo of the server. The windows draw the generated picture again when
    /// the event states the removal.
    pub fn remove_server_logo(&mut self, server_id: ServerId, cx: &mut Context<Self>) {
        self.admin_ask(
            server_id,
            AskKind::RemoveLogo,
            |api| async move { api.remove_server_logo().await },
            |_, (), _| {},
            cx,
        );
    }
}
