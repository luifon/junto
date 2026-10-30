use gpui::TestAppContext;
use junto_client_core::model::PlaceId;
use junto_client_core::protocol::message::SendMessage;
use junto_client_core::transfers::{DownloadStage, UploadSource};

use crate::harness::{World, member};
use crate::messages::document;
use crate::pictures::send_files;

#[gpui::test]
fn an_open_file_list_withdraws_changed_files_and_discards_a_delayed_page(cx: &mut TestAppContext) {
    use sqlx::ConnectOptions;
    use std::str::FromStr;
    let world = World::new();
    let server = world.server("Example Studio");
    let admin = world.owner(&server);
    let proxy = world.run(crate::support::FaultProxy::start(&server));
    let (_directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Rin");
    window.until("history is loaded", |window| {
        window.feed().is_some_and(|feed| feed.loaded)
    });
    let channel = window.selected_channel();
    let deleted = send_files(
        &world,
        &admin,
        PlaceId::Channel(channel),
        "Delete this",
        vec![("deleted.txt", vec![1])],
    );
    let changed = send_files(
        &world,
        &admin,
        PlaceId::Channel(channel),
        "Keep one file",
        vec![("removed.txt", vec![2]), ("kept.txt", vec![3])],
    );
    let gone = changed.attachments[0].id;
    let kept = changed.attachments[1].id;
    window.until_message("Keep one file");
    window.click("place-files");
    window.until("both files are listed", |window| {
        window.shows_named(format!("listed-file:{gone}"))
            && window.shows_named(format!("listed-file:{}", deleted.attachments[0].id))
    });
    proxy.hold_answer_ending("GET", "/search");
    window.in_shell(|shell, _, cx| shell.state().update(cx, |state, cx| state.search_again(cx)));
    window.until("the earlier page waits at the proxy", |_| {
        proxy.answers_waiting() == 1
    });
    world
        .run(admin.handle.api().delete_message(deleted.id))
        .unwrap();
    world.run(async {
        let mut db = sqlx::postgres::PgConnectOptions::from_str(server.database_url())
            .unwrap()
            .connect()
            .await
            .unwrap();
        sqlx::query("update attachments set removed_at=now() where id=$1")
            .bind(gone)
            .execute(&mut db)
            .await
            .unwrap();
        admin
            .handle
            .api()
            .edit_message(changed.id, document("One file remains"))
            .await
            .unwrap();
    });
    window.until_message("One file remains");
    window.until(
        "changed file metadata is withdrawn while fresh pages wait",
        |window| {
            !window.shows_named(format!("listed-file:{gone}"))
                && !window.shows_named(format!("listed-file:{}", deleted.attachments[0].id))
                && window
                    .view()
                    .workspace()
                    .and_then(|workspace| workspace.search.as_ref())
                    .is_some_and(|panel| {
                        panel.state == junto_client::state::search::SearchShown::Searching
                    })
        },
    );
    window.until("replacement pages wait too", |_| {
        proxy.answers_waiting() >= 2
    });
    let old = proxy
        .all()
        .iter()
        .position(|seen| {
            seen.method == "GET"
                && seen.path.starts_with("/api/v1/search?")
                && seen.answer.is_none()
        })
        .unwrap();
    proxy.release_answers_except(old);
    window.until(
        "the surviving attachment returns from fresh metadata",
        |window| window.shows_named(format!("listed-file:{kept}")),
    );
    proxy.release();
    window.until(
        "every search answer, including the oldest, was received",
        |window| {
            let requests = proxy.all();
            let pages: Vec<_> = requests
                .iter()
                .filter(|seen| seen.method == "GET" && seen.path.starts_with("/api/v1/search?"))
                .collect();
            requests[old].answer.is_some()
                && pages.iter().all(|seen| seen.answer.is_some())
                && window
                    .state
                    .read_with(&window.cx, |state, _| state.search_answers())
                    == pages.len() as u64
        },
    );
    assert!(!window.shows_named(format!("listed-file:{gone}")));
    assert!(!window.shows_named(format!("listed-file:{}", deleted.attachments[0].id)));
    assert!(window.shows_named(format!("listed-file:{kept}")));
}

#[gpui::test]
fn channel_files_include_thread_files_and_offer_copy_download_and_the_message(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Example Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Rin");
    window.until("history is loaded", |window| {
        window.feed().is_some_and(|feed| feed.loaded)
    });
    let channel = window.selected_channel();
    let payload = b"The project notes".to_vec();
    let root = send_files(
        &world,
        &admin,
        PlaceId::Channel(channel),
        "The notes",
        vec![("notes.txt", payload.clone())],
    );
    let file = root.attachments[0].id;
    let reply = world.run(async {
        let id = admin
            .handle
            .transfers()
            .upload(
                PlaceId::Thread {
                    channel_id: channel,
                    root_id: root.id,
                },
                UploadSource::Bytes {
                    name: "thread.txt".to_owned(),
                    bytes: std::sync::Arc::new(b"A thread file".to_vec()),
                },
            )
            .finished()
            .await
            .unwrap();
        admin
            .handle
            .api()
            .reply_in_thread(
                root.id,
                &SendMessage {
                    document: document("The thread notes"),
                    reply_to: None,
                    attachments: vec![id],
                },
            )
            .await
            .unwrap()
    });
    let other = admin.text_channel(&world, &mut window, "other");
    let excluded = send_files(
        &world,
        &admin,
        PlaceId::Channel(other),
        "Other notes",
        vec![("other.txt", b"Other notes".to_vec())],
    );
    window.until_message("The notes");
    window.click("place-files");
    window.until("channel and thread files are listed", |window| {
        window.shows_named(format!("listed-file:{file}"))
            && window.shows_named(format!("listed-file:{}", reply.attachments[0].id))
    });
    assert!(!window.shows_named(format!("listed-file:{}", excluded.attachments[0].id)));
    window.right_click_named(format!("listed-file:{file}"));
    window.click("context-action:Copy file name");
    assert_eq!(
        window.cx.read_from_clipboard().unwrap().text().unwrap(),
        "notes.txt"
    );
    let downloads = tempfile::tempdir().unwrap();
    let target = downloads.path().to_path_buf();
    window.in_shell(move |shell, _, cx| {
        shell
            .state()
            .update(cx, |state, _| state.set_download_directory(target));
    });
    window.click_named(format!("listed-download:{file}"));
    window.until("the listed file is saved", |window| {
        window.view().downloads.iter().any(|((_, id), progress)| {
            *id == file && matches!(progress.stage, DownloadStage::Saved(_))
        })
    });
    assert_eq!(
        std::fs::read(downloads.path().join("notes.txt")).unwrap(),
        payload
    );
    window.click_named(format!("search-result:{}", root.id));
    window.until("the original message is marked", |window| {
        window
            .messages()
            .iter()
            .any(|row| row.message.id == root.id && row.target)
    });
    window.click_named(format!("search-result:{}", reply.id));
    window.until("the thread file leads to its reply", |window| {
        window
            .view()
            .workspace()
            .and_then(|workspace| workspace.thread.as_ref())
            .is_some_and(|thread| thread.root_id == root.id)
            && window.replies().contains(&"The thread notes".to_owned())
    });
    assert!(!window.shows("file-list-panel"));
    window.click("close-thread");
    window.until("closing the thread restores the file browser", |window| {
        window.shows("file-list-panel")
    });
    let original = window
        .channels()
        .into_iter()
        .find(|row| row.id == channel)
        .unwrap()
        .name;
    window.click_channel("other");
    window.until("navigating away closes the old file scope", |window| {
        !window.shows("file-list-panel")
    });
    window.click_channel(&original);
    window.click("place-files");
    window.until("the original file scope can be reopened", |window| {
        window.shows_named(format!("listed-file:{file}"))
    });
    let workspace = window.view().workspace().unwrap().id;
    let roles = world
        .run(admin.handle.api().workspace_roles(workspace))
        .unwrap();
    let everyone = roles.iter().find(|role| role.is_everyone).unwrap().id;
    world
        .run(admin.handle.api().set_channel_rule(
            channel,
            junto_client_core::protocol::structure::RuleSubject::Role { role_id: everyone },
            &junto_client_core::protocol::structure::SetChannelOverride {
                allow: Vec::new(),
                deny: vec![junto_client_core::protocol::WorkspacePermission::ViewChannel],
            },
        ))
        .unwrap();
    window.until("the hidden source withdraws the file browser", |window| {
        !window.shows("file-list-panel")
    });
}

#[gpui::test]
fn direct_files_work_after_the_last_workspace_membership_is_removed(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Example Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Rin");
    let workspace = window.view().workspace().unwrap().id;
    let account = window.own_account();
    let api = admin.handle.api();
    let conversation = world.run(api.open_conversation(account)).unwrap();
    let message = send_files(
        &world,
        &admin,
        PlaceId::Direct(conversation.id),
        "A direct file",
        vec![("direct.txt", b"The direct notes".to_vec())],
    );
    window.until("the conversation is listed", |window| {
        window.shows_named(format!("direct-row:{}", conversation.id))
    });
    window.click_named(format!("direct-row:{}", conversation.id));
    world
        .run(api.remove_workspace_member(workspace, account, &Default::default()))
        .unwrap();
    window.until("the conversation has no workspace", |window| {
        window
            .view()
            .lists()
            .is_some_and(|lists| lists.workspace.is_none() && lists.conversation.is_some())
            && window.feed().is_some_and(|feed| feed.loaded)
    });
    assert!(window.shows("search-field"));
    window.search_for("direct");
    let searched = window.until_searched();
    assert!(searched.messages().iter().any(|row| row.target == junto_client::state::search::ResultTarget::Message(message.id)));
    window.click("search-direct");
    assert_eq!(window.until_searched().messages().len(), 1);
    window.click("search-has-file");
    assert_eq!(window.until_searched().messages().len(), 1);
    let author = message.author_id.unwrap();
    window.until(
        "the conversation participant is an author choice without a workspace",
        |window| {
            window
                .view()
                .lists()
                .and_then(|lists| lists.search.as_ref())
                .is_some_and(|panel| panel.authors.iter().any(|(id, _)| *id == author))
        },
    );
    window.in_shell(move |shell, _, cx| {
        shell.set_search_filters(
            junto_client::state::search::Filters {
                author: Some(author),
                direct: true,
                has_file: true,
                date: junto_client::state::search::DateFilter::Today,
                ..Default::default()
            },
            cx,
        )
    });
    assert_eq!(window.until_searched().messages().len(), 1);
    window.search_for("");
    assert_eq!(window.until_searched().messages().len(), 1);
    assert!(window.shows("search-panel"));
    assert!(!window.shows("file-list-panel"));
    window.click("close-search");

    window.click("place-files");
    window.until("the direct file is listed without a workspace", |window| {
        window.shows_named(format!("listed-file:{}", message.attachments[0].id))
    });
    window.click_named(format!("listed-open:{}", message.attachments[0].id));
    window.until("the file is opened", |window| {
        window.shows("linked-file-dialog")
    });
}

#[gpui::test]
fn file_pages_show_older_and_newer_files_without_accumulating_history(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Example Studio");
    let admin = world.owner(&server);
    let proxy = world.run(crate::support::FaultProxy::start(&server));
    let (_directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Rin");
    window.until("history is loaded", |window| {
        window.feed().is_some_and(|feed| feed.loaded)
    });
    let channel = window.selected_channel();
    let mut ids = Vec::new();
    let mut oldest_message = None;
    for number in 0..51 {
        let name = format!("file-{number}.txt");
        let message = send_files(
            &world,
            &admin,
            PlaceId::Channel(channel),
            &format!("File {number}"),
            vec![(name.as_str(), vec![1, 2, 3])],
        );
        if number == 0 {
            oldest_message = Some(message.id);
        }
        ids.push(message.attachments[0].id);
    }
    window.until_message("File 50");
    window.click("place-files");
    let oldest = ids[0];
    let newest = ids[50];
    window.until("the newest page is shown", |window| {
        window.shows_named(format!("listed-file:{newest}"))
    });
    assert!(!window.shows_named(format!("listed-file:{oldest}")));
    let attempts = junto_client_core::api::RetryPolicy::default().attempts as usize;
    proxy.fail_ending(
        "/search",
        std::iter::repeat_n(crate::support::Fault::Busy, attempts),
    );
    window.click("more-results");
    window.until("the failed older page is stated", |window| {
        window.shows("search-go-failed")
    });
    assert!(window.shows_named(format!("listed-file:{newest}")));
    assert!(
        !window
            .view()
            .workspace()
            .unwrap()
            .search
            .as_ref()
            .unwrap()
            .previous_files
    );
    window.click("more-results");
    window.until("the older page replaces the newest page", |window| {
        window.shows_named(format!("listed-file:{oldest}"))
            && !window.shows_named(format!("listed-file:{newest}"))
    });
    assert!(!window.shows("search-go-failed"));
    window.click("previous-files");
    window.until("the newer page is restored", |window| {
        window.shows_named(format!("listed-file:{newest}"))
            && !window.shows_named(format!("listed-file:{oldest}"))
    });
    proxy.hold_events();
    proxy.hold_request("GET", "/search");
    let requests = proxy.all().len();
    window.click("more-results");
    window.until(
        "the older-file request waits before reaching the server",
        |_| {
            proxy
                .all()
                .iter()
                .skip(requests)
                .any(|seen| seen.method == "GET" && seen.path.starts_with("/api/v1/search"))
        },
    );
    world
        .run(admin.handle.api().delete_message(oldest_message.unwrap()))
        .unwrap();
    proxy.release_requests();
    window.until("the deleted older page is empty", |window| {
        window.shows("search-nothing")
    });
    window.click("previous-files");
    window.until("an empty older page can return to newer files", |window| {
        window.shows_named(format!("listed-file:{newest}"))
    });
    proxy.release_events();
}

#[gpui::test]
#[ignore = "needs the test media server"]
fn files_in_a_voice_call_keep_the_call_connected(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server_with_media("Example Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Rin");
    crate::calls::voice_channel(&world, &admin, &mut window, "Daily");
    window.click_channel("Daily");
    window.click("join-call");
    window.until("the voice call is connected", |window| {
        window
            .view()
            .call
            .is_some_and(|call| call.phase == junto_media::calls::Phase::Connected)
    });
    let identity = window.state.read_with(&window.cx, |state, _| {
        state.calls().view().call.unwrap().identity
    });
    let channel = window.selected_channel();
    let message = send_files(
        &world,
        &admin,
        PlaceId::Channel(channel),
        "Call notes",
        vec![("call.txt", b"Notes during a call".to_vec())],
    );
    window.click("place-files");
    window.until("the call files are listed", |window| {
        window.shows_named(format!("listed-file:{}", message.attachments[0].id))
    });
    assert_eq!(
        window.state.read_with(&window.cx, |state, _| state
            .calls()
            .view()
            .call
            .unwrap()
            .identity),
        identity
    );
    window.click_named(format!("search-result:{}", message.id));
    window.until("the call chat shows the original message", |window| {
        window.shows_named(format!("message:{}", message.id)) && !window.shows("file-list-panel")
    });
    assert_eq!(
        window.state.read_with(&window.cx, |state, _| state
            .calls()
            .view()
            .call
            .unwrap()
            .identity),
        identity
    );
    window.click("leave-call");
    window.until("the call ends", |window| window.view().call.is_none());
}
