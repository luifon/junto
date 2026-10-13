//! The people of a server as the window shows them (ADR 0783): the name and the avatar of
//! each account that the window shows, read from the profile route, and the image of each
//! avatar, read from the avatar route and decoded away from the window.
//!
//! [`People`] is the cache. It is plain data with every rule of the reads, so the rules are
//! tested without a window and without a server. The state of the application starts the
//! requests that the cache names and gives it their answers.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::hash::Hash;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{AppContext as _, Context, RenderImage};
use junto_client_core::ServerHandle;
use junto_client_core::api::{ApiError, AvatarImage, ErrorCategory};
use junto_client_core::book::ProfileSync;
use junto_client_core::flow::FlowError;
use junto_client_core::flow::access::AccessState;
use junto_client_core::live::Notice;
use junto_client_core::profile::{self, InvalidAvatar, ProfileStore};
use junto_client_core::protocol::account::AccountProfile;
use junto_client_core::protocol::call::CallPlace;
use junto_client_core::protocol::events::Event;
use junto_client_core::protocol::ids::{AccountId, ServerId};
use junto_media::calls::CallTarget;

use super::AppState;
use super::admin::view::people::MemberList;
use super::admin::view::{AdminView, Listed, Scope, ServerPage};
use super::feed::{Feed, FeedRow, MessageRow, Quote};
use super::view::{Content, Members, PinsState, ServerSnapshot, ShellView, shown_model};
use crate::text;

/// The profiles that the window holds of one server.
pub const MOST_PROFILES: usize = 512;
/// The decoded images of avatars that the window holds of one server.
pub const MOST_IMAGES: usize = 256;
/// The requests for profiles and images of one server that are on their way at one time.
pub const MOST_REQUESTS: usize = 4;
/// How often a profile or an image is asked for when no answer arrives.
pub const TRIES: u8 = 3;
/// The side in pixels of a decoded image. The largest avatar of the window has 51 design
/// pixels, which are 153 pixels of a display at 150 % and two pixels per point.
pub const PICTURE_SIDE: u32 = 192;
/// The largest side in pixels of an avatar that a server stores (ADR 0119). The decoder
/// refuses a larger image of another account, whatever the server states.
const AVATAR_SIDE: u32 = 1024;
/// The largest side in pixels of the picture of the preset that the settings show. The
/// preset does not read the size in pixels (ADR 0717).
const PRESET_SIDE: u32 = 4096;
/// The characters of a name of an account (ADR 0119).
const NAME_CHARACTERS: usize = 64;

/// The SHA-256 hash of the image of an avatar (ADR 0119).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PictureHash([u8; 32]);

impl PictureHash {
    /// The hash as a server writes it: 64 hexadecimal digits in lower case. Other text of
    /// a server is no hash.
    pub fn parse(text: &str) -> Option<Self> {
        let digits = text.as_bytes();
        if digits.len() != 64 {
            return None;
        }
        let digit = |digit: u8| match digit {
            b'0'..=b'9' => Some(digit - b'0'),
            b'a'..=b'f' => Some(digit - b'a' + 10),
            _ => None,
        };
        let mut hash = [0u8; 32];
        let (pairs, _) = digits.as_chunks::<2>();
        for (byte, [high, low]) in hash.iter_mut().zip(pairs) {
            *byte = digit(*high)? << 4 | digit(*low)?;
        }
        Some(Self(hash))
    }

    /// The hash of these bytes, computed on this device.
    pub fn of(image: &[u8]) -> Self {
        Self::parse(&profile::avatar_hash(image)).unwrap_or(Self([0; 32]))
    }
}

impl fmt::Display for PictureHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for PictureHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "PictureHash({self})")
    }
}

/// The picture of a person as the window shows it: the hash of the avatar that the
/// account has, and its image once the window holds it. Without the image the window
/// draws the generated avatar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Face {
    pub hash: PictureHash,
    pub image: Option<Arc<RenderImage>>,
}

/// The pictures of the people of one server.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServerFaces {
    /// The account of the session. Its avatar is the one that the state of the server
    /// tells, and not one of a profile that was read.
    own: Option<(AccountId, Option<Face>)>,
    others: Arc<HashMap<AccountId, Face>>,
}

impl ServerFaces {
    /// The picture of an account. Absent for an account without an avatar, and for an
    /// account whose profile was not read.
    pub fn face(&self, account_id: AccountId) -> Option<&Face> {
        match &self.own {
            Some((own, face)) if *own == account_id => face.as_ref(),
            _ => self.others.get(&account_id),
        }
    }

    /// The image of the avatar of an account, when the window holds it.
    pub fn image(&self, account_id: AccountId) -> Option<Arc<RenderImage>> {
        self.face(account_id).and_then(|face| face.image.clone())
    }
}

/// The pictures of the people that the window shows, by server.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Faces {
    servers: HashMap<ServerId, ServerFaces>,
}

impl Faces {
    /// The pictures of one server. A render function takes them once and asks for each
    /// person by the account.
    pub fn of(&self, server_id: ServerId) -> ServerFaces {
        self.servers.get(&server_id).cloned().unwrap_or_default()
    }

    pub fn face(&self, server_id: ServerId, account_id: AccountId) -> Option<&Face> {
        self.servers.get(&server_id)?.face(account_id)
    }
}

