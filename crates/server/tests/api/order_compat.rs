//! The order requests of a client of release 0.1.0 against this server, and the place
//! requests of this release (ADR 0206, ADR 0304). The window of 0.1.0 moves a category with
//! Up and Down as several requests computed from one read, and a channel with one request of
//! several moves; its algorithm is copied here from that release.

use crate::support::{Member, TestServer};
use junto_protocol::events::{Event, Level};
use junto_protocol::ids::{CategoryId, ChannelId, WorkspaceId};
use junto_protocol::structure::{Category, Channel, ChannelList};
use reqwest::StatusCode;
use serde_json::json;

/// The new positions that put `moving` at the place `to` of a group, as the window of
/// release 0.1.0 computes them (`placed` of `crates/client/src/state/admin/structure.rs` at
/// that release).
fn placed<Id: Copy + Ord>(order: &[(Id, i32)], moving: Id, to: usize) -> Vec<(Id, i32)> {
    placings(order, moving, to)
        .into_iter()
        .next()
        .unwrap_or_default()
}

/// `placings` of release 0.1.0, unchanged.
fn placings<Id: Copy + Ord>(order: &[(Id, i32)], moving: Id, to: usize) -> Vec<Vec<(Id, i32)>> {
    let from = order.iter().position(|(id, _)| *id == moving);
    let others: Vec<(Id, i32)> = order
        .iter()
        .copied()
        .filter(|(id, _)| *id != moving)
        .collect();
    let at = to.min(others.len());
    if from == Some(at) {
        return Vec::new();
    }
    let (before, after) = (&others[..at], &others[at..]);
    let key = |(id, position): (Id, i32)| (position, id);
    let mut starts: Vec<i32> = Vec::new();
    match (before.last(), after.first()) {
        (None, None) => starts.push(0),
        (previous, next) => {
            if let Some((_, position)) = previous {
                starts.extend(position.checked_add(1));
            }
            if let Some((_, position)) = next {
                starts.extend(position.checked_sub(1));
            }
            starts.extend(previous.map(|(_, position)| *position));
            starts.extend(next.map(|(_, position)| *position));
        }
    }
    starts.dedup();
    let mut ways: Vec<(Vec<(Id, i32)>, bool)> = Vec::new();
    for start in starts {
        let mut way = vec![(moving, start)];
        let mut fits = true;
        let mut last = (moving, start);
        for entry in after {
            if key(*entry) > key(last) {
                break;
            }
            let lowest = if entry.0 > last.0 {
                Some(last.1)
            } else {
                last.1.checked_add(1)
            };
            match lowest {
                Some(position) => {
                    last = (entry.0, position);
                    way.push(last);
                }
                None => {
                    fits = false;
                    break;
                }
            }
        }
        let mut first = (moving, start);
        for entry in before.iter().rev() {
            if !fits || key(*entry) < key(first) {
                break;
            }
            let highest = if entry.0 < first.0 {
                Some(first.1)
            } else {
                first.1.checked_sub(1)
            };
            match highest {
                Some(position) => {
                    first = (entry.0, position);
                    way.push(first);
                }
                None => {
                    fits = false;
                    break;
                }
            }
        }
        if !fits {
            continue;
        }
        let shared = others.iter().any(|(_, position)| *position == start);
        ways.push((way, shared));
    }
    ways.sort_by_key(|(way, shared)| (way.len(), *shared));
    let mut found: Vec<Vec<(Id, i32)>> = Vec::with_capacity(ways.len());
    for (way, _) in ways {
        if !found.contains(&way) {
            found.push(way);
        }
    }
    found
}

async fn list(server: &TestServer, member: &Member, workspace_id: WorkspaceId) -> ChannelList {
    server
        .get(
            member,
            &format!("/api/v1/workspaces/{workspace_id}/channels"),
        )
        .await
        .ok(StatusCode::OK)
}

/// The categories as a person reads them, in the order of the window: position, identifier.
async fn categories(
    server: &TestServer,
    member: &Member,
    workspace_id: WorkspaceId,
) -> Vec<(CategoryId, i32)> {
    let mut held: Vec<(CategoryId, i32)> = list(server, member, workspace_id)
        .await
        .categories
        .iter()
        .map(|category| (category.id, category.position))
        .collect();
    held.sort_by_key(|(id, position)| (*position, *id));
    held
}

/// The root channels as a person reads them, in the order of the window.
async fn root(
    server: &TestServer,
    member: &Member,
    workspace_id: WorkspaceId,
) -> Vec<(ChannelId, i32)> {
    let mut held: Vec<(ChannelId, i32)> = list(server, member, workspace_id)
        .await
        .channels
        .iter()
        .filter(|channel| channel.category_id.is_none())
        .map(|channel| (channel.id, channel.position))
        .collect();
    held.sort_by_key(|(id, position)| (*position, *id));
    held
}

