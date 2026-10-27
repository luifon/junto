//! The pictures of messages that the window holds decoded (ADR 0740). A picture belongs
//! to the saved server and the account that it was read for, and to one file with one
//! hash (ADR 0745): an identifier that a second server copies, and the next account of a
//! server, find nothing that was read before.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use junto_client_core::Owner;
use junto_client_core::protocol::ids::AttachmentId;
use junto_client_core::protocol::message::Attachment;

/// How many pictures of messages the window holds.
pub(crate) const MOST_PICTURES: usize = 128;
/// The most bytes of the pictures that the window holds. Above it the pictures that the
/// last drawing did not ask for are dropped, the oldest first; the pictures that one
/// drawing asks for are kept above it.
pub(crate) const MOST_HELD_BYTES: u64 = 96 * 1024 * 1024;
/// How many pictures of one owner are read at one time. A picture that finds no free read
/// is asked for again when the window is drawn after a read has ended. The reads of one
/// server do not count for another, so a server that answers slowly delays its own
/// pictures only.
pub(crate) const MOST_READS: usize = 4;
/// The wait before a picture whose read got no answer is read again. Each further read
/// without an answer doubles it, up to [`LONGEST_WAIT`].
pub(crate) const FIRST_WAIT: Duration = Duration::from_secs(30);
pub(crate) const LONGEST_WAIT: Duration = Duration::from_secs(16 * 60);

/// What a held picture is the picture of.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct PictureKey {
    pub owner: Owner,
    pub attachment_id: AttachmentId,
    pub sha256: String,
    /// The size in pixels of the device that the picture was reduced to fit. A window that
    /// draws pictures larger after a change of its scale reads them again at that size; a
    /// window that draws them smaller uses the held copy ([`Pictures::covering`]).
    pub fit: (u32, u32),
}

impl PictureKey {
    pub(crate) fn of(owner: Owner, attachment: &Attachment, fit: (u32, u32)) -> Self {
        Self {
            owner,
            attachment_id: attachment.id,
            sha256: attachment.sha256.clone(),
            fit,
        }
    }
}

/// The bytes that a held picture takes.
pub(crate) trait Weight {
    fn bytes(&self) -> u64;
}

impl Weight for std::sync::Arc<gpui::RenderImage> {
    fn bytes(&self) -> u64 {
        super::decode::held_bytes(self)
    }
}

/// A picture of a message in the window.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Picture<Image> {
    /// The read with this number is on its way.
    Reading(u64),
    Ready(Image),
    /// The server answered, and its answer decides: it refused the file, or it sent bytes
    /// that are not the file or not a picture. The picture is not read again while the
    /// window holds it.
    Failed,
    /// The read got no answer: the server was not reached, did not finish its answer, or
    /// failed. The picture is read again at the first drawing after `again`.
    Unanswered {
        again: Instant,
    },
}

/// How a read of a picture ended.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Outcome<Image> {
    Read(Image),
    /// The answer of the server decides (see [`Picture::Failed`]).
    Refused,
    /// See [`Picture::Unanswered`].
    Unanswered,
}

struct Held<Image> {
    picture: Picture<Image>,
    /// The number of the last drawing of the window that asked for the picture.
    used: u64,
    /// The reads in a row that got no answer.
    unanswered: u32,
}

/// The wait after this many reads in a row without an answer.
fn wait_after(unanswered: u32) -> Duration {
    let doubled = 1u32
        .checked_shl(unanswered.saturating_sub(1))
        .unwrap_or(u32::MAX);
    FIRST_WAIT.saturating_mul(doubled).min(LONGEST_WAIT)
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
    /// The bytes of the held images.
    bytes: u64,
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
            bytes: 0,
        }
    }
}

