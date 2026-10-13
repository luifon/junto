//! The page of the recordings of a workspace (ADR 0784): the recordings that the server
//! gives the account, the newest first, as the page has read them.

use std::time::Duration;

use junto_client_core::model::{ServerModel, WorkspaceModel};
use junto_client_core::protocol::ids::{RecordingId, ServerId, WorkspaceId};
use junto_client_core::protocol::recording::{Recording, RecordingFailure, RecordingState};
use junto_client_core::protocol::transcript::TranscriptState;
use time::OffsetDateTime;

use super::records::Names;
use crate::state::admin::AdminState;
use crate::state::view::Known;

/// The number of recordings that one read asks for.
pub const RECORDINGS_PAGE: u32 = 50;
/// The most recordings that the page keeps.
pub const MOST_RECORDINGS: usize = 2000;

/// Where the read of the list stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordingsRead {
    /// The page has not asked for the list of this workspace.
    NotRead,
    /// A read is on its way: the first page, or the page after the loaded recordings.
    Reading,
    Loaded,
    /// The last read failed. The loaded recordings stay.
    Failed {
        sentence: String,
        /// The read asked for the page after the loaded recordings.
        older: bool,
    },
}

/// One recording of the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordingLine {
    pub id: RecordingId,
    /// The channel as a title names it. Absent: the state does not hold the channel.
    pub channel: Option<String>,
    /// When the recorder started, in the time zone of the person. For a recording whose
    /// recorder has not started: when it was asked for.
    pub since: OffsetDateTime,
    /// The recorder started: `since` is the start of the recording.
    pub started: bool,
    /// The time from the start to the end. Absent while the recording runs, and for a
    /// recording whose recorder never started.
    pub length: Option<Duration>,
    pub state: RecordingState,
    /// Why nothing was saved, for the state `failed`.
    pub failure: Option<RecordingFailure>,
    /// The files that failed, and all files of the recording.
    pub failed_segments: u32,
    pub segments: u32,
    /// The name of the account that started it. Absent: the state does not hold it.
    pub started_by: Option<String>,
    pub transcript: TranscriptState,
    pub transcript_lines: u32,
    /// The bytes of the files. Absent until the server has processed them.
    pub bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordingsPage {
    pub workspace_id: WorkspaceId,
    pub workspace_name: String,
    pub state: RecordingsRead,
    /// The loaded recordings, the newest first.
    pub rows: Vec<RecordingLine>,
    /// The server holds older recordings, and the page reads them on request.
    pub older: bool,
    /// The page holds the most recordings that it keeps. It reads no older one.
    pub at_bound: bool,
}

fn line(recording: &Recording, names: &Names<'_>) -> RecordingLine {
    let since = recording.recording_since;
    let length = since
        .zip(recording.ended_at)
        .and_then(|(since, ended)| Duration::try_from(ended - since).ok());
    RecordingLine {
        id: recording.id,
        channel: names.channel(recording.channel_id),
        since: names.local(since.unwrap_or(recording.requested_at)),
        started: since.is_some(),
        length,
        state: recording.state,
        failure: recording.failure,
        failed_segments: recording.failed_segments,
        segments: recording.segments,
        started_by: names.account(recording.started_by),
        transcript: recording.transcript,
        transcript_lines: recording.transcript_lines,
        bytes: recording.bytes,
    }
}

