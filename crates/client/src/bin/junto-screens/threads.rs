//! The screens of threads, pins, permanent messages and direct conversations (ADR 0741):
//! the thread panel of view A with the states of a fork, the pin list, the channel with
//! permanent messages of view J, and a direct conversation.

use std::sync::Arc;

use junto_client::shell::{Open, Pane, Shell};
use junto_client::state::view::{ForkAsked, ForkState, ServerSnapshot};
use junto_client_core::model::{
    Loadable, Named, PendingMessage, PinList, PlaceId, ReadFailure, Reading, SendFailure,
    SendState, ServerModel, Timeline,
};
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::ids::{ChannelId, ConversationId, MessageId, ServerId};
use junto_client_core::protocol::message::{
    Conversation, Message, MessagePlace, MessageState, ThreadState,
};
use junto_client_core::protocol::structure::ChannelKind;
use time::Duration;

use super::messages::{
    GENERAL, Made, acme_with, document, general, morning, none, place, prepare, reaction, state,
};
use super::{Screen, Studio, harbor, id, person, select_channel};

const ALPHA: u32 = 31;
const DEPLOYS: u32 = 32;

fn server_id() -> ServerId {
    id(1)
}

fn general_id() -> ChannelId {
    id(GENERAL)
}

/// The messages of `#general` with the thread of the design under the message of Diego,
/// and a pin on the first message.
fn general_with_thread() -> (Timeline, Arc<Message>, Vec<Message>) {
    let mut made = general();
    // The first message of a person is pinned (view A).
    Arc::make_mut(&mut made.messages[1]).pinned = true;
    let root_index = 4;
    let root_id = made.messages[root_index].id;
    let reply = |number: u32, author: &str, minutes: i64, text: &str| Message {
        id: id(5900 + number),
        place: place(GENERAL),
        position: 20 + u64::from(number),
        author_id: Some(person(author)),
        author_name: Some(author.to_owned()),
        created_at: morning() + Duration::minutes(minutes),
        edited_at: None,
        document: Some(document(text)),
        system: None,
        reply_to: None,
        permanent: false,
        deleted: false,
        attachments: Vec::new(),
        reactions: Vec::new(),
        thread_root: Some(root_id),
        reply_count: 0,
        last_reply: None,
        reply_authors: Vec::new(),
        pinned: false,
        files_removed_by_retention: false,
    };
    let mut replies = vec![
        reply(
            1,
            "Ana",
            36,
            "One per speaker. Transcripts are much better with separate tracks.",
        ),
        reply(
            2,
            "Bruno",
            41,
            "Agreed, with a limit: keep the tracks for 30 days, then keep only the mixdown.",
        ),
        reply(
            3,
            "Luca",
            55,
            "30 days works. Make it a channel setting named `tracks_days`.\n\n\
             ```TOML\n[retention]\ntracks_days = 30\n```",
        ),
        reply(
            4,
            "Diego",
            72,
            "Done, @Luca. I am writing it up now, and I will fork this thread if it grows.",
        ),
    ];
    replies[1].edited_at = Some(morning() + Duration::minutes(42));
    replies[1].reactions = vec![reaction("\u{1f44d}", 2, false)];
    replies[3].reply_to = Some(replies[2].id);
    let root = Arc::make_mut(&mut made.messages[root_index]);
    root.reply_count = replies.len() as u64;
    root.last_reply = replies.last().map(|reply| reply.id);
    // The first authors of the replies, as the server states them (ADR 0413).
    root.reply_authors = vec![person("Ana"), person("Bruno"), person("Luca")];
    let root = made.messages[root_index].clone();
    (made.timeline(), root, replies)
}

fn thread_place(root: &Message) -> PlaceId {
    PlaceId::Thread {
        channel_id: general_id(),
        root_id: root.id,
    }
}

fn thread_timeline(root: &Arc<Message>, replies: Vec<Message>) -> Timeline {
    Timeline {
        place: thread_place(root),
        messages: replies.into_iter().map(Arc::new).collect(),
        more_before: false,
        more_after: false,
        reading: Reading::default(),
        loaded: true,
        failed: None,
        stale: false,
        unread_after: None,
        root: Some(root.clone()),
        forks: Vec::new(),
    }
}

/// Gives the "everyone" role of "Studio" these permissions too.
fn allow(model: &mut ServerModel, more: &[WorkspacePermission]) {
    let studio = Arc::make_mut(&mut model.workspaces[0]);
    if let Some(everyone) = studio.roles.iter_mut().find(|role| role.is_everyone) {
        everyone.permissions.extend_from_slice(more);
    }
}

