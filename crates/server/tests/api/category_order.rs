use crate::stream_support::Stream;
use crate::support::{Member, TestServer};
use junto_protocol::WorkspacePermission as W;
use junto_protocol::events::{Event, Level};
use junto_protocol::ids::{CategoryId, WorkspaceId};
use junto_protocol::structure::{Category, Channel, ChannelList};
use junto_protocol::structure_file::StructureExport;
use reqwest::StatusCode;
use serde_json::json;

/// Three categories in the order First, Hidden, Last, each with a channel. The reader does
/// not see Hidden; the owner sees all three.
struct Scene {
    server: TestServer,
    owner: Member,
    reader: Member,
    workspace_id: WorkspaceId,
    first: Category,
    hidden: Category,
    last: Category,
}

async fn scene() -> Scene {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let workspace_id = server.default_workspace(&owner).await.id;
    let mut made = Vec::new();
    for name in ["First", "Hidden", "Last"] {
        let category: Category = server
            .post(
                &owner,
                &format!("/api/v1/workspaces/{workspace_id}/categories"),
                &json!({ "name": name }),
            )
            .await
            .ok(StatusCode::CREATED);
        let _: Channel = server
            .post(
                &owner,
                &format!("/api/v1/workspaces/{workspace_id}/channels"),
                &json!({"name": name, "kind": "text", "category_id": category.id}),
            )
            .await
            .ok(StatusCode::CREATED);
        made.push(category);
    }
    let [first, hidden, last] = made.try_into().unwrap();
    sqlx::query("insert into category_person_overrides(category_id,account_id,workspace_id,allow,deny) values($1,$2,$3,0,1)")
        .bind(hidden.id).bind(reader.account_id).bind(workspace_id).execute(&server.db).await.unwrap();
    Scene {
        server,
        owner,
        reader,
        workspace_id,
        first,
        hidden,
        last,
    }
}

impl Scene {
    async fn listed(&self, member: &Member) -> Vec<(CategoryId, i32)> {
        let list: ChannelList = self
            .server
            .get(
                member,
                &format!("/api/v1/workspaces/{}/channels", self.workspace_id),
            )
            .await
            .ok(StatusCode::OK);
        let from_list: Vec<_> = list.categories.iter().map(|c| (c.id, c.position)).collect();
        let snapshot = self.server.snapshot(member).await;
        let from_snapshot: Vec<_> = snapshot
            .workspaces
            .iter()
            .find(|held| held.workspace.id == self.workspace_id)
            .unwrap()
            .categories
            .iter()
            .map(|c| (c.id, c.position))
            .collect();
        assert_eq!(from_list, from_snapshot);
        from_list
    }

    fn category_path(&self, category: &Category) -> String {
        format!(
            "/api/v1/workspaces/{}/categories/{}",
            self.workspace_id, category.id
        )
    }

