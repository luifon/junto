//! What the application does for the messages of the window (ADR 0740): it holds the place
//! that the window shows open in the client core, sends what the person writes, follows
//! the files that go up and down, and reads the messages that replies name.

use std::path::PathBuf;

use gpui::Context;
use junto_client_core::Owner;
use junto_client_core::api::{ApiError, ServerApi};
use junto_client_core::flow::FlowError;
use junto_client_core::flow::access::AccessState;
use junto_client_core::live::LoadError;
use junto_client_core::model::PlaceId;
use junto_client_core::protocol::ids::{AccountId, AttachmentId, MessageId, ServerId};
use junto_client_core::protocol::message::{Attachment, Block, Document, Message};
use junto_client_core::transfers::{TransferFailure, Upload, UploadSource, UploadStage};

use super::AppState;
use super::feed;
use super::view::{DraftFile, Shown};
use crate::text;

/// How many reads of quoted messages are on their way at one time.
const QUOTE_READS: usize = 8;

/// Why a file was not attached to a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachRefusal {
    /// The file is larger than the server stores.
    TooLarge {
        name: String,
        limit: u64,
    },
    /// The message has as many files as the server takes.
    TooMany {
        limit: u32,
    },
    Unreadable {
        name: String,
    },
}

/// What a save of a file reads from the file system ([`AppState::find_for_save`]).
pub(super) struct Found {
    /// The directory into which files are saved: the one that a test set, or the download
    /// directory of the system.
    pub directory: Option<PathBuf>,
    /// A file that was saved before, and whether its path still names a file of the saved
    /// length.
    pub saved: Option<(PathBuf, bool)>,
}

impl Found {
    /// Asks the file system, which can wait for a slow or a network volume: `dirs` reads
    /// the download directory from a file on Linux and from the account database on macOS.
    /// Call it on a background thread, not on the thread of the window (ADR 0730).
    #[allow(clippy::disallowed_methods)]
    fn read(fixed: Option<PathBuf>, saved: Option<(PathBuf, u64)>) -> Self {
        Self {
            directory: fixed.or_else(dirs::download_dir),
            saved: saved.map(|(path, bytes)| {
                let kept = std::fs::symlink_metadata(&path)
                    .is_ok_and(|file| file.is_file() && file.len() == bytes);
                (path, kept)
            }),
        }
    }
}

/// A file or a picture to attach, with its size: absent for a path that names no file
/// that could be read. Only [`measure`] makes one, so the state does not ask the file
/// system itself.
pub struct Measured {
    source: UploadSource,
    bytes: Option<u64>,
}

/// Reads the size of each file. It asks the file system, which can wait for a slow or a
/// network volume: call it on a background thread, not on the thread of the window
/// (ADR 0730).
#[allow(clippy::disallowed_methods)]
pub fn measure(sources: Vec<UploadSource>) -> Vec<Measured> {
    sources
        .into_iter()
        .map(|source| {
            let bytes = match &source {
                UploadSource::File(path) => std::fs::metadata(path)
                    .ok()
                    .filter(std::fs::Metadata::is_file)
                    .map(|metadata| metadata.len()),
                UploadSource::Bytes { bytes, .. } => Some(bytes.len() as u64),
            };
            Measured { source, bytes }
        })
        .collect()
}

impl AppState {
    /// The places whose messages the window shows are held open in the client core: the
    /// place of the main area and the thread of the side panel. Every other place is
    /// released (ADR 0721). The pin list of the side panel is held in the same way.
    pub(super) fn follow_places(&mut self, shown: Shown, cx: &mut Context<Self>) {
        let wanted: Vec<(ServerId, PlaceId)> = shown
            .server
            .into_iter()
            .flat_map(|server_id| {
                shown
                    .place()
                    .into_iter()
                    .chain(shown.thread_place())
                    .map(move |place| (server_id, place))
            })
            .collect();
        let gone: Vec<(ServerId, PlaceId)> = self
            .open_places
            .keys()
            .filter(|key| !wanted.contains(key))
            .copied()
            .collect();
        for key in gone {
            self.open_places.remove(&key);
            self.anchors.remove(&key);
            if let Some(server) = self.client.server(key.0) {
                server.live().release_messages(key.1);
            }
        }
        for key in wanted {
            if self.open_places.contains_key(&key) {
                continue;
            }
            let Some(server) = self.client.server(key.0) else {
                continue;
            };
            let opening = server.live().open_messages(key.1);
            let task = cx.spawn(async move |this, cx| {
                let result = opening.await;
                let _ = this.update(cx, |this, cx| {
                    // The client core did not take the place: the next change of the state
                    // asks again. A read that failed leaves the place open with its
                    // failure.
                    if matches!(result, Err(LoadError::Off | LoadError::NotInState)) {
                        this.open_places.remove(&key);
                    }
                    cx.notify();
                });
            });
            self.open_places.insert(key, task);
        }
        self.follow_pins(shown, cx);
    }