/// A walk of moves that is the same in every run: the index of the entry and Up or Down.
fn walk(count: usize, steps: usize) -> Vec<(usize, bool)> {
    let mut state: u64 = 7;
    (0..steps)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            let index = usize::try_from(state >> 33).unwrap() % count;
            (index, (state >> 20) & 1 == 1)
        })
        .collect()
}

/// Up and Down of the window of 0.1.0 send one request per changed category, all computed
/// from one read; the categories end in the order that the window drew. The administrator
/// sees every category, reads the stored positions and the server stores each as given.
#[tokio::test]
async fn up_and_down_of_a_0_1_0_window_move_a_category_by_one_place() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace_id = server.default_workspace(&owner).await.id;
    for name in ["A", "B", "C", "D", "E"] {
        server.category(&owner, workspace_id, name, 0).await;
    }
    for (index, up) in walk(5, 40) {
        let order = categories(&server, &owner, workspace_id).await;
        let (moving, _) = order[index];
        let Some(to) = (if up {
            index.checked_sub(1)
        } else {
            Some(index + 1).filter(|to| *to < order.len())
        }) else {
            continue;
        };
        let mut wanted: Vec<CategoryId> = order.iter().map(|(id, _)| *id).collect();
        let held = wanted.remove(index);
        wanted.insert(to, held);
        for (category_id, position) in placed(&order, moving, to) {
            let _: Category = server
                .patch(
                    &owner,
                    &format!("/api/v1/workspaces/{workspace_id}/categories/{category_id}"),
                    &json!({ "position": position }),
                )
                .await
                .ok(StatusCode::OK);
        }
        let after: Vec<CategoryId> = categories(&server, &owner, workspace_id)
            .await
            .iter()
            .map(|(id, _)| *id)
            .collect();
        assert_eq!(after, wanted, "{} {index} up {up}", order.len());
    }
}

/// A drag of the window of 0.1.0 sends one request with a move of every changed channel,
/// from the positions that it read; the channels end in the order that the window drew.
#[tokio::test]
async fn a_drag_of_a_0_1_0_window_puts_the_channel_where_it_was_dropped() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace_id = server.default_workspace(&owner).await.id;
    for name in ["a", "b", "c", "d"] {
        server.channel(&owner, workspace_id, name).await;
    }
    let count = root(&server, &owner, workspace_id).await.len();
    let mut state: u64 = 11;
    for _ in 0..30 {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        let from = usize::try_from(state >> 33).unwrap() % count;
        let to = usize::try_from(state >> 40).unwrap() % count;
        let order = root(&server, &owner, workspace_id).await;
        let (moving, _) = order[from];
        let changes = placed(&order, moving, to);
        if changes.is_empty() {
            continue;
        }
        let mut wanted: Vec<ChannelId> = order.iter().map(|(id, _)| *id).collect();
        let held = wanted.remove(from);
        wanted.insert(to.min(wanted.len()), held);
        let moves: Vec<_> = changes
            .iter()
            .map(|(channel_id, position)| {
                let read = order.iter().find(|(id, _)| id == channel_id).unwrap().1;
                json!({
                    "channel_id": channel_id,
                    "from": {"category_id": null, "position": read},
                    "to": {"category_id": null, "position": position},
                })
            })
            .collect();
        server
            .patch(
                &owner,
                &format!("/api/v1/workspaces/{workspace_id}/channels/positions"),
                &json!({ "moves": moves }),
            )
            .await
            .empty(StatusCode::NO_CONTENT);
        let after: Vec<ChannelId> = root(&server, &owner, workspace_id)
            .await
            .iter()
            .map(|(id, _)| *id)
            .collect();
        assert_eq!(after, wanted, "{from} to {to}");
    }
}

/// The refreshes of a workspace among events.
fn refreshes(events: &[(u64, Event)], workspace_id: WorkspaceId) -> usize {
    events
        .iter()
        .filter(|(_, event)| {
            matches!(event, Event::PermissionsChanged { workspace_id: Some(id) } if *id == workspace_id)
        })
        .count()
}

