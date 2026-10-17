//! The private moderation notices of an account (ADR 0120) as the window states them: a
//! person who was removed from a workspace reads the notice of that removal at the top of
//! the main area until the person puts it away, and a person who was removed from the
//! server or banned reads the notice of that action beside the state of the server.

use gpui::Context;
use junto_client_core::flow::FlowError;
use junto_client_core::flow::access::Notice;
use junto_client_core::protocol::account::{ModerationNotice, ModerationNoticeKind};
use junto_client_core::protocol::ids::ServerId;
use uuid::Uuid;

use super::AppState;
use super::view::{Known, ServerSnapshot, ServerStatus, shown_model, status};
use crate::text;

/// The notices that a device read with a proof of its key, for an account that has no
/// session: a removed or banned account (ADR 0120).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModerationRead {
    /// The read with this number is on its way.
    Reading(u64),
    Read(Vec<ModerationNotice>),
    Failed,
}

/// One notice as the window shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModerationLine {
    pub id: Uuid,
    pub kind: ModerationNoticeKind,
    /// The name of the workspace or of the server at the time of the action.
    pub name: String,
    /// The day of the action, in the time zone of the person.
    pub day: String,
    /// The message that the moderator wrote for the person.
    pub message: Option<String>,
    /// The person puts the notice away: a notice of the removal from a workspace. The
    /// notice of a removal from the server and of a ban stays with the state of the
    /// server.
    pub dismissable: bool,
}

/// Text of another person on one line: without control characters.
fn plain(text: &str) -> String {
    text.chars()
        .filter(|character| !character.is_control())
        .collect::<String>()
        .trim()
        .to_owned()
}

fn line(notice: &ModerationNotice, dismissable: bool, known: &Known) -> ModerationLine {
    ModerationLine {
        id: notice.id,
        kind: notice.kind,
        name: plain(&notice.name),
        day: text::day(known.zone.local(notice.at).date(), known.today),
        message: notice
            .message
            .as_deref()
            .map(plain)
            .filter(|message| !message.is_empty()),
        dismissable,
    }
}

/// The notices that the window shows for a server: the removals from workspaces that the
/// person has not put away, for an account with a session; the newest notice of the
/// removal from the server or of the ban, for an account without one.
pub fn lines(server: &ServerSnapshot, known: &Known) -> Vec<ModerationLine> {
    if let Some(model) = shown_model(server) {
        let shown = known.moderation_shown.get(&server.id);
        return model
            .moderation_notices
            .iter()
            .filter(|notice| notice.kind == ModerationNoticeKind::WorkspaceRemoved)
            .filter(|notice| shown.is_none_or(|shown| !shown.contains(&notice.id)))
            .map(|notice| line(notice, true, known))
            .collect();
    }
    let wanted = match status(server) {
        ServerStatus::NoAccount(Some(Notice::Removed)) => ModerationNoticeKind::ServerRemoved,
        ServerStatus::Banned => ModerationNoticeKind::Banned,
        _ => return Vec::new(),
    };
    match known.moderation_read.get(&server.id) {
        Some(ModerationRead::Read(notices)) => notices
            .iter()
            .filter(|notice| notice.kind == wanted)
            .max_by_key(|notice| notice.at)
            .map(|notice| line(notice, false, known))
            .into_iter()
            .collect(),
        _ => Vec::new(),
    }
}

/// True for a state of a server whose notices are read with a proof of the key.
fn without_session(server: &ServerSnapshot) -> bool {
    matches!(
        status(server),
        ServerStatus::NoAccount(Some(Notice::Removed)) | ServerStatus::Banned
    )
}

impl AppState {
    /// Reads the notices of each server whose account was removed or banned, once for
    /// each time that it comes into that state, and takes the notices that the person put
    /// away from the server book.
    pub(super) fn follow_moderation(&mut self, servers: &[ServerSnapshot], cx: &mut Context<Self>) {
        self.known.moderation_read.retain(|server_id, _| {
            servers
                .iter()
                .any(|server| server.id == *server_id && without_session(server))
        });
        for server in servers.iter().filter(|server| without_session(server)) {
            if self.known.moderation_read.contains_key(&server.id) {
                continue;
            }
            let Some(handle) = self.client.server(server.id) else {
                continue;
            };
            let server_id = server.id;
            self.moderation_asks += 1;
            let number = self.moderation_asks;
            self.known
                .moderation_read
                .insert(server_id, ModerationRead::Reading(number));
            let reading = handle.moderation_notices();
            cx.spawn(async move |this, cx| {
                let read = reading.await;
                let _ = this.update(cx, |this, cx| {
                    // The answer belongs to the read that is the last one of that state.
                    if let Some(held) = this
                        .known
                        .moderation_read
                        .get_mut(&server_id)
                        .filter(|held| **held == ModerationRead::Reading(number))
                    {
                        *held = match read {
                            Ok(page) => ModerationRead::Read(page.notices),
                            Err(_) => ModerationRead::Failed,
                        };
                    }
                    this.changed(cx);
                });
            })
            .detach();
        }
        for server in servers
            .iter()
            .filter(|server| shown_model(server).is_some())
        {
            if let Some(handle) = self.client.server(server.id) {
                let shown = handle.moderation_notices_shown();
                if self.known.moderation_shown.get(&server.id) != Some(&shown) {
                    self.known.moderation_shown.insert(server.id, shown);
                }
            }
        }
    }

    /// The person put the notice of a removal from a workspace away. The server book keeps
    /// that on this device.
    pub fn put_moderation_notice_away(
        &mut self,
        server_id: ServerId,
        notice_id: Uuid,
        cx: &mut Context<Self>,
    ) {
        let Some(server) = self.client.server(server_id) else {
            return;
        };
        // The book is a file of the data directory: it is written in the queue of the
        // writes, and the notice goes when the write is done (ADR 0730).
        Self::queue_write(
            cx,
            move || {
                server
                    .put_moderation_notice_away(notice_id)
                    .map(|()| server.moderation_notices_shown())
            },
            move |this, written, cx| {
                match written {
                    Ok(shown) => {
                        this.known.moderation_away_failed = None;
                        this.known.moderation_shown.insert(server_id, shown);
                    }
                    Err(error) => {
                        this.known.moderation_away_failed =
                            Some((server_id, text::flow_error(&FlowError::from(error))));
                    }
                }
                this.changed(cx);
            },
        );
    }
}