/// The people of one server that the window shows now.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Wanted {
    /// The account of the session, with the avatar that the state of the server tells.
    pub own: Option<(AccountId, Option<PictureHash>)>,
    /// The other accounts, the most visible first.
    pub accounts: Vec<AccountId>,
}

/// The answer to one request, as the cache counts it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer<T> {
    Read(T),
    /// The server refused, or what it sent cannot be used. It is not asked again.
    Final,
    /// No answer arrived, or the server limits the requests. It is asked again with a
    /// later change of the state, at most [`TRIES`] times.
    None,
}

impl<T> Answer<T> {
    pub fn of(read: Result<T, ApiError>) -> Self {
        match read {
            Ok(read) => Self::Read(read),
            Err(error)
                if error.is_unavailable() || error.category() == Some(ErrorCategory::Limited) =>
            {
                Self::None
            }
            Err(_) => Self::Final,
        }
    }
}

/// What a request reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Asked {
    Profile(AccountId),
    /// The image with this hash, from the avatar route of an account that has it.
    Picture {
        account_id: AccountId,
        hash: PictureHash,
    },
}

/// One request that the cache wants to be sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ask {
    pub server_id: ServerId,
    /// The number of the request. No other request of the cache has it, for any server
    /// or account, so an answer is taken only by the cache that asked.
    number: u64,
    pub what: Asked,
}

/// Where the read of one profile or one image stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Read {
    /// Not asked for yet, or to be read again because the state was replaced.
    Wanted,
    /// The request with this number is on its way. It is the `tries`-th request.
    Asking { tries: u8, number: u64 },
    /// No request is on its way, and this number of requests got no answer.
    Unanswered(u8),
    /// The value was read, or the read ended without one: it is not asked for again.
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry<T> {
    read: Read,
    value: Option<T>,
    /// The number of the change of the wanted people at which the entry was shown last.
    used: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Profile {
    /// Absent when the server sent no character that a name can hold.
    name: Option<String>,
    avatar: Option<PictureHash>,
}

impl Profile {
    /// A profile is text of another party: the name is cut to the length that a server
    /// allows and holds no control character, and a hash in another form is no avatar.
    fn of(profile: &AccountProfile) -> Self {
        let name: String = profile
            .name
            .chars()
            .filter(|character| !character.is_control())
            .take(NAME_CHARACTERS)
            .collect();
        Self {
            name: (!name.trim().is_empty()).then_some(name),
            avatar: profile
                .avatar
                .as_ref()
                .and_then(|avatar| PictureHash::parse(&avatar.hash)),
        }
    }
}

/// Every entry is shown now or is being read: nothing gives way.
struct Full;

/// Makes room for one more entry. The entry that was shown longest ago gives way; an
/// entry that is shown now, or whose request is on its way, stays.
fn make_room<K: Copy + Eq + Hash, T>(
    held: &mut HashMap<K, Entry<T>>,
    most: usize,
    now: u64,
) -> Result<Option<T>, Full> {
    if held.len() < most {
        return Ok(None);
    }
    let oldest = held
        .iter()
        .filter(|(_, entry)| entry.used < now && !matches!(entry.read, Read::Asking { .. }))
        .min_by_key(|(_, entry)| entry.used)
        .map(|(key, _)| *key)
        .ok_or(Full)?;
    Ok(held.remove(&oldest).and_then(|entry| entry.value))
}

/// What the cache holds of one server.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Held {
    /// The account of the session that the entries were read for.
    owner: Option<AccountId>,
    profiles: HashMap<AccountId, Entry<Profile>>,
    images: HashMap<PictureHash, Entry<Arc<RenderImage>>>,
    wanted: Vec<AccountId>,
    own_picture: Option<PictureHash>,
    /// The numbers of the requests that are on their way.
    on_way: HashSet<u64>,
    /// Counts the changes of the wanted people.
    clock: u64,
    faces: Arc<HashMap<AccountId, Face>>,
}

impl Held {
    /// After every change: gives the wanted people their entries within the bounds, and
    /// derives the pictures.
    fn settle(&mut self, dropped: &mut Vec<Arc<RenderImage>>) {
        let now = self.clock;
        for account_id in &self.wanted {
            if let Some(entry) = self.profiles.get_mut(account_id) {
                entry.used = now;
                continue;
            }
            if make_room(&mut self.profiles, MOST_PROFILES, now).is_err() {
                break;
            }
            self.profiles.insert(
                *account_id,
                Entry {
                    read: Read::Wanted,
                    value: None,
                    used: now,
                },
            );
        }
        let hashes: Vec<PictureHash> = self
            .own_picture
            .into_iter()
            .chain(
                self.wanted
                    .iter()
                    .filter_map(|account_id| self.profiles.get(account_id)?.value.as_ref()?.avatar),
            )
            .collect();
        for hash in hashes {
            if let Some(entry) = self.images.get_mut(&hash) {
                entry.used = now;
                continue;
            }
            match make_room(&mut self.images, MOST_IMAGES, now) {
                Ok(gone) => dropped.extend(gone),
                Err(Full) => break,
            }
            self.images.insert(
                hash,
                Entry {
                    read: Read::Wanted,
                    value: None,
                    used: now,
                },
            );
        }
        let faces: HashMap<AccountId, Face> = self
            .profiles
            .iter()
            .filter_map(|(account_id, entry)| {
                let hash = entry.value.as_ref()?.avatar?;
                Some((
                    *account_id,
                    Face {
                        hash,
                        image: self.image(hash),
                    },
                ))
            })
            .collect();
        if *self.faces != faces {
            self.faces = Arc::new(faces);
        }
    }

