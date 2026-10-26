//! System notifications of single events of a server (ADR 0742): each one is told while no
//! window of the application is active, and a press leads to its place. The events that
//! need a media server are in `live::alerts`.

use std::sync::Arc;

use gpui::TestAppContext;
use junto_client::state::account::AccountPage;
use junto_client::state::admin::{AdminOpen, AdminPage};
use junto_client::state::view::ServerStatus;
use junto_client_core::live::Notice;
use junto_client_core::protocol::account::{DeviceOrigin, RecoveryNotice};
use junto_client_core::protocol::events::Event;
use junto_client_core::protocol::export::{Export, ExportState};
use junto_client_core::protocol::ids::{
    CallId, DeviceId, ExportId, RecordingId, RecoveryId, ServerId,
};
use junto_client_core::protocol::invite::ApproveJoinRequest;
use junto_client_core::protocol::keys::PublicKey;
use junto_client_core::protocol::message::MessagePlace;
use junto_client_core::protocol::recording::{Recording, RecordingState};
use junto_client_core::protocol::transcript::TranscriptState;

use crate::calls::voice_channel;
use crate::harness::{Window, World, member, named};

impl Window {
    /// Gives the state one event of a server, as the stream of the server tells it.
    pub fn event(&mut self, server_id: ServerId, event: Event) {
        let notice = Notice::Event {
            id: 1,
            at: time::OffsetDateTime::now_utc(),
            event: Arc::new(event),
        };
        self.state.update(&mut self.cx, |state, cx| {
            state.notice_fixture(server_id, &notice, cx)
        });
        self.settle();
    }

    /// The titles of the system notifications that are shown now.
    pub fn shown_titles(&mut self) -> Vec<String> {
        self.kept()
            .shown
            .iter()
            .map(|alert| alert.title.clone())
            .collect()
    }

    /// Presses the system notification with this title.
    pub fn press_alert(&mut self, title: &str) {
        let tag = self
            .kept()
            .shown
            .iter()
            .find(|alert| alert.title == title)
            .map(|alert| alert.tag.clone())
            .unwrap_or_else(|| panic!("no system notification {title}"));
        let state = self.state.clone();
        self.cx
            .update(|_, cx| state.update(cx, |state, cx| state.alert_pressed(&tag, cx)));
        self.settle();
    }
}

#[gpui::test]
fn the_answer_to_a_join_request_is_told_while_the_window_is_not_active(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let (_directory, mut window) = named(cx, "Luca");
    window.find_server(&server.address());
    window.click("request-access");
    window.until("the request waits", |window| {
        matches!(
            window.status(),
            Some(ServerStatus::WaitingForApproval { .. })
        )
    });
    assert!(window.kept().shown.is_empty());
    let requests = world.run(admin.handle.api().join_requests()).unwrap();
    world
        .run(
            admin
                .handle
                .api()
                .approve_join_request(requests[0].id, &ApproveJoinRequest::default()),
        )
        .unwrap();
    window.until("the approval is told", |window| {
        window
            .shown_titles()
            .contains(&"Your request to join Harbor was approved".to_owned())
    });
    window.until_ready();

    // A second device asks and is refused.
    let (_other_directory, mut other) = named(cx, "Ines");
    other.find_server(&server.address());
    other.click("request-access");
    other.until("the second request waits", |window| {
        matches!(
            window.status(),
            Some(ServerStatus::WaitingForApproval { .. })
        )
    });
    let requests = world.run(admin.handle.api().join_requests()).unwrap();
    world
        .run(admin.handle.api().deny_join_request(requests[0].id))
        .unwrap();
    other.until("the refusal is told", |window| {
        window
            .shown_titles()
            .contains(&"Your request to join Harbor was refused".to_owned())
    });
}

#[gpui::test]
fn an_addition_to_a_workspace_is_told_and_leads_to_the_workspace(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let own = window.own_account();
    let lab = world
        .run(admin.handle.api().create_workspace("Lab".to_owned()))
        .unwrap();
    world
        .run(admin.handle.api().add_workspace_member(lab.id, own))
        .unwrap();
    window.until("the addition is told", |window| {
        window
            .shown_titles()
            .contains(&"You were added to Lab".to_owned())
    });
    window.press_alert("You were added to Lab");
    window.until("the workspace is shown", |window| {
        window
            .view()
            .workspace()
            .is_some_and(|workspace| workspace.id == lab.id)
    });
}