    async fn updated_position(&self, member: &Member, stream: &mut Stream, id: CategoryId) -> i32 {
        let events = self.server.traffic(member, stream).await;
        events
            .iter()
            .find_map(|(_, event)| match event {
                Event::CategoryUpdated { category } if category.id == id => Some(category.position),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{events:?}"))
    }
}

#[tokio::test]
async fn a_member_reads_category_positions_with_no_gap_of_a_hidden_category() {
    let scene = scene().await;
    assert_eq!(
        scene.listed(&scene.reader).await,
        [(scene.first.id, 0), (scene.last.id, 1)]
    );
    assert_eq!(
        scene.listed(&scene.owner).await,
        [
            (scene.first.id, 0),
            (scene.hidden.id, 1),
            (scene.last.id, 2)
        ]
    );

    let mut reader_stream = scene.server.subscribed(&scene.reader, Level::Full).await;
    let mut owner_stream = scene.server.subscribed(&scene.owner, Level::Full).await;
    let renamed: Category = scene
        .server
        .patch(
            &scene.owner,
            &scene.category_path(&scene.last),
            &json!({"name": "Later"}),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(renamed.position, 2);
    let id = scene.last.id;
    assert_eq!(
        scene
            .updated_position(&scene.reader, &mut reader_stream, id)
            .await,
        1
    );
    assert_eq!(
        scene
            .updated_position(&scene.owner, &mut owner_stream, id)
            .await,
        2
    );
}

/// A deleted category leaves no gap: the remaining categories are numbered from 0 again, for
/// the owner, who reads the stored positions, and in the export (ADR 0304).
#[tokio::test]
async fn a_deleted_category_leaves_no_gap() {
    let scene = scene().await;
    scene
        .server
        .delete(&scene.owner, &scene.category_path(&scene.first))
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(
        scene.listed(&scene.owner).await,
        [(scene.hidden.id, 0), (scene.last.id, 1)]
    );
    assert_eq!(scene.listed(&scene.reader).await, [(scene.last.id, 0)]);
    let export: StructureExport = scene
        .server
        .get(&scene.owner, "/api/v1/structure")
        .await
        .ok(StatusCode::OK);
    let positions: Vec<_> = export
        .structure
        .workspaces
        .iter()
        .flat_map(|workspace| workspace.categories.iter().map(|c| c.position))
        .collect();
    assert_eq!(positions, [Some(0), Some(1)]);
}

#[tokio::test]
async fn a_position_is_a_place_among_the_categories_that_the_caller_sees() {
    let scene = scene().await;
    scene
        .server
        .grant_workspace(
            &scene.owner,
            &scene.reader,
            scene.workspace_id,
            &[W::ManageChannel],
        )
        .await;
    let mut owner_stream = scene.server.subscribed(&scene.owner, Level::Full).await;
    // The reader sees First and Last; place 0 is before First.
    let moved: Category = scene
        .server
        .patch(
            &scene.reader,
            &scene.category_path(&scene.last),
            &json!({"place": 0}),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(moved.position, 0);
    assert_eq!(
        scene.listed(&scene.reader).await,
        [(scene.last.id, 0), (scene.first.id, 1)]
    );
    assert_eq!(
        scene.listed(&scene.owner).await,
        [
            (scene.last.id, 0),
            (scene.first.id, 1),
            (scene.hidden.id, 2)
        ]
    );
    // The places of the other categories changed for the owner, who reads the structure
    // again.
    let events = scene.server.traffic(&scene.owner, &mut owner_stream).await;
    let refreshes = events
        .iter()
        .filter(|(_, event)| {
            matches!(event, Event::PermissionsChanged { workspace_id: Some(id) } if *id == scene.workspace_id)
        })
        .count();
    assert_eq!(refreshes, 1, "{events:?}");
    assert!(
        events.iter().any(|(_, event)| matches!(
            event,
            Event::CategoryUpdated { category } if category.id == scene.last.id && category.position == 0
        )),
        "{events:?}"
    );

    // A place past the last places the category after the last that the caller sees; the
    // hidden category keeps its place after First.
    let moved: Category = scene
        .server
        .patch(
            &scene.reader,
            &scene.category_path(&scene.last),
            &json!({"place": 7}),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(moved.position, 1);
    assert_eq!(
        scene.listed(&scene.owner).await,
        [
            (scene.first.id, 0),
            (scene.last.id, 1),
            (scene.hidden.id, 2)
        ]
    );

    // A new category at a position of API v1 in the caller's view: 0 with First, which
    // the identifier of First orders first. The hidden category stays at the end, where no
    // category that the request leaves in place follows it.
    let created: Category = scene
        .server
        .post(
            &scene.reader,
            &format!("/api/v1/workspaces/{}/categories", scene.workspace_id),
            &json!({"name": "New", "position": 0}),
        )
        .await
        .ok(StatusCode::CREATED);
    assert_eq!(created.position, 1);
    assert_eq!(
        scene.listed(&scene.owner).await,
        [
            (scene.first.id, 0),
            (created.id, 1),
            (scene.last.id, 2),
            (scene.hidden.id, 3)
        ]
    );
}

/// The channels of a category as a person reads them: identifier and position.
async fn channels_in(
    server: &TestServer,
    member: &Member,
    workspace_id: WorkspaceId,
    category_id: CategoryId,
) -> Vec<(junto_protocol::ids::ChannelId, i32)> {
    let list: ChannelList = server
        .get(
            member,
            &format!("/api/v1/workspaces/{workspace_id}/channels"),
        )
        .await
        .ok(StatusCode::OK);
    list.channels
        .iter()
        .filter(|channel| channel.category_id == Some(category_id))
        .map(|channel| (channel.id, channel.position))
        .collect()
}

/// The position of a channel is its place among the channels of its group that the reader
/// sees, in the lists, the events and the answers, and a request names a place in the view
/// of the caller (ADR 0304).
#[tokio::test]
async fn a_member_reads_channel_positions_with_no_gap_of_a_hidden_channel() {
    let scene = scene().await;
    let (server, owner, reader) = (&scene.server, &scene.owner, &scene.reader);
    let workspace_id = scene.workspace_id;
    let category = scene.first.id;
    let mut made = Vec::new();
    for name in ["one", "hidden", "two"] {
        let channel: Channel = server
            .post(
                owner,
                &format!("/api/v1/workspaces/{workspace_id}/channels"),
                &json!({"name": name, "kind": "text", "category_id": category}),
            )
            .await
            .ok(StatusCode::CREATED);
        made.push(channel);
    }
    // The category holds "First" (made by the scene), "one", "hidden" and "two".
    let first = channels_in(server, owner, workspace_id, category).await[0].0;
    let [one, hidden, two] = <[Channel; 3]>::try_from(made).unwrap();
    sqlx::query("update channels set permissions_synced = false where id = $1")
        .bind(hidden.id)
        .execute(&server.db)
        .await
        .unwrap();
    sqlx::query("insert into channel_person_overrides(channel_id,account_id,workspace_id,allow,deny) values($1,$2,$3,0,1)")
        .bind(hidden.id).bind(reader.account_id).bind(workspace_id).execute(&server.db).await.unwrap();
    assert_eq!(
        channels_in(server, reader, workspace_id, category).await,
        [(first, 0), (one.id, 1), (two.id, 2)]
    );
    assert_eq!(
        channels_in(server, owner, workspace_id, category).await,
        [(first, 0), (one.id, 1), (hidden.id, 2), (two.id, 3)]
    );
    let snapshot = server.snapshot(reader).await;
    let held = snapshot
        .workspaces
        .iter()
        .find(|held| held.workspace.id == workspace_id)
        .unwrap()
        .channels
        .iter()
        .find(|held| held.id == two.id)
        .unwrap()
        .position;
    assert_eq!(held, 2);

    // A rename reaches each reader with the place in that reader's view.
    let mut reader_stream = server.subscribed(reader, Level::Full).await;
    let renamed: Channel = server
        .patch(
            owner,
            &format!("/api/v1/channels/{}", two.id),
            &json!({"name": "second"}),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(renamed.position, 3);
    let events = server.traffic(reader, &mut reader_stream).await;
    assert!(
        events.iter().any(|(_, event)| matches!(
            event,
            Event::ChannelUpdated { channel } if channel.id == two.id && channel.position == 2
        )),
        "{events:?}"
    );

    // A manager who does not see "hidden" puts "two" at the place 1 of the view without
    // "two": before "one". The positions route checks the place where the manager read it.
    server
        .grant_workspace(owner, reader, workspace_id, &[W::ManageChannel])
        .await;
    server
        .patch(
            reader,
            &format!("/api/v1/workspaces/{workspace_id}/channels/positions"),
            &json!({"moves": [{
                "channel_id": two.id,
                "from": {"category_id": category, "position": 2},
                "to": {"category_id": category, "position": 1, "place": 1},
            }]}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(
        channels_in(server, reader, workspace_id, category).await,
        [(first, 0), (two.id, 1), (one.id, 2)]
    );
    assert_eq!(
        channels_in(server, owner, workspace_id, category).await,
        [(first, 0), (two.id, 1), (one.id, 2), (hidden.id, 3)]
    );
}