    fn image(&self, hash: PictureHash) -> Option<Arc<RenderImage>> {
        self.images.get(&hash)?.value.clone()
    }

    fn free(&self, asks: &[Ask]) -> bool {
        self.on_way.len() + asks.len() < MOST_REQUESTS
    }

    /// True when `ask` is a request of this cache that is on its way: its answer is
    /// taken, and its place is free. The cache of another account of the session, and
    /// the cache of a server that was removed and added again, did not ask for it.
    fn takes(&mut self, ask: &Ask) -> bool {
        self.on_way.remove(&ask.number)
    }

    fn ask_picture(
        &mut self,
        server_id: ServerId,
        account_id: AccountId,
        hash: PictureHash,
        again: bool,
        asked: &mut u64,
        asks: &mut Vec<Ask>,
    ) {
        let Some(entry) = self.images.get_mut(&hash) else {
            return;
        };
        if let Some(tries) = entry.read.next(again) {
            *asked += 1;
            entry.read = Read::Asking {
                tries,
                number: *asked,
            };
            asks.push(Ask {
                server_id,
                number: *asked,
                what: Asked::Picture { account_id, hash },
            });
        }
    }
}

/// The profiles and the images of avatars that the window holds, by server.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct People {
    servers: HashMap<ServerId, Held>,
    /// The images that left the cache and that the window can still hold as textures.
    dropped: Vec<Arc<RenderImage>>,
    /// The number of the last request. It counts the requests of every server while the
    /// application runs, also across the removal of a server.
    asked: u64,
}