impl<Image: Weight> Pictures<Image> {
    /// The bytes of the held images.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn bytes(&self) -> u64 {
        self.bytes
    }

    pub(crate) fn get(&self, key: &PictureKey) -> Option<&Picture<Image>> {
        self.held.get(key).map(|held| &held.picture)
    }

    /// The key of the smallest read copy of the file of `key` for its owner that was reduced
    /// to fit a size at least as large as `key.fit`: a picture drawn smaller after a change
    /// of the scale of the window uses it, and nothing is read again (review M7).
    pub(crate) fn covering(&self, key: &PictureKey) -> Option<PictureKey> {
        self.held
            .iter()
            .filter(|(held, picture)| {
                held.owner == key.owner
                    && held.attachment_id == key.attachment_id
                    && held.sha256 == key.sha256
                    && held.fit.0 >= key.fit.0
                    && held.fit.1 >= key.fit.1
                    && matches!(picture.picture, Picture::Ready(_))
            })
            .min_by_key(|(held, _)| u64::from(held.fit.0) * u64::from(held.fit.1))
            .map(|(held, _)| held.clone())
    }

    /// How many reads of pictures the window started. A test uses it.
    #[cfg(feature = "test-support")]
    pub(crate) fn reads(&self) -> u64 {
        self.reads
    }

    /// A drawing of the window starts. The pictures that it asks for are in use.
    pub(crate) fn drawing_starts(&mut self) {
        self.drawing += 1;
    }

    /// A drawing of the window asks for a picture. True when the drawing uses what is
    /// held; false when the picture is not held, or is due to be read again.
    pub(crate) fn touch(&mut self, key: &PictureKey, now: Instant) -> bool {
        match self.held.get_mut(key) {
            Some(held) => {
                held.used = self.drawing;
                !matches!(held.picture, Picture::Unanswered { again } if again <= now)
            }
            None => false,
        }
    }

    /// Starts the read of a picture that the last drawing asked for and that is not held,
    /// or that is due to be read again, and returns the number of the read that the caller
    /// starts for it. The caller asks
    /// after the drawing has ended, when every picture that it uses is known.
    ///
    /// At most [`MOST_READS`] reads of one owner run. A new picture takes the place of the
    /// oldest one that is not being read and that the last drawing did not ask for. So a
    /// read that runs is never forgotten and never started a second time, and the
    /// pictures of one view do not take the places of each other: a view with more
    /// pictures than the window holds leaves the others unread. A held picture whose
    /// read got no answer is read again in its own place once its wait has passed.
    pub(crate) fn start(&mut self, key: &PictureKey, now: Instant) -> Option<u64> {
        let again = match self.held.get(key) {
            None => false,
            Some(held) => match held.picture {
                Picture::Unanswered { again } if again <= now => true,
                _ => return None,
            },
        };
        if self.flying.get(&key.owner).copied().unwrap_or(0) >= MOST_READS {
            return None;
        }
        if !again && self.order.len() >= MOST_PICTURES {
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
        let reading = Picture::Reading(self.reads);
        match self.held.get_mut(key) {
            Some(held) => {
                held.picture = reading;
                held.used = self.drawing;
            }
            None => {
                let held = Held {
                    picture: reading,
                    used: self.drawing,
                    unanswered: 0,
                };
                self.held.insert(key.clone(), held);
                self.order.push_back(key.clone());
            }
        }
        Some(self.reads)
    }

    fn end_read(&mut self, key: &PictureKey) {
        if let Some(flying) = self.flying.get_mut(&key.owner) {
            *flying = flying.saturating_sub(1);
            if *flying == 0 {
                self.flying.remove(&key.owner);
            }
        }
    }

    /// Takes the end of the read with this number at the time `now`. The outcome is held
    /// only when the picture still waits for exactly this read. True when a held picture
    /// changed.
    pub(crate) fn settle(
        &mut self,
        key: &PictureKey,
        number: u64,
        outcome: Outcome<Image>,
        now: Instant,
    ) -> bool {
        self.end_read(key);
        match self.held.get_mut(key) {
            Some(held) if matches!(held.picture, Picture::Reading(of) if of == number) => {
                let read = matches!(outcome, Outcome::Read(_));
                held.picture = match outcome {
                    Outcome::Read(image) => {
                        self.bytes += image.bytes();
                        Picture::Ready(image)
                    }
                    Outcome::Refused => Picture::Failed,
                    Outcome::Unanswered => {
                        held.unanswered = held.unanswered.saturating_add(1);
                        let wait = wait_after(held.unanswered);
                        match now.checked_add(wait) {
                            Some(again) => Picture::Unanswered { again },
                            None => Picture::Failed,
                        }
                    }
                };
                if read {
                    self.shed(key);
                }
                true
            }
            _ => {
                if let Outcome::Read(image) = outcome {
                    self.dropped.push(image);
                }
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
            self.bytes = self.bytes.saturating_sub(image.bytes());
            self.dropped.push(image);
        }
    }

    /// Drops the oldest read pictures that the last drawing did not ask for, other than
    /// `kept`, while the held images take more than [`MOST_HELD_BYTES`].
    fn shed(&mut self, kept: &PictureKey) {
        while self.bytes > MOST_HELD_BYTES {
            let drawing = self.drawing;
            let oldest = self.order.iter().position(|key| {
                key != kept
                    && self.held.get(key).is_some_and(|held| {
                        matches!(held.picture, Picture::Ready(_)) && held.used < drawing
                    })
            });
            let Some(gone) = oldest.and_then(|at| self.order.remove(at)) else {
                return;
            };
            self.forget(&gone);
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

    /// A held image of this file that is read.
    #[cfg(feature = "test-support")]
    pub(crate) fn ready_image_of(&self, attachment_id: AttachmentId) -> Option<&Image> {
        self.held
            .iter()
            .find_map(|(key, held)| match &held.picture {
                Picture::Ready(image) if key.attachment_id == attachment_id => Some(image),
                _ => None,
            })
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

    impl Weight for &str {
        fn bytes(&self) -> u64 {
            0
        }
    }

    impl Weight for usize {
        fn bytes(&self) -> u64 {
            0
        }
    }

    /// A picture of this many bytes.
    #[derive(Debug, PartialEq, Eq)]
    struct Weighed(u64);

    impl Weight for Weighed {
        fn bytes(&self) -> u64 {
            self.0
        }
    }

    fn key(owner: Owner, attachment_id: AttachmentId, sha256: &str) -> PictureKey {
        PictureKey {
            owner,
            attachment_id,
            sha256: sha256.to_owned(),
            fit: (584, 440),
        }
    }

    fn now() -> Instant {
        Instant::now()
    }

    /// One drawing of the window that asks for these pictures, and the reads that start
    /// after it.
    fn draw<Image: Weight>(
        pictures: &mut Pictures<Image>,
        asked: &[PictureKey],
    ) -> Vec<(PictureKey, u64)> {
        draw_at(pictures, asked, now())
    }

    #[test]
    fn the_held_pictures_stay_within_their_bytes_except_the_ones_in_use() {
        const MIB: u64 = 1024 * 1024;
        let mut pictures = Pictures::<Weighed>::default();
        let asked = owner();
        let keys: Vec<PictureKey> = (0..8)
            .map(|_| key(asked, AttachmentId::generate(), "aa"))
            .collect();
        // Each view shows two pictures of 30 MiB.
        for view in keys.chunks(2) {
            for (key, number) in draw(&mut pictures, view) {
                pictures.settle(&key, number, Outcome::Read(Weighed(30 * MIB)), now());
            }
            assert!(pictures.bytes() <= MOST_HELD_BYTES, "{}", pictures.bytes());
        }
        // The pictures of the last view and the newest one before it are held.
        assert_eq!(pictures.bytes(), 90 * MIB);
        for gone in &keys[..5] {
            assert_eq!(pictures.get(gone), None);
        }
        for held in &keys[5..] {
            assert_eq!(pictures.get(held), Some(&Picture::Ready(Weighed(30 * MIB))));
        }
        assert_eq!(pictures.take_dropped().len(), 5);
        // One view with four such pictures holds all four, above the bound.
        let mut started = draw(&mut pictures, &keys[..4]);
        started.sort_by_key(|(_, number)| *number);
        for (key, number) in started {
            pictures.settle(&key, number, Outcome::Read(Weighed(30 * MIB)), now());
        }
        for in_view in &keys[..4] {
            assert!(pictures.get(in_view).is_some());
        }
        assert_eq!(pictures.bytes(), 120 * MIB);
    }

    fn draw_at<Image: Weight>(
        pictures: &mut Pictures<Image>,
        asked: &[PictureKey],
        now: Instant,
    ) -> Vec<(PictureKey, u64)> {
        pictures.drawing_starts();
        let waiting: Vec<&PictureKey> = asked
            .iter()
            .filter(|key| !pictures.touch(key, now))
            .collect();
        waiting
            .into_iter()
            .filter_map(|key| pictures.start(key, now).map(|number| (key.clone(), number)))
            .collect()
    }

    /// Asks for one picture in a drawing and takes the number of its read.
    fn read<Image: Weight>(pictures: &mut Pictures<Image>, key: &PictureKey) -> u64 {
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
        assert!(pictures.settle(
            &of_first,
            number,
            Outcome::Read("the picture of the first server"),
            now()
        ));
        // The picture is held: it is not read a second time.
        assert!(draw(&mut pictures, std::slice::from_ref(&of_first)).is_empty());

        // A second server states a file with the same identifier. Its picture is read
        // from that server, and the picture of the first server is not drawn for it.
        let of_second = key(owner(), file, "bb");
        assert_eq!(pictures.get(&of_second), None);
        let number = read(&mut pictures, &of_second);
        assert_eq!(pictures.get(&of_second), Some(&Picture::Reading(number)));
        assert!(pictures.settle(
            &of_second,
            number,
            Outcome::Read("the picture of the second server"),
            now()
        ));

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
            assert!(pictures.settle(&other, number, Outcome::Refused, now()));
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
            pictures.settle(key, number, Outcome::Read(image), now());
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
        assert!(!pictures.settle(&late, number, Outcome::Read("late"), now()));
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
        assert!(!pictures.settle(&of, first, Outcome::Read("of the read before"), now()));
        assert_eq!(pictures.get(&of), Some(&Picture::Reading(second)));
        assert!(pictures.settle(&of, second, Outcome::Read("of this read"), now()));
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
        assert!(pictures.settle(
            &of_other,
            number,
            Outcome::Read("of the other server"),
            now()
        ));
        // The first server gets no further read, also after its account is gone.
        assert!(draw(&mut pictures, &of_slow).is_empty());
        pictures.keep(|owner| *owner == other);
        assert!(draw(&mut pictures, &of_slow).is_empty());
        // One of its reads ends: one further read of that server can start.
        let (ended, number) = started[0].clone();
        pictures.settle(&ended, number, Outcome::Refused, now());
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
                assert!(pictures.settle(&key, number, Outcome::Read(ended), now()));
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
        assert!(pictures.settle(last, started[0].1, Outcome::Read(MOST_PICTURES), now()));
        assert_eq!(pictures.get(&keys[0]), None);
        assert_eq!(pictures.take_dropped(), [0]);
        assert_eq!(pictures.order.len(), MOST_PICTURES);
        assert_eq!(pictures.held.len(), MOST_PICTURES);
    }

    #[test]
    fn a_picture_whose_read_got_no_answer_is_read_again_after_a_wait() {
        let mut pictures = Pictures::<&str>::default();
        let of = key(owner(), AttachmentId::generate(), "aa");
        let start = now();
        let number = draw_at(&mut pictures, std::slice::from_ref(&of), start)[0].1;
        assert!(pictures.settle(&of, number, Outcome::Unanswered, start));
        assert_eq!(
            pictures.get(&of),
            Some(&Picture::Unanswered {
                again: start + FIRST_WAIT
            })
        );
        // Drawings before the wait has passed read nothing.
        let early = start + FIRST_WAIT - Duration::from_secs(1);
        assert!(draw_at(&mut pictures, std::slice::from_ref(&of), early).is_empty());
        // The first drawing after it reads the picture again, in its own place. Each
        // further read without an answer doubles the wait, up to the longest wait.
        let mut at = start + FIRST_WAIT;
        let mut wait = FIRST_WAIT;
        for _ in 0..8 {
            let started = draw_at(&mut pictures, std::slice::from_ref(&of), at);
            assert_eq!(started.len(), 1, "the picture is read again");
            assert!(pictures.settle(&of, started[0].1, Outcome::Unanswered, at));
            wait = (wait * 2).min(LONGEST_WAIT);
            assert_eq!(
                pictures.get(&of),
                Some(&Picture::Unanswered { again: at + wait })
            );
            let early = at + wait - Duration::from_secs(1);
            assert!(draw_at(&mut pictures, std::slice::from_ref(&of), early).is_empty());
            at += wait;
        }
        assert_eq!(wait, LONGEST_WAIT);
        assert_eq!(pictures.order.len(), 1);
        let started = draw_at(&mut pictures, std::slice::from_ref(&of), at);
        assert!(pictures.settle(&of, started[0].1, Outcome::Read("arrived"), at));
        assert_eq!(pictures.get(&of), Some(&Picture::Ready("arrived")));

        // A picture that the server refused, or whose bytes are no picture, is not read
        // again while the window holds it.
        let refused = key(owner(), AttachmentId::generate(), "bb");
        let number = draw_at(&mut pictures, std::slice::from_ref(&refused), start)[0].1;
        assert!(pictures.settle(&refused, number, Outcome::Refused, start));
        let later = start + LONGEST_WAIT * 4;
        assert!(draw_at(&mut pictures, std::slice::from_ref(&refused), later).is_empty());
        assert_eq!(pictures.get(&refused), Some(&Picture::Failed));
    }
}
