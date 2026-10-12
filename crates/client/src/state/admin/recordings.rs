//! The reads of the list of the recordings of a workspace (ADR 0784). The list is no
//! collection of the client core: the page asks for pages of it, and reads it again from
//! the start when the server tells a change of a recording of that workspace.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use gpui::{Context, Task};
use junto_client_core::live::Notice;
use junto_client_core::protocol::events::Event;
use junto_client_core::protocol::ids::{AccountId, RecordingId, ServerId, WorkspaceId};
use junto_client_core::protocol::recording::{Recording, RecordingQuery};

use super::view::recordings::{MOST_RECORDINGS, RECORDINGS_PAGE};
use super::{AdminOpen, AdminPage};
use crate::state::AppState;
use crate::text;

/// The number of the last read of a list of recordings. It does not start again when the
/// list is put away with the administration, so a late answer never has the number of a
/// later read.
static RECORDING_READS: AtomicU64 = AtomicU64::new(0);

/// The least time between the start of a read and the start of the next read that a change
/// of the server causes. A server that sends events without end causes at most one request
/// in this time, and never two at once.
pub(crate) const RELOAD_INTERVAL: Duration = Duration::from_secs(1);

/// Why the last read of the list failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordingsFailure {
    pub(crate) sentence: String,
    /// The read asked for the page after the loaded recordings.
    pub(crate) older: bool,
}

/// What a change of the server does to the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reload {
    /// The list is read now.
    Now,
    /// The list is read after this time: the read before started less than the interval
    /// ago.
    After(Duration),
    /// A read follows already: the one that waits for the end of the interval, or the
    /// one that follows the read that is on its way. It shows this change too.
    Waits,
}

/// When the reads of a list begin. One read runs at a time, and a read that a change of
/// the server causes begins not sooner than [`RELOAD_INTERVAL`] after the start of the
/// read before.
#[derive(Debug, Default)]
pub(crate) struct Reloads {
    /// When the last read began.
    last: Option<Instant>,
    /// A change arrived that the read on its way may not show: one read follows it.
    again: bool,
    /// A read waits for the end of the interval.
    waiting: bool,
}

impl Reloads {
    /// A read begins at `now`.
    pub(crate) fn started(&mut self, now: Instant) {
        self.last = Some(now);
    }

    /// A change of the server arrived at `now`. `reading` is true while a read is on its
    /// way.
    pub(crate) fn asked(&mut self, now: Instant, reading: bool) -> Reload {
        if self.waiting {
            return Reload::Waits;
        }
        if reading {
            self.again = true;
            return Reload::Waits;
        }
        self.next(now)
    }

    /// A read ended at `now`. The changes that arrived while it was on its way cause one
    /// further read.
    pub(crate) fn ended(&mut self, now: Instant) -> Option<Reload> {
        std::mem::take(&mut self.again).then(|| self.next(now))
    }

    /// The wait ended at `now`. While another read is on its way, the read that waited
    /// follows that read. A read that began and ended during the wait moved the interval:
    /// the read that waited then waits for the rest of it.
    pub(crate) fn waited(&mut self, now: Instant, reading: bool) -> Reload {
        self.waiting = false;
        if reading {
            self.again = true;
            return Reload::Waits;
        }
        self.next(now)
    }

    /// The wait was given up: its page is closed or shows another list.
    pub(crate) fn gave_up(&mut self) {
        self.waiting = false;
    }

    /// A read is to begin and none is on its way: now, or at the end of the interval.
    fn next(&mut self, now: Instant) -> Reload {
        let since = self
            .last
            .map(|last| now.saturating_duration_since(last))
            .filter(|since| *since < RELOAD_INTERVAL);
        match since {
            Some(since) => {
                self.waiting = true;
                Reload::After(RELOAD_INTERVAL - since)
            }
            None => {
                self.last = Some(now);
                Reload::Now
            }
        }
    }
}

/// The recordings of one workspace that the page has read.
#[derive(Default)]
pub struct RecordingList {
    /// The workspace whose list the page asked for.
    pub(crate) shown: Option<(ServerId, WorkspaceId)>,
    /// The account that the list was asked for. The server answers with what that
    /// account may see.
    pub(crate) account: Option<AccountId>,
    /// The newest first.
    pub(crate) recordings: Vec<Recording>,
    /// The server holds recordings before this one. Absent after the last page.
    pub(crate) next_before: Option<RecordingId>,
    /// A read is on its way.
    pub(crate) reading: bool,
    pub(crate) failed: Option<RecordingsFailure>,
    /// The number of the read whose answer the list takes.
    pub(crate) read: u64,
    pub(crate) reloads: Reloads,
    /// The wait before a read that a change of the server started. It ends with the list.
    pub(crate) waits: Option<Task<()>>,
}