impl People {
    /// Tells the account of the session of a server. With another account than the one
    /// that the entries were read for, the cache of the server starts empty: a profile is
    /// readable for one account and not for another (ADR 0119).
    pub fn belongs(&mut self, server_id: ServerId, account: Option<AccountId>) {
        let held = self.servers.entry(server_id).or_default();
        let Some(account) = account else {
            return;
        };
        if held.owner == Some(account) {
            return;
        }
        self.dropped
            .extend(held.images.drain().filter_map(|(_, entry)| entry.value));
        *held = Held {
            owner: Some(account),
            ..Held::default()
        };
    }

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
                self.dropped
                    .extend(held.images.into_values().filter_map(|entry| entry.value));
            }
        }
    }

    /// Takes the people of a server that the window shows now. At most [`MOST_PROFILES`]
    /// of them get a profile: the first of the list.
    pub fn want(&mut self, server_id: ServerId, wanted: &Wanted) {
        let Some(held) = self.servers.get_mut(&server_id) else {
            return;
        };
        let owner = held.owner;
        let mut seen = HashSet::new();
        held.clock += 1;
        held.wanted = wanted
            .accounts
            .iter()
            .copied()
            .filter(|account_id| Some(*account_id) != owner && seen.insert(*account_id))
            .take(MOST_PROFILES)
            .collect();
        held.own_picture = wanted
            .own
            .filter(|(account_id, _)| Some(*account_id) == owner)
            .and_then(|(_, picture)| picture);
        held.settle(&mut self.dropped);
    }

    /// Takes the people that the window shows now, for every server of the cache. A
    /// server that the list does not name shows nobody.
    pub fn want_all(&mut self, wanted: &HashMap<ServerId, Wanted>) {
        let nobody = Wanted::default();
        let servers: Vec<ServerId> = self.servers.keys().copied().collect();
        for server_id in servers {
            self.want(server_id, wanted.get(&server_id).unwrap_or(&nobody));
        }
    }

    /// The requests that start now: for the most visible people first, and at most
    /// [`MOST_REQUESTS`] on their way for one server. `again` is true after a change of
    /// the state of the application: then a request that got no answer is sent again.
    pub fn next(&mut self, server_id: ServerId, again: bool) -> Vec<Ask> {
        let mut asks = Vec::new();
        let Some(held) = self.servers.get_mut(&server_id) else {
            return asks;
        };
        let Some(owner) = held.owner else {
            return asks;
        };
        let asked = &mut self.asked;
        if let Some(hash) = held.own_picture
            && held.free(&asks)
        {
            held.ask_picture(server_id, owner, hash, again, asked, &mut asks);
        }
        for account_id in held.wanted.clone() {
            if !held.free(&asks) {
                break;
            }
            let Some(entry) = held.profiles.get_mut(&account_id) else {
                continue;
            };
            if let Some(tries) = entry.read.next(again) {
                *asked += 1;
                entry.read = Read::Asking {
                    tries,
                    number: *asked,
                };
                asks.push(Ask {
                    server_id,
                    number: *asked,
                    what: Asked::Profile(account_id),
                });
                continue;
            }
            if let Some(hash) = entry.value.as_ref().and_then(|profile| profile.avatar) {
                held.ask_picture(server_id, account_id, hash, again, asked, &mut asks);
            }
        }
        held.on_way.extend(asks.iter().map(|ask| ask.number));
        asks
    }

    pub fn profile_answered(&mut self, ask: &Ask, answer: Answer<AccountProfile>) {
        let Asked::Profile(account_id) = ask.what else {
            return;
        };
        let Some(held) = self.servers.get_mut(&ask.server_id) else {
            return;
        };
        if !held.takes(ask) {
            return;
        }
        // An event can have told the profile while the request was on its way, and the
        // state of the server can have been replaced: then the entry waits for no answer,
        // or for the answer to a later request.
        if let Some(entry) = held.profiles.get_mut(&account_id)
            && let Read::Asking { tries, number } = entry.read
            && number == ask.number
        {
            match answer {
                Answer::Read(profile) if profile.id == account_id => {
                    entry.value = Some(Profile::of(&profile));
                    entry.read = Read::Settled;
                }
                Answer::Read(_) | Answer::Final => {
                    entry.value = None;
                    entry.read = Read::Settled;
                }
                Answer::None => entry.read = Read::Unanswered(tries),
            }
        }
        held.settle(&mut self.dropped);
    }

    pub fn picture_answered(&mut self, ask: &Ask, answer: Answer<Arc<RenderImage>>) {
        let Asked::Picture { hash, .. } = ask.what else {
            return;
        };
        let Some(held) = self.servers.get_mut(&ask.server_id) else {
            return;
        };
        if !held.takes(ask) {
            return;
        }
        if let Some(entry) = held.images.get_mut(&hash)
            && let Read::Asking { tries, number } = entry.read
            && number == ask.number
        {
            match answer {
                Answer::Read(image) => {
                    entry.value = Some(image);
                    entry.read = Read::Settled;
                }
                Answer::Final => entry.read = Read::Settled,
                Answer::None => entry.read = Read::Unanswered(tries),
            }
        }
        held.settle(&mut self.dropped);
    }

    /// The event `account_profile_updated`: the profile of an account that the cache holds
    /// is replaced. True when something that the window shows changed.
    pub fn profile_updated(&mut self, server_id: ServerId, profile: &AccountProfile) -> bool {
        let Some(held) = self.servers.get_mut(&server_id) else {
            return false;
        };
        let Some(entry) = held.profiles.get_mut(&profile.id) else {
            return false;
        };
        entry.value = Some(Profile::of(profile));
        entry.read = Read::Settled;
        held.settle(&mut self.dropped);
        true
    }

    /// The state of a server was replaced, or events were missed: every profile can have
    /// changed without an event. Each is read again when it is shown; what was read
    /// before is shown until the answer arrives. A request that is on its way started
    /// before this moment: its answer is not used, and it keeps its place among the
    /// requests until it ends. An image belongs to its hash and stays.
    pub fn stale(&mut self, server_id: ServerId) {
        let Some(held) = self.servers.get_mut(&server_id) else {
            return;
        };
        for entry in held.profiles.values_mut() {
            entry.read = Read::Wanted;
        }
        for entry in held.images.values_mut() {
            if matches!(entry.read, Read::Unanswered(_)) {
                entry.read = Read::Wanted;
            }
        }
    }

    /// The images that left the cache since the last call.
    pub fn take_dropped(&mut self) -> Vec<Arc<RenderImage>> {
        std::mem::take(&mut self.dropped)
    }

    /// The name of an account as its profile gave it.
    pub fn name(&self, server_id: ServerId, account_id: AccountId) -> Option<String> {
        self.servers
            .get(&server_id)?
            .profiles
            .get(&account_id)?
            .value
            .as_ref()?
            .name
            .clone()
    }

    /// The names that were read of the accounts of one server.
    pub fn names(&self, server_id: ServerId) -> impl Iterator<Item = (AccountId, &str)> {
        self.servers
            .get(&server_id)
            .into_iter()
            .flat_map(|held| held.profiles.iter())
            .filter_map(|(account_id, entry)| {
                Some((*account_id, entry.value.as_ref()?.name.as_deref()?))
            })
    }

    /// The pictures of the accounts of one server whose profile names an avatar.
    pub fn faces(&self, server_id: ServerId) -> Arc<HashMap<AccountId, Face>> {
        self.servers
            .get(&server_id)
            .map(|held| held.faces.clone())
            .unwrap_or_default()
    }

    /// The image with this hash, when the window holds it.
    pub fn image(&self, server_id: ServerId, hash: PictureHash) -> Option<Arc<RenderImage>> {
        self.servers.get(&server_id)?.image(hash)
    }

    /// The number of requests of a server that are on their way.
    pub fn on_way(&self, server_id: ServerId) -> usize {
        self.servers
            .get(&server_id)
            .map_or(0, |held| held.on_way.len())
    }

    /// The number of requests in a row for the profile of an account that got no answer,
    /// while no request for it is on its way.
    pub fn unanswered(&self, server_id: ServerId, account_id: AccountId) -> Option<u8> {
        match self
            .servers
            .get(&server_id)?
            .profiles
            .get(&account_id)?
            .read
        {
            Read::Unanswered(tries) => Some(tries),
            Read::Wanted | Read::Asking { .. } | Read::Settled => None,
        }
    }

    /// Holds a profile as if it was read, and the image of its avatar. A test of the
    /// derivation and the screen tool use it.
    pub fn hold(
        &mut self,
        server_id: ServerId,
        account_id: AccountId,
        name: &str,
        picture: Option<(PictureHash, Option<Arc<RenderImage>>)>,
    ) {
        let held = self.servers.entry(server_id).or_default();
        let used = held.clock;
        held.profiles.insert(
            account_id,
            Entry {
                read: Read::Settled,
                value: Some(Profile {
                    name: Some(name.to_owned()),
                    avatar: picture.as_ref().map(|(hash, _)| *hash),
                }),
                used,
            },
        );
        if let Some((hash, image)) = picture {
            held.images.insert(
                hash,
                Entry {
                    read: Read::Settled,
                    value: image,
                    used,
                },
            );
        }
        held.settle(&mut self.dropped);
    }

    /// Holds the name of an account as if its profile was read.
    pub fn read_name(&mut self, server_id: ServerId, account_id: AccountId, name: &str) {
        self.hold(server_id, account_id, name, None);
    }
}