/// "Acme Studio" with the thread of the design loaded.
fn with_thread(
    change: impl FnOnce(&mut ServerModel, &mut Timeline),
) -> (Vec<ServerSnapshot>, MessageId) {
    let (channel, root, replies) = general_with_thread();
    let root_id = root.id;
    let mut thread = thread_timeline(&root, replies);
    let acme = acme_with(|model| {
        allow(
            model,
            &[
                WorkspacePermission::ManageChannel,
                WorkspacePermission::ManageMessages,
            ],
        );
        change(model, &mut thread);
        model.timelines = vec![Arc::new(channel), Arc::new(thread)];
    });
    (vec![acme, harbor()], root_id)
}

fn open_thread(
    shell: &mut Shell,
    root_id: MessageId,
    window: &mut gpui::Window,
    cx: &mut gpui::Context<Shell>,
) {
    prepare(shell, none(), cx);
    shell.open_thread(server_id(), general_id(), root_id, window, cx);
}

fn direct_place(conversation: &Conversation) -> MessagePlace {
    MessagePlace::Direct {
        conversation_id: conversation.id,
        low_account_id: conversation.low_account_id,
        high_account_id: conversation.high_account_id,
    }
}

fn conversation(number: u32, with: &str) -> Conversation {
    let (own, other) = (person("Luca"), person(with));
    Conversation {
        id: id(8000 + number),
        low_account_id: own.min(other),
        high_account_id: own.max(other),
    }
}

/// "Acme Studio" with two direct conversations, the one with Ana with messages.
/// `closed`: the server does not give the conversation with Ana any more, and one
/// message of the person waits in it.
fn with_directs(closed: bool) -> (Vec<ServerSnapshot>, ConversationId) {
    let (with_ana, with_marta) = (conversation(1, "Ana"), conversation(2, "Marta"));
    let ana_id = with_ana.id;
    let mut made = Made::new(GENERAL);
    made.message("Ana", 20, "Do you have ten minutes after the daily?");
    made.message("Luca", 22, "Yes. I will call you at 10:30.");
    made.message("Ana", 23, "Good. Bring the retention spec.");
    // The lines of two calls of the conversation (ADR 0518): one that took place, and
    // one that the person did not answer.
    made.system(30, "direct_call_started", Some("Luca"), Some("Ana"));
    made.system(43, "direct_call_ended", Some("Luca"), Some("Ana"));
    if let Some(ended) = made.messages.last_mut()
        && let Some(system) = Arc::make_mut(ended).system.as_mut()
    {
        system.duration_seconds = Some(754);
    }
    made.system(95, "direct_call_started", Some("Ana"), Some("Luca"));
    made.system(96, "direct_call_not_answered", Some("Ana"), Some("Luca"));
    let direct = direct_place(&with_ana);
    for message in &mut made.messages {
        Arc::make_mut(message).place = direct;
    }
    let mut timeline = made.timeline();
    timeline.place = PlaceId::Direct(ana_id);
    if closed {
        timeline.messages.clear();
        timeline.loaded = false;
        timeline.failed = Some(ReadFailure::NotFound);
    }
    let acme = acme_with(|model| {
        if closed {
            let mut pending = PendingMessage::new(
                1,
                PlaceId::Direct(ana_id),
                document("I am on my way."),
                None,
                Vec::new(),
                morning() + Duration::minutes(24),
            );
            pending.state = SendState::Failed(SendFailure::NotSent);
            model.outbox = vec![Arc::new(pending)];
        }
        let unread = |conversation: &Conversation, unread: u64| MessageState {
            place: direct_place(conversation),
            last_read: 2,
            latest: 3,
            unread,
            mentions: 0,
            unread_sign: unread > 0,
            notification_mentions: 0,
            muted: false,
        };
        model.message_states.push(unread(&with_ana, 0));
        model.message_states.push(unread(&with_marta, 2));
        model.conversations = vec![with_ana, with_marta];
        model.timelines = vec![Arc::new(general().timeline()), Arc::new(timeline)];
    });
    (vec![acme, harbor()], ana_id)
}

/// The root of a thread of `#general` that the loaded messages of the channel do not
/// hold.
fn earlier_root(number: u32, author: &str, text: &str) -> Message {
    Message {
        id: id(5800 + number),
        place: place(GENERAL),
        position: u64::from(number),
        author_id: Some(person(author)),
        author_name: Some(author.to_owned()),
        created_at: morning() - Duration::days(i64::from(number)),
        edited_at: None,
        document: Some(document(text)),
        system: None,
        reply_to: None,
        permanent: false,
        deleted: false,
        attachments: Vec::new(),
        reactions: Vec::new(),
        thread_root: None,
        reply_count: 3,
        last_reply: None,
        reply_authors: Vec::new(),
        pinned: false,
        files_removed_by_retention: false,
    }
}

