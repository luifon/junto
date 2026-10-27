//! The requests of the administration about the server itself: its logo (ADR 0101) and
//! the limits of the files of messages (ADR 0408).

use std::path::PathBuf;

use gpui::Context;
use junto_client_core::profile;
use junto_client_core::protocol::ids::ServerId;
use junto_client_core::protocol::message::AttachmentLimits;

use super::{Answered, AskKind};
use crate::state::AppState;
use crate::text::admin::server as txt;

impl AppState {
    /// Opens the cut of a file for the logo of the server (ADR 0119).
    pub fn set_server_logo(&mut self, server_id: ServerId, file: PathBuf, cx: &mut Context<Self>) {
        self.choose_picture(
            crate::state::picture_crop::PicturePurpose::Logo(server_id),
            file,
            cx,
        );
    }

    /// A chosen file that is not a picture, told at the logo control of the page.
    pub(in crate::state) fn server_logo_refused(&mut self, reason: &'static str) {
        let stamp = self.admin.begin(AskKind::SetLogo);
        self.admin.settle(stamp, Some(reason.to_owned()));
    }

    /// Sends a cut picture as the logo of the server. Its size and its first bytes are
    /// checked before a request. The image itself arrives with the event that states the
    /// new logo.
    pub(in crate::state) fn send_server_logo(
        &mut self,
        server_id: ServerId,
        bytes: Vec<u8>,
        cx: &mut Context<Self>,
    ) {
        let what = AskKind::SetLogo;
        let stamp = self.admin.begin(what);
        let checked = match profile::check_avatar(&bytes) {
            Ok(_) => Ok(bytes),
            Err(invalid) => Err(txt::invalid_logo(invalid)),
        };
        cx.spawn(async move |this, cx| {
            let read = checked;
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
