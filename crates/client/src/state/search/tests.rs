use junto_client_core::protocol::account::OwnAccount;
use junto_client_core::protocol::events::Cursor;
use junto_client_core::protocol::ids::{ConversationId, StreamEpoch};
use junto_client_core::protocol::message::Conversation;
use junto_client_core::protocol::snapshot::{Snapshot, WorkspaceState};
use junto_client_core::protocol::structure::{Channel, Workspace, WorkspaceMember};
use time::{Duration, UtcOffset};

use super::*;

/// Thursday, 8 October 2026 at 09:00 UTC.
fn morning() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_791_450_000).unwrap()
}

struct Scene {
    server_id: ServerId,
    model: ServerModel,
    known: Known,
    own: AccountId,
    ana: AccountId,
    studio: WorkspaceId,
    general: ChannelId,
    daily: ChannelId,
    conversation: Conversation,
}

fn scene() -> Scene {
    let (own, ana) = (AccountId::generate(), AccountId::generate());
    let studio = WorkspaceId::generate();
    let channel = |kind, name: &str, position| Channel {
        id: ChannelId::generate(),
        workspace_id: studio,
        category_id: None,
        kind,
        name: name.to_owned(),
        position,
        permanent_messages: false,
        user_limit: None,
        hides_occupants: false,
    };
    let general = channel(ChannelKind::Text, "general", 0);
    let daily = channel(ChannelKind::Voice, "Daily", 1);
    let snapshot = Snapshot {
        cursor: Cursor {
            epoch: StreamEpoch::generate(),
            position: 1,
        },
        account: OwnAccount {
            id: own,
            name: "Luca".to_owned(),
            avatar: None,
            is_owner: false,
            server_role_ids: Vec::new(),
            server_permissions: Vec::new(),
            workspace_ids: vec![studio],
        },
        workspaces: vec![WorkspaceState {
            workspace: Workspace {
                id: studio,
                name: "Studio".to_owned(),
                is_default: true,
            },
            own_role_ids: Vec::new(),
            roles: Vec::new(),
            categories: Vec::new(),
            channels: vec![general.clone(), daily.clone()],
            calls: Vec::new(),
        }],
        own_call: None,
        direct_calls: Vec::new(),
        call_invitations: Vec::new(),
        direct_mutes: Vec::new(),
        messages: Vec::new(),
        threads: Vec::new(),
        threads_more: false,
        threads_next: None,
        notifications: Vec::new(),
        notification_settings: Vec::new(),
    };
    let conversation = Conversation {
        id: ConversationId::generate(),
        low_account_id: own.min(ana),
        high_account_id: own.max(ana),
    };
    let mut model = ServerModel::from_snapshot(snapshot);
    model.conversations = vec![conversation.clone()];
    let server_id = ServerId::generate();
    let mut known = Known {
        zone: Zone::Fixed(UtcOffset::UTC),
        today: Some(morning().date()),
        clock: Some(morning()),
        ..Known::default()
    };
    known.names.insert((server_id, ana), "Ana".to_owned());
    Scene {
        server_id,
        model,
        known,
        own,
        ana,
        studio,
        general: general.id,
        daily: daily.id,
        conversation,
    }
}

impl Scene {
    fn hit(&self, place_id: Uuid, text: &str, minutes: i64) -> Hit {
        Hit {
            message_id: Some(MessageId::generate()),
            recording_id: None,
            line: None,
            place_id,
            author_id: self.ana,
            created_at: morning() - Duration::minutes(minutes),
            text: text.to_owned(),
        }
    }

    fn spoken(&self, text: &str, line: u32) -> Hit {
        Hit {
            message_id: None,
            recording_id: Some(RecordingId::generate()),
            line: Some(line),
            place_id: self.daily.as_uuid(),
            author_id: self.own,
            created_at: morning() - Duration::days(2),
            text: text.to_owned(),
        }
    }

    fn search(&self, text: &str, hits: Vec<Hit>) -> Search {
        Search {
            server_id: self.server_id,
            text: text.to_owned(),
            filters: Filters::default(),
            progress: Progress::Done,
            hits,
            more: false,
            reading_more: false,
            open: true,
            going: None,
            go_failed: None,
        }
    }

    fn panel(&self, search: Search) -> SearchPanel {
        let mut known = self.known.clone();
        known.search = Some(search);
        let workspace = self.model.workspace(self.studio).unwrap();
        search_panel(self.server_id, workspace, &self.model, &known).unwrap()
    }
}