/// Decodes an image to a square of at most [`PICTURE_SIDE`] pixels: the middle of the
/// image, reduced. The decoder refuses an image with a side of more than `most_side`
/// pixels, whatever its sender states, and bounds the memory of the decoding.
pub fn decode(bytes: &[u8], most_side: u32) -> Option<Arc<RenderImage>> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(most_side);
    limits.max_image_height = Some(most_side);
    limits.max_alloc = Some(u64::from(most_side) * u64::from(most_side) * 8);
    reader.limits(limits);
    let picture = reader.decode().ok()?;
    let side = picture.width().min(picture.height());
    if side == 0 {
        return None;
    }
    let square = picture.crop_imm(
        (picture.width() - side) / 2,
        (picture.height() - side) / 2,
        side,
        side,
    );
    let square = if side > PICTURE_SIDE {
        square.resize_exact(
            PICTURE_SIDE,
            PICTURE_SIDE,
            image::imageops::FilterType::Triangle,
        )
    } else {
        square
    };
    let mut pixels = square.into_rgba8();
    // The window takes the colours in the order blue, green, red.
    for pixel in pixels.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(pixels)])))
}

/// What the avatar route gave for the image with the hash `wanted`. An avatar is a file
/// of an unknown sender (ADR 0714): bytes with another hash than the profile names, and
/// bytes that are no image, are not used and not asked for again.
pub fn picture_of(
    wanted: PictureHash,
    read: Result<AvatarImage, ApiError>,
) -> Answer<Arc<RenderImage>> {
    match Answer::of(read) {
        Answer::Read(image) => {
            if PictureHash::parse(&image.hash) != Some(wanted) {
                return Answer::Final;
            }
            decode(&image.bytes, AVATAR_SIDE).map_or(Answer::Final, Answer::Read)
        }
        Answer::Final => Answer::Final,
        Answer::None => Answer::None,
    }
}

/// The pictures of the people of every server whose content the window shows.
pub(super) fn faces(servers: &[ServerSnapshot], people: &People) -> Faces {
    Faces {
        servers: servers
            .iter()
            .filter_map(|server| {
                let account = &shown_model(server)?.account;
                let own = account
                    .avatar
                    .as_ref()
                    .and_then(|avatar| PictureHash::parse(&avatar.hash))
                    .map(|hash| Face {
                        hash,
                        image: people.image(server.id, hash),
                    });
                Some((
                    server.id,
                    ServerFaces {
                        own: Some((account.id, own)),
                        others: people.faces(server.id),
                    },
                ))
            })
            .collect(),
    }
}

/// The accounts that the rows of a message list name: the author of each message, of the
/// message that it answers, and of the last reply of its thread.
fn feed_people(feed: Option<&Feed>, people: &mut Vec<AccountId>) {
    let Some(feed) = feed else {
        return;
    };
    for row in feed.rows.iter() {
        match row {
            FeedRow::Message(row) => message_people(row, people),
            FeedRow::Waiting(row) => quote_people(row.reply.as_ref(), people),
            _ => {}
        }
    }
}

fn quote_people(quote: Option<&Quote>, people: &mut Vec<AccountId>) {
    people.extend(quote.and_then(|quote| quote.author_id));
}

fn message_people(row: &MessageRow, people: &mut Vec<AccountId>) {
    people.extend(row.message.author_id);
    quote_people(row.reply.as_ref(), people);
    people.extend(
        row.thread
            .as_ref()
            .and_then(|thread| thread.last.as_ref())
            .and_then(|last| last.author_id),
    );
}