/// True when a step of the event stream can have changed the list of a workspace: an
/// event about a recording of that workspace, a state that replaced the one before, and a
/// reader that missed steps (`None`).
pub(crate) fn concerns(notice: Option<&Notice>, workspace_id: WorkspaceId) -> bool {
    match notice {
        None | Some(Notice::Reloaded { .. }) => true,
        Some(Notice::Ended { .. }) => false,
        Some(Notice::Event { event, .. }) => match event.as_ref() {
            Event::RecordingStarted { recording } | Event::RecordingUpdated { recording } => {
                recording.workspace_id == workspace_id
            }
            Event::RecordingRemoved {
                workspace_id: of, ..
            } => *of == workspace_id,
            _ => false,
        },
    }
}

impl AppState {
    /// The workspace whose list of recordings the open page shows.
    fn open_recordings(&self) -> Option<(ServerId, WorkspaceId)> {
        let shown = self.admin.recordings.shown?;
        matches!(
            self.admin.open,
            Some(AdminOpen::Server { server_id, page: AdminPage::Recordings }) if server_id == shown.0
        )
        .then_some(shown)
    }

    /// Shows the recordings of a workspace and reads the first page. A second call for
    /// the same workspace reads the list again from the start; the loaded recordings stay
    /// until the answer arrives. One read runs at a time: while a read of the list is on
    /// its way, the read from the start follows it.
    pub fn show_recordings(
        &mut self,
        server_id: ServerId,
        workspace_id: WorkspaceId,
        cx: &mut Context<Self>,
    ) {
        // The account that the layer was opened for (ADR 0761).
        let account = self.admin.account;
        let list = &mut self.admin.recordings;
        if list.shown != Some((server_id, workspace_id)) || list.account != account {
            // The recordings belong to the workspace and to the account that they were
            // read for.
            *list = RecordingList {
                shown: Some((server_id, workspace_id)),
                account,
                ..RecordingList::default()
            };
        }
        if list.reading {
            list.reloads.asked(Instant::now(), true);
            return;
        }
        self.read_recordings(server_id, workspace_id, None, cx);
    }

    /// Reads the page of recordings after the loaded ones. Nothing is read while a read
    /// is on its way, after the last page, and when the page holds the most recordings
    /// that it keeps.
    pub fn read_older_recordings(&mut self, server_id: ServerId, cx: &mut Context<Self>) {
        let list = &self.admin.recordings;
        let Some((shown, workspace_id)) = list.shown else {
            return;
        };
        if shown != server_id || list.reading || list.recordings.len() >= MOST_RECORDINGS {
            return;
        }
        let Some(before) = list.next_before else {
            return;
        };
        self.read_recordings(server_id, workspace_id, Some(before), cx);
    }

