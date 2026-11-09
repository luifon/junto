//! The order of the entries of one group, the categories of a workspace or the channels of a
//! category or of the root, as a person reads it and as a request changes it (ADR 0304,
//! ADR 0206).
//!
//! A person who sees every entry of a group reads the stored positions: they hide nothing
//! from that person. Any other person reads the place of each entry that the person sees
//! among those entries, counted from 0, so that a gap does not tell how many are hidden.
//!
//! A request names a new position in one of two ways. `position`, the meaning of API v1
//! that the clients of 0.1.0 use, is a position in the caller's view as the caller reads it:
//! the entries that the caller sees are ordered by position and identifier with the new
//! value. `place` is the place among the entries that the caller sees after the change.

use std::collections::HashMap;

/// One entry of a group, in the stored order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Held<Id> {
    pub id: Id,
    pub position: i32,
    /// The person sees the entry in the lists.
    pub listed: bool,
    /// The person may see the entry; an archived channel is not listed and may be seen.
    pub viewable: bool,
}

/// What a request asks for one entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// A position in the caller's view, the meaning of API v1.
    Position(i32),
    /// A place among the entries that the caller sees after the change.
    Place(i32),
}

/// True when the person sees every entry of the group.
pub fn sees_all<Id>(group: &[Held<Id>]) -> bool {
    group.iter().all(|held| held.viewable)
}

/// The position that the person reads of each entry of the group that the person sees.
/// `group` is in the stored order.
pub fn read_positions<Id: Copy + Eq + std::hash::Hash>(group: &[Held<Id>]) -> HashMap<Id, i32> {
    let stored = sees_all(group);
    group
        .iter()
        .filter(|held| held.listed)
        .zip(0..)
        .map(|(held, place)| (held.id, if stored { held.position } else { place }))
        .collect()
}

/// Sorts entries into the stored order: by position, then by identifier.
pub fn stored_order<Id: Copy + Ord>(group: &mut [Held<Id>]) {
    group.sort_by_key(|held| (held.position, held.id));
}