/// The people that a derived view names, by server: the account of the session with the
/// avatar that the state of its server tells, and the other accounts in the order in
/// which the window shows them, from the channel column to the member list.
pub(super) fn wanted(servers: &[ServerSnapshot], view: &ShellView) -> HashMap<ServerId, Wanted> {
    let mut wanted: HashMap<ServerId, Wanted> = HashMap::new();
    let account = view.selected.as_ref().and_then(|selected| {
        servers
            .iter()
            .find(|server| server.id == selected.id)
            .and_then(shown_model)
            .filter(|_| !matches!(selected.content, Content::State))
            .map(|model| (selected.id, &model.account))
    });
    if let Some((server_id, account)) = account {
        let mut people = Vec::new();
        if let Some(workspace) = view.workspace() {
            let rows = workspace.sections.iter().flat_map(|section| &section.rows);
            people.extend(
                rows.filter_map(|row| row.voice.as_ref())
                    .flat_map(|voice| &voice.occupants)
                    .map(|occupant| occupant.account_id),
            );
            if let Some(call) = &workspace.call {
                people.extend(
                    call.stage
                        .iter()
                        .flat_map(|stage| &stage.shares)
                        .map(|share| share.account_id),
                );
            }
            feed_people(workspace.feed.as_ref(), &mut people);
            if let Some(thread) = &workspace.thread {
                if let Some(root) = &thread.root {
                    message_people(root, &mut people);
                }
                feed_people(thread.feed.as_ref(), &mut people);
            }
            if let Some(PinsState::Loaded(pinned)) = workspace.pins.as_ref().map(|pins| &pins.state)
            {
                for row in pinned {
                    message_people(row, &mut people);
                }
            }
            people.extend(workspace.directs.iter().map(|direct| direct.account_id));
            if let Members::Loaded(members) = &workspace.members {
                people.extend(members.iter().map(|member| member.account_id));
            }
        }
        wanted.insert(
            server_id,
            Wanted {
                own: Some((
                    account.id,
                    account
                        .avatar
                        .as_ref()
                        .and_then(|avatar| PictureHash::parse(&avatar.hash)),
                )),
                accounts: people,
            },
        );
    }
    // The people who invite the person into a call, each on the server of that call.
    for invited in &view.invitations {
        wanted
            .entry(invited.target.server_id)
            .or_default()
            .accounts
            .push(invited.caller);
    }
    // The people who ask to enter the call of this device, on the server of that call.
    if let Some(call) = &view.call
        && !view.entry_prompts.is_empty()
    {
        wanted.entry(call.server_id).or_default().accounts.splice(
            0..0,
            view.entry_prompts.iter().map(|prompt| prompt.account_id),
        );
    }
    wanted
}

/// Names the people of the call of this device. The window of the call and the strip of
/// the call show them, also when the main window shows another server or another workspace
/// of the server (ADR 0772). They are named first: the bound on the profiles of one server
/// keeps the people who are named first (ADR 0783).
///
/// `shown` is the view of the main window. `call_view` derives the view at the place of
/// the call, and is used only when `shown` does not hold that place.
pub(super) fn add_call_people(
    wanted: &mut HashMap<ServerId, Wanted>,
    shown: &ShellView,
    call: CallTarget,
    call_view: impl FnOnce() -> ShellView,
) {
    let on_server = shown.selected.as_ref().map(|selected| selected.id) == Some(call.server_id);
    let holds_place = on_server
        && match call.place {
            CallPlace::Channel { workspace_id, .. } => shown
                .workspace()
                .is_some_and(|workspace| workspace.id == workspace_id),
            CallPlace::Direct { .. } => true,
        };
    let other = (!holds_place).then(call_view);
    let view = other.as_ref().unwrap_or(shown);
    let people = match call.place {
        CallPlace::Channel { channel_id, .. } => view
            .workspace()
            .into_iter()
            .flat_map(|workspace| &workspace.sections)
            .flat_map(|section| &section.rows)
            .filter(|row| row.id == channel_id)
            .filter_map(|row| row.voice.as_ref())
            .flat_map(|voice| &voice.occupants)
            .map(|occupant| occupant.account_id)
            .collect(),
        CallPlace::Direct {
            low_account_id,
            high_account_id,
        } => vec![low_account_id, high_account_id],
    };
    let own = other
        .as_ref()
        .and_then(|view| view.wanted_people.get(&call.server_id))
        .and_then(|wanted| wanted.own);
    let held = wanted.entry(call.server_id).or_default();
    held.own = held.own.or(own);
    held.accounts.splice(0..0, people);
}

/// The accounts that a page of the administration lists.
fn admin_people(view: &AdminView) -> Vec<AccountId> {
    let Scope::Server { page, .. } = &view.scope else {
        return Vec::new();
    };
    match page {
        ServerPage::People(page) => match &page.accounts {
            Listed::Loaded(accounts) => accounts.iter().map(|account| account.id).collect(),
            _ => Vec::new(),
        },
        ServerPage::Members(page) => {
            let members = match &page.members {
                MemberList::Loaded(members) => members.as_slice(),
                MemberList::Loading | MemberList::Failed => &[],
            };
            let addable = match &page.addable {
                Some(Listed::Loaded(addable)) => addable.as_slice(),
                _ => &[],
            };
            members
                .iter()
                .map(|member| member.account_id)
                .chain(addable.iter().map(|account| account.account_id))
                .collect()
        }
        _ => Vec::new(),
    }
}

/// Why a file did not become the picture of the profile preset.
#[derive(Debug)]
enum PictureRefusal {
    TooLarge,
    Unsupported,
    /// The file could not be read.
    Unreadable,
    /// The data directory did not take the picture.
    Storage(String),
}

impl PictureRefusal {
    fn sentence(&self) -> String {
        match self {
            Self::TooLarge => text::people::PICTURE_TOO_LARGE.to_owned(),
            Self::Unsupported => text::people::PICTURE_UNSUPPORTED.to_owned(),
            Self::Unreadable => text::people::PICTURE_UNREADABLE.to_owned(),
            Self::Storage(reason) => text::flow_error(&FlowError::Storage(reason.clone())),
        }
    }
}