    fn follow_pins(&mut self, shown: Shown, cx: &mut Context<Self>) {
        let wanted = shown.server.zip(shown.pins);
        if self.open_pins.as_ref().map(|(key, _)| *key) == wanted {
            return;
        }
        if let Some(((server_id, channel_id), _)) = self.open_pins.take()
            && let Some(server) = self.client.server(server_id)
        {
            server.live().release_pins(channel_id);
        }
        let Some(key) = wanted else {
            return;
        };
        let Some(server) = self.client.server(key.0) else {
            return;
        };
        let opening = server.live().open_pins(key.1);
        let task = cx.spawn(async move |this, cx| {
            let result = opening.await;
            let _ = this.update(cx, |this, cx| {
                if matches!(result, Err(LoadError::Off | LoadError::NotInState))
                    && this
                        .open_pins
                        .as_ref()
                        .is_some_and(|(held, _)| *held == key)
                {
                    // The client core holds no list: the next change asks again.
                    this.open_pins = None;
                }
                cx.notify();
            });
        });
        self.open_pins = Some((key, task));
    }

    /// Reads each message that a row of the shown places names and that the loaded
    /// messages do not hold: the message that a reply answers, and the last reply of a
    /// thread. What the rows of those places do not name any more is put away, and at
    /// most [`QUOTE_READS`] reads are on their way.
    pub(super) fn ask_missing_quotes(&mut self, shown: Shown, cx: &mut Context<Self>) {
        let server = shown
            .server
            .and_then(|server_id| self.client.server(server_id));
        let model = server
            .as_ref()
            .and_then(|server| server.live().view().model);
        let mut named = std::collections::HashSet::new();
        let mut held = false;
        if let Some(model) = &model {
            for place in shown.place().into_iter().chain(shown.thread_place()) {
                if let Some(timeline) = model.timeline(place) {
                    held = true;
                    named.extend(feed::named_quotes(timeline));
                }
            }
        }
        let (Some(server_id), Some(server), true) = (shown.server, server, held) else {
            self.known.quoted.clear();
            self.asked_quotes.clear();
            return;
        };
        let wanted = |(of, message_id): &(ServerId, MessageId)| {
            *of == server_id && named.contains(message_id)
        };
        self.known.quoted.retain(|key, _| wanted(key));
        self.asked_quotes.retain(|key, _| wanted(key));
        let on_their_way = self
            .asked_quotes
            .keys()
            .filter(|key| !self.known.quoted.contains_key(key))
            .count();
        let mut missing: Vec<MessageId> = named
            .into_iter()
            .filter(|message_id| !self.asked_quotes.contains_key(&(server_id, *message_id)))
            .collect();
        missing.sort_unstable();
        missing.truncate(QUOTE_READS.saturating_sub(on_their_way));
        for message_id in missing {
            let key = (server_id, message_id);
            self.quote_asks += 1;
            let number = self.quote_asks;
            self.asked_quotes.insert(key, number);
            let api = server.api();
            cx.spawn(async move |this, cx| {
                let read = api.message(message_id).await;
                let _ = this.update(cx, |this, cx| {
                    // The message changed, or the window shows another place: this answer
                    // is not the one that was asked for last.
                    if this.asked_quotes.get(&key) != Some(&number) {
                        return;
                    }
                    match read {
                        Ok(message) => feed::hold_quote(&mut this.known.quoted, key, message),
                        Err(error) => match feed::quote_without_answer(&error) {
                            Some(quoted) => {
                                this.known.quoted.insert(key, quoted);
                            }
                            // No answer: the message is asked for again with a later
                            // change.
                            None => {
                                this.asked_quotes.remove(&key);
                                return;
                            }
                        },
                    }
                    this.changed(cx);
                });
            })
            .detach();
        }
    }

