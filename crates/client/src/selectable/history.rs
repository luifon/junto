use std::{
    cell::Cell,
    collections::{BTreeMap, BTreeSet},
    ops::Range,
    rc::Rc,
    sync::Arc,
};

use crate::state::feed::{FeedRow, Names};
use gpui::ListState;
use junto_client_core::{model::MOST_LOADED_WEIGHT, protocol::ids::MessageId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Key {
    pub lane: u8,
    pub position: u64,
    pub id: MessageId,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Anchor {
    pub key: Key,
    pub byte: usize,
}

pub(super) struct Paint {
    pub key: Key,
    pub text: Rc<str>,
    pub cursor: Cell<usize>,
}

#[derive(Default)]
pub(super) struct History {
    messages: BTreeMap<Key, Rc<str>>,
    pub start: Option<Anchor>,
    pub end: Option<Anchor>,
    pub lists: BTreeMap<u8, ListState>,
    pub revisions: BTreeMap<u8, (Arc<Vec<FeedRow>>, Arc<Names>)>,
}

impl History {
    pub fn clear(&mut self) {
        self.start = None;
        self.end = None;
    }

    fn ordered(&self) -> Option<(Anchor, Anchor)> {
        let (start, end) = (self.start?, self.end?);
        (start.key.lane == end.key.lane).then_some(if start <= end {
            (start, end)
        } else {
            (end, start)
        })
    }

    pub fn seed(&mut self, lane: u8, messages: Vec<(Key, String)>) {
        let retained: BTreeSet<_> = messages.iter().map(|(key, _)| *key).collect();
        let selected = self.ordered();
        self.messages.retain(|key, _| {
            key.lane != lane
                || retained.contains(key)
                || selected.is_some_and(|(from, to)| *key >= from.key && *key <= to.key)
        });
        for (key, text) in messages {
            if self
                .messages
                .get(&key)
                .is_some_and(|old| old.as_ref() != text)
                && selected.is_some_and(|(from, to)| key >= from.key && key <= to.key)
            {
                self.clear();
            }
            self.messages.insert(key, text.into());
        }
    }

    pub fn paint(&self, key: Key) -> Option<Rc<Paint>> {
        Some(Rc::new(Paint {
            key,
            text: self.messages.get(&key)?.clone(),
            cursor: Cell::new(0),
        }))
    }

    pub fn extend(&mut self, anchor: Anchor) -> bool {
        let Some(start) = self.start else {
            return false;
        };
        if start.key.lane != anchor.key.lane {
            return false;
        }
        let (from, to) = if start <= anchor {
            (start, anchor)
        } else {
            (anchor, start)
        };
        let bytes: usize = self
            .messages
            .range(from.key..=to.key)
            .map(|(_, text)| text.len())
            .sum();
        if bytes > MOST_LOADED_WEIGHT {
            return false;
        }
        self.end = Some(anchor);
        true
    }

    pub fn range(&self, key: Key, len: usize) -> Range<usize> {
        let Some((from, to)) = self.ordered() else {
            return 0..0;
        };
        if key < from.key || key > to.key {
            return 0..0;
        }
        let start = if key == from.key {
            from.byte.min(len)
        } else {
            0
        };
        let end = if key == to.key { to.byte.min(len) } else { len };
        start..end
    }

    pub fn text(&self) -> Option<String> {
        let (from, to) = self.ordered()?;
        Some(
            self.messages
                .range(from.key..=to.key)
                .filter_map(|(key, text)| {
                    let range = self.range(*key, text.len());
                    (!range.is_empty()).then(|| text.get(range).unwrap_or_default())
                })
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }
}