/// A manager who does not see the channel after `x` asks for the place where `x` already
/// is, by `place` and by `position`, and creates a channel at the end of the view: nothing
/// changes, and nobody reads the structure again (ADR 0304). Before, the hidden channel
/// was put after `x` and every person got a refresh, which told the manager that it exists.
#[tokio::test]
async fn a_request_that_keeps_the_view_of_its_caller_changes_nothing() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let manager = server.join(&owner, "Manager").await;
    let workspace_id = server.default_workspace(&owner).await.id;
    server
        .grant_workspace(
            &owner,
            &manager,
            workspace_id,
            &[junto_protocol::WorkspacePermission::ManageChannel],
        )
        .await;
    let x = server.channel(&owner, workspace_id, "x").await;
    let hidden = server.channel(&owner, workspace_id, "hidden").await;
    server.channel(&owner, workspace_id, "b").await;
    let last = server.channel(&owner, workspace_id, "last-hidden").await;
    for channel in [hidden.id, last.id] {
        sqlx::query("insert into channel_person_overrides(channel_id,workspace_id,account_id,allow,deny) values($1,$2,$3,0,1)")
            .bind(channel).bind(workspace_id).bind(manager.account_id).execute(&server.db).await.unwrap();
    }
    let stored = |server: &TestServer| {
        let db = server.db.clone();
        async move {
            sqlx::query_as::<_, (uuid::Uuid, i32)>(
                "select id, position from channels where workspace_id = $1 and category_id is null and deleted_at is null order by position, id",
            )
            .bind(workspace_id)
            .fetch_all(&db)
            .await
            .unwrap()
        }
    };
    let before = stored(&server).await;
    let seen = root(&server, &manager, workspace_id).await;
    let place = seen.iter().position(|(id, _)| *id == x.id).unwrap();
    let read = seen[place].1;
    let mut stream = server.subscribed(&owner, Level::Full).await;
    // The place of `x`, and a position of the view of 0.1.0 that sorts `x` before `b`, the
    // next channel that the manager sees: `x` was made first and its identifier is lower.
    for to in [
        json!({"category_id": null, "position": read, "place": place}),
        json!({"category_id": null, "position": read + 1}),
    ] {
        server
            .patch(
                &manager,
                &format!("/api/v1/workspaces/{workspace_id}/channels/positions"),
                &json!({ "moves": [{
                    "channel_id": x.id,
                    "from": {"category_id": null, "position": read},
                    "to": to,
                }]}),
            )
            .await
            .empty(StatusCode::NO_CONTENT);
    }
    let made: Channel = server
        .post(
            &manager,
            &format!("/api/v1/workspaces/{workspace_id}/channels"),
            &json!({"name": "new", "kind": "text", "position": seen.len()}),
        )
        .await
        .ok(StatusCode::CREATED);
    let after = stored(&server).await;
    assert_eq!(&after[..before.len()], &before[..]);
    assert_eq!(after.last().unwrap().0, made.id.as_uuid());
    let events = server.traffic(&owner, &mut stream).await;
    assert_eq!(refreshes(&events, workspace_id), 0, "{events:?}");
}

/// A root channel that leaves the lists, by an archive or a deletion, changes the places of
/// the root channels after it for a person who does not see every root channel: every
/// person reads the structure again, and the order that such a person reads afterwards
/// matches a new list (ADR 0304).
#[tokio::test]
async fn the_removal_of_a_root_channel_refreshes_the_people_of_the_workspace() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let workspace_id = server.default_workspace(&owner).await.id;
    let b = server.channel(&owner, workspace_id, "b").await;
    let c = server.channel(&owner, workspace_id, "c").await;
    let hidden = server.channel(&owner, workspace_id, "hidden").await;
    let f = server.channel(&owner, workspace_id, "f").await;
    sqlx::query("insert into channel_person_overrides(channel_id,workspace_id,account_id,allow,deny) values($1,$2,$3,0,1)")
        .bind(hidden.id).bind(workspace_id).bind(reader.account_id).execute(&server.db).await.unwrap();
    let mut stream = server.subscribed(&reader, Level::Full).await;
    server
        .delete(&owner, &format!("/api/v1/channels/{}", b.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    let events = server.traffic(&reader, &mut stream).await;
    assert_eq!(refreshes(&events, workspace_id), 1, "{events:?}");
    let _: serde_json::Value = server
        .put(
            &owner,
            &format!("/api/v1/channels/{}/archive", c.id),
            &json!({}),
        )
        .await
        .ok(StatusCode::OK);
    let events = server.traffic(&reader, &mut stream).await;
    assert_eq!(refreshes(&events, workspace_id), 1, "{events:?}");
    // The last channel leaves no place behind it: no refresh.
    server
        .delete(&owner, &format!("/api/v1/channels/{}", f.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    let events = server.traffic(&reader, &mut stream).await;
    assert_eq!(refreshes(&events, workspace_id), 0, "{events:?}");
    let places: Vec<i32> = root(&server, &reader, workspace_id)
        .await
        .iter()
        .map(|(_, position)| *position)
        .collect();
    assert_eq!(
        places,
        (0..i32::try_from(places.len()).unwrap()).collect::<Vec<_>>()
    );
}
