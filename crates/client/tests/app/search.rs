//! Search in the window against the real server (ADR 0743, view H of the design): a query
//! finds messages and a line of a transcript, the filters narrow it, a result leads to its
//! message, a further page is read, and a result of a channel that the person can no
//! longer read leaves the panel.

use std::str::FromStr;

use gpui::TestAppContext;
use junto_client::state::notices::MuteFor;
use junto_client::state::notices::Whereabouts;
use junto_client::state::search::{DateFilter, Filters, ResultTarget, SearchPanel, SearchShown};
use junto_client::text;
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::ids::{AccountId, ChannelId, WorkspaceId};
use junto_client_core::protocol::structure::{ChannelKind, SetChannelOverride};
use sqlx::ConnectOptions;

use crate::harness::{Window, World, member};
use crate::messages::document;
use crate::support::{Fault, FaultProxy, TestServer};

/// Stores a finished recording of a voice channel with one transcript line, as the server
/// holds it after the transcription.
async fn seed_transcript(
    server: &TestServer,
    workspace_id: WorkspaceId,
    voice: ChannelId,
    speaker: AccountId,
    text: &str,
) -> uuid::Uuid {
    let options = sqlx::postgres::PgConnectOptions::from_str(&server.database_url).unwrap();
    let mut connection = options.connect().await.unwrap();
    let recording = uuid::Uuid::now_v7();
    let segment = uuid::Uuid::now_v7();
    sqlx::query(
        "insert into recordings
             (id, workspace_id, channel_id, call_id, room_name, state, started_by, language,
              requested_at, recording_since, stop_reason, stop_requested_at, ended_at,
              processed_at)
         values ($1, $2, $3, gen_random_uuid(), $4, 'ready', $5, 'auto', now(), now(),
                 'call_ended', now(), now(), now())",
    )
    .bind(recording)
    .bind(workspace_id.as_uuid())
    .bind(voice.as_uuid())
    .bind(format!("jr-{}-{}", "0".repeat(12), recording.simple()))
    .bind(speaker.as_uuid())
    .execute(&mut connection)
    .await
    .unwrap();
    sqlx::query(
        "insert into recording_segments
             (id, recording_id, source, state, job_id, job_started_at, requested_path,
              requested_at)
         values ($1, $2, 'composite', 'ended', $3, now(), 'seeded/composite.mp4', now())",
    )
    .bind(segment)
    .bind(recording)
    .bind(format!("seeded-{}", segment.simple()))
    .execute(&mut connection)
    .await
    .unwrap();
    sqlx::query(
        "insert into transcripts (recording_id, state, requested_at, finished_at, lines)
         values ($1, 'ready', now(), now(), 1)",
    )
    .bind(recording)
    .execute(&mut connection)
    .await
    .unwrap();
    sqlx::query(
        "insert into transcript_lines
             (recording_id, line, start_ms, end_ms, segment_id, source, account_id, text, language)
         values ($1, 0, 0, 500, $2, 'microphone', $3, $4, 'en')",
    )
    .bind(recording)
    .bind(segment)
    .bind(speaker.as_uuid())
    .bind(text)
    .execute(&mut connection)
    .await
    .unwrap();
    recording
}

impl Window {
    fn search_panel(&mut self) -> Option<SearchPanel> {
        self.view().workspace()?.search.clone()
    }

    /// Types a search text into the field of the header and sends it with Enter.
    fn search_for(&mut self, text: &str) {
        self.in_shell(|shell, window, cx| {
            shell.type_search(text, window, cx);
            shell.focus_search(window, cx);
        });
        self.press("enter");
    }

    /// Waits for the answer of the search that runs.
    #[track_caller]
    fn until_searched(&mut self) -> SearchPanel {
        self.until("the search has its answer", |window| {
            window
                .search_panel()
                .is_some_and(|panel| panel.state != SearchShown::Searching && !panel.reading_more)
        });
        self.search_panel().unwrap()
    }
}