/// The screens of followed threads (ADR 0412, ADR 0413): the control of the thread panel
/// in each state, and the followed threads under the shown channel.
fn draw_followed(studio: &mut Studio) -> Result<(), String> {
    let (servers, root_id) = with_thread(|_, _| {});
    let follow = |state: (bool, Option<bool>), busy: bool, refused: Option<&'static str>| {
        move |shell: &mut Shell, window: &mut gpui::Window, cx: &mut gpui::Context<Shell>| {
            open_thread(shell, root_id, window, cx);
            shell.state().clone().update(cx, |app, cx| {
                app.show_follow_fixture(
                    (server_id(), root_id),
                    state,
                    busy,
                    refused.map(str::to_owned),
                    cx,
                );
            });
        }
    };
    // The four states: a choice to follow, a choice not to follow, and no choice with
    // and without the rule of the server.
    for (name, state) in [
        ("follow-by-choice", (true, Some(true))),
        ("follow-not-by-choice", (false, Some(false))),
        ("follow-by-rule", (true, None)),
        ("follow-no-choice", (false, None)),
    ] {
        studio.draw(
            Screen::new(name, servers.clone()),
            follow(state, false, None),
        )?;
    }
    // A change on its way, and a change that the server refused.
    studio.draw(
        Screen::new("follow-sending", servers.clone()),
        follow((false, Some(false)), true, None),
    )?;
    studio.draw(
        Screen::new("follow-refused", servers),
        follow(
            (true, None),
            false,
            Some(junto_client::text::followed::THREAD_GONE),
        ),
    )?;

    // The channel column: a thread with something unread, the followed threads that are
    // read, and the row that offers the further ones.
    let (unread, _) = with_thread(|model, _| {
        let (_, root, _) = general_with_thread();
        model.threads = vec![ThreadState {
            root_id: root.id,
            last_read: 22,
            latest: 24,
            unread: 2,
            mentions: 1,
            following: true,
            follow_setting: None,
        }];
        model.named = vec![(root.id, Named::Read(root))];
    });
    let roots: Vec<Message> = [
        ("Ana", "Retention of the call recordings"),
        ("Bruno", "Which build goes to Alpha on Monday?"),
        ("Carla", "Notes from the Alpha review"),
        ("Marta", "The invoice for October"),
        ("Diego", "Transcripts with separate tracks"),
        ("Elisa", "The export of a channel is slow"),
        ("Ana", "Names of the new voice channels"),
        ("Bruno", "The checklist for v0.9"),
        ("Carla", "A second recorder for the studio"),
        ("Luca", "The order of the demo on Friday"),
        ("Marta", "Access for the legal contact"),
    ]
    .into_iter()
    .zip(1u32..)
    .map(|((author, text), number)| earlier_root(number, author, text))
    .collect();
    // The server states the thread with the newest reply first: the unread one, then the
    // read ones.
    let followed: Vec<ThreadState> = std::iter::once(root_id)
        .chain(roots.iter().map(|root| root.id))
        .map(|root_id| ThreadState {
            root_id,
            last_read: 24,
            latest: 24,
            unread: 0,
            mentions: 0,
            following: true,
            follow_setting: None,
        })
        .collect();
    let list = |expanded: bool, more: bool| {
        let (threads, roots) = (followed.clone(), roots.clone());
        move |shell: &mut Shell, _: &mut gpui::Window, cx: &mut gpui::Context<Shell>| {
            prepare(shell, none(), cx);
            shell.state().clone().update(cx, |app, cx| {
                app.show_followed_fixture(
                    (server_id(), general_id()),
                    threads,
                    more,
                    expanded,
                    roots,
                    cx,
                );
            });
        }
    };
    studio.draw(
        Screen::new("follow-list", unread.clone()),
        list(false, false),
    )?;
    studio.draw(
        Screen::new("follow-list-expanded", unread.clone()),
        list(true, false),
    )?;
    // The server lists further pages: the row stays after the held threads are listed.
    studio.draw(
        Screen::new("follow-list-more-pages", unread),
        list(true, true),
    )
}

