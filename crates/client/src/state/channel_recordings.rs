//! The recordings of one voice channel, for the tab "Recordings" of its Files panel
//! (ADR 0784). The server lists the recordings of a workspace in pages; the list keeps
//! those of the channel.

use gpui::Context;
use junto_client_core::protocol::ids::{AccountId, ChannelId, RecordingId, ServerId, WorkspaceId};
use junto_client_core::protocol::recording::Recording;

use super::AppState;

/// The recordings of a workspace that one read asks for.
const PAGE: u32 = 50;
/// The pages that one read follows to find recordings of the channel.
const PAGES_PER_READ: usize = 6;

/// The list of the recordings of one voice channel, read for one account.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChannelRecordings {
    pub key: Option<(ServerId, AccountId, WorkspaceId, ChannelId)>,
    /// Newest first.
    pub recordings: Vec<Recording>,
    /// The workspace has older recordings than the list has read.
    pub older: Option<RecordingId>,
    pub reading: bool,
    /// The last read failed, with the sentence of its error.
    pub failed: Option<String>,
    read: u64,
}

impl AppState {
    /// Reads the recordings of a voice channel from the newest one.
    pub fn show_channel_recordings(
        &mut self,
        server_id: ServerId,
        workspace_id: WorkspaceId,
        channel_id: ChannelId,
        cx: &mut Context<Self>,
    ) {
        let Some(account_id) = self
            .client
            .server(server_id)
            .and_then(|server| server.account())
        else {
            return;
        };
        self.channel_recordings = ChannelRecordings {
            key: Some((server_id, account_id, workspace_id, channel_id)),
            read: self.channel_recordings.read,
            ..ChannelRecordings::default()
        };
        self.read_channel_recordings(None, cx);
    }

    /// Reads the recordings of the channel that are older than the listed ones.
    pub fn read_older_channel_recordings(&mut self, cx: &mut Context<Self>) {
        let list = &self.channel_recordings;
        if list.reading {
            return;
        }
        if let Some(before) = list.older {
            self.read_channel_recordings(Some(before), cx);
        }
    }

    pub fn channel_recordings(&self) -> &ChannelRecordings {
        &self.channel_recordings
    }

    /// When a recording started, or was asked for, on the clock of the person.
    pub fn recording_time(&self, recording: &Recording) -> String {
        let at = recording.recording_since.unwrap_or(recording.requested_at);
        crate::text::admin::date_time(self.known.zone.local(at), self.hour_cycle())
    }

    fn read_channel_recordings(&mut self, before: Option<RecordingId>, cx: &mut Context<Self>) {
        let Some((server_id, account_id, workspace_id, channel_id)) = self.channel_recordings.key
        else {
            return;
        };
        let Some(server) = self.client.server(server_id) else {
            return;
        };
        let list = &mut self.channel_recordings;
        list.read += 1;
        let number = list.read;
        list.reading = true;
        list.failed = None;
        // A request fails when the device has another account by then.
        let api = server.api_for(account_id);
        let key = list.key;
        cx.spawn(async move |this, cx| {
            let mut found = Vec::new();
            let mut next = before;
            let mut failed = None;
            for page in 0..PAGES_PER_READ {
                if page > 0 && next.is_none() {
                    break;
                }
                match api.recordings(workspace_id, next, Some(PAGE)).await {
                    Ok(read) => {
                        // A server is not trusted to answer with what was asked for.
                        found.extend(read.recordings.into_iter().filter(|recording| {
                            recording.workspace_id == workspace_id
                                && recording.channel_id == channel_id
                        }));
                        next = read.next_before;
                        if next.is_none() || !found.is_empty() {
                            break;
                        }
                    }
                    Err(error) => {
                        failed = Some(crate::text::admin::recordings::read_failure(&error));
                        break;
                    }
                }
            }
            let _ = this.update(cx, |this, cx| {
                let list = &mut this.channel_recordings;
                // A late answer to an earlier read, or to a read of another channel or
                // account, is not the one that the list waits for.
                if list.read != number || list.key != key {
                    return;
                }
                list.reading = false;
                match failed {
                    Some(sentence) => list.failed = Some(sentence),
                    None => {
                        for recording in found {
                            if !list.recordings.iter().any(|held| held.id == recording.id) {
                                list.recordings.push(recording);
                            }
                        }
                        list.older = next;
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}