/// The stored positions that change when the request `targets` is applied to `group`, which
/// is in the stored order and holds every entry that the request moves, an entry that enters
/// the group included. Empty when the order that the caller sees stays as it is.
///
/// A caller who sees every entry and names positions gets those positions stored as given,
/// as API v1 stored them: a client that sends several requests computed from one read ends in
/// the order that it computed. Otherwise the caller's view is ordered anew, each entry that
/// the caller does not see is put before the entry that it was before, or the next one that
/// the request does not move, and the group is numbered again from 0.
pub fn reordered<Id: Copy + Ord + std::hash::Hash>(
    group: &[Held<Id>],
    targets: &[(Id, Target)],
) -> Vec<(Id, i32)> {
    let all = sees_all(group);
    let target_of = |id: Id| {
        targets
            .iter()
            .rev()
            .find(|(held, _)| *held == id)
            .map(|(_, target)| *target)
    };
    if all
        && targets
            .iter()
            .all(|(_, target)| matches!(target, Target::Position(_)))
    {
        return group
            .iter()
            .filter_map(|held| match target_of(held.id) {
                Some(Target::Position(position)) if position != held.position => {
                    Some((held.id, position))
                }
                _ => None,
            })
            .collect();
    }
    let view = read_positions(group);
    let before: Vec<Id> = group
        .iter()
        .filter(|held| held.listed)
        .map(|held| held.id)
        .collect();
    let mut after: Vec<Id> = before.clone();
    let positions: Vec<(Id, i32)> = targets
        .iter()
        .filter_map(|(id, target)| match target {
            Target::Position(position) => Some((*id, *position)),
            Target::Place(_) => None,
        })
        .collect();
    if !positions.is_empty() {
        after.sort_by_key(|id| {
            let position = positions
                .iter()
                .rev()
                .find(|(held, _)| held == id)
                .map_or_else(|| view.get(id).copied().unwrap_or(i32::MAX), |(_, at)| *at);
            (position, *id)
        });
    }
    for (id, target) in targets {
        if let Target::Place(place) = target
            && let Some(at) = after.iter().position(|held| held == id)
        {
            let held = after.remove(at);
            let place = usize::try_from(*place).unwrap_or(0).min(after.len());
            after.insert(place, held);
        }
    }
    if after == before {
        return Vec::new();
    }
    let moved = |id: Id| targets.iter().any(|(held, _)| *held == id);
    // Each entry that the caller does not see goes before the first entry after it that the
    // caller sees and that the request does not move; without one it stays at the end.
    let mut anchored: Vec<(Option<Id>, Id)> = Vec::new();
    for (at, held) in group.iter().enumerate() {
        if held.listed {
            continue;
        }
        let anchor = group[at + 1..]
            .iter()
            .find(|next| next.listed && !moved(next.id))
            .map(|next| next.id);
        anchored.push((anchor, held.id));
    }
    let mut order: Vec<Id> = Vec::with_capacity(group.len());
    for id in &after {
        order.extend(
            anchored
                .iter()
                .filter(|(anchor, _)| *anchor == Some(*id))
                .map(|(_, hidden)| *hidden),
        );
        order.push(*id);
    }
    order.extend(
        anchored
            .iter()
            .filter(|(anchor, _)| anchor.is_none())
            .map(|(_, hidden)| *hidden),
    );
    order
        .into_iter()
        .zip(0..)
        .filter(|(id, position)| {
            group
                .iter()
                .find(|held| held.id == *id)
                .is_none_or(|held| held.position != *position)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(entries: &[(u8, bool)]) -> Vec<Held<u8>> {
        entries
            .iter()
            .zip(0..)
            .map(|(&(id, listed), position)| Held {
                id,
                position,
                listed,
                viewable: listed,
            })
            .collect()
    }

    fn applied(group: &[Held<u8>], targets: &[(u8, Target)]) -> Vec<u8> {
        let changes = reordered(group, targets);
        let mut after = group.to_vec();
        for (id, position) in changes {
            if let Some(held) = after.iter_mut().find(|held| held.id == id) {
                held.position = position;
            }
        }
        stored_order(&mut after);
        after.iter().map(|held| held.id).collect()
    }

    #[test]
    fn a_place_that_keeps_the_view_changes_nothing_whatever_is_hidden() {
        // 1, 2, 3 are seen; 9 is hidden after 2.
        let held = group(&[(1, true), (2, true), (9, false), (3, true)]);
        assert!(reordered(&held, &[(2, Target::Place(1))]).is_empty());
        assert!(reordered(&held, &[(3, Target::Place(2))]).is_empty());
        assert!(reordered(&held, &[(3, Target::Place(7))]).is_empty());
        // A position that keeps the order of the view changes nothing either.
        assert!(reordered(&held, &[(2, Target::Position(1))]).is_empty());
    }

    #[test]
    fn a_hidden_entry_stays_before_the_entry_that_followed_it() {
        let held = group(&[(1, true), (9, false), (2, true), (3, true)]);
        // 3 goes first: 9 stays before 2.
        assert_eq!(applied(&held, &[(3, Target::Place(0))]), [3, 1, 9, 2]);
        // 2 goes last: 9 goes before the next entry that does not move, 3.
        assert_eq!(applied(&held, &[(2, Target::Place(2))]), [1, 9, 3, 2]);
        // A position in the caller's view, the meaning of API v1: 3 at 0 sorts before 1.
        assert_eq!(applied(&held, &[(3, Target::Position(-1))]), [3, 1, 9, 2]);
    }

    #[test]
    fn a_caller_who_sees_everything_stores_the_positions_as_given() {
        let held = group(&[(1, true), (2, true), (3, true)]);
        assert_eq!(reordered(&held, &[(3, Target::Position(1))]), [(3, 1)]);
        assert!(reordered(&held, &[(3, Target::Position(2))]).is_empty());
    }
}
