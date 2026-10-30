use super::AppState;
use crate::drafts::{Desk, FileSource, SavedDraft, SavedFiles, Snapshot};
use gpui::{Context, Task};
use junto_client_core::{drafts::DraftStore, storage::StorageError, transfers::UploadSource};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

#[derive(Clone)]
pub(super) enum DraftSource {
    Fresh(UploadSource),
    Stored(FileSource),
}
#[derive(Clone)]
struct Source {
    upload: u64,
    value: DraftSource,
}

pub(super) struct Drafts {
    texts: HashMap<Desk, SavedDraft>,
    entries: HashMap<junto_client_core::protocol::ids::ServerId, u64>,
    files: HashMap<Desk, Vec<FileSource>>,
    restoring: HashSet<Desk>,
    sources: HashMap<Desk, Vec<Source>>,
    debounce: Option<Task<()>>,
    writes: u64,
    pub error: Option<String>,
}
impl Drafts {
    pub fn load(client: &junto_client_core::Client) -> Self {
        let store = DraftStore::new(client.data_dir().clone());
        let entries: HashMap<_, _> = client
            .servers()
            .into_iter()
            .map(|server| (server.id(), server.entry().generation))
            .collect();
        let (snapshot, mut error) = match store.load::<Snapshot>() {
            Ok(mut snapshots) => (snapshots.pop().unwrap_or_default(), None),
            Err(error) => (
                Snapshot::default(),
                Some(super::text::flow_error(&super::FlowError::Storage(
                    error.to_string(),
                ))),
            ),
        };
        if error.is_none() {
            let kept = snapshot
                .files
                .iter()
                .filter(|files| entries.get(&files.desk.0) == Some(&files.entry_generation))
                .flat_map(|files| &files.sources)
                .filter_map(|source| match source {
                    FileSource::Picture { hash, .. } => Some(hash.clone()),
                    _ => None,
                })
                .collect();
            error = store.tidy(&kept).err().map(|error| {
                super::text::flow_error(&super::FlowError::Storage(error.to_string()))
            });
        }
        Self {
            texts: snapshot
                .texts
                .into_iter()
                .filter(|draft| entries.get(&draft.desk.0) == Some(&draft.entry_generation))
                .map(|draft| (draft.desk, draft))
                .collect(),
            files: snapshot
                .files
                .into_iter()
                .filter(|files| entries.get(&files.desk.0) == Some(&files.entry_generation))
                .map(|files| (files.desk, files.sources))
                .collect(),
            entries,
            restoring: HashSet::new(),
            sources: HashMap::new(),
            debounce: None,
            writes: 0,
            error,
        }
    }
}
impl AppState {
    pub(crate) fn draft_entry_generation(
        &self,
        server: junto_client_core::protocol::ids::ServerId,
    ) -> Option<u64> {
        #[cfg(feature = "test-support")]
        if self
            .fixture
            .as_ref()
            .is_some_and(|servers| servers.iter().any(|entry| entry.id == server))
        {
            return Some(0);
        }
        self.client
            .server(server)
            .map(|server| server.entry().generation)
    }
    pub(super) fn reconcile_draft_entries(&mut self, cx: &mut Context<Self>) {
        let entries: HashMap<_, _> = self
            .client
            .servers()
            .into_iter()
            .map(|server| (server.id(), server.entry().generation))
            .collect();
        let changed: HashSet<_> = self
            .drafts
            .entries
            .iter()
            .filter(|(server, generation)| entries.get(server) != Some(generation))
            .map(|(server, _)| *server)
            .collect();
        if !changed.is_empty() {
            self.drafts
                .texts
                .retain(|desk, _| !changed.contains(&desk.0));
            self.drafts
                .files
                .retain(|desk, _| !changed.contains(&desk.0));
            self.drafts
                .sources
                .retain(|desk, _| !changed.contains(&desk.0));
            self.schedule_drafts(cx);
        }
        self.drafts.entries = entries;
    }
    pub(crate) fn draft_account(
        &self,
        server: junto_client_core::protocol::ids::ServerId,
    ) -> Option<junto_client_core::protocol::ids::AccountId> {
        #[cfg(feature = "test-support")]
        if let Some(entry) = self
            .fixture
            .as_ref()
            .and_then(|servers| servers.iter().find(|entry| entry.id == server))
        {
            return super::view::shown_model(entry).map(|model| model.account.id);
        }
        self.account_of(server)
    }
    pub(crate) fn draft_threads(
        &self,
        server: junto_client_core::protocol::ids::ServerId,
        channel: junto_client_core::protocol::ids::ChannelId,
    ) -> Vec<(junto_client_core::protocol::ids::MessageId, String)> {
        let Some(account) = self.draft_account(server) else {
            return Vec::new();
        };
        let mut roots = std::collections::BTreeMap::new();
        let root = |desk: &Desk| match desk {
            (
                saved,
                owner,
                junto_client_core::model::PlaceId::Thread {
                    channel_id,
                    root_id,
                },
            ) if *saved == server && *owner == account && *channel_id == channel => Some(*root_id),
            _ => None,
        };
        for (desk, draft) in &self.drafts.texts {
            if let Some(root) = root(desk) {
                roots.insert(
                    root,
                    draft
                        .text
                        .lines()
                        .find(|line| !line.trim().is_empty())
                        .map(|line| line.chars().take(48).collect())
                        .unwrap_or_else(|| crate::text::THREAD.to_owned()),
                );
            }
        }
        for desk in self
            .drafts
            .files
            .iter()
            .filter(|(_, sources)| !sources.is_empty())
            .map(|(desk, _)| desk)
            .chain(
                self.drafts
                    .sources
                    .iter()
                    .filter(|(_, sources)| !sources.is_empty())
                    .map(|(desk, _)| desk),
            )
        {
            if let Some(root) = root(desk) {
                roots
                    .entry(root)
                    .or_insert_with(|| crate::text::THREAD.to_owned());
            }
        }
        roots.into_iter().collect()
    }
    pub(crate) fn recovering_draft_files(&self, desk: Desk) -> bool {
        self.account_of(desk.0) == Some(desk.1)
            && (self.drafts.restoring.contains(&desk)
                || self
                    .drafts
                    .files
                    .get(&desk)
                    .is_some_and(|files| !files.is_empty())
                    && !self.uploads.contains_key(&(desk.0, desk.2)))
    }
    pub(crate) fn saved_draft(&self, desk: Desk) -> Option<&SavedDraft> {
        self.drafts
            .texts
            .get(&desk)
            .filter(|draft| self.draft_entry_generation(desk.0) == Some(draft.entry_generation))
    }
    pub(crate) fn has_draft(&self, desk: Desk) -> bool {
        self.drafts
            .sources
            .get(&desk)
            .is_some_and(|sources| !sources.is_empty())
            || self.drafts.texts.contains_key(&desk)
            || self
                .drafts
                .files
                .get(&desk)
                .is_some_and(|files| !files.is_empty())
            || self
                .uploads
                .get(&(desk.0, desk.2))
                .is_some_and(|files| !files.is_empty())
                && self.account_of(desk.0) == Some(desk.1)
    }
    pub(crate) fn keep_draft(&mut self, draft: SavedDraft, cx: &mut Context<Self>) {
        let desk = draft.desk;
        if self.draft_entry_generation(desk.0) != Some(draft.entry_generation) {
            return;
        }
        if draft.empty() {
            if self.drafts.texts.remove(&desk).is_none() {
                return;
            }
        } else {
            if self.drafts.texts.get(&desk) == Some(&draft) {
                return;
            }
            self.drafts.texts.insert(desk, draft);
        }
        self.schedule_drafts(cx);
        cx.notify();
    }
    pub(super) fn remember_attachment_source(
        &mut self,
        desk: Desk,
        upload: u64,
        source: UploadSource,
    ) {
        let value = match source {
            UploadSource::File(path) => DraftSource::Stored(FileSource::File(path)),
            UploadSource::Bytes { .. } => DraftSource::Fresh(source),
            UploadSource::Remote { .. } => return,
        };
        self.drafts
            .sources
            .entry(desk)
            .or_default()
            .push(Source { upload, value });
    }
    pub(super) fn remember_recovered_source(
        &mut self,
        desk: Desk,
        upload: u64,
        value: DraftSource,
    ) {
        self.drafts
            .sources
            .entry(desk)
            .or_default()
            .push(Source { upload, value });
    }
    pub(super) fn forget_attachment_source(
        &mut self,
        server: junto_client_core::protocol::ids::ServerId,
        place: junto_client_core::model::PlaceId,
        upload: u64,
        cx: &mut Context<Self>,
    ) {
        if let Some(account) = self.account_of(server) {
            let desk = (server, account, place);
            if let Some(sources) = self.drafts.sources.get_mut(&desk) {
                sources.retain(|source| source.upload != upload);
                if sources.is_empty() {
                    self.drafts.sources.remove(&desk);
                }
            }
        }
        self.schedule_drafts(cx);
    }
    pub(super) fn schedule_drafts(&mut self, cx: &mut Context<Self>) {
        #[cfg(feature = "test-support")]
        if self.fixture.is_some() {
            return;
        }
        self.drafts.writes = self.drafts.writes.wrapping_add(1);
        self.drafts.debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(300))
                .await;
            let _ = this.update(cx, |this, cx| this.flush_drafts(cx));
        }));
    }
    pub(crate) fn flush_drafts(&mut self, cx: &mut Context<Self>) {
        #[cfg(feature = "test-support")]
        if self.fixture.is_some() {
            return;
        }
        self.drafts.debounce = None;
        let texts = self.drafts.texts.values().cloned().collect();
        let entries = self.drafts.entries.clone();
        let mut files = self.drafts.files.clone();
        let mut sources = self.drafts.sources.clone();
        self.drafts.writes = self.drafts.writes.wrapping_add(1);
        let number = self.drafts.writes;
        let store = DraftStore::new(self.client.data_dir().clone());
        Self::queue_write(
            cx,
            move || -> Result<HashMap<Desk, Vec<Source>>, StorageError> {
                let old: Vec<Snapshot> = match store.load() {
                    Ok(old) => old,
                    Err(StorageError::Corrupted { .. }) => Vec::new(),
                    Err(error) => return Err(error),
                };
                for (desk, sources) in &mut sources {
                    let mut kept = Vec::new();
                    for source in sources {
                        let stored = match &source.value {
                            DraftSource::Stored(stored) => stored.clone(),
                            DraftSource::Fresh(UploadSource::File(path)) => {
                                FileSource::File(path.clone())
                            }
                            DraftSource::Fresh(UploadSource::Bytes { name, bytes }) => {
                                FileSource::Picture {
                                    name: name.clone(),
                                    hash: store.save_picture(bytes)?,
                                }
                            }
                            DraftSource::Fresh(UploadSource::Remote { .. }) => continue,
                        };
                        source.value = DraftSource::Stored(stored.clone());
                        kept.push(stored);
                    }
                    if kept.is_empty() {
                        files.remove(desk);
                    } else {
                        files.insert(*desk, kept);
                    }
                }
                let snapshot = Snapshot {
                    texts,
                    files: files
                        .into_iter()
                        .filter_map(|(desk, sources)| {
                            Some(SavedFiles {
                                desk,
                                sources,
                                entry_generation: *entries.get(&desk.0)?,
                            })
                        })
                        .collect(),
                };
                let hashes: HashSet<_> = snapshot
                    .files
                    .iter()
                    .flat_map(|files| &files.sources)
                    .filter_map(|source| match source {
                        FileSource::Picture { hash, .. } => Some(hash.clone()),
                        _ => None,
                    })
                    .collect();
                store.save(&[snapshot])?;
                for hash in old
                    .iter()
                    .flat_map(|snapshot| &snapshot.files)
                    .flat_map(|files| &files.sources)
                    .filter_map(|source| match source {
                        FileSource::Picture { hash, .. } => Some(hash),
                        _ => None,
                    })
                {
                    if !hashes.contains(hash) {
                        store.remove_picture(hash)?;
                    }
                }
                Ok(sources)
            },
            move |this, result, cx| {
                if number == this.drafts.writes {
                    match result {
                        Ok(sources) => {
                            this.drafts.sources = sources;
                            this.drafts.error = None;
                        }
                        Err(error) => {
                            this.drafts.error = Some(super::text::flow_error(
                                &super::FlowError::Storage(error.to_string()),
                            ))
                        }
                    }
                    cx.notify();
                }
            },
        );
    }
    fn draft_place_visible(&self, desk: Desk) -> bool {
        self.account_of(desk.0) == Some(desk.1)
            && (self.shown.server == Some(desk.0)
                && [self.shown.place(), self.shown.thread_place()].contains(&Some(desk.2))
                || self.call_window_shows == Some((desk.0, desk.2)))
    }
    pub(super) fn restore_draft_files(&mut self, cx: &mut Context<Self>) {
        let visible = |desk: &Desk| {
            self.draft_place_visible(*desk)
                && !self.drafts.restoring.contains(desk)
                && !self
                    .uploads
                    .get(&(desk.0, desk.2))
                    .is_some_and(|uploads| !uploads.is_empty())
        };
        let mut wanted: HashMap<_, _> = self
            .drafts
            .files
            .iter()
            .filter(|(desk, _)| visible(desk))
            .map(|(desk, files)| {
                (
                    *desk,
                    files
                        .iter()
                        .cloned()
                        .map(DraftSource::Stored)
                        .collect::<Vec<_>>(),
                )
            })
            .collect();
        for (desk, sources) in self
            .drafts
            .sources
            .iter()
            .filter(|(desk, _)| visible(desk) && !self.uploads.contains_key(&(desk.0, desk.2)))
        {
            wanted.insert(
                *desk,
                sources.iter().map(|source| source.value.clone()).collect(),
            );
        }
        for (desk, files) in wanted {
            let Some(owner) = self.owner(desk.0) else {
                continue;
            };
            self.drafts.restoring.insert(desk);
            let store = DraftStore::new(self.client.data_dir().clone());
            let missing = self
                .client
                .data_dir()
                .root()
                .join("unavailable-draft-files");
            cx.spawn(async move |this, cx| {
                let sources = cx
                    .background_executor()
                    .spawn(async move {
                        files
                            .into_iter()
                            .map(|saved| {
                                let source = match &saved {
                                    DraftSource::Fresh(source) => source.clone(),
                                    DraftSource::Stored(FileSource::File(path)) => {
                                        UploadSource::File(path.clone())
                                    }
                                    DraftSource::Stored(FileSource::Picture { name, hash }) => {
                                        match store.picture(hash) {
                                            Ok(Some(bytes)) => UploadSource::Bytes {
                                                name: name.clone(),
                                                bytes: std::sync::Arc::new(bytes),
                                            },
                                            _ => UploadSource::File(
                                                missing.join(
                                                    std::path::Path::new(name)
                                                        .file_name()
                                                        .unwrap_or_default(),
                                                ),
                                            ),
                                        }
                                    }
                                };
                                (source, saved)
                            })
                            .collect()
                    })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.drafts.restoring.remove(&desk);
                    if this.owner(desk.0) != Some(owner) || !this.draft_place_visible(desk) {
                        return;
                    }
                    this.drafts.files.remove(&desk);
                    this.drafts.sources.remove(&desk);
                    this.recover_attachments(desk, sources, cx);
                    this.flush_drafts(cx);
                });
            })
            .detach();
        }
    }
    pub(super) fn forget_server_drafts(
        &mut self,
        server: junto_client_core::protocol::ids::ServerId,
        cx: &mut Context<Self>,
    ) {
        self.drafts.texts.retain(|desk, _| desk.0 != server);
        self.drafts.files.retain(|desk, _| desk.0 != server);
        self.drafts.sources.retain(|desk, _| desk.0 != server);
        self.schedule_drafts(cx);
    }
    pub(super) fn removed_draft_files(
        &mut self,
        server: junto_client_core::protocol::ids::ServerId,
        place: junto_client_core::model::PlaceId,
        cx: &mut Context<Self>,
    ) {
        if let Some(account) = self.account_of(server) {
            self.drafts.files.remove(&(server, account, place));
            self.drafts.sources.remove(&(server, account, place));
        }
        self.schedule_drafts(cx);
    }
}
