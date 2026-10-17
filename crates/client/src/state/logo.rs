//! The logo of each server as the window shows it (ADR 0101): the image that an
//! administrator set, read from the public route of the server and decoded away from the
//! window.
//!
//! [`Logos`] is the cache. It is plain data with every rule of the reads, so the rules are
//! tested without a window and without a server. The state of the application starts the
//! request that the cache names and gives it the answer.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::{AppContext as _, Context, RenderImage};
use junto_client_core::ServerHandle;
use junto_client_core::api::{ApiError, AvatarImage};
use junto_client_core::live::Notice;
use junto_client_core::model::LogoState;
use junto_client_core::protocol::events::Event;
use junto_client_core::protocol::ids::ServerId;

use super::AppState;
use super::people::{AVATAR_SIDE, Answer, PictureHash, TRIES, decode};
use super::view::shown_model;

/// What the state of a server and the server book tell of the logo of the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Told {
    /// No event stated the logo since the state of the server was loaded. `named` is true
    /// when the server book names the path of a logo: the server had one at the last
    /// contact.
    NotStated { named: bool },
    /// The last event stated that the server has no logo.
    Absent,
    /// The last event stated a logo. The hash is absent when the server wrote one in
    /// another form than 64 hexadecimal digits in lower case.
    Present(Option<PictureHash>),
}

impl Told {
    pub fn of(logo: &LogoState, named: bool) -> Self {
        match logo {
            LogoState::NotStated => Self::NotStated { named },
            LogoState::Stated(None) => Self::Absent,
            LogoState::Stated(Some(description)) => {
                Self::Present(PictureHash::parse(&description.hash))
            }
        }
    }
}

/// A logo that was read: the hash that this device computed of its bytes, and the decoded
/// image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Logo {
    pub hash: PictureHash,
    pub image: Arc<RenderImage>,
}

/// One read of a logo that the cache wants to be started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ask {
    pub server_id: ServerId,
    /// The number of the request. No other request of the cache has it, for any server,
    /// so an answer is taken only by the cache that asked.
    number: u64,
}

/// Where the read of one logo stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Read {
    /// To be read: not asked for yet, or to be read again.
    Wanted,
    /// The request with this number is on its way. It is the `tries`-th request.
    Asking { tries: u8, number: u64 },
    /// No request is on its way, and this number of requests got no answer.
    Unanswered(u8),
    /// The logo was read, or nothing is to be read: it is not asked for again.
    #[default]
    Settled,
}

impl Read {
    /// The number of the request that may start now. `again` allows a request after one
    /// that got no answer.
    fn next(self, again: bool) -> Option<u8> {
        match self {
            Self::Wanted => Some(1),
            Self::Unanswered(tries) if again && tries < TRIES => Some(tries + 1),
            Self::Unanswered(_) | Self::Asking { .. } | Self::Settled => None,
        }
    }
}

/// What the cache holds of one server.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Held {
    /// What was told last. Absent until the first time.
    told: Option<Told>,
    read: Read,
    /// The image that the window shows, with the hash of its bytes.
    image: Option<Logo>,
    /// The last read found no logo, and nothing else was told since.
    none_found: bool,
    /// The number of the request that is on its way.
    on_way: Option<u64>,
}

impl Held {
    fn hash(&self) -> Option<PictureHash> {
        self.image.as_ref().map(|logo| logo.hash)
    }

    fn image(&self) -> Option<Arc<RenderImage>> {
        self.image.as_ref().map(|logo| logo.image.clone())
    }

    /// True when the server has a logo, as far as this device knows.
    fn exists(&self) -> bool {
        match self.told {
            Some(Told::Present(_)) => true,
            Some(Told::Absent) | None => false,
            // The image of a read is newer than the path of the server book.
            Some(Told::NotStated { named }) => self.image.is_some() || (named && !self.none_found),
        }
    }
}

/// The logo of one server as the view takes it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShownLogo {
    /// The server has a logo, as far as this device knows.
    pub exists: bool,
    /// The image of the logo, when the window holds it.
    pub image: Option<Arc<RenderImage>>,
}

/// The logos of the servers that have one, as the view takes them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShownLogos {
    servers: HashMap<ServerId, ShownLogo>,
}

impl ShownLogos {
    pub fn of(&self, server_id: ServerId) -> ShownLogo {
        self.servers.get(&server_id).cloned().unwrap_or_default()
    }

    /// The image of the logo of a server, when the window holds it.
    pub fn image(&self, server_id: ServerId) -> Option<Arc<RenderImage>> {
        self.servers.get(&server_id)?.image.clone()
    }
}

/// The images of the logos that the window holds, by server.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Logos {
    servers: HashMap<ServerId, Held>,
    /// The images that left the cache and that the window can still hold as textures.
    dropped: Vec<Arc<RenderImage>>,
    /// The number of the last request. It counts the requests of every server while the
    /// application runs, also across the removal of a server.
    asked: u64,
}