#[gpui::test]
fn a_search_finds_messages_and_a_transcript_line_and_leads_to_a_message(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    window.until("the messages of the text channel are read", |window| {
        window.feed().is_some_and(|feed| feed.loaded)
    });
    let general = window.selected_channel();
    let workspace_id = window.view().workspace().unwrap().id;
    let own = window.own_account();
    let voice = window
        .channels()
        .into_iter()
        .find(|row| row.kind == ChannelKind::Voice)
        .unwrap();
    let random = admin.text_channel(&world, &mut window, "random");
    let first = admin.send(&world, general, document("the quartz retention spec"));
    admin.send(&world, general, document("nothing of that kind"));
    admin.send(&world, random, document("quartz again, in another channel"));
    window.until_message("nothing of that kind");
    world.run(seed_transcript(
        &server,
        workspace_id,
        voice.id,
        own,
        "we said quartz aloud",
    ));

    // No search runs: the header has the field, and no panel is shown.
    assert!(window.shows("search-field") && !window.shows("search-panel"));
    assert!(window.search_panel().is_none());

    // A query finds the two messages and the line of the transcript.
    window.search_for("quartz");
    let panel = window.until_searched();
    assert!(window.shows("search-panel") && window.shows("search-results"));
    assert_eq!(panel.state, SearchShown::Results);
    assert_eq!((panel.messages.len(), panel.transcripts.len()), (2, 1));
    assert!(!panel.more);
    let in_general = panel
        .messages
        .iter()
        .find(|row| row.target == ResultTarget::Message(first.id))
        .unwrap();
    assert_eq!(in_general.author, "Ada");
    assert_eq!(in_general.text, "the quartz retention spec");
    assert_eq!(&in_general.text[in_general.marks[0].clone()], "quartz");
    let line = &panel.transcripts[0];
    assert_eq!(
        line.place,
        Whereabouts::Channel {
            name: voice.name.clone(),
            kind: ChannelKind::Voice
        }
    );
    assert_eq!(
        (line.author.as_str(), line.text.as_str()),
        ("Luca", "we said quartz aloud")
    );

    // The filter for a channel narrows the search to it, and the filter for an author to
    // the messages of that person.
    window.click("search-channel");
    assert!(window.shows("search-filter-menu"));
    window.click_named(format!("search-channel:{random}"));
    let panel = window.until_searched();
    assert_eq!((panel.messages.len(), panel.transcripts.len()), (1, 0));
    assert_eq!(panel.channel.as_ref().unwrap().0, random);
    window.in_shell(|shell, _, cx| {
        shell.set_search_filters(
            Filters {
                channel: None,
                author: Some(own),
                date: DateFilter::Today,
            },
            cx,
        );
    });
    let panel = window.until_searched();
    assert_eq!((panel.messages.len(), panel.transcripts.len()), (0, 1));
    window.in_shell(|shell, _, cx| shell.set_search_filters(Filters::default(), cx));
    let panel = window.until_searched();
    assert_eq!(panel.messages.len(), 2);

    // A result leads to its message: the channel is shown, and the list marks the message.
    window.click_channel("random");
    window.until("the other channel is shown", |window| {
        window.selected_channel() == random
    });
    window.click_named(format!("search-result:{}", first.id));
    window.until("the message of the result is shown", |window| {
        window.selected_channel() == general
            && window
                .messages()
                .iter()
                .any(|row| row.message.id == first.id && row.target)
    });
    // The results stay beside the channel.
    assert!(window.shows("search-panel"));

    // A result states its position: the window goes there without a read of the message
    // (ADR 0413), and a person who then goes to another channel stays there.
    window.click_channel("random");
    window.until("the other channel is shown again", |window| {
        window.selected_channel() == random
    });
    let state = window.state.clone();
    let answers = |window: &mut Window| {
        window
            .state
            .read_with(&window.cx, |state, _| state.result_answers())
    };
    let before = answers(&mut window);
    window.cx.update(|_, cx| {
        state.update(cx, |state, cx| {
            state.go_to_result(first.id, cx);
            state.select_channel(workspace_id, random, cx);
        });
    });
    window.cx.run_until_parked();
    assert_eq!(window.selected_channel(), random);
    assert_eq!(
        answers(&mut window),
        before,
        "the message of the result was read"
    );

    // A message that was deleted after the search: the list of its place holds the
    // messages around its position and not the message, so the result is gone, and the
    // panel says so.
    let gone = window
        .search_panel()
        .unwrap()
        .messages
        .iter()
        .find_map(|row| match row.target {
            ResultTarget::Message(id) if id != first.id => Some(id),
            _ => None,
        })
        .unwrap();
    world.run(admin.handle.api().delete_message(gone)).unwrap();
    window.until("the list of the channel lost the message", |window| {
        window.feed().is_some_and(|feed| feed.loaded)
            && window.messages().iter().all(|row| row.message.id != gone)
    });
    window.click_named(format!("search-result:{gone}"));
    window.until("the result is gone", |window| {
        window.search_panel().is_some_and(|panel| {
            panel.go_failed.as_deref() == Some(text::RESULT_GONE)
                && panel
                    .messages
                    .iter()
                    .all(|row| row.target != ResultTarget::Message(gone))
        })
    });
    assert_eq!(answers(&mut window), before);

    // The result of the transcript shows the voice channel of its recording.
    window.click("search-transcript-0");
    window.until("the voice channel is shown", |window| {
        window.selected_channel() == voice.id
    });

    // A query that finds nothing says so, and a text that is too long is not sent.
    window.search_for("zirconium");
    let panel = window.until_searched();
    assert_eq!(panel.state, SearchShown::Nothing);
    assert!(window.shows("search-nothing") && !window.shows("search-results"));
    window.search_for(&"long ".repeat(300));
    let panel = window.until_searched();
    assert_eq!(
        panel.state,
        SearchShown::Refused(text::SEARCH_TEXT_TOO_LONG.to_owned())
    );
    assert!(window.shows("search-refused"));

    // The close control ends the search and empties the field.
    window.click("close-search");
    assert!(!window.shows("search-panel"));
    assert_eq!(window.in_shell(|shell, _, cx| shell.search_text(cx)), "");
}

