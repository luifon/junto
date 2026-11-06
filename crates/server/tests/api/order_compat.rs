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

/// The channels of a group as a person reads them, in the order of the window.
async fn group(
    server: &TestServer,
    member: &Member,
    workspace_id: WorkspaceId,
    category_id: Option<CategoryId>,
) -> Vec<(ChannelId, i32)> {
    let mut held: Vec<(ChannelId, i32)> = list(server, member, workspace_id)
        .await
        .channels
        .iter()
        .filter(|channel| channel.category_id == category_id)
        .map(|channel| (channel.id, channel.position))
        .collect();
    held.sort_by_key(|(id, position)| (*position, *id));
    held
}

/// The moves that the window of release 0.1.0 sends for a drop of `moving` at the place `to`
/// of the group `into` (`plan` of `crates/client/src/state/admin/view.rs` at that release):
/// a move of every channel that `placed` changes, from where the window read it, and the
/// channels next to the place as checks.
/// The channels of each group as a person read them.
type Read = [(Option<CategoryId>, Vec<(ChannelId, i32)>)];

fn plan_of_0_1_0(
    read: &Read,
    moving: ChannelId,
    into: Option<CategoryId>,
    to: usize,
) -> Vec<serde_json::Value> {
    let place_of = |id: ChannelId| {
        read.iter()
            .find_map(|(group, held)| {
                held.iter()
                    .find(|(held, _)| *held == id)
                    .map(|(_, position)| json!({"category_id": group, "position": position}))
            })
            .unwrap()
    };
    let order = &read.iter().find(|(group, _)| *group == into).unwrap().1;
    let others: Vec<ChannelId> = order
        .iter()
        .map(|(id, _)| *id)
        .filter(|id| *id != moving)
        .collect();
    let at = to.min(others.len());
    let mut moves: Vec<serde_json::Value> = placed(order, moving, to)
        .into_iter()
        .map(|(id, position)| {
            json!({
                "channel_id": id,
                "from": place_of(id),
                "to": {"category_id": into, "position": position},
            })
        })
        .collect();
    if let Some(own) = moves
        .iter()
        .position(|held| held["channel_id"] == json!(moving))
    {
        let own = moves.remove(own);
        moves.insert(0, own);
    }
    let neighbours = at
        .checked_sub(1)
        .and_then(|before| others.get(before))
        .into_iter()
        .chain(others.get(at));
    for id in neighbours {
        if moves.iter().all(|held| held["channel_id"] != json!(id)) {
            moves.push(json!({"channel_id": id, "from": place_of(*id), "to": place_of(*id)}));
        }
    }
    moves
}