    /// A quoted message changed, or with `None` every message of a server can have
    /// changed: what the window read of it is put away, and it is read again when a reply
    /// still names it.
    pub(super) fn forget_quotes(
        &mut self,
        server_id: ServerId,
        message_id: Option<MessageId>,
        cx: &mut Context<Self>,
    ) {
        let hit = |(of, held): &(ServerId, MessageId)| {
            *of == server_id && message_id.is_none_or(|message_id| message_id == *held)
        };
        let before = self.known.quoted.len() + self.asked_quotes.len();
        self.known.quoted.retain(|key, _| !hit(key));
        self.asked_quotes.retain(|key, _| !hit(key));
        if self.known.quoted.len() + self.asked_quotes.len() != before {
            self.changed(cx);
        }
    }

    /// The entry of the server book and the account that the device has on a server now.
    /// What the window keeps of a server is kept for one owner, and what an owner asked
    /// for is taken only while the server still has that owner (ADR 0745, ADR 0746).
    pub fn owner(&self, server_id: ServerId) -> Option<Owner> {
        self.client.server(server_id)?.owner()
    }

    /// The account that this device has on a server.
    fn account_of(&self, server_id: ServerId) -> Option<AccountId> {
        match self.client.server(server_id)?.access().state {
            AccessState::LoggedIn { account_id, .. } => Some(account_id),
            _ => None,
        }
    }

    /// The files that wait for a message belong to the account that uploaded them. When
    /// a server has another account, or none, their uploads are ended.
    pub(super) fn end_uploads_of_other_accounts(&mut self) {
        let owners = std::mem::take(&mut self.upload_owners);
        let mut ended = false;
        for (server_id, owner) in owners {
            if self.account_of(server_id) == Some(owner) {
                self.upload_owners.insert(server_id, owner);
                continue;
            }
            let drafts: Vec<(ServerId, PlaceId)> = self
                .uploads
                .keys()
                .filter(|key| key.0 == server_id)
                .copied()
                .collect();
            let sent: Vec<(ServerId, u64)> = self
                .sent_uploads
                .keys()
                .filter(|key| key.0 == server_id)
                .copied()
                .collect();
            let uploads = drafts
                .iter()
                .filter_map(|key| self.uploads.remove(key))
                .chain(sent.iter().filter_map(|key| self.sent_uploads.remove(key)))
                .flatten();
            for upload in uploads {
                upload.cancel();
            }
            ended = true;
        }
        if ended {
            self.show_uploads();
        }
    }

    /// Looks at the owner of each server. For a server whose owner is another one than at
    /// the last look, or none, the window ends the downloads that it follows and forgets
    /// what it shows of them and the quoted messages that it read: the next owner sees
    /// neither the progress, nor the path of a saved file, nor a text of the owner before.
    pub(super) fn follow_owners(&mut self) {
        let present: std::collections::HashMap<ServerId, Owner> = self
            .client
            .servers()
            .iter()
            .filter_map(|server| Some((server.id(), server.owner()?)))
            .collect();
        let before = std::mem::replace(&mut self.owners, present);
        for (server_id, owner) in before {
            if self.owners.get(&server_id) == Some(&owner) {
                continue;
            }
            self.downloads.retain(|key, download| {
                if key.0 == server_id {
                    download.cancel();
                }
                key.0 != server_id
            });
            self.known.downloads.retain(|key, _| key.0 != server_id);
            self.known.quoted.retain(|key, _| key.0 != server_id);
            self.asked_quotes.retain(|key, _| key.0 != server_id);
            self.forget_recording_downloads_of(server_id);
            // A change of a transcript that the owner before sent is told to nobody.
            self.known
                .transcript_asks
                .retain(|key, _| key.0 != server_id);
            self.known
                .transcripts_requesting
                .retain(|key| key.0 != server_id);
            // The remembered choices of the server belong to the owner too.
            self.selection.workspaces.remove(&server_id);
            self.selection
                .channels
                .retain(|(of, _), _| *of != server_id);
            self.selection.conversations.remove(&server_id);
        }
    }

