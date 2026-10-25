//! The page of the recordings of a workspace (ADR 0784): the recordings that the server
//! gives the account, the newest first, as the page has read them.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use junto_client_core::model::{ServerModel, WorkspaceModel};
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::ids::{AccountId, RecordingId, ServerId, WorkspaceId};
use junto_client_core::protocol::recording::{Recording, RecordingFailure, RecordingState};
use junto_client_core::protocol::transcript::TranscriptState;
use time::OffsetDateTime;

use super::records::Names;
use crate::state::admin::AdminState;
use crate::state::recorded::{self, RecordingCard};
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
    /// The row offers its files and its transcript for download.
    pub downloads: bool,
    /// The files of the recording, its transcript as text and their downloads, as the
    /// card of the recording states them (ADR 0744), while the row lists its files.
    pub files: Option<RecordingCard>,
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
        downloads: false,
        files: None,
    }
}

/// True when the page offers the files of a recording: the server has saved them, the
/// state names its channel, and the permissions of the account in that channel, when the
/// state holds them, include reading it. The list holds only recordings that the server
/// lets the account read, by the rule of the routes of the files (ADR 0604), so a channel
/// whose permissions the state does not hold, such as an archived one, takes the answer
/// of the list.
fn offers_downloads(recording: &Recording, model: &ServerModel) -> bool {
    let saved = matches!(
        recording.state,
        RecordingState::Ready | RecordingState::PartlyFailed
    );
    let readable = match model.permissions_in(recording.channel_id) {
        Some(held) => held.contains(&WorkspacePermission::ViewChannel),
        None => model.readable_channel(recording.channel_id).is_some(),
    };
    saved && readable
}