#[test]
fn the_words_of_a_query_are_marked_in_a_text() {
    let text = "Can you review the retention settings? Retention, not retentions.";
    let marked = |query: &str| -> Vec<&str> {
        marks(text, query)
            .into_iter()
            .map(|range| &text[range])
            .collect()
    };
    // A word is found in every case, and only as a whole word.
    assert_eq!(marked("retention"), ["retention", "Retention"]);
    assert_eq!(
        marked("RETENTION review"),
        ["review", "retention", "Retention"]
    );
    // The words of a phrase are marked. A word after a minus is not searched for, and
    // "or" is a part of the syntax (ADR 0409).
    assert_eq!(
        marked("\"retention settings\" -review or can"),
        ["Can", "retention", "settings", "Retention"]
    );
    assert_eq!(marked("  "), Vec::<&str>::new());
    assert_eq!(marked("-retention"), Vec::<&str>::new());
    // A phrase after a minus is not searched for either.
    assert_eq!(marked("-\"retention settings\""), Vec::<&str>::new());
    assert_eq!(
        marked("review -\"retention settings\" can"),
        ["Can", "review"]
    );
    // A letter whose lower case has another length keeps the marks on the letters.
    let text = "AÇÃO feita, İstanbul ação";
    let found: Vec<&str> = marks(text, "ação")
        .into_iter()
        .map(|range| &text[range])
        .collect();
    assert_eq!(found, ["AÇÃO", "ação"]);
    // The marks never overlap and stand in the order of the text.
    let ranges = marks("a a a", "a a");
    assert_eq!(ranges, [0..1, 2..3, 4..5]);
}

#[test]
fn a_date_choice_gives_the_first_moment_that_is_searched() {
    let zone = Zone::Fixed(UtcOffset::from_hms(-3, 0, 0).unwrap());
    let now = morning();
    assert_eq!(DateFilter::AnyTime.from(now, zone), None);
    // 09:00 UTC is 06:00 three hours behind UTC: the day began six hours before.
    assert_eq!(
        DateFilter::Today.from(now, zone),
        Some(now - Duration::hours(6))
    );
    assert_eq!(
        DateFilter::Last7Days.from(now, zone),
        Some(now - Duration::days(7))
    );
    assert_eq!(
        DateFilter::Last30Days.from(now, zone),
        Some(now - Duration::days(30))
    );
}

#[test]
fn results_are_listed_as_messages_and_as_transcript_lines_with_their_places() {
    let scene = scene();
    let in_general = scene.hit(scene.general.as_uuid(), "the retention spec", 12);
    let direct = scene.hit(
        scene.conversation.id.as_uuid(),
        "retention, you know",
        60 * 54,
    );
    // A result of a place that the state does not hold is not listed.
    let elsewhere = scene.hit(Uuid::now_v7(), "retention elsewhere", 5);
    let spoken = scene.spoken("Diego writes the retention spec", 212);
    let recording_id = spoken.recording_id.unwrap();
    let message_id = in_general.message_id.unwrap();
    let panel = scene.panel(scene.search("retention", vec![in_general, spoken, elsewhere, direct]));
    assert_eq!(panel.state, SearchShown::Results);
    assert_eq!((panel.messages.len(), panel.transcripts.len()), (2, 1));
    assert!(!panel.more);

    let first = &panel.messages[0];
    assert_eq!(
        first.place,
        Whereabouts::Channel {
            name: "general".to_owned(),
            kind: ChannelKind::Text
        }
    );
    assert_eq!(
        (
            first.author.as_str(),
            first.day.as_str(),
            first.time.as_str()
        ),
        ("Ana", "Today", "08:48")
    );
    assert_eq!(first.text, "the retention spec");
    assert_eq!(first.marks, vec![4..13]);
    assert_eq!(first.target, ResultTarget::Message(message_id));

    let second = &panel.messages[1];
    assert_eq!(
        second.place,
        Whereabouts::Direct {
            name: Some("Ana".to_owned())
        }
    );
    assert_eq!(second.day, "Tuesday, 6 October");

    let line = &panel.transcripts[0];
    assert_eq!(
        line.place,
        Whereabouts::Channel {
            name: "Daily".to_owned(),
            kind: ChannelKind::Voice
        }
    );
    assert_eq!(line.author, "Luca");
    assert_eq!(line.time, crate::text::transcript_line(212));
    assert_eq!(
        line.target,
        ResultTarget::Transcript {
            workspace_id: scene.studio,
            channel_id: scene.daily,
            recording_id,
            line: 212
        }
    );
}