impl Logos {
    /// Drops what is held of every server that is not one of these.
    pub fn retain(&mut self, servers: &HashSet<ServerId>) {
        let gone: Vec<ServerId> = self
            .servers
            .keys()
            .filter(|server_id| !servers.contains(server_id))
            .copied()
            .collect();
        for server_id in gone {
            if let Some(held) = self.servers.remove(&server_id) {
                self.dropped.extend(held.image.map(|logo| logo.image));
            }
        }
    }

    /// Takes what the state of a server and the server book tell of the logo now. The same
    /// statement as before changes nothing.
    pub fn want(&mut self, server_id: ServerId, told: Told) {
        let held = self.servers.entry(server_id).or_default();
        if held.told == Some(told) {
            return;
        }
        let before = held.told.replace(told);
        held.none_found = false;
        let (keeps, reads) = match (told, before) {
            (Told::Absent, _) => (false, false),
            (Told::Present(Some(hash)), _) if held.hash() == Some(hash) => (true, false),
            // The held image stays shown until the new one arrives.
            (Told::Present(_), _) => (true, true),
            // The state was loaded again after an event stated the logo. The server book
            // can name what the server had before that event, so the route is asked.
            (Told::NotStated { .. }, Some(Told::Absent | Told::Present(_))) => (true, true),
            (Told::NotStated { named }, _) => (named, named),
        };
        if !keeps {
            self.dropped
                .extend(held.image.take().map(|logo| logo.image));
        }
        // A request that is on its way started before this statement: its answer is not
        // used, and it keeps its place until it ends.
        held.read = if reads { Read::Wanted } else { Read::Settled };
    }

    /// The request that starts now: one for a server at a time. `again` is true after a
    /// change of the state of the application: then a request that got no answer is sent
    /// again.
    pub fn next(&mut self, server_id: ServerId, again: bool) -> Option<Ask> {
        let held = self.servers.get_mut(&server_id)?;
        if held.on_way.is_some() {
            return None;
        }
        let tries = held.read.next(again)?;
        self.asked += 1;
        held.read = Read::Asking {
            tries,
            number: self.asked,
        };
        held.on_way = Some(self.asked);
        Some(Ask {
            server_id,
            number: self.asked,
        })
    }

    /// Takes the answer to a request. An absent logo in a read answer: the server has no
    /// logo.
    pub fn answered(&mut self, ask: &Ask, answer: Answer<Option<Logo>>) {
        let Some(held) = self.servers.get_mut(&ask.server_id) else {
            return;
        };
        // The cache of a server that was removed and added again did not ask for it.
        if held.on_way != Some(ask.number) {
            return;
        }
        held.on_way = None;
        let Read::Asking { tries, number } = held.read else {
            return;
        };
        if number != ask.number {
            return;
        }
        match answer {
            Answer::Read(Some(logo)) => {
                // The same bytes as before: the held image stays the texture of the window.
                if held.hash() != Some(logo.hash) {
                    let before = held.image.replace(logo);
                    self.dropped.extend(before.map(|logo| logo.image));
                }
                held.none_found = false;
                held.read = Read::Settled;
            }
            Answer::Read(None) => {
                self.dropped
                    .extend(held.image.take().map(|logo| logo.image));
                held.none_found = true;
                held.read = Read::Settled;
            }
            Answer::Final => held.read = Read::Settled,
            Answer::None => held.read = Read::Unanswered(tries),
        }
    }

    /// The state of a server was replaced, or events were missed: a logo that no event
    /// stated since can have changed without an event. It is read again once; the held
    /// image is shown until the answer arrives. A request that is on its way started
    /// before this moment: its answer is not used.
    pub fn stale(&mut self, server_id: ServerId) {
        let Some(held) = self.servers.get_mut(&server_id) else {
            return;
        };
        if matches!(held.told, Some(Told::NotStated { .. })) {
            held.read = Read::Wanted;
        }
    }

    /// The images that left the cache since the last call.
    pub fn take_dropped(&mut self) -> Vec<Arc<RenderImage>> {
        std::mem::take(&mut self.dropped)
    }

    /// The image of the logo of a server, when the window holds it.
    pub fn image(&self, server_id: ServerId) -> Option<Arc<RenderImage>> {
        self.servers.get(&server_id)?.image()
    }

    /// True when the server has a logo, as far as this device knows: an event stated one,
    /// or no event stated the logo and the server book names one or a read found one.
    pub fn exists(&self, server_id: ServerId) -> bool {
        self.servers.get(&server_id).is_some_and(Held::exists)
    }

    /// True while a request for the logo of a server is on its way.
    pub fn on_way(&self, server_id: ServerId) -> bool {
        self.servers
            .get(&server_id)
            .is_some_and(|held| held.on_way.is_some())
    }

    /// The logos as the view takes them.
    pub fn shown(&self) -> ShownLogos {
        ShownLogos {
            servers: self
                .servers
                .iter()
                .filter(|(_, held)| held.exists())
                .map(|(server_id, held)| {
                    (
                        *server_id,
                        ShownLogo {
                            exists: true,
                            image: held.image(),
                        },
                    )
                })
                .collect(),
        }
    }