    /// Asks a server for a change of a message, for the account that the device has on it
    /// now. A refusal is told in [`super::Pending::message_refused`]. The answer of the
    /// server is not content of the state: the change arrives with its event, in the
    /// order of the events (ADR 0746).
    fn ask_messages<F, R>(
        &mut self,
        server_id: ServerId,
        cx: &mut Context<Self>,
        ask: impl FnOnce(ServerApi) -> F,
    ) where
        F: Future<Output = Result<R, ApiError>> + 'static,
        R: 'static,
    {
        self.ask_about_messages(server_id, cx, ask, |error| {
            text::flow_error(&FlowError::from(error))
        });
    }

    /// Like [`Self::ask_messages`], with the sentence that tells a refusal.
    pub(super) fn ask_about_messages<F, R>(
        &mut self,
        server_id: ServerId,
        cx: &mut Context<Self>,
        ask: impl FnOnce(ServerApi) -> F,
        refusal: fn(&ApiError) -> String,
    ) where
        F: Future<Output = Result<R, ApiError>> + 'static,
        R: 'static,
    {
        let Some(server) = self.client.server(server_id) else {
            return;
        };
        let Some(owner) = server.owner() else {
            // The person is told that nothing was sent.
            self.refuse(text::flow_error(&FlowError::from(&ApiError::NotLoggedIn)));
            cx.notify();
            return;
        };
        let asking = ask(server.api_for(owner.account_id));
        self.message_asks += 1;
        self.message_asks_flying += 1;
        let (number, navigation) = (self.message_asks, self.navigations);
        self.pending.message_refused = None;
        cx.spawn(async move |this, cx| {
            let result = asking.await;
            let _ = this.update(cx, |this, cx| {
                this.message_asks_flying -= 1;
                // A late refusal of an earlier request does not replace what the person
                // did afterwards, a refusal for an account that the device has no more
                // is not told to the next one, and a refusal that arrives after the person
                // selected another place is not told at the composer of that place.
                if let Err(error) = result
                    && this.message_asks == number
                    && this.navigations == navigation
                    && this.owner(server_id) == Some(owner)
                {
                    this.refuse(refusal(&error));
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// How many requests for a change of a message have no answer yet.
    pub fn message_asks_flying(&self) -> usize {
        self.message_asks_flying
    }

    /// Why the server did not do what the person asked for with a message or a file. A
    /// refusal belongs to the selection that the person made the request in: after the
    /// person selected another server, workspace, channel or conversation, it is not
    /// shown (ADR 0746).
    pub fn message_refusal(&self) -> Option<&str> {
        self.pending
            .message_refused
            .as_ref()
            .filter(|refusal| refusal.navigation == self.navigations)
            .map(|refusal| refusal.sentence.as_str())
    }

    /// Tells a refusal of a request that the person made in the present selection.
    pub(super) fn refuse(&mut self, sentence: String) {
        self.pending.message_refused = Some(super::Refusal {
            sentence,
            navigation: self.navigations,
        });
    }

    pub fn dismiss_message_refusal(&mut self, cx: &mut Context<Self>) {
        self.pending.message_refused = None;
        cx.notify();
    }

    /// Sends a message with the files that the person attached in this place. False when
    /// the server has no state now: the text stays with the person.
    pub fn send_message(
        &mut self,
        server_id: ServerId,
        place: PlaceId,
        document: Document,
        reply_to: Option<MessageId>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(server) = self.client.server(server_id) else {
            return false;
        };
        let key = (server_id, place);
        let uploads = self.uploads.remove(&key).unwrap_or_default();
        match server.transfers().send(place, document, reply_to, &uploads) {
            Some(id) => {
                if !uploads.is_empty() {
                    self.sent_uploads.insert((server_id, id), uploads);
                }
                self.pending.message_refused = None;
                self.show_uploads();
                cx.notify();
                true
            }
            None => {
                if !uploads.is_empty() {
                    self.uploads.insert(key, uploads);
                }
                false
            }
        }
    }

    /// Sends again a message that failed or that got no answer.
    pub fn send_again(&mut self, server_id: ServerId, id: u64) {
        if let Some(server) = self.client.server(server_id) {
            server.live().send_again(id);
        }
    }

    /// Gives up a message that waits. Its uploads are ended.
    pub fn discard_message(&mut self, server_id: ServerId, id: u64, cx: &mut Context<Self>) {
        for upload in self
            .sent_uploads
            .remove(&(server_id, id))
            .unwrap_or_default()
        {
            upload.cancel();
        }
        if let Some(server) = self.client.server(server_id) {
            server.live().discard_message(id);
        }
        self.show_uploads();
        cx.notify();
    }

    pub fn edit_message(
        &mut self,
        server_id: ServerId,
        message_id: MessageId,
        document: Document,
        cx: &mut Context<Self>,
    ) {
        self.ask_messages(server_id, cx, move |api| async move {
            api.edit_message(message_id, document).await
        });
    }

    pub fn delete_message(
        &mut self,
        server_id: ServerId,
        message_id: MessageId,
        cx: &mut Context<Self>,
    ) {
        self.ask_messages(server_id, cx, move |api| async move {
            api.delete_message(message_id).await
        });
    }

    /// Adds the reaction of the person to a message, or takes it away.
    pub fn react(
        &mut self,
        server_id: ServerId,
        message_id: MessageId,
        emoji: String,
        add: bool,
        cx: &mut Context<Self>,
    ) {
        self.ask_messages(server_id, cx, move |api| async move {
            if add {
                api.add_reaction(message_id, &emoji).await
            } else {
                api.remove_reaction(message_id, &emoji).await
            }
        });
    }

    /// Ticks a check box of a message of the person, or clears it. This is an edit of the
    /// message (ADR 0401).
    pub fn set_check(
        &mut self,
        server_id: ServerId,
        message: &Message,
        block: usize,
        item: usize,
        checked: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(mut document) = message.document.clone() else {
            return;
        };
        let Some(Block::Checklist { items }) = document.blocks.get_mut(block) else {
            return;
        };
        let Some(held) = items.get_mut(item) else {
            return;
        };
        if held.checked == checked {
            return;
        }
        held.checked = checked;
        self.edit_message(server_id, message.id, document, cx);
    }

    /// Reads the page before the first loaded message of a place.
    pub fn read_older(&mut self, server_id: ServerId, place: PlaceId) {
        if let Some(server) = self.client.server(server_id) {
            server.live().read_older_messages(place);
        }
    }

    pub fn read_newer(&mut self, server_id: ServerId, place: PlaceId) {
        if let Some(server) = self.client.server(server_id) {
            server.live().read_newer_messages(place);
        }
    }

    /// Replaces the loaded messages of a place by the end of its history.
    pub fn read_latest(&mut self, server_id: ServerId, place: PlaceId) {
        if let Some(server) = self.client.server(server_id) {
            server.live().read_latest_messages(place);
        }
    }

    /// Tells the client core where the list of a place stands: at a position of the
    /// history, or at its end.
    pub fn set_anchor(&mut self, server_id: ServerId, place: PlaceId, position: Option<u64>) {
        let key = (server_id, place);
        if self.anchors.get(&key).copied().flatten() == position && self.anchors.contains_key(&key)
        {
            return;
        }
        self.anchors.insert(key, position);
        if let Some(server) = self.client.server(server_id) {
            server.live().set_message_anchor(place, position);
        }
    }

    /// The person has seen the messages of a place up to a position. The event stream
    /// client of the server sends a position once for the account that it has, and
    /// leaves out one that the account has read already (ADR 0721), so the window keeps
    /// no record of its own.
    pub fn mark_read(&mut self, server_id: ServerId, place: PlaceId, position: u64) {
        if let Some(server) = self.client.server(server_id) {
            server.live().mark_read(place, position);
        }
    }

    /// Moves the read position of a place to its latest message.
    pub fn mark_place_as_read(&mut self, server_id: ServerId, place: PlaceId) {
        let latest = self
            .client
            .server(server_id)
            .and_then(|server| server.live().view().model)
            .and_then(|model| model.message_state(place).map(|state| state.latest));
        if let Some(latest) = latest {
            self.mark_read(server_id, place, latest);
        }
    }

    /// Attaches files to the next message of a place and starts their uploads. Returns
    /// why a file was not attached. The sizes were read before, off the thread of the
    /// window ([`measure`]).
    pub fn attach(
        &mut self,
        server_id: ServerId,
        place: PlaceId,
        sources: Vec<Measured>,
        cx: &mut Context<Self>,
    ) -> Vec<AttachRefusal> {
        let Some(server) = self.client.server(server_id) else {
            return Vec::new();
        };
        let limits = server
            .live()
            .view()
            .model
            .and_then(|model| model.attachment_limits.clone());
        let transfers = server.transfers();
        let key = (server_id, place);
        let mut refused = Vec::new();
        for Measured { source, bytes } in sources {
            let name = source.name();
            let held = self.uploads.get(&key).map_or(0, Vec::len);
            if let Some(limits) = &limits
                && held >= limits.files_per_message as usize
            {
                refused.push(AttachRefusal::TooMany {
                    limit: limits.files_per_message,
                });
                break;
            }
            let Some(bytes) = bytes else {
                refused.push(AttachRefusal::Unreadable { name });
                continue;
            };
            if let Some(limits) = &limits
                && bytes > limits.bytes_per_file
            {
                refused.push(AttachRefusal::TooLarge {
                    name,
                    limit: limits.bytes_per_file,
                });
                continue;
            }
            let Some(owner) = self.account_of(server_id) else {
                refused.push(AttachRefusal::Unreadable { name });
                continue;
            };
            let upload = transfers.upload(place, source);
            self.watch_upload(&upload, cx);
            self.upload_owners.insert(server_id, owner);
            self.uploads.entry(key).or_default().push(upload);
        }
        self.show_uploads();
        cx.notify();
        refused
    }

    /// Takes a file out of the next message of a place and ends its upload.
    pub fn remove_attachment(
        &mut self,
        server_id: ServerId,
        place: PlaceId,
        upload: u64,
        cx: &mut Context<Self>,
    ) {
        if let Some(uploads) = self.uploads.get_mut(&(server_id, place)) {
            uploads.retain(|held| {
                if held.id == upload {
                    held.cancel();
                }
                held.id != upload
            });
        }
        self.show_uploads();
        cx.notify();
    }

    fn watch_upload(&mut self, upload: &Upload, cx: &mut Context<Self>) {
        let mut watch = upload.watch();
        cx.spawn(async move |this, cx| {
            loop {
                let ended = watch.changed().await.is_err()
                    || matches!(
                        watch.borrow().stage,
                        UploadStage::Complete(_) | UploadStage::Failed(_) | UploadStage::Cancelled
                    );
                let told = this.update(cx, |this, cx| {
                    this.show_uploads();
                    cx.notify();
                });
                if ended || told.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Puts the progress of every upload that the window can show into what the view is
    /// derived from.
    pub(super) fn show_uploads(&mut self) {
        self.known.drafts = self
            .uploads
            .iter()
            .filter(|(_, uploads)| !uploads.is_empty())
            .map(|(key, uploads)| {
                let files = uploads
                    .iter()
                    .map(|upload| DraftFile {
                        upload: upload.id,
                        name: upload.name.clone(),
                        progress: upload.progress(),
                    })
                    .collect();
                (*key, files)
            })
            .collect();
        self.known.uploads = self
            .sent_uploads
            .iter()
            .flat_map(|((server_id, _), uploads)| {
                uploads
                    .iter()
                    .map(|upload| ((*server_id, upload.id), upload.progress()))
            })
            .collect();
    }

    /// The uploads of a waiting message are held until the message is sent or given up.
    pub(super) fn drop_sent_uploads(&mut self) {
        let servers = self.client.servers();
        let waits = |server_id: ServerId, id: u64| {
            servers
                .iter()
                .find(|server| server.id() == server_id)
                .and_then(|server| server.live().view().model)
                .is_some_and(|model| model.outbox.iter().any(|pending| pending.id == id))
        };
        let before = self.sent_uploads.len();
        self.sent_uploads
            .retain(|(server_id, id), _| waits(*server_id, *id));
        if self.sent_uploads.len() != before {
            self.show_uploads();
        }
    }

    /// Reads on a background thread what a save of a file needs from the file system, and
    /// then runs `then` on the thread of the window, while the server has the owner that
    /// asked (ADR 0746). `saved` is a file that was saved before, with its length.
    pub(super) fn find_for_save(
        &mut self,
        server_id: ServerId,
        saved: Option<(PathBuf, u64)>,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, Found, &mut Context<Self>) + 'static,
    ) {
        let owner = self.owner(server_id);
        let fixed = self.download_directory.clone();
        let finding = cx
            .background_executor()
            .spawn(async move { Found::read(fixed, saved) });
        cx.spawn(async move |this, cx| {
            let found = finding.await;
            let _ = this.update(cx, |this, cx| {
                if this.owner(server_id) == owner {
                    then(this, found, cx);
                }
            });
        })
        .detach();
    }

    /// Saves a file of a message to the download directory, under a name that no file has.
    pub fn download(
        &mut self,
        server_id: ServerId,
        attachment: &Attachment,
        cx: &mut Context<Self>,
    ) {
        let attachment = attachment.clone();
        self.find_for_save(server_id, None, cx, move |this, found, cx| {
            this.download_into(server_id, &attachment, found.directory, cx);
        });
    }

    fn download_into(
        &mut self,
        server_id: ServerId,
        attachment: &Attachment,
        directory: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        // What the window holds of the downloads of a server belongs to its present owner.
        self.follow_owners();
        if !self.owners.contains_key(&server_id) {
            self.refuse(text::flow_error(&FlowError::from(&ApiError::NotLoggedIn)));
            cx.notify();
            return;
        }
        let key = (server_id, attachment.id);
        if self.downloads.contains_key(&key) {
            return;
        }
        let (Some(server), Some(directory)) = (self.client.server(server_id), directory) else {
            self.refuse(text::NO_DOWNLOAD_DIRECTORY.to_owned());
            cx.notify();
            return;
        };
        let download = server.transfers().download(attachment, directory);
        let mut watch = download.watch();
        self.known.downloads.insert(key, download.progress());
        self.downloads.insert(key, download);
        cx.spawn(async move |this, cx| {
            loop {
                let closed = watch.changed().await.is_err();
                let progress = watch.borrow().clone();
                let told = this.update(cx, |this, cx| {
                    if this.downloads.contains_key(&key) {
                        this.known.downloads.insert(key, progress);
                        this.changed(cx);
                    }
                });
                if closed || told.is_err() {
                    break;
                }
            }
        })
        .detach();
        self.changed(cx);
    }

    /// Ends a download, or puts away what the window shows of one that ended.
    pub fn forget_download(
        &mut self,
        server_id: ServerId,
        attachment_id: AttachmentId,
        cx: &mut Context<Self>,
    ) {
        let key = (server_id, attachment_id);
        if let Some(download) = self.downloads.remove(&key) {
            download.cancel();
        }
        self.known.downloads.remove(&key);
        self.changed(cx);
    }

    /// Draws these files and this statement in place of what the transfers of the client
    /// tell, with the times of messages in one time zone. The screen tool uses it.
    #[cfg(feature = "test-support")]
    pub fn show_messages_fixture(
        &mut self,
        zone: super::feed::Zone,
        drafts: std::collections::HashMap<(ServerId, PlaceId), Vec<DraftFile>>,
        uploads: std::collections::HashMap<
            (ServerId, u64),
            junto_client_core::transfers::UploadProgress,
        >,
        downloads: std::collections::HashMap<
            (ServerId, AttachmentId),
            junto_client_core::transfers::DownloadProgress,
        >,
        refused: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.known.zone = zone;
        self.known.drafts = drafts;
        self.known.uploads = uploads;
        self.known.downloads = downloads;
        self.pending.message_refused = None;
        if let Some(refused) = refused {
            self.refuse(refused);
        }
        self.changed(cx);
    }

    /// The times of messages are shown in this time zone. A test uses it.
    #[cfg(feature = "test-support")]
    pub fn set_zone(&mut self, zone: super::feed::Zone, cx: &mut Context<Self>) {
        self.known.zone = zone;
        self.changed(cx);
    }

    /// The files of messages are saved here in place of the download directory of the
    /// person. A test uses it.
    #[cfg(feature = "test-support")]
    pub fn set_download_directory(&mut self, directory: PathBuf) {
        self.download_directory = Some(directory);
    }

    /// Reads a picture of a message into memory for its owner, for the window to draw
    /// it. Nothing is read when the server has another owner by now.
    pub fn picture(
        &self,
        owner: Owner,
        attachment: &Attachment,
    ) -> impl Future<Output = Result<Vec<u8>, TransferFailure>> + 'static + use<> {
        let reading = self
            .client
            .server(owner.entry.server_id)
            .map(|server| server.transfers().picture(owner, attachment));
        async move {
            match reading {
                Some(reading) => reading.await,
                None => Err(TransferFailure::Stopped),
            }
        }
    }
}