impl From<profile::ProfileError> for PictureRefusal {
    fn from(error: profile::ProfileError) -> Self {
        match error {
            profile::ProfileError::Avatar(InvalidAvatar::TooLarge) => Self::TooLarge,
            profile::ProfileError::Avatar(InvalidAvatar::Unsupported) => Self::Unsupported,
            other => Self::Storage(other.to_string()),
        }
    }
}

/// Reads a file of the person and stores it as the picture of the preset. Only a regular
/// file is read, and a file that is larger than an avatar can be is not read into memory.
fn store_picture(
    store: &ProfileStore,
    file: &Path,
) -> Result<Option<Arc<RenderImage>>, PictureRefusal> {
    let bytes = profile::read_avatar_file(file).map_err(|_| PictureRefusal::Unreadable)?;
    store.set_avatar(&bytes)?;
    Ok(decode(&bytes, PRESET_SIDE))
}

/// The picture of the profile preset, as the settings show it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PresetPicture {
    /// The preset has a picture.
    pub stored: bool,
    /// The picture as an image of the window. Absent without a picture, and for a stored
    /// picture that this build cannot decode.
    pub image: Option<Arc<RenderImage>>,
    /// A change of the picture is being written.
    pub saving: bool,
    /// Why the last change was not made.
    pub failed: Option<String>,
}

impl AppState {
    /// Before the view is derived: the cache of a server belongs to the account of its
    /// session, and ends with its server.
    pub(super) fn keep_people(&mut self, cx: &mut Context<Self>) {
        let servers = self.snapshots();
        self.known
            .people
            .retain(&servers.iter().map(|server| server.id).collect());
        for server in &servers {
            let account = match &server.access {
                AccessState::LoggedIn { account_id, .. } => Some(*account_id),
                _ => None,
            };
            self.known.people.belongs(server.id, account);
        }
        self.drop_people_images(cx);
        let refused: HashSet<ServerId> = self
            .client
            .servers()
            .iter()
            .filter(|server| server.profile_sync() == ProfileSync::Refused)
            .map(ServerHandle::id)
            .collect();
        // The servers of a screen of the screen tool are not servers of the client.
        #[cfg(feature = "test-support")]
        let refused = if self.fixture.is_some() {
            self.known.preset_refused.clone()
        } else {
            refused
        };
        self.known.preset_refused = refused;
    }

    /// After the view is derived: the profiles and the images of the people that the
    /// window shows are read.
    pub(super) fn follow_people(&mut self, view: &ShellView, cx: &mut Context<Self>) {
        let mut wanted = view.wanted_people.clone();
        let call = self.media_calls().call.as_ref().map(|call| call.target);
        if let Some(call) = call {
            add_call_people(&mut wanted, view, call, || self.call_view());
        }
        if let Some(admin) = self.admin_view() {
            wanted
                .entry(admin.server_id)
                .or_default()
                .accounts
                .extend(admin_people(&admin));
        }
        self.known.people.want_all(&wanted);
        self.read_people(true, cx);
    }

    /// Starts the requests that the cache names. Each answer starts the next ones.
    fn read_people(&mut self, again: bool, cx: &mut Context<Self>) {
        for server in self.client.servers() {
            for ask in self.known.people.next(server.id(), again) {
                let api = server.api();
                cx.spawn(async move |this, cx| match ask.what {
                    Asked::Profile(account_id) => {
                        let answer = Answer::of(api.profile(account_id).await);
                        let _ = this.update(cx, |this, cx| {
                            this.known.people.profile_answered(&ask, answer);
                            this.people_changed(cx);
                        });
                    }
                    Asked::Picture { account_id, hash } => {
                        let read = api.avatar(account_id).await;
                        let answer = cx
                            .background_spawn(async move { picture_of(hash, read) })
                            .await;
                        let _ = this.update(cx, |this, cx| {
                            this.known.people.picture_answered(&ask, answer);
                            this.people_changed(cx);
                        });
                    }
                })
                .detach();
            }
        }
        self.drop_people_images(cx);
    }

    fn people_changed(&mut self, cx: &mut Context<Self>) {
        self.read_people(false, cx);
        cx.notify();
    }

    /// An image that left the cache is no texture of a window any more.
    fn drop_people_images(&mut self, cx: &mut Context<Self>) {
        for image in self.known.people.take_dropped() {
            cx.drop_image(image, None);
        }
    }

    /// One change of the state of a server, for the people of that server.
    pub(super) fn people_noticed(
        &mut self,
        server_id: ServerId,
        notice: &Notice,
        cx: &mut Context<Self>,
    ) {
        match notice {
            Notice::Event { event, .. } => {
                if let Event::AccountProfileUpdated { profile } = event.as_ref()
                    && self.known.people.profile_updated(server_id, profile)
                {
                    self.people_changed(cx);
                }
            }
            Notice::Reloaded { .. } => self.people_missed(server_id, cx),
            Notice::Ended { .. } => {}
        }
    }