/// The page of the recordings of the shown workspace.
pub fn recordings(
    server_id: ServerId,
    model: &ServerModel,
    workspace: &WorkspaceModel,
    known: &Known,
    admin: &AdminState,
) -> RecordingsPage {
    let workspace_id = workspace.workspace.id;
    let list = &admin.recordings;
    // The loaded recordings belong to the workspace and to the account that they were
    // read for. Another workspace, another server and another account show none of them.
    let read =
        list.shown == Some((server_id, workspace_id)) && list.account == Some(model.account.id);
    let names = Names::new(server_id, model, known);
    let rows: Vec<RecordingLine> = if read {
        list.recordings
            .iter()
            .filter(|recording| recording.workspace_id == workspace_id)
            .map(|recording| line(recording, &names))
            .collect()
    } else {
        Vec::new()
    };
    let at_bound = read && list.recordings.len() >= MOST_RECORDINGS;
    let state = match (read, &list.failed) {
        (false, _) => RecordingsRead::NotRead,
        (true, Some(failed)) => RecordingsRead::Failed {
            sentence: failed.sentence.clone(),
            older: failed.older,
        },
        (true, None) if list.reading => RecordingsRead::Reading,
        (true, None) => RecordingsRead::Loaded,
    };
    RecordingsPage {
        workspace_id,
        workspace_name: workspace.workspace.name.clone(),
        state,
        rows,
        older: read && list.next_before.is_some() && !at_bound,
        at_bound,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use junto_client_core::model::{AdminModel, Collection};
    use junto_client_core::protocol::account::OwnAccount;
    use junto_client_core::protocol::events::Cursor;
    use junto_client_core::protocol::ids::{AccountId, CallId, ChannelId, StreamEpoch};
    use junto_client_core::protocol::recording::{RecordingStop, RecordingStopReason};
    use junto_client_core::protocol::snapshot::{Snapshot, WorkspaceState};
    use junto_client_core::protocol::structure::{
        ArchivedChannel, Channel, ChannelKind, Workspace, WorkspaceMember,
    };
    use time::UtcOffset;

    use super::*;
    use crate::state::admin::recordings::{RecordingList, RecordingsFailure};
    use crate::state::feed::Zone;

    fn time() -> OffsetDateTime {
        // 15 January 2027, 08:00 UTC.
        OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap()
    }

    fn known() -> Known {
        Known {
            zone: Zone::Fixed(UtcOffset::from_hms(2, 0, 0).unwrap()),
            ..Known::default()
        }
    }

    /// One server with the workspaces "Studio" and "Lab". "Studio" has the voice channels
    /// "Daily" and "Retro". The account "Luca" is a member of both.
    struct Scene {
        server_id: ServerId,
        own: AccountId,
        ana: AccountId,
        studio: WorkspaceId,
        lab: WorkspaceId,
        daily: ChannelId,
        retro: Channel,
        snapshot: Snapshot,
        admin: AdminModel,
    }

    fn voice(workspace_id: WorkspaceId, name: &str) -> Channel {
        Channel {
            id: ChannelId::generate(),
            workspace_id,
            category_id: None,
            kind: ChannelKind::Voice,
            name: name.to_owned(),
            position: 0,
            permanent_messages: false,
            user_limit: None,
            hides_occupants: false,
        }
    }

    fn scene() -> Scene {
        let (own, ana) = (AccountId::generate(), AccountId::generate());
        let (studio, lab) = (WorkspaceId::generate(), WorkspaceId::generate());
        let (daily, retro) = (voice(studio, "Daily"), voice(studio, "Retro"));
        let workspace = |id, name: &str, is_default, channels| WorkspaceState {
            workspace: Workspace {
                id,
                name: name.to_owned(),
                is_default,
            },
            own_role_ids: Vec::new(),
            roles: Vec::new(),
            categories: Vec::new(),
            channels,
            calls: Vec::new(),
            permissions: Vec::new(),
            channel_permissions: Vec::new(),
            notification_default: None,
        };
        Scene {
            server_id: ServerId::generate(),
            own,
            ana,
            studio,
            lab,
            daily: daily.id,
            retro: retro.clone(),
            snapshot: Snapshot {
                cursor: Cursor {
                    epoch: StreamEpoch::generate(),
                    position: 1,
                },
                account: OwnAccount {
                    id: own,
                    name: "Luca".to_owned(),
                    avatar: None,
                    is_owner: false,
                    server_role_ids: Vec::new(),
                    server_permissions: Vec::new(),
                    workspace_ids: vec![studio, lab],
                },
                workspaces: vec![
                    workspace(studio, "Studio", true, vec![daily, retro]),
                    workspace(lab, "Lab", false, Vec::new()),
                ],
                own_call: None,
                direct_calls: Vec::new(),
                call_invitations: Vec::new(),
                sent_call_invitations: Vec::new(),
                call_moves: Vec::new(),
                entry_requests: Vec::new(),
                direct_mutes: Vec::new(),
                messages: Vec::new(),
                threads: Vec::new(),
                threads_more: false,
                threads_next: None,
                notifications: Vec::new(),
                notification_settings: Vec::new(),
            },
            admin: AdminModel::default(),
        }
    }

    impl Scene {
        fn model(&self) -> ServerModel {
            let mut model =
                ServerModel::from_snapshot(crate::state::fixtures::stated(self.snapshot.clone()));
            model.admin = Arc::new(self.admin.clone());
            model
        }

        fn recording(&self, channel_id: ChannelId, state: RecordingState) -> Recording {
            Recording {
                id: RecordingId::generate(),
                workspace_id: self.studio,
                channel_id,
                call_id: CallId::generate(),
                state,
                started_by: self.own,
                requested_at: time(),
                recording_since: Some(time() + time::Duration::seconds(4)),
                stop: None,
                ended_at: None,
                failure: None,
                language: "auto".to_owned(),
                segments: 3,
                failed_segments: 0,
                bytes: None,
                timeline_started_at: None,
                transcript: TranscriptState::None,
                transcript_lines: 0,
            }
        }

        fn list(&self, recordings: Vec<Recording>) -> AdminState {
            AdminState {
                recordings: RecordingList {
                    shown: Some((self.server_id, self.studio)),
                    account: Some(self.own),
                    recordings,
                    ..RecordingList::default()
                },
                ..AdminState::default()
            }
        }

        fn page(&self, admin: &AdminState) -> RecordingsPage {
            self.page_of(admin, self.studio)
        }

        fn page_of(&self, admin: &AdminState, workspace_id: WorkspaceId) -> RecordingsPage {
            let model = self.model();
            let workspace = model.workspace(workspace_id).unwrap();
            recordings(self.server_id, &model, workspace, &known(), admin)
        }
    }

    #[test]
    fn a_page_that_was_not_read_shows_nothing_and_says_so() {
        let scene = scene();
        let page = scene.page(&AdminState::default());
        assert_eq!(page.state, RecordingsRead::NotRead);
        assert_eq!(page.workspace_id, scene.studio);
        assert_eq!(page.workspace_name, "Studio");
        assert!(page.rows.is_empty() && !page.older && !page.at_bound);

        let mut admin = scene.list(Vec::new());
        admin.recordings.reading = true;
        assert_eq!(scene.page(&admin).state, RecordingsRead::Reading);
        admin.recordings.reading = false;
        // An empty list that was read is loaded and has no row.
        let page = scene.page(&admin);
        assert_eq!(page.state, RecordingsRead::Loaded);
        assert!(page.rows.is_empty() && !page.older);
    }

    #[test]
    fn a_row_shows_the_channel_the_start_the_length_the_state_and_who_started_it() {
        let scene = scene();
        let mut ready = scene.recording(scene.daily, RecordingState::Ready);
        ready.ended_at = Some(time() + time::Duration::seconds(4 + 36 * 60 + 5));
        ready.stop = Some(RecordingStop {
            reason: RecordingStopReason::Stopped,
            by: Some(scene.own),
            at: time() + time::Duration::minutes(36),
        });
        ready.bytes = Some(48_200_000);
        ready.transcript = TranscriptState::Ready;
        ready.transcript_lines = 212;
        let page = scene.page(&scene.list(vec![ready.clone()]));
        assert_eq!(page.state, RecordingsRead::Loaded);
        let row = &page.rows[0];
        assert_eq!(row.id, ready.id);
        assert_eq!(row.channel.as_deref(), Some("Daily"));
        // The start is the moment the recorder started, in the time zone of the person.
        assert!(row.started);
        assert_eq!(
            row.since,
            (time() + time::Duration::seconds(4)).to_offset(UtcOffset::from_hms(2, 0, 0).unwrap())
        );
        assert_eq!(row.since.hour(), 10);
        assert_eq!(row.length, Some(Duration::from_secs(36 * 60 + 5)));
        assert_eq!(row.state, RecordingState::Ready);
        assert_eq!(row.started_by.as_deref(), Some("Luca"));
        assert_eq!(
            (row.transcript, row.transcript_lines),
            (TranscriptState::Ready, 212)
        );
        assert_eq!(row.bytes, Some(48_200_000));
        assert_eq!((row.failed_segments, row.segments), (0, 3));
    }

    #[test]
    fn a_recording_that_runs_has_no_length_and_one_that_never_started_shows_its_request() {
        let scene = scene();
        let running = scene.recording(scene.daily, RecordingState::Recording);
        let mut starting = scene.recording(scene.daily, RecordingState::Starting);
        starting.recording_since = None;
        let mut failed = scene.recording(scene.daily, RecordingState::Failed);
        failed.recording_since = None;
        failed.ended_at = Some(time() + time::Duration::seconds(30));
        failed.failure = Some(RecordingFailure::RecorderUnavailable);
        failed.segments = 0;
        // An end before the start gives no length.
        let mut backwards = scene.recording(scene.daily, RecordingState::Ready);
        backwards.ended_at = Some(time());
        let page = scene.page(&scene.list(vec![running, starting, failed, backwards]));
        let [running, starting, failed, backwards] = &page.rows[..] else {
            panic!("four rows");
        };
        assert!(running.started && running.length.is_none());
        assert!(!starting.started && starting.length.is_none());
        assert_eq!(starting.since, known().zone.local(time()));
        assert!(!failed.started && failed.length.is_none());
        assert_eq!(failed.failure, Some(RecordingFailure::RecorderUnavailable));
        assert_eq!(backwards.length, None);
    }

    #[test]
    fn a_channel_and_a_person_that_the_state_does_not_hold_have_no_name() {
        let mut scene = scene();
        let mut gone = scene.recording(ChannelId::generate(), RecordingState::Ready);
        gone.started_by = scene.ana;
        let page = scene.page(&scene.list(vec![gone.clone()]));
        assert_eq!(page.rows[0].channel, None);
        assert_eq!(page.rows[0].started_by, None);

        // The name of a member is known from the loaded members of a workspace.
        let mut model = scene.model();
        Arc::make_mut(&mut model.workspaces[0]).members =
            junto_client_core::model::Loadable::Loaded(vec![WorkspaceMember {
                account_id: scene.ana,
                name: "Ana".to_owned(),
                role_ids: Vec::new(),
            }]);
        let admin = scene.list(vec![gone.clone()]);
        let workspace = model.workspace(scene.studio).unwrap();
        let page = recordings(scene.server_id, &model, workspace, &known(), &admin);
        assert_eq!(page.rows[0].started_by.as_deref(), Some("Ana"));

        // An archived channel is named while the state holds the archived list.
        let retro = scene.retro.clone();
        scene.snapshot.workspaces[0]
            .channels
            .retain(|channel| channel.id != retro.id);
        let of_retro = scene.recording(retro.id, RecordingState::Ready);
        let page = scene.page(&scene.list(vec![of_retro.clone()]));
        assert_eq!(page.rows[0].channel, None);
        scene.admin.archived = vec![(
            scene.studio,
            Collection::Loaded(vec![ArchivedChannel {
                channel: retro,
                archived_at: time(),
                deletes_at: None,
            }]),
        )];
        let page = scene.page(&scene.list(vec![of_retro]));
        assert_eq!(page.rows[0].channel.as_deref(), Some("Retro"));
    }

    #[test]
    fn the_rows_keep_the_order_of_the_server_and_the_page_offers_older_ones() {
        let scene = scene();
        let recordings: Vec<Recording> = (0..3)
            .map(|_| scene.recording(scene.daily, RecordingState::Ready))
            .collect();
        let ids: Vec<RecordingId> = recordings.iter().map(|recording| recording.id).collect();
        let mut admin = scene.list(recordings);
        let page = scene.page(&admin);
        assert_eq!(page.rows.iter().map(|row| row.id).collect::<Vec<_>>(), ids);
        assert!(!page.older && !page.at_bound);
        admin.recordings.next_before = ids.last().copied();
        assert!(scene.page(&admin).older);
        // While the older page is read, the loaded rows stay.
        admin.recordings.reading = true;
        let page = scene.page(&admin);
        assert_eq!(page.state, RecordingsRead::Reading);
        assert_eq!(page.rows.len(), 3);
    }

    #[test]
    fn a_read_that_failed_keeps_the_loaded_rows_and_states_its_sentence() {
        let scene = scene();
        let mut admin = scene.list(vec![scene.recording(scene.daily, RecordingState::Ready)]);
        admin.recordings.failed = Some(RecordingsFailure {
            sentence: "No.".to_owned(),
            older: true,
        });
        let page = scene.page(&admin);
        assert_eq!(
            page.state,
            RecordingsRead::Failed {
                sentence: "No.".to_owned(),
                older: true
            }
        );
        assert_eq!(page.rows.len(), 1);
        // A failure of the first read has no row.
        let mut admin = scene.list(Vec::new());
        admin.recordings.failed = Some(RecordingsFailure {
            sentence: "No.".to_owned(),
            older: false,
        });
        admin.recordings.reading = true;
        let page = scene.page(&admin);
        assert!(matches!(
            page.state,
            RecordingsRead::Failed { older: false, .. }
        ));
        assert!(page.rows.is_empty());
    }

    #[test]
    fn the_page_holds_at_most_2000_rows_and_then_offers_no_older_one() {
        let scene = scene();
        let one = scene.recording(scene.daily, RecordingState::Ready);
        let recordings: Vec<Recording> = (0..MOST_RECORDINGS)
            .map(|_| Recording {
                id: RecordingId::generate(),
                ..one.clone()
            })
            .collect();
        let mut admin = scene.list(recordings);
        admin.recordings.next_before = Some(RecordingId::generate());
        let page = scene.page(&admin);
        assert_eq!(page.rows.len(), 2000);
        assert!(page.at_bound && !page.older);
        assert_eq!(MOST_RECORDINGS, 2000);
        assert_eq!(RECORDINGS_PAGE, 50);
        // One row less: the page offers the older ones.
        admin.recordings.recordings.pop();
        let page = scene.page(&admin);
        assert!(!page.at_bound && page.older);
    }

    #[test]
    fn the_list_of_another_workspace_server_or_account_is_not_shown() {
        let mut scene = scene();
        let admin = scene.list(vec![scene.recording(scene.daily, RecordingState::Ready)]);
        assert_eq!(scene.page(&admin).rows.len(), 1);

        // The page shows another workspace than the one that was read.
        let page = scene.page_of(&admin, scene.lab);
        assert_eq!(page.state, RecordingsRead::NotRead);
        assert!(page.rows.is_empty() && !page.older && !page.at_bound);
        assert_eq!(page.workspace_name, "Lab");

        // The list was read on another server.
        let mut elsewhere = scene.list(vec![scene.recording(scene.daily, RecordingState::Ready)]);
        elsewhere.recordings.shown = Some((ServerId::generate(), scene.studio));
        let page = scene.page(&elsewhere);
        assert_eq!(page.state, RecordingsRead::NotRead);
        assert!(page.rows.is_empty());

        // The list was read for another account of the same server.
        let mut other_account =
            scene.list(vec![scene.recording(scene.daily, RecordingState::Ready)]);
        other_account.recordings.account = Some(scene.ana);
        other_account.recordings.next_before = Some(RecordingId::generate());
        let page = scene.page(&other_account);
        assert_eq!(page.state, RecordingsRead::NotRead);
        assert!(page.rows.is_empty() && !page.older);

        // A recording that names another workspace than the one that was asked for is
        // not shown: the answer of a server is not trusted to hold only what was asked.
        let mut foreign = scene.recording(scene.daily, RecordingState::Ready);
        foreign.workspace_id = scene.lab;
        let admin = scene.list(vec![
            foreign,
            scene.recording(scene.daily, RecordingState::Ready),
        ]);
        assert_eq!(scene.page(&admin).rows.len(), 1);
        scene.snapshot.account.id = scene.ana;
        assert!(scene.page(&admin).rows.is_empty());
    }
}