#[gpui::test]
fn a_search_reads_a_further_page_and_loses_the_results_of_a_hidden_channel(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    window.until("the messages of the text channel are read", |window| {
        window.feed().is_some_and(|feed| feed.loaded)
    });
    let general = window.selected_channel();
    let random = admin.text_channel(&world, &mut window, "random");
    for number in 0..52 {
        admin.send(&world, general, document(&format!("paged entry {number}")));
    }
    admin.send(&world, random, document("paged entry elsewhere"));
    window.until_message("paged entry 51");

    // The first page has 50 results, and the panel offers the next one.
    window.search_for("paged");
    let panel = window.until_searched();
    assert_eq!(panel.messages.len(), 50);
    assert!(panel.more);
    // The control stands after the 50 results.
    window.in_shell(|shell, _, cx| shell.show_end_of_results(cx));
    assert!(window.shows("more-results"));
    window.click("more-results");
    let panel = window.until_searched();
    assert_eq!(panel.messages.len(), 53);
    assert!(!panel.more && !window.shows("more-results"));

    // The administrator hides the first channel from the role of every member: its
    // results leave the panel, and the one of the other channel stays.
    let api = admin.handle.api();
    let workspace_id = window.view().workspace().unwrap().id;
    let roles = world.run(api.workspace_roles(workspace_id)).unwrap();
    let everyone = roles.iter().find(|role| role.is_everyone).unwrap().id;
    let hide = SetChannelOverride {
        allow: Vec::new(),
        deny: vec![WorkspacePermission::ViewChannel],
    };
    world
        .run(api.set_channel_override(general, everyone, &hide))
        .unwrap();
    window.until("the results of the hidden channel are gone", |window| {
        window.search_panel().is_some_and(|panel| {
            panel.messages.len() == 1 && panel.messages[0].text == "paged entry elsewhere"
        })
    });
    // A new search of the same text answers in the same way.
    window.search_for("paged");
    let panel = window.until_searched();
    assert_eq!(panel.messages.len(), 1);
}

#[gpui::test]
fn a_reload_of_the_state_reads_the_search_again_and_leaves_the_side_panel(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    window.until("the messages of the text channel are read", |window| {
        window.feed().is_some_and(|feed| feed.loaded)
    });
    let general = window.selected_channel();
    let server_id = window.view().selected.unwrap().id;
    for number in 0..52 {
        admin.send(&world, general, document(&format!("paged entry {number}")));
    }
    window.until_message("paged entry 51");
    let reloads = |window: &mut Window| {
        window
            .state
            .read_with(&window.cx, |state, _| state.reloads_of(server_id))
    };
    let mute = |window: &mut Window, length: Option<MuteFor>| {
        let before = reloads(window);
        let state = window.state.clone();
        window.cx.update(|_, cx| {
            state.update(cx, |state, cx| {
                state.set_channel_mute(server_id, general, length, cx);
            });
        });
        // A change of the notification settings replaces the state (ADR 0722).
        window.until("the state was replaced", |window| reloads(window) > before);
    };

    // The notices lie over a search. The state is replaced: the notices stay, and the
    // search does not return into the side panel.
    window.search_for("paged");
    assert_eq!(window.until_searched().messages.len(), 50);
    window.click("notices-row");
    assert!(window.shows("notices-panel") && !window.shows("search-panel"));
    mute(&mut window, Some(MuteFor::OneHour));
    assert!(window.shows("notices-panel") && !window.shows("search-panel"));
    window.click("close-notices");

    // A search that the side panel shows is read again from its first page.
    window.search_for("paged");
    assert_eq!(window.until_searched().messages.len(), 50);
    window.in_shell(|shell, _, cx| shell.show_end_of_results(cx));
    window.click("more-results");
    assert_eq!(window.until_searched().messages.len(), 52);
    mute(&mut window, None);
    window.until("the first page was read again", |window| {
        window.search_panel().is_some_and(|panel| {
            panel.state == SearchShown::Results && panel.messages.len() == 50 && panel.more
        })
    });
    assert!(window.shows("search-panel"));
}