    /// Events of a server were not told: each profile is read again.
    pub(super) fn people_missed(&mut self, server_id: ServerId, cx: &mut Context<Self>) {
        self.known.people.stale(server_id);
        self.read_people(true, cx);
    }

    /// The picture of the profile preset.
    pub fn preset_picture(&self) -> &PresetPicture {
        &self.preset_picture
    }

    fn show_preset_picture(&mut self, cx: &mut Context<Self>) {
        self.known
            .preset_picture
            .clone_from(&self.preset_picture.image);
        cx.notify();
    }

    /// Reads the picture of the preset at the start, away from the window.
    pub(super) fn load_preset_picture(&mut self, cx: &mut Context<Self>) {
        let store = self.client.profile().clone();
        let reading = cx.background_spawn(async move {
            match store.avatar() {
                Ok(Some((_, bytes))) => Some(decode(&bytes, PRESET_SIDE)),
                Ok(None) => None,
                Err(error) => {
                    tracing::warn!(%error, "the picture of the profile was not read");
                    None
                }
            }
        });
        cx.spawn(async move |this, cx| {
            let read = reading.await;
            let _ = this.update(cx, |this, cx| {
                // A change that the person made meanwhile is the later truth.
                if this.preset_picture.saving || this.preset_picture.stored {
                    return;
                }
                this.preset_picture.stored = read.is_some();
                this.preset_picture.image = read.flatten();
                this.show_preset_picture(cx);
            });
        })
        .detach();
    }

    /// Stores a file as the picture of the profile preset. The file is read, checked and
    /// written on the background queue of the writes; the result is in
    /// [`AppState::preset_picture`].
    pub fn set_preset_picture(&mut self, file: PathBuf, cx: &mut Context<Self>) {
        if self.preset_picture.saving {
            return;
        }
        self.preset_picture.saving = true;
        self.preset_picture.failed = None;
        let store = self.client.profile().clone();
        Self::queue_write(
            cx,
            move || store_picture(&store, &file),
            |this, stored, cx| {
                this.preset_picture.saving = false;
                match stored {
                    Ok(image) => {
                        if let Some(before) = this.preset_picture.image.take() {
                            cx.drop_image(before, None);
                        }
                        this.preset_picture.stored = true;
                        this.preset_picture.image = image;
                    }
                    Err(refusal) => this.preset_picture.failed = Some(refusal.sentence()),
                }
                this.show_preset_picture(cx);
            },
        );
        cx.notify();
    }

    /// Removes the picture of the profile preset.
    pub fn remove_preset_picture(&mut self, cx: &mut Context<Self>) {
        if self.preset_picture.saving {
            return;
        }
        self.preset_picture.saving = true;
        self.preset_picture.failed = None;
        let store = self.client.profile().clone();
        Self::queue_write(
            cx,
            move || store.clear_avatar(),
            |this, cleared, cx| {
                this.preset_picture.saving = false;
                match cleared {
                    Ok(_) => {
                        if let Some(before) = this.preset_picture.image.take() {
                            cx.drop_image(before, None);
                        }
                        this.preset_picture.stored = false;
                    }
                    Err(error) => {
                        this.preset_picture.failed = Some(PictureRefusal::from(error).sentence());
                    }
                }
                this.show_preset_picture(cx);
            },
        );
        cx.notify();
    }

    /// The number of requests for profiles and images of a server that are on their way.
    /// A test waits with it for the end of a request.
    #[cfg(feature = "test-support")]
    pub fn people_on_way(&self, server_id: ServerId) -> usize {
        self.known.people.on_way(server_id)
    }

    /// The number of requests in a row for the profile of an account that got no answer.
    /// A test reads it.
    #[cfg(feature = "test-support")]
    pub fn profile_unanswered(&self, server_id: ServerId, account_id: AccountId) -> Option<u8> {
        self.known.people.unanswered(server_id, account_id)
    }

    /// Draws these profiles and images in place of what the servers gave. The screen tool
    /// uses it, after the servers of the screen are shown.
    #[cfg(feature = "test-support")]
    pub fn show_people_fixture(
        &mut self,
        server_id: ServerId,
        people: Vec<(AccountId, String, Option<Vec<u8>>)>,
        cx: &mut Context<Self>,
    ) {
        for (account_id, name, picture) in people {
            let picture =
                picture.map(|bytes| (PictureHash::of(&bytes), decode(&bytes, AVATAR_SIDE)));
            self.known
                .people
                .hold(server_id, account_id, &name, picture);
        }
        self.changed(cx);
    }

    /// Draws these servers of a screen as servers that refused the picture of the preset.
    /// The screen tool uses it.
    #[cfg(feature = "test-support")]
    pub fn show_preset_refused_fixture(
        &mut self,
        servers: HashSet<ServerId>,
        cx: &mut Context<Self>,
    ) {
        self.known.preset_refused = servers;
        self.changed(cx);
    }

    /// Draws this picture as the picture of the profile preset. The screen tool uses it.
    #[cfg(feature = "test-support")]
    pub fn show_preset_picture_fixture(&mut self, bytes: &[u8], cx: &mut Context<Self>) {
        self.preset_picture.stored = true;
        self.preset_picture.image = decode(bytes, PRESET_SIDE);
        self.show_preset_picture(cx);
    }
}

#[cfg(test)]
mod tests;