/// A drag of the window of 0.1.0 sends one request with a move of every changed channel and
/// checks of the channels next to the place, from the positions that it read; the channels
/// end in the order that the window drew, inside a group and between the root and two
/// categories.
#[tokio::test]
async fn a_drag_of_a_0_1_0_window_puts_the_channel_where_it_was_dropped() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace_id = server.default_workspace(&owner).await.id;
    let first = server.category(&owner, workspace_id, "First", 0).await;
    let second = server.category(&owner, workspace_id, "Second", 1).await;
    for name in ["a", "b", "c", "d"] {
        server.channel(&owner, workspace_id, name).await;
    }
    for (category, name) in [(first.id, "f1"), (first.id, "f2"), (second.id, "s1")] {
        let _: Channel = server
            .post(
                &owner,
                &format!("/api/v1/workspaces/{workspace_id}/channels"),
                &json!({"name": name, "kind": "text", "category_id": category}),
            )
            .await
            .ok(StatusCode::CREATED);
    }
    let groups = [None, Some(first.id), Some(second.id)];
    let mut state: u64 = 11;
    let mut next = |bound: usize| {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        usize::try_from(state >> 33).unwrap() % bound
    };
    for _ in 0..40 {
        let mut read = Vec::new();
        for group_id in groups {
            read.push((
                group_id,
                group(&server, &owner, workspace_id, group_id).await,
            ));
        }
        let from_group = next(3);
        let into = next(3);
        if read[from_group].1.is_empty() {
            continue;
        }
        let from = next(read[from_group].1.len());
        let (moving, _) = read[from_group].1[from];
        let size = read[into].1.len() + usize::from(from_group != into);
        let to = next(size);
        let mut wanted: Vec<Vec<ChannelId>> = read
            .iter()
            .map(|(_, held)| held.iter().map(|(id, _)| *id).collect())
            .collect();
        wanted[from_group].retain(|id| *id != moving);
        let at = to.min(wanted[into].len());
        wanted[into].insert(at, moving);
        let moves = plan_of_0_1_0(&read, moving, groups[into], to);
        if moves.iter().all(|held| held["to"] == held["from"]) {
            continue;
        }
        server
            .patch(
                &owner,
                &format!("/api/v1/workspaces/{workspace_id}/channels/positions"),
                &json!({ "moves": moves }),
            )
            .await
            .empty(StatusCode::NO_CONTENT);
        for (index, group_id) in groups.into_iter().enumerate() {
            let after: Vec<ChannelId> = group(&server, &owner, workspace_id, group_id)
                .await
                .iter()
                .map(|(id, _)| *id)
                .collect();
            assert_eq!(
                after, wanted[index],
                "group {index}: {from_group}/{from} to {into}/{to}"
            );
        }
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

/// Denies a member "view channel" in a channel by a rule of that person.
async fn hide_from(
    server: &TestServer,
    workspace_id: WorkspaceId,
    member: &Member,
    channel: ChannelId,
) {
    sqlx::query("insert into channel_person_overrides(channel_id,workspace_id,account_id,allow,deny) values($1,$2,$3,0,1)")
        .bind(channel).bind(workspace_id).bind(member.account_id).execute(&server.db).await.unwrap();
}

/// The positions of a list, which must be 0 to n-1.
fn without_gap(positions: &[i32]) -> bool {
    positions
        .iter()
        .copied()
        .eq(0..i32::try_from(positions.len()).unwrap())
}

/// An entry that leaves a group leaves no gap in the stored positions: a member who did not
/// see it, and sees every remaining entry afterwards, reads the stored positions, which then
/// do not show where the hidden entry was (ADR 0304). The paths: a deletion, the deletion of
/// an archived channel, a move to another category, and the deletion of a category.
#[tokio::test]
async fn an_entry_that_leaves_a_group_leaves_no_gap() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let member = server.join(&owner, "Member").await;
    let workspace_id = server.default_workspace(&owner).await.id;
    let positions = |list: Vec<(ChannelId, i32)>| list.iter().map(|(_, p)| *p).collect::<Vec<_>>();

    // A deletion of a root channel that the member does not see.
    let secret = server.channel(&owner, workspace_id, "secret").await;
    server.channel(&owner, workspace_id, "random").await;
    hide_from(&server, workspace_id, &member, secret.id).await;
    server
        .delete(&owner, &format!("/api/v1/channels/{}", secret.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    let read = positions(root(&server, &member, workspace_id).await);
    assert!(without_gap(&read), "{read:?}");

    // The deletion of an archived channel that stood before a listed one.
    let archived = server.channel(&owner, workspace_id, "archived").await;
    server.channel(&owner, workspace_id, "after").await;
    hide_from(&server, workspace_id, &member, archived.id).await;
    let _: serde_json::Value = server
        .put(
            &owner,
            &format!("/api/v1/channels/{}/archive", archived.id),
            &json!({}),
        )
        .await
        .ok(StatusCode::OK);
    server
        .delete(&owner, &format!("/api/v1/channels/{}", archived.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    let read = positions(root(&server, &member, workspace_id).await);
    assert!(without_gap(&read), "{read:?}");

    // A move of a hidden channel into a category.
    let moved = server.channel(&owner, workspace_id, "moved").await;
    server.channel(&owner, workspace_id, "last").await;
    hide_from(&server, workspace_id, &member, moved.id).await;
    let category = server.category(&owner, workspace_id, "Elsewhere", 0).await;
    let _: Channel = server
        .post(
            &owner,
            &format!("/api/v1/workspaces/{workspace_id}/channels"),
            &json!({"name": "seen", "kind": "text", "category_id": category.id}),
        )
        .await
        .ok(StatusCode::CREATED);
    let _: Channel = server
        .patch(
            &owner,
            &format!("/api/v1/channels/{}", moved.id),
            &json!({"category_id": category.id}),
        )
        .await
        .ok(StatusCode::OK);
    let read = positions(root(&server, &member, workspace_id).await);
    assert!(without_gap(&read), "{read:?}");

    // The deletion of a category that the member does not see.
    let first = server.category(&owner, workspace_id, "First", 9).await;
    let staff = server.category(&owner, workspace_id, "Staff", 10).await;
    let last = server.category(&owner, workspace_id, "Last", 11).await;
    for held in [&first, &last] {
        let _: Channel = server
            .post(
                &owner,
                &format!("/api/v1/workspaces/{workspace_id}/channels"),
                &json!({"name": "open", "kind": "text", "category_id": held.id}),
            )
            .await
            .ok(StatusCode::CREATED);
    }
    server
        .delete(
            &owner,
            &format!("/api/v1/workspaces/{workspace_id}/categories/{}", staff.id),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let read: Vec<i32> = categories(&server, &member, workspace_id)
        .await
        .iter()
        .map(|(_, p)| *p)
        .collect();
    assert!(without_gap(&read), "{read:?}");
    let all: Vec<i32> = categories(&server, &owner, workspace_id)
        .await
        .iter()
        .map(|(_, p)| *p)
        .collect();
    assert!(without_gap(&all), "{all:?}");
}

/// A new entry that a person may not see makes every person of the workspace read the
/// structure again when the numbers of that person change: from the stored positions of the
/// group to places, which differ where an archived channel stands before a listed one or a
/// position was stored as given (ADR 0304). An empty category is listed only to the people
/// who manage channels; a new channel of a category takes the rules of the category, which
/// hide it from the member here. A root channel that every person sees records none.
#[tokio::test]
async fn a_new_entry_hidden_from_a_person_refreshes_the_workspace() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let member = server.join(&owner, "Member").await;
    let workspace_id = server.default_workspace(&owner).await.id;
    // A position stored as given, as a client of API v1 stores it: 5 for the only category.
    let given = server.category(&owner, workspace_id, "Given", 5).await;
    let mut made = Vec::new();
    for name in ["first", "listed"] {
        let channel: Channel = server
            .post(
                &owner,
                &format!("/api/v1/workspaces/{workspace_id}/channels"),
                &json!({"name": name, "kind": "text", "category_id": given.id}),
            )
            .await
            .ok(StatusCode::CREATED);
        made.push(channel);
    }
    let _: serde_json::Value = server
        .put(
            &owner,
            &format!("/api/v1/channels/{}/archive", made[0].id),
            &json!({}),
        )
        .await
        .ok(StatusCode::OK);
    // The category denies the member "view channel"; its two channels have rules of their own
    // and stay visible to the member, a new channel takes the rules of the category.
    sqlx::query("update channels set permissions_synced = false where category_id = $1")
        .bind(given.id)
        .execute(&server.db)
        .await
        .unwrap();
    sqlx::query("insert into category_person_overrides(category_id,account_id,workspace_id,allow,deny) values($1,$2,$3,0,1)")
        .bind(given.id).bind(member.account_id).bind(workspace_id).execute(&server.db).await.unwrap();
    let listed_position = |list: ChannelList| {
        list.channels
            .iter()
            .find(|channel| channel.id == made[1].id)
            .map(|channel| channel.position)
    };
    assert_eq!(
        categories(&server, &member, workspace_id).await,
        [(given.id, 5)]
    );
    assert_eq!(
        listed_position(list(&server, &member, workspace_id).await),
        Some(1)
    );
    let mut stream = server.subscribed(&member, Level::Full).await;
    let _: Category = server
        .post(
            &owner,
            &format!("/api/v1/workspaces/{workspace_id}/categories"),
            &json!({"name": "New"}),
        )
        .await
        .ok(StatusCode::CREATED);
    let events = server.traffic(&member, &mut stream).await;
    assert_eq!(refreshes(&events, workspace_id), 1, "{events:?}");
    assert_eq!(
        categories(&server, &member, workspace_id).await,
        [(given.id, 0)]
    );
    let _: Channel = server
        .post(
            &owner,
            &format!("/api/v1/workspaces/{workspace_id}/channels"),
            &json!({"name": "inside", "kind": "text", "category_id": given.id}),
        )
        .await
        .ok(StatusCode::CREATED);
    let events = server.traffic(&member, &mut stream).await;
    assert_eq!(refreshes(&events, workspace_id), 1, "{events:?}");
    assert_eq!(
        listed_position(list(&server, &member, workspace_id).await),
        Some(0)
    );
    server.channel(&owner, workspace_id, "open").await;
    let events = server.traffic(&member, &mut stream).await;
    assert_eq!(refreshes(&events, workspace_id), 0, "{events:?}");
}

/// The channels of a deleted category go after the channels of the root in their stored
/// order, whatever positions were stored as given: a negative one, as a drag of the window
/// of 0.1.0 to the top of a group stores, one near the largest position, and the largest
/// position in the root (ADR 0304).
#[tokio::test]
async fn the_channels_of_a_deleted_category_go_after_the_root_in_their_order() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace_id = server.default_workspace(&owner).await.id;
    let category = server.category(&owner, workspace_id, "Projects", 0).await;
    let mut made = Vec::new();
    for (name, position) in [("top", -3), ("last", 2_147_483_645)] {
        let channel: Channel = server
            .post(
                &owner,
                &format!("/api/v1/workspaces/{workspace_id}/channels"),
                &json!({"name": name, "kind": "text", "category_id": category.id}),
            )
            .await
            .ok(StatusCode::CREATED);
        let _: Channel = server
            .patch(
                &owner,
                &format!("/api/v1/channels/{}", channel.id),
                &json!({"position": position}),
            )
            .await
            .ok(StatusCode::OK);
        made.push(channel.id);
    }
    // The last root channel at the largest position that the API accepts.
    let largest = server.channel(&owner, workspace_id, "largest").await;
    let _: Channel = server
        .patch(
            &owner,
            &format!("/api/v1/channels/{}", largest.id),
            &json!({"position": i32::MAX}),
        )
        .await
        .ok(StatusCode::OK);
    let root_before: Vec<ChannelId> = root(&server, &owner, workspace_id)
        .await
        .iter()
        .map(|(id, _)| *id)
        .collect();
    server
        .delete(
            &owner,
            &format!(
                "/api/v1/workspaces/{workspace_id}/categories/{}",
                category.id
            ),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let after = root(&server, &owner, workspace_id).await;
    let ids: Vec<ChannelId> = after.iter().map(|(id, _)| *id).collect();
    let mut wanted = root_before;
    wanted.extend(made);
    assert_eq!(ids, wanted);
    let positions: Vec<i32> = after.iter().map(|(_, position)| *position).collect();
    assert!(without_gap(&positions), "{positions:?}");
}

/// The changes of one structure apply refer to the numbers that the file was made from: a
/// group that a channel leaves is numbered once, after the last change, and not before a
/// later change of the same apply sets a position in it (ADR 0304).
#[tokio::test]
async fn a_structure_apply_numbers_a_group_that_a_channel_left_after_its_last_change() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace_id = server.default_workspace(&owner).await.id;
    let projects = server.category(&owner, workspace_id, "Projects", 0).await;
    let other = server.category(&owner, workspace_id, "Other", 1).await;
    for name in ["a", "b", "c", "d"] {
        let _: Channel = server
            .post(
                &owner,
                &format!("/api/v1/workspaces/{workspace_id}/channels"),
                &json!({"name": name, "kind": "text", "category_id": projects.id}),
            )
            .await
            .ok(StatusCode::CREATED);
    }
    let export = server.structure(&owner).await;
    let entry = |name: &str| {
        export.structure.workspaces[0]
            .channels
            .iter()
            .find(|channel| channel.name == name)
            .unwrap()
            .id
    };
    let category = export.structure.workspaces[0]
        .categories
        .iter()
        .find(|category| category.name == other.name)
        .unwrap()
        .id;
    // The wanted order of Projects is a, d, c: the file moves b away first, then gives d the
    // position 1 between a at 0 and c at 2.
    let changes = json!([
        { "change": "update_channel", "id": entry("b"), "category": category },
        { "change": "update_channel", "id": entry("d"), "position": 1 },
    ]);
    let _: serde_json::Value = server
        .change_structure(&owner, &export.revision, changes)
        .await
        .ok(StatusCode::OK);
    let list = list(&server, &owner, workspace_id).await;
    let mut group: Vec<(i32, ChannelId, String)> = list
        .channels
        .iter()
        .filter(|channel| channel.category_id == Some(projects.id))
        .map(|channel| (channel.position, channel.id, channel.name.clone()))
        .collect();
    group.sort();
    let names: Vec<&str> = group.iter().map(|(_, _, name)| name.as_str()).collect();
    assert_eq!(names, ["a", "d", "c"]);
    let positions: Vec<i32> = group.iter().map(|(position, _, _)| *position).collect();
    assert!(without_gap(&positions), "{positions:?}");
}

/// A group whose last entry holds the largest position that the API accepts, as a client of
/// API v1 can store it, takes a new entry at its end: a created channel, a channel moved into
/// it and a created category go last, and no request fails (ADR 0304).
#[tokio::test]
async fn a_group_at_the_largest_position_takes_a_new_entry_at_its_end() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace_id = server.default_workspace(&owner).await.id;
    let largest = server.channel(&owner, workspace_id, "largest").await;
    let _: Channel = server
        .patch(
            &owner,
            &format!("/api/v1/channels/{}", largest.id),
            &json!({"position": i32::MAX}),
        )
        .await
        .ok(StatusCode::OK);
    let created = server.channel(&owner, workspace_id, "created").await;
    let projects = server.category(&owner, workspace_id, "Projects", 0).await;
    let moved: Channel = server
        .post(
            &owner,
            &format!("/api/v1/workspaces/{workspace_id}/channels"),
            &json!({"name": "moved", "kind": "text", "category_id": projects.id}),
        )
        .await
        .ok(StatusCode::CREATED);
    let _: Channel = server
        .patch(
            &owner,
            &format!("/api/v1/channels/{}", moved.id),
            &json!({"category_id": null}),
        )
        .await
        .ok(StatusCode::OK);
    let order: Vec<ChannelId> = root(&server, &owner, workspace_id)
        .await
        .iter()
        .map(|(id, _)| *id)
        .collect();
    assert_eq!(order[order.len() - 3..], [largest.id, created.id, moved.id]);

    let _: Category = server
        .patch(
            &owner,
            &format!(
                "/api/v1/workspaces/{workspace_id}/categories/{}",
                projects.id
            ),
            &json!({"position": i32::MAX}),
        )
        .await
        .ok(StatusCode::OK);
    let last = server.category(&owner, workspace_id, "Last", 0).await;
    let _: Category = server
        .patch(
            &owner,
            &format!("/api/v1/workspaces/{workspace_id}/categories/{}", last.id),
            &json!({"position": i32::MAX}),
        )
        .await
        .ok(StatusCode::OK);
    let after: Category = server
        .post(
            &owner,
            &format!("/api/v1/workspaces/{workspace_id}/categories"),
            &json!({"name": "After"}),
        )
        .await
        .ok(StatusCode::CREATED);
    let order: Vec<CategoryId> = categories(&server, &owner, workspace_id)
        .await
        .iter()
        .map(|(id, _)| *id)
        .collect();
    assert_eq!(order.last(), Some(&after.id));
}