#[gpui::test]
fn a_result_of_an_archived_channel_is_listed_and_leads_to_its_message(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    window.until("the messages of the text channel are read", |window| {
        window.feed().is_some_and(|feed| feed.loaded)
    });
    let general = window.selected_channel();
    let random = admin.text_channel(&world, &mut window, "random");
    admin.send(&world, general, document("the quartz plan"));
    let archived = admin.send(&world, random, document("quartz in the old channel"));
    admin.send(&world, general, document("quartz once more"));
    window.until_message("quartz once more");

    // The channel is archived: it leaves the channel list, and the state does not know it
    // until the archived list of its workspace is read.
    world
        .run(admin.handle.api().archive_channel(random))
        .unwrap();
    window.until("the archived channel leaves the list", |window| {
        !window.channel_names().contains(&"random".to_owned())
    });

    // The search lists the result of the archived channel with its place.
    window.search_for("quartz");
    let panel = window.until_searched();
    assert_eq!(panel.state, SearchShown::Results);
    assert_eq!(panel.messages.len(), 3);
    let old = panel
        .messages
        .iter()
        .find(|row| row.target == ResultTarget::Message(archived.id))
        .unwrap();
    assert_eq!(
        old.place,
        Whereabouts::Channel {
            name: "random".to_owned(),
            kind: ChannelKind::Text
        }
    );

    // The result leads to its message: the main area shows the archived channel and marks
    // the message.
    window.click_named(format!("search-result:{}", archived.id));
    window.until("the message in the archived channel is shown", |window| {
        let shows_archived = window.view().workspace().is_some_and(|workspace| {
            workspace.archived.is_some()
                && workspace
                    .channel
                    .as_ref()
                    .is_some_and(|channel| channel.id == random)
        });
        shows_archived
            && window
                .messages()
                .iter()
                .any(|row| row.message.id == archived.id && row.target)
    });
    assert!(window.shows("search-panel"));
}

/// A window of a member behind a proxy, with a text channel "random" that holds one
/// message with the word "quartz" and a mention of the member, archived while the window
/// shows another channel.
fn archived_scene(
    cx: &mut TestAppContext,
    world: &World,
) -> (
    TestServer,
    FaultProxy,
    crate::harness::Admin,
    tempfile::TempDir,
    Window,
    junto_client_core::protocol::message::Message,
) {
    let server = world.server("Acme Studio");
    let proxy = world.run(FaultProxy::start(&server));
    let admin = world.owner(&server);
    let (directory, mut window) = member(cx, world, &proxy.address(), &admin, "Luca");
    window.until("the messages of the text channel are read", |window| {
        window.feed().is_some_and(|feed| feed.loaded)
    });
    let own = window.own_account();
    let random = admin.text_channel(world, &mut window, "random");
    let archived = admin.mention(world, random, "quartz in the old channel, ", own);
    world
        .run(admin.handle.api().archive_channel(random))
        .unwrap();
    window.until("the archived channel leaves the list", |window| {
        !window.channel_names().contains(&"random".to_owned())
    });
    (server, proxy, admin, directory, window, archived)
}

/// The requests that read an archived list.
fn archived_reads(proxy: &FaultProxy) -> usize {
    proxy
        .all()
        .iter()
        .filter(|seen| seen.method == "GET" && seen.path.ends_with("/archived-channels"))
        .count()
}