/// The events of a recording, of an export and of the devices of the account, given to
/// the state of a real server: each is told once under the rules, and leads to its place.
#[gpui::test]
fn a_recording_an_export_and_a_device_are_told_and_lead_to_their_place(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let own = window.own_account();
    let general = window.selected_channel();
    voice_channel(&world, &admin, &mut window, "Daily");
    let view = window.view();
    let server_id = view.selected.as_ref().unwrap().id;
    let workspace_id = view.workspace().unwrap().id;
    let daily = window
        .channels()
        .into_iter()
        .find(|row| row.name == "Daily")
        .unwrap()
        .id;
    let recording = |state, transcript| Recording {
        id: RecordingId::from_uuid(uuid::Uuid::from_u128(7)),
        workspace_id,
        channel_id: daily,
        call_id: CallId::generate(),
        state,
        started_by: own,
        requested_at: time::OffsetDateTime::now_utc(),
        recording_since: None,
        stop: None,
        ended_at: None,
        failure: None,
        language: "auto".to_owned(),
        segments: 1,
        failed_segments: 0,
        bytes: None,
        timeline_started_at: None,
        transcript,
        transcript_lines: 0,
    };

    // The own recording is ready: told once, and a press shows its voice channel.
    for _ in 0..2 {
        window.event(
            server_id,
            Event::RecordingUpdated {
                recording: Box::new(recording(RecordingState::Ready, TranscriptState::Waiting)),
            },
        );
    }
    let ready = "The recording of Daily is ready";
    assert_eq!(
        window
            .shown_titles()
            .iter()
            .filter(|title| *title == ready)
            .count(),
        1
    );
    window.press_alert(ready);
    window.until("the voice channel of the recording is shown", |window| {
        window.selected_channel() == daily
    });

    // Its transcript is ready.
    window.event(
        server_id,
        Event::RecordingUpdated {
            recording: Box::new(recording(RecordingState::Ready, TranscriptState::Ready)),
        },
    );
    assert!(
        window
            .shown_titles()
            .contains(&"The transcript of Daily is ready".to_owned())
    );

    // An export of the person is ready: a press opens the page of the exports.
    window.event(
        server_id,
        Event::ExportUpdated {
            export: Export {
                id: ExportId::generate(),
                place: MessagePlace::Channel {
                    workspace_id,
                    channel_id: general,
                },
                state: ExportState::Ready,
                failure: None,
                requested_at: time::OffsetDateTime::now_utc(),
                finished_at: None,
                expires_at: None,
                bytes: Some(10),
            },
        },
    );
    let general_name = window.channel_names()[0].clone();
    let export = format!("Your export of #{general_name} is ready");
    window.until("the export is told", |window| {
        window.shown_titles().contains(&export)
    });
    window.press_alert(&export);
    window.until("the page of the exports opens", |window| {
        window
            .state
            .read_with(&window.cx, |state, _| state.admin_open())
            == Some(AdminOpen::Server {
                server_id,
                page: AdminPage::Exports,
            })
    });
    window.press("escape");

    // One server shows three system notifications in ten seconds at most.
    world.run(async { tokio::time::sleep(std::time::Duration::from_millis(10_500)).await });

    // Another device was added to the account, and the owner recovered it: a press opens
    // the page of the account.
    window.event(
        server_id,
        Event::DeviceAdded {
            device_id: DeviceId::generate(),
            name: "Laptop".to_owned(),
            public_key: PublicKey::from_bytes([3; 32]),
            origin: DeviceOrigin::Link,
            created_at: time::OffsetDateTime::now_utc(),
        },
    );
    window.event(
        server_id,
        Event::AccountRecovered {
            recovery: RecoveryNotice {
                id: RecoveryId::generate(),
                device_id: None,
                performed_by: None,
                performed_at: time::OffsetDateTime::now_utc(),
            },
        },
    );
    let titles = window.shown_titles();
    assert!(
        titles.contains(&"A device was added to your account".to_owned()),
        "{titles:?}"
    );
    assert!(
        titles.contains(&"The owner gave a new device access to your account".to_owned()),
        "{titles:?}"
    );
    window.press_alert("A device was added to your account");
    window.until("the devices of the account are shown", |window| {
        window
            .state
            .read_with(&window.cx, |state, _| state.account_open())
            .is_some_and(|open| open.page == AccountPage::Devices)
    });

    // While the window is active, nothing is told.
    window.cx.update(|window, _| window.activate_window());
    window.event(
        server_id,
        Event::AccountRecovered {
            recovery: RecoveryNotice {
                id: RecoveryId::generate(),
                device_id: None,
                performed_by: None,
                performed_at: time::OffsetDateTime::now_utc(),
            },
        },
    );
    assert_eq!(window.kept().told.len(), 5);
}