    fn read_recordings(
        &mut self,
        server_id: ServerId,
        workspace_id: WorkspaceId,
        before: Option<RecordingId>,
        cx: &mut Context<Self>,
    ) {
        let Some(server) = self.client.server(server_id) else {
            return;
        };
        let Some(account_id) = self.admin.recordings.account else {
            return;
        };
        let number = RECORDING_READS.fetch_add(1, Ordering::Relaxed) + 1;
        let list = &mut self.admin.recordings;
        list.read = number;
        list.reading = true;
        list.failed = None;
        list.reloads.started(Instant::now());
        // A request fails when the device has another account by then.
        let api = server.api_for(account_id);
        cx.spawn(async move |this, cx| {
            let query = RecordingQuery {
                before,
                limit: Some(RECORDINGS_PAGE),
            };
            let read = api.recordings(workspace_id, &query).await;
            let _ = this.update(cx, |this, cx| {
                let now = this
                    .client
                    .server(server_id)
                    .and_then(|server| server.account());
                let list = &mut this.admin.recordings;
                // A late answer to an earlier read, to a read of another workspace or
                // server, and to a read of another account is not the one that the page
                // waits for.
                if list.read != number
                    || list.shown != Some((server_id, workspace_id))
                    || list.account != Some(account_id)
                    || now != Some(account_id)
                {
                    return;
                }
                list.reading = false;
                match read {
                    Ok(page) => {
                        if before.is_none() {
                            list.recordings.clear();
                        }
                        // A server is not trusted to answer with what was asked for: a
                        // recording of another workspace, and one that the list holds
                        // already, is left out.
                        for recording in page.recordings {
                            if recording.workspace_id == workspace_id
                                && list.recordings.len() < MOST_RECORDINGS
                                && !list.recordings.iter().any(|held| held.id == recording.id)
                            {
                                list.recordings.push(recording);
                            }
                        }
                        list.next_before = page.next_before;
                    }
                    Err(error) => {
                        list.failed = Some(RecordingsFailure {
                            sentence: text::admin::recordings::read_failure(&error),
                            older: before.is_some(),
                        });
                    }
                }
                // The changes that arrived during the read are shown by one further read.
                if let Some(reload) = list.reloads.ended(Instant::now()) {
                    this.reload_recordings(reload, server_id, workspace_id, cx);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// A step of the event stream of a server, or with `None` steps that the reader
    /// missed. While the page of the recordings of that server is open and the step can
    /// have changed its list, the list is read again from the start: at once, after the
    /// read that is on its way, or at the end of the interval when the read before
    /// started less than [`RELOAD_INTERVAL`] ago.
    pub(in crate::state) fn recordings_noticed(
        &mut self,
        server_id: ServerId,
        notice: Option<&Notice>,
        cx: &mut Context<Self>,
    ) {
        let Some((shown, workspace_id)) = self.open_recordings() else {
            return;
        };
        if shown != server_id || !concerns(notice, workspace_id) {
            return;
        }
        let list = &mut self.admin.recordings;
        let reload = list.reloads.asked(Instant::now(), list.reading);
        self.reload_recordings(reload, server_id, workspace_id, cx);
    }

    /// Does what the reloads of the list named: reads the list from the start now, or
    /// after a wait.
    fn reload_recordings(
        &mut self,
        reload: Reload,
        server_id: ServerId,
        workspace_id: WorkspaceId,
        cx: &mut Context<Self>,
    ) {
        match reload {
            Reload::Now => self.show_recordings(server_id, workspace_id, cx),
            Reload::Waits => {}
            Reload::After(wait) => {
                let task = cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(wait).await;
                    let _ = this.update(cx, |this, cx| {
                        this.admin.recordings.waits = None;
                        // The page can show another workspace by now, or be closed.
                        if this.open_recordings() != Some((server_id, workspace_id)) {
                            this.admin.recordings.reloads.gave_up();
                            return;
                        }
                        let list = &mut this.admin.recordings;
                        let reload = list.reloads.waited(Instant::now(), list.reading);
                        this.reload_recordings(reload, server_id, workspace_id, cx);
                    });
                });
                self.admin.recordings.waits = Some(task);
            }
        }
    }

    /// Draws this list of recordings in place of what a server answered. The page starts
    /// no read for a list that names its workspace. The screen tool uses it.
    #[cfg(feature = "test-support")]
    pub fn show_recordings_fixture(&mut self, list: RecordingList, cx: &mut Context<Self>) {
        self.admin.recordings = list;
        self.changed(cx);
    }
}

#[cfg(feature = "test-support")]
impl RecordingList {
    /// The list of a workspace as a read for an account left it: these recordings, the
    /// newest first. With `older` the server holds recordings after them.
    pub fn loaded(
        server_id: ServerId,
        workspace_id: WorkspaceId,
        account_id: AccountId,
        recordings: Vec<Recording>,
        older: bool,
    ) -> Self {
        Self {
            shown: Some((server_id, workspace_id)),
            account: Some(account_id),
            next_before: recordings
                .last()
                .filter(|_| older)
                .map(|recording| recording.id),
            recordings,
            ..Self::default()
        }
    }

    /// A read is on its way: the first page for a list without recordings, and the page
    /// after the loaded recordings otherwise.
    pub fn with_reading(mut self) -> Self {
        self.reading = true;
        self.failed = None;
        self
    }

    /// The last read failed, with the sentence that the page states.
    pub fn with_failure(mut self, sentence: impl Into<String>) -> Self {
        self.reading = false;
        self.failed = Some(RecordingsFailure {
            sentence: sentence.into(),
            older: !self.recordings.is_empty(),
        });
        self
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use junto_client_core::protocol::events::{Cursor, EndReason};
    use junto_client_core::protocol::ids::{CallId, ChannelId, StreamEpoch};
    use junto_client_core::protocol::recording::RecordingState;
    use junto_client_core::protocol::transcript::TranscriptState;
    use time::OffsetDateTime;

    use super::*;

    fn recording(workspace_id: WorkspaceId) -> Recording {
        let at = OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap();
        Recording {
            id: RecordingId::generate(),
            workspace_id,
            channel_id: ChannelId::generate(),
            call_id: CallId::generate(),
            state: RecordingState::Recording,
            started_by: AccountId::generate(),
            requested_at: at,
            recording_since: Some(at),
            stop: None,
            ended_at: None,
            failure: None,
            language: "auto".to_owned(),
            segments: 1,
            failed_segments: 0,
            bytes: None,
            timeline_started_at: None,
            transcript: TranscriptState::None,
            transcript_lines: 0,
        }
    }

    fn told(event: Event) -> Notice {
        Notice::Event {
            id: 7,
            at: OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap(),
            event: Arc::new(event),
        }
    }

    #[test]
    fn an_event_of_a_recording_of_the_shown_workspace_reads_the_list_again() {
        let (shown, other) = (WorkspaceId::generate(), WorkspaceId::generate());
        let of = |workspace_id| Box::new(recording(workspace_id));
        let removed = |workspace_id| Event::RecordingRemoved {
            recording_id: RecordingId::generate(),
            workspace_id,
            channel_id: ChannelId::generate(),
        };
        for (event, concerned) in [
            (
                Event::RecordingStarted {
                    recording: of(shown),
                },
                true,
            ),
            (
                Event::RecordingUpdated {
                    recording: of(shown),
                },
                true,
            ),
            (removed(shown), true),
            (
                Event::RecordingStarted {
                    recording: of(other),
                },
                false,
            ),
            (
                Event::RecordingUpdated {
                    recording: of(other),
                },
                false,
            ),
            (removed(other), false),
            (
                Event::WorkspaceDeleted {
                    workspace_id: shown,
                },
                false,
            ),
        ] {
            let name = event.name();
            assert_eq!(concerns(Some(&told(event)), shown), concerned, "{name}");
        }
        // A state that replaced the one before, and steps that the reader missed, can
        // hide such an event.
        let reloaded = Notice::Reloaded {
            cursor: Cursor {
                epoch: StreamEpoch::generate(),
                position: 1,
            },
        };
        assert!(concerns(Some(&reloaded), shown));
        assert!(concerns(None, shown));
        let ended = Notice::Ended {
            reason: EndReason::SessionEnded,
            recovery: None,
        };
        assert!(!concerns(Some(&ended), shown));
    }

    #[test]
    fn changes_of_the_server_start_one_read_in_a_second() {
        let start = Instant::now();
        let at = |millis: u64| start + Duration::from_millis(millis);
        let mut reloads = Reloads::default();
        // The first change reads at once.
        assert_eq!(reloads.asked(at(0), false), Reload::Now);
        // A change 300 ms later waits for the rest of the second.
        assert_eq!(
            reloads.asked(at(300), false),
            Reload::After(Duration::from_millis(700))
        );
        // Every further change in that time is shown by the read that waits.
        for millis in [301, 500, 999, 1500] {
            assert_eq!(reloads.asked(at(millis), false), Reload::Waits, "{millis}");
        }
        // The wait ended: the read began, and the next change waits a whole second
        // from it.
        assert_eq!(reloads.waited(at(1000), false), Reload::Now);
        assert_eq!(
            reloads.asked(at(1001), false),
            Reload::After(Duration::from_millis(999))
        );
        assert_eq!(reloads.waited(at(2000), false), Reload::Now);
        // A change one second or more after the last read reads at once.
        assert_eq!(reloads.asked(at(3000), false), Reload::Now);
        assert_eq!(reloads.asked(at(4000), false), Reload::Now);
        assert_eq!(RELOAD_INTERVAL, Duration::from_secs(1));
        // A read that the person started counts as the read before.
        reloads.started(at(4600));
        assert_eq!(
            reloads.asked(at(5000), false),
            Reload::After(Duration::from_millis(600))
        );
    }

    #[test]
    fn a_wait_that_ends_after_a_read_of_the_person_waits_for_the_rest_of_the_second() {
        let start = Instant::now();
        let at = |millis: u64| start + Duration::from_millis(millis);
        let mut reloads = Reloads::default();
        assert_eq!(reloads.asked(at(0), false), Reload::Now);
        assert_eq!(
            reloads.asked(at(200), false),
            Reload::After(Duration::from_millis(800))
        );
        // The person reads again at 800 ms, and that read has ended when the wait ends.
        reloads.started(at(800));
        assert_eq!(reloads.ended(at(900)), None);
        assert_eq!(
            reloads.waited(at(1000), false),
            Reload::After(Duration::from_millis(800))
        );
        assert_eq!(reloads.waited(at(1800), false), Reload::Now);
    }

    #[test]
    fn a_change_during_a_read_starts_one_further_read_after_it() {
        let start = Instant::now();
        let at = |millis: u64| start + Duration::from_millis(millis);
        let mut reloads = Reloads::default();
        assert_eq!(reloads.asked(at(0), false), Reload::Now);
        // The answer takes 2.5 seconds. No change in that time starts a second read.
        for millis in [100, 900, 1000, 1700, 2400] {
            assert_eq!(reloads.asked(at(millis), true), Reload::Waits, "{millis}");
        }
        // The read ended: one further read shows those changes. It starts at once,
        // because the read before started more than a second ago.
        assert_eq!(reloads.ended(at(2500)), Some(Reload::Now));
        // That read ended without a change: nothing follows it.
        assert_eq!(reloads.ended(at(2600)), None);

        // A change during a read that ends soon: the further read waits for the rest of
        // the second since the start of the read before.
        let mut reloads = Reloads::default();
        assert_eq!(reloads.asked(at(0), false), Reload::Now);
        assert_eq!(reloads.asked(at(100), true), Reload::Waits);
        assert_eq!(
            reloads.ended(at(300)),
            Some(Reload::After(Duration::from_millis(700)))
        );
        assert_eq!(reloads.asked(at(400), false), Reload::Waits);
        assert_eq!(reloads.waited(at(1000), false), Reload::Now);

        // The wait ends while a read of the person is on its way: the read that waited
        // follows that read, and still not sooner than a second after its start.
        assert_eq!(
            reloads.asked(at(1100), false),
            Reload::After(Duration::from_millis(900))
        );
        reloads.started(at(1800));
        assert_eq!(reloads.waited(at(2000), true), Reload::Waits);
        assert_eq!(
            reloads.ended(at(2100)),
            Some(Reload::After(Duration::from_millis(700)))
        );
    }

    /// The reads of a list while the server tells a change every `every` milliseconds
    /// and answers a read after `answer` milliseconds, for `span` milliseconds. Returns
    /// the start and the end of each read that ended.
    fn storm(every: u64, answer: u64, span: u64) -> Vec<(u64, u64)> {
        let start = Instant::now();
        let mut reloads = Reloads::default();
        let mut reads = Vec::new();
        // The read that is on its way, and the wait that runs, each with its end.
        let (mut reading, mut waits): (Option<(u64, u64)>, Option<u64>) = (None, None);
        for now in 0..span {
            let at = start + Duration::from_millis(now);
            let mut reload = None;
            if reading.is_some_and(|(_, end)| end == now) {
                reads.extend(reading.take());
                reload = reloads.ended(at);
            }
            if waits == Some(now) {
                waits = None;
                assert_eq!(reload, None);
                reload = Some(reloads.waited(at, reading.is_some()));
            }
            if now % every == 0 && reload.is_none() {
                reload = Some(reloads.asked(at, reading.is_some()));
            }
            match reload {
                Some(Reload::Now) => {
                    assert_eq!(reading, None, "two reads at once at {now} ms");
                    reloads.started(at);
                    reading = Some((now, now + answer));
                }
                Some(Reload::After(wait)) => {
                    assert_eq!(waits, None, "two waits at once at {now} ms");
                    waits = Some(now + wait.as_millis() as u64);
                }
                Some(Reload::Waits) | None => {}
            }
        }
        reads
    }

    #[test]
    fn a_server_that_tells_changes_without_end_gets_one_read_at_a_time_and_each_is_shown() {
        // A change in every second, and an answer after two seconds: each read ends
        // before the next one starts, so each answer is the one that the page waits for.
        let reads = storm(1000, 2000, 60_000);
        assert!((28..=30).contains(&reads.len()), "{}", reads.len());
        for pair in reads.windows(2) {
            assert!(pair[1].0 >= pair[0].1, "{pair:?}");
        }
        // A change in every millisecond and answers after 50 ms: one read in a second.
        let reads = storm(1, 50, 10_000);
        assert!((9..=10).contains(&reads.len()), "{}", reads.len());
        for pair in reads.windows(2) {
            assert!(pair[1].0 >= pair[0].0 + 1000, "{pair:?}");
        }
        // Without a change after the first one nothing is read again.
        assert_eq!(storm(100_000, 2000, 10_000), [(0, 2000)]);
    }
}