pub(crate) fn draw(studio: &mut Studio) -> Result<(), String> {
    // View A of the design: the thread panel beside the list of its channel.
    let (servers, root_id) = with_thread(|_, _| {});
    studio.draw(
        Screen::new("thread-panel", servers.clone()),
        move |shell, window, cx| open_thread(shell, root_id, window, cx),
    )?;
    studio.draw(
        Screen::new("thread-panel-150", servers.clone()).scale(150),
        move |shell, window, cx| open_thread(shell, root_id, window, cx),
    )?;
    // A reply that is written, with a reply to a message of the thread.
    studio.draw(
        Screen::new("thread-reply", servers.clone()),
        move |shell, window, cx| {
            open_thread(shell, root_id, window, cx);
            shell.reply_to_message(Pane::Thread, id(5903), window, cx);
            shell.type_message(Pane::Thread, "`tracks_days` it is.", window, cx);
        },
    )?;
    // The menu of a message with the actions of a thread and of a pin.
    studio.draw(
        Screen::new("thread-message-menu", servers.clone()),
        move |shell, window, cx| {
            prepare(shell, none(), cx);
            shell.open_message_menu(Pane::Main, root_id, window, cx);
        },
    )?;

    // A person without "manage channel": the fork is not offered.
    let (channel, root, replies) = general_with_thread();
    let thread = thread_timeline(&root, replies);
    let member = acme_with(|model| {
        model.timelines = vec![Arc::new(channel), Arc::new(thread)];
    });
    studio.draw(
        Screen::new("thread-panel-no-fork", vec![member, harbor()]),
        move |shell, window, cx| open_thread(shell, root_id, window, cx),
    )?;

    // The threads with something new: under their channel, and on their message.
    let (unread, _) = with_thread(|model, _| {
        let (_, root, _) = general_with_thread();
        model.threads = vec![ThreadState {
            root_id: root.id,
            last_read: 22,
            latest: 24,
            unread: 2,
            mentions: 1,
            following: true,
            follow_setting: None,
        }];
        model.named = vec![(root.id, Named::Read(root))];
    });
    studio.draw(Screen::new("thread-unread", unread), |shell, _, cx| {
        prepare(shell, none(), cx);
    })?;

    // The fork of a thread: the dialog, a request on its way, a refusal, and the link.
    studio.draw(
        Screen::new("thread-fork-dialog", servers.clone()),
        move |shell, window, cx| {
            open_thread(shell, root_id, window, cx);
            shell.open_fork_dialog(root_id, ChannelKind::Text, window, cx);
            shell.type_fork_name("retention", window, cx);
        },
    )?;
    let fork_state = |state: ForkState| {
        move |shell: &mut Shell, window: &mut gpui::Window, cx: &mut gpui::Context<Shell>| {
            open_thread(shell, root_id, window, cx);
            shell.open_fork_dialog(root_id, ChannelKind::Voice, window, cx);
            shell.type_fork_name("retention", window, cx);
            shell.state().clone().update(cx, |app, cx| {
                app.show_threads_fixture(
                    Some(ForkAsked {
                        server_id: server_id(),
                        root_id,
                        state,
                    }),
                    None,
                    cx,
                );
            });
        }
    };
    studio.draw(
        Screen::new("thread-fork-sending", servers.clone()),
        fork_state(ForkState::Sending),
    )?;
    studio.draw(
        Screen::new("thread-fork-refused", servers.clone()),
        fork_state(ForkState::Refused(
            junto_client::text::FORK_EXISTS.to_owned(),
        )),
    )?;
    let (forked, _) = with_thread(|_, thread| thread.forks = vec![id(DEPLOYS)]);
    studio.draw(
        Screen::new("thread-forked", forked),
        move |shell, window, cx| open_thread(shell, root_id, window, cx),
    )?;

    // A thread without a reply, a thread that is read, and one that is gone.
    let (empty, _) = with_thread(|_, thread| thread.messages.clear());
    studio.draw(
        Screen::new("thread-empty", empty),
        move |shell, window, cx| open_thread(shell, root_id, window, cx),
    )?;
    let (loading, _) = with_thread(|_, thread| {
        thread.loaded = false;
        thread.root = None;
        thread.messages.clear();
    });
    studio.draw(
        Screen::new("thread-loading", loading),
        move |shell, window, cx| open_thread(shell, root_id, window, cx),
    )?;
    let (gone, _) = with_thread(|_, thread| {
        thread.loaded = false;
        thread.root = None;
        thread.messages.clear();
        thread.failed = Some(ReadFailure::NotFound);
    });
    studio.draw(
        Screen::new("thread-gone", gone),
        move |shell, window, cx| open_thread(shell, root_id, window, cx),
    )?;

    // The pin list of a channel, and its states.
    let pins = |messages: Loadable<Vec<Arc<Message>>>, failed: Option<ReadFailure>| {
        let (servers, _) = with_thread(|model, _| {
            model.pins = vec![PinList {
                channel_id: general_id(),
                messages,
                failed,
            }];
        });
        servers
    };
    let show_pins = |shell: &mut Shell, _: &mut gpui::Window, cx: &mut gpui::Context<Shell>| {
        prepare(shell, none(), cx);
        shell.open_pins(server_id(), general_id(), cx);
    };
    let (channel, _, replies) = general_with_thread();
    let mut pinned_reply = replies[2].clone();
    pinned_reply.pinned = true;
    let pinned = vec![channel.messages[1].clone(), Arc::new(pinned_reply)];
    studio.draw(
        Screen::new("pins-panel", pins(Loadable::Loaded(pinned), None)),
        show_pins,
    )?;
    studio.draw(
        Screen::new("pins-empty", pins(Loadable::Loaded(Vec::new()), None)),
        show_pins,
    )?;
    studio.draw(
        Screen::new("pins-loading", pins(Loadable::Loading, None)),
        show_pins,
    )?;
    studio.draw(
        Screen::new(
            "pins-failed",
            pins(Loadable::NotLoaded, Some(ReadFailure::NoAnswer)),
        ),
        show_pins,
    )?;

    // View J of the design: a channel whose messages are permanent.
    let mut agreed = Made::new(ALPHA);
    agreed.system(0, "channel_updated", Some("Bruno"), None);
    agreed.message(
        "Marta",
        20,
        "We approve the **v0.9** scope as listed in the proposal. Please start on Monday.",
    );
    let ana = agreed.message(
        "Ana",
        26,
        "Confirmed. Work starts on Monday, 12 October. @Luca owns the demo.",
    );
    ana.reactions = vec![reaction("\u{1f44d}", 2, false)];
    agreed.system(62, "permanent_message_deleted", Some("Ana"), None);
    agreed.message(
        "Luca",
        65,
        "The demo is on Friday, 16 October, at 15:00. I will send the invite to Marta today.",
    );
    agreed.message(
        "Marta",
        74,
        "Agreed. @Luca, please add our legal contact to the invite.",
    );
    for message in &mut agreed.messages {
        let message = Arc::make_mut(message);
        message.permanent = message.author_id.is_some();
    }
    let own = agreed.messages[4].id;
    let permanent = acme_with(|model| {
        let studio = Arc::make_mut(&mut model.workspaces[0]);
        for channel in &mut studio.channels {
            if channel.channel.id == id::<ChannelId>(ALPHA) {
                channel.channel.permanent_messages = true;
            }
        }
        model.message_states = vec![state(DEPLOYS, 3, 0)];
        model.timelines = vec![Arc::new(agreed.timeline())];
    });
    studio.draw(
        Screen::new("permanent-channel", vec![permanent.clone(), harbor()]),
        |shell, _, cx| {
            prepare(shell, none(), cx);
            select_channel(shell, "client-alpha", cx);
        },
    )?;
    studio.draw(
        Screen::new("permanent-message-menu", vec![permanent, harbor()]),
        move |shell, window, cx| {
            prepare(shell, none(), cx);
            select_channel(shell, "client-alpha", cx);
            shell.open_message_menu(Pane::Main, own, window, cx);
        },
    )?;

    draw_followed(studio)?;

    // Direct conversations: in the channel column, one that is open, and a refusal.
    let (directs, with_ana) = with_directs(false);
    studio.draw(
        Screen::new("direct-list", directs.clone()),
        |shell, _, cx| prepare(shell, none(), cx),
    )?;
    studio.draw(
        Screen::new("direct-conversation", directs.clone()),
        move |shell, _, cx| {
            prepare(shell, none(), cx);
            shell.state().clone().update(cx, |app, cx| {
                app.select_conversation(server_id(), with_ana, cx);
            });
        },
    )?;
    let (closed, _) = with_directs(true);
    studio.draw(Screen::new("direct-closed", closed), move |shell, _, cx| {
        prepare(shell, none(), cx);
        shell.state().clone().update(cx, |app, cx| {
            app.select_conversation(server_id(), with_ana, cx);
        });
    })?;
    studio.draw(
        Screen::new("direct-refused", directs),
        |shell, window, cx| {
            prepare(shell, none(), cx);
            shell.open(Open::Members, window, cx);
            shell.state().clone().update(cx, |app, cx| {
                app.show_threads_fixture(
                    None,
                    Some((
                        server_id(),
                        person("Carla"),
                        junto_client::text::NO_CONTACT.to_owned(),
                    )),
                    cx,
                );
            });
        },
    )
}
