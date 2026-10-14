//! The pictures of messages that the window holds decoded (ADR 0740). A picture belongs
//! to the saved server and the account that it was read for, and to one file with one
//! hash (ADR 0745): an identifier that a second server copies, and the next account of a
//! server, find nothing that was read before.

use std::collections::{HashMap, VecDeque};

use junto_client_core::Owner;
use junto_client_core::protocol::ids::AttachmentId;
use junto_client_core::protocol::message::Attachment;

/// How many pictures of messages the window holds.
pub(crate) const MOST_PICTURES: usize = 48;
/// How many pictures of one owner are read at one time. A picture that finds no free read
/// is asked for again when the window is drawn after a read has ended. The reads of one
/// server do not count for another, so a server that answers slowly delays its own
/// pictures only.
pub(crate) const MOST_READS: usize = 4;

/// What a held picture is the picture of.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct PictureKey {
    pub owner: Owner,
    pub attachment_id: AttachmentId,
    pub sha256: String,
}

impl PictureKey {
    pub(crate) fn of(owner: Owner, attachment: &Attachment) -> Self {
        Self {
            owner,
            attachment_id: attachment.id,
            sha256: attachment.sha256.clone(),
        }
    }
}

/// A picture of a message in the window.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Picture<Image> {
    /// The read with this number is on its way.
    Reading(u64),
    Ready(Image),
    Failed,
}

struct Held<Image> {
    picture: Picture<Image>,
    /// The number of the last drawing of the window that asked for the picture.
    used: u64,
}

/// The pictures that the window holds, the oldest first.
pub(crate) struct Pictures<Image> {
    held: HashMap<PictureKey, Held<Image>>,
    order: VecDeque<PictureKey>,
    /// The number of the drawing of the window that runs, or that ran last.
    drawing: u64,
    /// The number of the last read.
    reads: u64,
    /// The reads of each owner that have not ended, also those whose picture is not held
    /// any more.
    flying: HashMap<Owner, usize>,
    /// The images that left the list and that the window has not released yet.
    dropped: Vec<Image>,
}

impl<Image> Default for Pictures<Image> {
    fn default() -> Self {
        Self {
            held: HashMap::new(),
            order: VecDeque::new(),
            drawing: 0,
            reads: 0,
            flying: HashMap::new(),
            dropped: Vec::new(),
        }
    }
}

impl<Image> Pictures<Image> {
    pub(crate) fn get(&self, key: &PictureKey) -> Option<&Picture<Image>> {
        self.held.get(key).map(|held| &held.picture)
    }

    /// A drawing of the window starts. The pictures that it asks for are in use.
    pub(crate) fn drawing_starts(&mut self) {
        self.drawing += 1;
    }

    /// A drawing of the window asks for a picture. True when the picture is held: the
    /// drawing uses it.
    pub(crate) fn touch(&mut self, key: &PictureKey) -> bool {
        match self.held.get_mut(key) {
            Some(held) => {
                held.used = self.drawing;
                true
            }
            None => false,
        }
    }

    /// Starts to hold a picture that the last drawing asked for and that is not held, and
    /// returns the number of the read that the caller starts for it. The caller asks
    /// after the drawing has ended, when every picture that it uses is known.
    ///
    /// At most [`MOST_READS`] reads of one owner run. A new picture takes the place of the
    /// oldest one that is not being read and that the last drawing did not ask for. So a
    /// read that runs is never forgotten and never started a second time, and the
    /// pictures of one view do not take the places of each other: a view with more
    /// pictures than the window holds leaves the others unread.
    pub(crate) fn start(&mut self, key: &PictureKey) -> Option<u64> {
        if self.held.contains_key(key) {
            return None;
        }
        if self.flying.get(&key.owner).copied().unwrap_or(0) >= MOST_READS {
            return None;
        }
        if self.order.len() >= MOST_PICTURES {
            let free = |held: &Held<Image>| {
                !matches!(held.picture, Picture::Reading(_)) && held.used < self.drawing
            };
            let oldest = self
                .order
                .iter()
                .position(|key| self.held.get(key).is_none_or(free))?;
            if let Some(gone) = self.order.remove(oldest) {
                self.forget(&gone);
            }
        }
        self.reads += 1;
        *self.flying.entry(key.owner).or_default() += 1;
        let held = Held {
            picture: Picture::Reading(self.reads),
            used: self.drawing,
        };
        self.held.insert(key.clone(), held);
        self.order.push_back(key.clone());
        Some(self.reads)
    }

    /// Takes the end of the read with this number: its image, or none when the picture
    /// was not read or not decoded. The image is held only when the picture still waits
    /// for exactly this read. True when a held picture changed.
    pub(crate) fn settle(&mut self, key: &PictureKey, number: u64, image: Option<Image>) -> bool {
        if let Some(flying) = self.flying.get_mut(&key.owner) {
            *flying = flying.saturating_sub(1);
            if *flying == 0 {
                self.flying.remove(&key.owner);
            }
        }
        match self.held.get_mut(key) {
            Some(held) if matches!(held.picture, Picture::Reading(of) if of == number) => {
                held.picture = image.map_or(Picture::Failed, Picture::Ready);
                true
            }
            _ => {
                self.dropped.extend(image);
                false
            }
        }
    }