#[gpui::test]
fn the_first_page_of_results_is_shown_whole_when_its_archived_list_answers(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let (server, proxy, _admin, _directory, mut window, archived) = archived_scene(cx, &world);
    let workspace_id = window.view().workspace().unwrap().id;
    let own = window.own_account();
    let voice = window
        .channels()
        .into_iter()
        .find(|row| row.kind == ChannelKind::Voice)
        .unwrap();
    // A transcript line that ranks above the message of the archived channel: it is the
    // first result of the page, and the transcripts stand under the messages.
    world.run(seed_transcript(
        &server,
        workspace_id,
        voice.id,
        own,
        "quartz quartz quartz quartz",
    ));

    // The answer of the archived list waits at the proxy: no result of the page is
    // shown, so none can move when the message of the archived channel is placed.
    proxy.hold_answer_ending("GET", "/archived-channels");
    window.search_for("quartz");
    window.until("the archived list waits at the proxy", |_| {
        proxy.answers_waiting() == 1
    });
    let line = "search-transcript-0";
    let message = crate::harness::selector(format!("search-result:{}", archived.id));
    let drawn: Vec<(&'static str, gpui::Bounds<gpui::Pixels>)> = [line, message]
        .into_iter()
        .filter_map(|selector| {
            window.settle();
            window
                .cx
                .debug_bounds(selector)
                .map(|bounds| (selector, bounds))
        })
        .collect();
    assert!(
        drawn.is_empty(),
        "rows of a waiting page are drawn: {drawn:?}"
    );
    assert_eq!(window.search_panel().unwrap().state, SearchShown::Searching);

    // The list answers: the page appears whole, the message above the transcript line.
    proxy.release();
    let panel = window.until_searched();
    assert_eq!((panel.messages.len(), panel.transcripts.len()), (1, 1));
    assert!(window.shows(line) && window.shows(message));
    let (line_at, message_at) = (
        window.cx.debug_bounds(line).unwrap(),
        window.cx.debug_bounds(message).unwrap(),
    );
    assert!(message_at.origin.y < line_at.origin.y);
}

#[gpui::test]
fn a_failed_archived_list_is_stated_with_a_retry_in_the_search_and_the_notices(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let (_server, proxy, _admin, _directory, mut window, archived) = archived_scene(cx, &world);
    let attempts = junto_client_core::api::RetryPolicy::default().attempts as usize;
    let fail_once = |proxy: &FaultProxy| {
        proxy.fail_ending(
            "/archived-channels",
            std::iter::repeat_n(Fault::Busy, attempts),
        );
    };
    let listed = |window: &mut Window| {
        window.search_panel().is_some_and(|panel| {
            panel
                .messages
                .iter()
                .any(|row| row.target == ResultTarget::Message(archived.id))
        })
    };

    // The read of the archived list fails: the search says so, beside the results that
    // it lists, and "Try again" reads the list again.
    fail_once(&proxy);
    window.search_for("quartz");
    window.until("the search states the failed archived list", |window| {
        window.shows("search-archive-failed")
    });
    assert!(!listed(&mut window));
    assert_eq!(window.search_panel().unwrap().state, SearchShown::Nothing);
    window.click("search-archive-retry");
    window.until("the result of the archived channel is listed", listed);
    assert!(!window.shows("search-archive-failed"));

    // A new search reads a failed archived list again.
    window.click("close-search");
    window.until("the archived list is released", |window| {
        window.search_panel().is_none()
    });
    fail_once(&proxy);
    window.search_for("quartz");
    window.until(
        "the search states the failed archived list again",
        |window| window.shows("search-archive-failed"),
    );
    let reads = archived_reads(&proxy);
    window.search_for("quartz");
    window.until("the new search lists the result", listed);
    assert!(archived_reads(&proxy) > reads);

    // The notices state the same failure, with the same retry.
    window.click("close-search");
    fail_once(&proxy);
    window.click("notices-row");
    window.until("the notices state the failed archived list", |window| {
        window.shows("notices-archive-failed")
    });
    let notices = |window: &mut Window| {
        window
            .view()
            .workspace()
            .and_then(|workspace| workspace.notices.clone())
            .map_or(0, |panel| panel.rows.len())
    };
    assert_eq!(notices(&mut window), 0);
    window.click("notices-archive-retry");
    window.until("the notice of the archived channel is listed", |window| {
        notices(window) == 1
    });
    assert!(!window.shows("notices-archive-failed"));
}