/// The files of a recording as its card states them, from the recording that the client
/// core holds open for the page.
fn files(
    server_id: ServerId,
    recording: &Recording,
    model: &ServerModel,
    known: &Known,
    accounts: &HashMap<AccountId, String>,
) -> RecordingCard {
    let channel = model
        .readable_channel(recording.channel_id)
        .map_or("", |channel| channel.name.as_str());
    recorded::card(&recorded::CardSource {
        server_id,
        recording_id: recording.id,
        open: model.recording(recording.id).map(Arc::as_ref),
        channel,
        channel_id: recording.channel_id,
        names: accounts,
        zone: known.zone,
        today: known.today,
        may_remove: false,
        may_request: false,
        downloads: &known.segment_downloads,
        texts: &known.text_downloads,
        removing: false,
        requesting: false,
        refused: known
            .recording_refused
            .get(&(server_id, recording.id))
            .map(String::as_str),
    })
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
    let accounts = list
        .files_open
        .filter(|_| read)
        .map(|_| crate::state::notices::account_names(server_id, model, known));
    let rows: Vec<RecordingLine> = if read {
        list.recordings
            .iter()
            .filter(|recording| recording.workspace_id == workspace_id)
            .map(|recording| {
                let mut row = line(recording, &names);
                row.downloads = offers_downloads(recording, model);
                if row.downloads
                    && list.files_open == Some(recording.id)
                    && let Some(accounts) = &accounts
                {
                    row.files = Some(files(server_id, recording, model, known, accounts));
                }
                row
            })
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
        (true, None) if list.reading || list.withdrawn.is_some() => RecordingsRead::Reading,
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

    use junto_client_core::model::{
        AdminModel, Collection, Loadable, OpenRecording, TranscriptLines,
    };
    use junto_client_core::protocol::account::OwnAccount;
    use junto_client_core::protocol::events::Cursor;
    use junto_client_core::protocol::ids::{
        AccountId, CallId, ChannelId, RecordingSegmentId, StreamEpoch,
    };
    use junto_client_core::protocol::recording::{
        FileCheck, RecordingDetail, RecordingSegment, RecordingStop, RecordingStopReason,
        SegmentFile, SegmentSource, SegmentState,
    };
    use junto_client_core::protocol::snapshot::{Snapshot, WorkspaceState};
    use junto_client_core::protocol::structure::{
        ArchivedChannel, Channel, ChannelKind, Workspace, WorkspaceMember,
    };
    use junto_client_core::transfers::{DownloadProgress, DownloadStage};
    use time::UtcOffset;

    use super::*;
    use crate::state::admin::recordings::{RecordingList, RecordingsFailure};
    use crate::state::feed::Zone;
    use crate::state::recorded::{CardState, FileKind};

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
            let mut model = ServerModel::from_snapshot(crate::state::fixtures::from_roles(
                self.snapshot.clone(),
            ));
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
    fn a_recording_at_the_last_moment_of_the_range_of_dates_is_shown() {
        use time::format_description::well_known::Rfc3339;

        // The zone of the page is two hours east of UTC, where that moment lies in the
        // year 10000: the row shows the moment as the server wrote it.
        let scene = scene();
        let last = OffsetDateTime::parse("9999-12-31T23:59:59Z", &Rfc3339).unwrap();
        let mut recording = scene.recording(scene.daily, RecordingState::Starting);
        recording.requested_at = last;
        recording.recording_since = None;
        let page = scene.page(&scene.list(vec![recording]));
        assert_eq!(page.rows[0].since, last);
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
        // A list that a change of permissions emptied is being read until a read answers.
        admin.recordings.withdraw();
        assert_eq!(scene.page(&admin).state, RecordingsRead::Reading);
        admin.recordings.withdrawn = None;
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
            crate::moment::in_offset(
                time() + time::Duration::seconds(4),
                UtcOffset::from_hms(2, 0, 0).unwrap()
            )
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
                avatar: None,
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

    /// The state of the scene in which the account may read the channels of "Studio".
    fn reading(scene: &Scene) -> ServerModel {
        let mut model = scene.model();
        Arc::make_mut(&mut model.workspaces[0]).permissions =
            vec![WorkspacePermission::ViewChannel];
        model
    }

    fn page_in(
        scene: &Scene,
        model: &ServerModel,
        admin: &AdminState,
        known: &Known,
    ) -> RecordingsPage {
        let workspace = model.workspace(scene.studio).unwrap();
        recordings(scene.server_id, model, workspace, known, admin)
    }

    fn offered(scene: &Scene, model: &ServerModel, admin: &AdminState) -> Vec<bool> {
        page_in(scene, model, admin, &known())
            .rows
            .iter()
            .map(|row| row.downloads)
            .collect()
    }

    #[test]
    fn a_row_offers_its_files_when_the_server_saved_them_and_the_account_reads_the_channel() {
        let mut scene = scene();
        let admin = scene.list(vec![
            scene.recording(scene.daily, RecordingState::Ready),
            scene.recording(scene.daily, RecordingState::PartlyFailed),
            scene.recording(scene.daily, RecordingState::Processing),
            scene.recording(scene.daily, RecordingState::Recording),
            scene.recording(scene.daily, RecordingState::Failed),
            // A channel that the state does not hold.
            scene.recording(ChannelId::generate(), RecordingState::Ready),
        ]);
        assert_eq!(
            offered(&scene, &reading(&scene), &admin),
            [true, true, false, false, false, false]
        );
        // The list of the channel does not hold "view channel".
        assert_eq!(offered(&scene, &scene.model(), &admin), [false; 6]);

        // An archived channel whose permissions the state does not hold takes the answer
        // of the list; its permissions decide once the state holds them.
        let retro = scene.retro.clone();
        scene.snapshot.workspaces[0]
            .channels
            .retain(|channel| channel.id != retro.id);
        scene.admin.archived = vec![(
            scene.studio,
            Collection::Loaded(vec![ArchivedChannel {
                channel: retro.clone(),
                archived_at: time(),
                deletes_at: None,
            }]),
        )];
        let admin = scene.list(vec![scene.recording(retro.id, RecordingState::Ready)]);
        assert_eq!(offered(&scene, &reading(&scene), &admin), [true]);
        scene.admin.channel_permissions = vec![(retro.id, Collection::Loaded(Vec::new()))];
        assert_eq!(offered(&scene, &reading(&scene), &admin), [false]);
        scene.admin.channel_permissions = vec![(
            retro.id,
            Collection::Loaded(vec![WorkspacePermission::ViewChannel]),
        )];
        assert_eq!(offered(&scene, &reading(&scene), &admin), [true]);
    }

    #[test]
    fn the_open_row_states_the_files_and_the_downloads_of_its_recording() {
        let scene = scene();
        let ready = scene.recording(scene.daily, RecordingState::Ready);
        let other = scene.recording(scene.daily, RecordingState::Ready);
        let processing = scene.recording(scene.daily, RecordingState::Processing);
        let mut admin = scene.list(vec![ready.clone(), other, processing.clone()]);
        let mut model = reading(&scene);
        let mut known = known();
        let page = page_in(&scene, &model, &admin, &known);
        assert!(page.rows.iter().all(|row| row.files.is_none()));

        // The row lists its files: the client core reads the recording first.
        admin.recordings.files_open = Some(ready.id);
        let page = page_in(&scene, &model, &admin, &known);
        let card = page.rows[0].files.clone().unwrap();
        assert_eq!(
            (card.server_id, card.recording_id),
            (scene.server_id, ready.id)
        );
        assert_eq!(card.state, CardState::Reading);
        assert!(page.rows[1..].iter().all(|row| row.files.is_none()));

        // The recording is held open with a video, a microphone and a segment without a
        // file.
        let segment = |source, account_id, saved: bool| RecordingSegment {
            id: RecordingSegmentId::generate(),
            source,
            account_id,
            state: if saved {
                SegmentState::Ended
            } else {
                SegmentState::Failed
            },
            failure: None,
            requested_at: time(),
            started_at: Some(time()),
            ended_at: None,
            duration_ms: None,
            file: saved.then(|| SegmentFile {
                media_type: "audio/ogg".to_owned(),
                bytes: 9_400,
                duration_ms: 2_000,
                starts_at: None,
                width: None,
                height: None,
                check: FileCheck::Passed,
                problems: Vec::new(),
                silent: false,
                recovered: false,
            }),
        };
        let segments = vec![
            segment(SegmentSource::Composite, None, true),
            segment(SegmentSource::Microphone, Some(scene.own), true),
            segment(SegmentSource::Microphone, Some(scene.ana), false),
        ];
        let microphone = segments[1].id;
        model.recordings = vec![Arc::new(OpenRecording {
            id: ready.id,
            detail: Loadable::Loaded(RecordingDetail {
                recording: ready.clone(),
                segments,
            }),
            transcript: Loadable::NotLoaded,
            lines: TranscriptLines::default(),
            failed: None,
            gone: false,
            stale: false,
        })];
        let receiving = DownloadProgress {
            stage: DownloadStage::Receiving,
            received: 4_000,
            bytes: 9_400,
            interrupted: false,
        };
        known
            .segment_downloads
            .insert((scene.server_id, microphone), receiving.clone());
        known
            .text_downloads
            .insert((scene.server_id, ready.id), receiving.clone());
        known
            .recording_refused
            .insert((scene.server_id, ready.id), "No folder.".to_owned());
        let page = page_in(&scene, &model, &admin, &known);
        let card = page.rows[0].files.clone().unwrap();
        assert_eq!(card.refused.as_deref(), Some("No folder."));
        let CardState::Shown(detail) = card.state else {
            panic!("the files are shown");
        };
        let files: Vec<(FileKind, Option<&str>, &str)> = detail
            .files
            .iter()
            .map(|file| (file.kind, file.person.as_deref(), file.stem.as_str()))
            .collect();
        // The files are named by the channel and the start in the time zone of the person.
        assert_eq!(
            files,
            [
                (FileKind::Video, None, "Daily 2027-01-15 10.00 video"),
                (
                    FileKind::Microphone,
                    Some("Luca"),
                    "Daily 2027-01-15 10.00 microphone Luca"
                ),
            ]
        );
        assert_eq!(detail.files[0].download, None);
        assert_eq!(detail.files[1].download, Some(receiving.clone()));
        assert_eq!(detail.text_stem, "Daily 2027-01-15 10.00 transcript");
        assert_eq!(detail.text_download, Some(receiving));

        // A row that offers no download lists no files, and a recording that the list
        // does not hold opens nothing.
        admin.recordings.files_open = Some(processing.id);
        let page = page_in(&scene, &model, &admin, &known);
        assert!(page.rows.iter().all(|row| row.files.is_none()));
        admin.recordings.files_open = Some(RecordingId::generate());
        let page = page_in(&scene, &model, &admin, &known);
        assert!(page.rows.iter().all(|row| row.files.is_none()));
    }
}