    /// Keeps the pictures of the owners that `current` names, and drops every other one:
    /// the pictures of an account that the device has no more, and of a server that was
    /// removed.
    pub(crate) fn keep(&mut self, current: impl Fn(&Owner) -> bool) {
        let gone: Vec<PictureKey> = self
            .order
            .iter()
            .filter(|key| !current(&key.owner))
            .cloned()
            .collect();
        for key in gone {
            self.order.retain(|held| *held != key);
            self.forget(&key);
        }
    }

    fn forget(&mut self, key: &PictureKey) {
        if let Some(Held {
            picture: Picture::Ready(image),
            ..
        }) = self.held.remove(key)
        {
            self.dropped.push(image);
        }
    }

    /// The images that are not held any more. The window releases them.
    pub(crate) fn take_dropped(&mut self) -> Vec<Image> {
        std::mem::take(&mut self.dropped)
    }

    /// The owners of the held pictures of this file that are read.
    #[cfg(feature = "test-support")]
    pub(crate) fn ready_of(&self, attachment_id: AttachmentId) -> Vec<Owner> {
        self.held
            .iter()
            .filter(|(key, held)| {
                key.attachment_id == attachment_id && matches!(held.picture, Picture::Ready(_))
            })
            .map(|(key, _)| key.owner)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use junto_client_core::EntryKey;
    use junto_client_core::protocol::ids::{AccountId, ServerId};

    use super::*;

    fn owner() -> Owner {
        Owner {
            entry: EntryKey {
                server_id: ServerId::generate(),
                generation: 1,
            },
            account_id: AccountId::generate(),
        }
    }

    fn key(owner: Owner, attachment_id: AttachmentId, sha256: &str) -> PictureKey {
        PictureKey {
            owner,
            attachment_id,
            sha256: sha256.to_owned(),
        }
    }

    /// One drawing of the window that asks for these pictures, and the reads that start
    /// after it.
    fn draw<Image>(pictures: &mut Pictures<Image>, asked: &[PictureKey]) -> Vec<(PictureKey, u64)> {
        pictures.drawing_starts();
        let waiting: Vec<&PictureKey> = asked.iter().filter(|key| !pictures.touch(key)).collect();
        waiting
            .into_iter()
            .filter_map(|key| pictures.start(key).map(|number| (key.clone(), number)))
            .collect()
    }

    /// Asks for one picture in a drawing and takes the number of its read.
    fn read<Image>(pictures: &mut Pictures<Image>, key: &PictureKey) -> u64 {
        let started = draw(pictures, std::slice::from_ref(key));
        assert_eq!(started.len(), 1, "the read starts");
        started[0].1
    }

    #[test]
    fn message_pictures_are_scoped_to_server_account_and_generation() {
        let mut pictures = Pictures::<&str>::default();
        let file = AttachmentId::generate();
        let first = owner();
        let of_first = key(first, file, "aa");
        let number = read(&mut pictures, &of_first);
        assert!(pictures.settle(&of_first, number, Some("the picture of the first server")));
        // The picture is held: it is not read a second time.
        assert!(draw(&mut pictures, std::slice::from_ref(&of_first)).is_empty());

        // A second server states a file with the same identifier. Its picture is read
        // from that server, and the picture of the first server is not drawn for it.
        let of_second = key(owner(), file, "bb");
        assert_eq!(pictures.get(&of_second), None);
        let number = read(&mut pictures, &of_second);
        assert_eq!(pictures.get(&of_second), Some(&Picture::Reading(number)));
        assert!(pictures.settle(&of_second, number, Some("the picture of the second server")));

        // Another account of the first server, the entry of the first server after it was
        // removed and added again, and the same file with another hash: none finds the
        // picture that was read before.
        let mut account = first;
        account.account_id = AccountId::generate();
        let mut added_again = first;
        added_again.entry.generation += 1;
        for other in [
            key(account, file, "aa"),
            key(added_again, file, "aa"),
            key(first, file, "cc"),
        ] {
            assert_eq!(pictures.get(&other), None);
            let number = read(&mut pictures, &other);
            assert!(pictures.settle(&other, number, None));
            assert_eq!(pictures.get(&other), Some(&Picture::Failed));
        }
        assert_eq!(
            pictures.get(&of_first),
            Some(&Picture::Ready("the picture of the first server"))
        );
        assert_eq!(
            pictures.get(&of_second),
            Some(&Picture::Ready("the picture of the second server"))
        );
    }

    #[test]
    fn the_pictures_of_an_account_that_the_device_has_no_more_are_dropped() {
        let mut pictures = Pictures::<&str>::default();
        let (stays, leaves) = (owner(), owner());
        let file = AttachmentId::generate();
        let (kept, gone) = (key(stays, file, "aa"), key(leaves, file, "aa"));
        for (key, image) in [(&kept, "kept"), (&gone, "gone")] {
            let number = read(&mut pictures, key);
            pictures.settle(key, number, Some(image));
        }
        // A read of the account that leaves is on its way.
        let late = key(leaves, AttachmentId::generate(), "dd");
        let number = read(&mut pictures, &late);

        pictures.keep(|owner| *owner == stays);
        assert_eq!(pictures.get(&gone), None);
        assert_eq!(pictures.get(&late), None);
        assert_eq!(pictures.get(&kept), Some(&Picture::Ready("kept")));
        assert_eq!(pictures.take_dropped(), ["gone"]);

        // The answer of that read arrives afterwards: nothing holds it.
        assert!(!pictures.settle(&late, number, Some("late")));
        assert_eq!(pictures.get(&late), None);
        assert_eq!(pictures.take_dropped(), ["late"]);
    }

    #[test]
    fn an_answer_fills_only_the_read_that_asked_for_it() {
        let mut pictures = Pictures::<&str>::default();
        let asked = owner();
        let of = key(asked, AttachmentId::generate(), "aa");
        let first = read(&mut pictures, &of);
        // The account leaves and comes back while the read is on its way, and the picture
        // is asked for again.
        pictures.keep(|_| false);
        let second = read(&mut pictures, &of);
        assert_ne!(first, second);
        assert!(!pictures.settle(&of, first, Some("of the read before")));
        assert_eq!(pictures.get(&of), Some(&Picture::Reading(second)));
        assert!(pictures.settle(&of, second, Some("of this read")));
        assert_eq!(pictures.get(&of), Some(&Picture::Ready("of this read")));
    }

    #[test]
    fn the_reads_of_one_server_do_not_wait_for_the_reads_of_another() {
        let mut pictures = Pictures::<&str>::default();
        let (slow, other) = (owner(), owner());
        let of_slow: Vec<PictureKey> = (0..MOST_READS + 2)
            .map(|_| key(slow, AttachmentId::generate(), "aa"))
            .collect();
        // The reads of the first server do not end.
        let started = draw(&mut pictures, &of_slow);
        assert_eq!(started.len(), MOST_READS);
        // The person selects another server: its pictures are read all the same.
        let of_other = key(other, AttachmentId::generate(), "bb");
        let number = read(&mut pictures, &of_other);
        assert!(pictures.settle(&of_other, number, Some("of the other server")));
        // The first server gets no further read, also after its account is gone.
        assert!(draw(&mut pictures, &of_slow).is_empty());
        pictures.keep(|owner| *owner == other);
        assert!(draw(&mut pictures, &of_slow).is_empty());
        // One of its reads ends: one further read of that server can start.
        let (ended, number) = started[0].clone();
        pictures.settle(&ended, number, None);
        assert_eq!(draw(&mut pictures, &of_slow).len(), 1);
    }

    #[test]
    fn image_cache_eviction_does_not_duplicate_pending_reads() {
        let mut pictures = Pictures::<usize>::default();
        let asked = owner();
        // One view states one picture more than the window holds.
        let keys: Vec<PictureKey> = (0..=MOST_PICTURES)
            .map(|_| key(asked, AttachmentId::generate(), "aa"))
            .collect();
        let mut started: Vec<(PictureKey, u64)> = Vec::new();
        let mut ended = 0;
        // The window is drawn again and again, and no read ends: no further read starts.
        for _ in 0..10 {
            started.extend(draw(&mut pictures, &keys));
        }
        assert_eq!(started.len(), MOST_READS);

        // Each read that ends is followed by a drawing. A picture is read once, at most
        // [`MOST_READS`] reads run, and the picture that finds no place is not read while
        // the view asks for the others.
        for _ in 0..200 {
            if let Some((key, number)) = started.get(ended).cloned() {
                assert!(pictures.settle(&key, number, Some(ended)));
                ended += 1;
            }
            started.extend(draw(&mut pictures, &keys));
            assert!(started.len() - ended <= MOST_READS);
        }
        assert_eq!(started.len(), MOST_PICTURES);
        assert_eq!(ended, MOST_PICTURES);
        let mut read: Vec<AttachmentId> =
            started.iter().map(|(key, _)| key.attachment_id).collect();
        read.sort_unstable();
        read.dedup();
        assert_eq!(read.len(), MOST_PICTURES, "no picture was read twice");
        let last = keys.last().unwrap();
        assert_eq!(pictures.get(last), None);

        // The window shows another view, with the last picture only. The first drawing of
        // that view starts its read, and the oldest picture of the view before gives way.
        let started = draw(&mut pictures, std::slice::from_ref(last));
        assert_eq!(started.len(), 1, "the first drawing of the next view reads");
        assert!(pictures.settle(last, started[0].1, Some(MOST_PICTURES)));
        assert_eq!(pictures.get(&keys[0]), None);
        assert_eq!(pictures.take_dropped(), [0]);
        assert_eq!(pictures.order.len(), MOST_PICTURES);
        assert_eq!(pictures.held.len(), MOST_PICTURES);
    }
}