    /// Holds a logo as if an event stated it and its image was read. A test of the
    /// derivation and the screen tool use it.
    pub fn hold(&mut self, server_id: ServerId, logo: Logo) {
        let held = self.servers.entry(server_id).or_default();
        held.told = Some(Told::Present(Some(logo.hash)));
        held.read = Read::Settled;
        held.none_found = false;
        let before = held.image.replace(logo);
        self.dropped.extend(before.map(|logo| logo.image));
    }
}

/// What the public route gave. A logo is a file of an unknown sender (ADR 0714): bytes
/// that are no image are not used and not asked for again. The hash is the one that the
/// client computed of the bytes.
pub fn logo_of(read: Result<Option<AvatarImage>, ApiError>) -> Answer<Option<Logo>> {
    match Answer::of(read) {
        Answer::Read(None) => Answer::Read(None),
        Answer::Read(Some(read)) => {
            let Some(hash) = PictureHash::parse(&read.hash) else {
                return Answer::Final;
            };
            decode(&read.bytes, AVATAR_SIDE).map_or(Answer::Final, |image| {
                Answer::Read(Some(Logo { hash, image }))
            })
        }
        Answer::Final => Answer::Final,
        Answer::None => Answer::None,
    }
}

impl AppState {
    /// Before the view is derived: the cache ends with its server, takes what the state of
    /// each server and the server book tell of the logo, and the reads that follow start.
    pub(super) fn keep_logos(&mut self, cx: &mut Context<Self>) {
        // The servers of a screen of the screen tool are not servers of the client: their
        // logos are the ones that the tool gave.
        #[cfg(feature = "test-support")]
        if self.fixture.is_some() {
            self.show_logos(cx);
            return;
        }
        let named: HashSet<ServerId> = self
            .client
            .servers()
            .iter()
            .filter(|server| server.entry().logo_path.is_some())
            .map(ServerHandle::id)
            .collect();
        let servers = self.snapshots();
        self.logos
            .retain(&servers.iter().map(|server| server.id).collect());
        for server in &servers {
            let named = named.contains(&server.id);
            // A server without a shown state stated nothing.
            let told = match shown_model(server) {
                Some(model) => Told::of(&model.logo, named),
                None => Told::NotStated { named },
            };
            self.logos.want(server.id, told);
        }
        self.read_logos(true, cx);
    }

    /// Starts the request that the cache names for each server. The answer starts the
    /// next one. The image is decoded away from the window.
    fn read_logos(&mut self, again: bool, cx: &mut Context<Self>) {
        for server in self.client.servers() {
            let Some(ask) = self.logos.next(server.id(), again) else {
                continue;
            };
            let reading = server.logo();
            cx.spawn(async move |this, cx| {
                let read = reading.await;
                let answer = cx.background_spawn(async move { logo_of(read) }).await;
                let _ = this.update(cx, |this, cx| {
                    this.logos.answered(&ask, answer);
                    this.logos_changed(cx);
                });
            })
            .detach();
        }
        self.show_logos(cx);
    }

    fn logos_changed(&mut self, cx: &mut Context<Self>) {
        self.read_logos(false, cx);
        cx.notify();
    }

    /// Gives the view the logos that the cache holds now. An image that left the cache is
    /// no texture of a window any more.
    fn show_logos(&mut self, cx: &mut Context<Self>) {
        for image in self.logos.take_dropped() {
            crate::images::release(image, cx);
        }
        let shown = self.logos.shown();
        if self.known.logos != shown {
            self.known.logos = shown;
        }
    }

    /// One change of the state of a server, for the logo of that server.
    pub(super) fn logos_noticed(
        &mut self,
        server_id: ServerId,
        notice: &Notice,
        cx: &mut Context<Self>,
    ) {
        match notice {
            Notice::Event { event, .. } => {
                // The state of the server holds what the event stated. The cache takes it
                // from the state, which is the truth also when this reader is behind.
                if matches!(event.as_ref(), Event::ServerLogoChanged { .. }) {
                    self.keep_logos(cx);
                    cx.notify();
                }
            }
            Notice::Reloaded { .. } => self.logos_missed(server_id, cx),
            Notice::Ended { .. } => {}
        }
    }

    /// Events of a server were not told: a logo that no event stated is read again.
    pub(super) fn logos_missed(&mut self, server_id: ServerId, cx: &mut Context<Self>) {
        self.logos.stale(server_id);
        self.read_logos(true, cx);
    }

    /// True while a request for the logo of a server is on its way. A test waits with it
    /// for the end of a request.
    #[cfg(feature = "test-support")]
    pub fn logo_on_way(&self, server_id: ServerId) -> bool {
        self.logos.on_way(server_id)
    }

    /// Draws this image as the logo of a server of a screen. The screen tool uses it,
    /// after the servers of the screen are shown.
    #[cfg(feature = "test-support")]
    pub fn show_logo_fixture(&mut self, server_id: ServerId, bytes: &[u8], cx: &mut Context<Self>) {
        if let Some(image) = decode(bytes, AVATAR_SIDE) {
            let hash = PictureHash::of(bytes);
            self.logos.hold(server_id, Logo { hash, image });
        }
        self.changed(cx);
    }
}

#[cfg(test)]
mod tests;