#[test]
fn the_panel_tells_a_search_that_runs_found_nothing_was_refused_or_failed() {
    let scene = scene();
    let mut search = scene.search("retention", Vec::new());
    assert_eq!(scene.panel(search.clone()).state, SearchShown::Nothing);
    search.progress = Progress::Searching;
    assert_eq!(scene.panel(search.clone()).state, SearchShown::Searching);
    search.progress = Progress::Refused("Not this text.".to_owned());
    assert_eq!(
        scene.panel(search.clone()).state,
        SearchShown::Refused("Not this text.".to_owned())
    );
    search.progress = Progress::Failed("No answer.".to_owned());
    assert_eq!(
        scene.panel(search.clone()).state,
        SearchShown::Failed("No answer.".to_owned())
    );
    // Results that are shown stay while a new search of the same panel runs.
    search.progress = Progress::Searching;
    search.hits = vec![scene.hit(scene.general.as_uuid(), "retention", 1)];
    let panel = scene.panel(search.clone());
    assert_eq!(panel.state, SearchShown::Searching);
    assert_eq!(panel.messages.len(), 1);
    // Every result lies in a place that the state does not hold any more.
    search.progress = Progress::Done;
    search.hits = vec![scene.hit(Uuid::now_v7(), "retention", 1)];
    search.more = true;
    let panel = scene.panel(search.clone());
    assert_eq!(panel.state, SearchShown::Nothing);
    assert!(panel.more);

    // A search that is closed, and one of another server, has no panel.
    let workspace = scene.model.workspace(scene.studio).unwrap();
    let mut known = scene.known.clone();
    search.open = false;
    known.search = Some(search.clone());
    assert!(search_panel(scene.server_id, workspace, &scene.model, &known).is_none());
    search.open = true;
    known.search = Some(search);
    assert!(search_panel(ServerId::generate(), workspace, &scene.model, &known).is_none());
}

#[test]
fn the_filters_offer_the_channels_and_the_members_of_the_shown_workspace() {
    let mut scene = scene();
    let workspace_id = scene.studio;
    let members = vec![
        WorkspaceMember {
            account_id: scene.own,
            name: "Luca".to_owned(),
            role_ids: Vec::new(),
        },
        WorkspaceMember {
            account_id: scene.ana,
            name: "Ana".to_owned(),
            role_ids: Vec::new(),
        },
    ];
    let mut workspace = scene.model.workspace(workspace_id).unwrap().clone();
    workspace.members = junto_client_core::model::Loadable::Loaded(members);
    scene.model.workspaces = vec![std::sync::Arc::new(workspace)];
    let mut search = scene.search("retention", Vec::new());
    search.filters = Filters {
        channel: Some(scene.general),
        author: Some(scene.ana),
        date: DateFilter::Last7Days,
    };
    search.going = Some(MessageId::generate());
    search.go_failed = Some("Gone.".to_owned());
    let panel = scene.panel(search);
    let channels: Vec<&str> = panel
        .channels
        .iter()
        .map(|(_, name, _)| name.as_str())
        .collect();
    assert_eq!(channels, ["general", "Daily"]);
    let authors: Vec<&str> = panel
        .authors
        .iter()
        .map(|(_, name)| name.as_str())
        .collect();
    assert_eq!(authors, ["Ana", "Luca"]);
    assert_eq!(panel.channel.as_ref().unwrap().1, "general");
    assert_eq!(panel.author.as_ref().unwrap().1, "Ana");
    assert_eq!(panel.date, DateFilter::Last7Days);
    assert!(panel.going.is_some());
    assert_eq!(panel.go_failed.as_deref(), Some("Gone."));
}

#[test]
fn a_search_at_its_bound_says_that_the_server_has_more_and_offers_no_page() {
    let scene = scene();
    let hits: Vec<Hit> = (0..MOST_HITS)
        .map(|number| scene.hit(scene.general.as_uuid(), "retention", number as i64))
        .collect();
    let mut search = scene.search("retention", hits);
    search.more = true;
    let panel = scene.panel(search.clone());
    assert_eq!(panel.messages.len(), MOST_HITS);
    assert!(panel.more && !panel.can_read_more);
    search.hits.truncate(50);
    let panel = scene.panel(search);
    assert!(panel.more && panel.can_read_more);
}

#[test]
fn the_text_of_a_result_is_held_up_to_a_bound() {
    let from_server = |text: String| {
        Hit::from(SearchHit {
            message_id: Some(MessageId::generate()),
            recording_id: None,
            line: None,
            place_id: Uuid::now_v7(),
            author_id: AccountId::generate(),
            created_at: morning(),
            text,
            rank: 1.0,
        })
    };
    assert_eq!(from_server("short".to_owned()).text, "short");
    let long = from_server("ação ".repeat(1_000));
    assert_eq!(long.text.chars().count(), MOST_HIT_CHARACTERS);
    assert!(long.text.ends_with('…'));
}
