//! The event stream connects again after a cut, in the set-up of the quote tests: a channel
//! with a reply to an original that the loaded messages do not hold, behind a proxy that loses
//! the answers of the reads of that original.

use gpui::TestAppContext;
use junto_client::state::view::ServerStatus;
use junto_client_core::live::StreamStatus;
use junto_client_core::model::PlaceId;
use junto_client_core::protocol::ids::ServerId;
use junto_client_core::protocol::message::SendMessage;
use junto_client_core::protocol::structure::ChannelKind;

use crate::harness::{Window, World, member};
use crate::messages::document;
use crate::support::{Fault, FaultProxy, TestServer};

/// The reply set-up behind a proxy that loses the answer of the first `losses` reads of the
/// original. The server is returned with the proxy: a dropped `TestServer` stops its process,
/// and the proxy would then refuse every new connection.
fn reply_behind_a_proxy(
    cx: &mut TestAppContext,
    losses: usize,
) -> (World, TestServer, FaultProxy, tempfile::TempDir, Window) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let proxy = world.run(FaultProxy::start(&server));
    let admin = world.owner(&server);
    let api = admin.handle.api();
    let snapshot = world.run(api.snapshot()).unwrap().snapshot;
    let channel = snapshot.workspaces[0]
        .channels
        .iter()
        .find(|channel| channel.kind == ChannelKind::Text)
        .unwrap()
        .id;
    let original = admin.send(&world, channel, document("the older original"));
    for index in 0..60 {
        admin.send(&world, channel, document(&format!("later {index}")));
    }
    let read = format!("/messages/{}", original.id);
    proxy.fail_ending(&read, std::iter::repeat_n(Fault::LoseAnswer, losses));
    let (directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Luca");
    window.until_message("later 59");
    world
        .run(api.send_message(
            PlaceId::Channel(channel),
            &SendMessage {
                document: document("the answer"),
                reply_to: Some(original.id),
                attachments: Vec::new(),
            },
        ))
        .unwrap();
    window.until_message("the answer");
    window.until("a read of the original was lost", |_| {
        proxy
            .all()
            .iter()
            .any(|seen| seen.method == "GET" && seen.path.ends_with(&read))
    });
    (world, server, proxy, directory, window)
}

fn stream_status(window: &mut Window, server_id: ServerId) -> Option<StreamStatus> {
    window.state.read_with(&window.cx, |state, _| {
        state
            .client()
            .server(server_id)
            .map(|server| server.live().view().status)
    })
}

/// After a cut while new connections are refused, the stream is live again when they are
/// accepted, also while the reads of a quoted original lose their answers. A failure names
/// each status of the stream that the wait saw, the connections of the stream that reached
/// the proxy, and the failures of the proxy, such as a server that does not run any more.
#[gpui::test]
fn the_stream_is_live_again_after_a_cut_behind_a_proxy_that_loses_quote_reads(
    cx: &mut TestAppContext,
) {
    let (_world, _server, proxy, _directory, mut window) = reply_behind_a_proxy(cx, 7);
    let server_id = window.view().selected.unwrap().id;
    let connections = |proxy: &FaultProxy| {
        proxy
            .all()
            .iter()
            .filter(|seen| seen.path.ends_with("/api/v1/events"))
            .count()
    };
    let before = connections(&proxy);
    proxy.block_streams(true);
    proxy.cut_streams();
    window.until("the interruption is shown", |window| {
        window.status() == Some(ServerStatus::Reconnecting)
    });
    proxy.block_streams(false);
    let mut seen: Vec<Option<StreamStatus>> = Vec::new();
    let started = std::time::Instant::now();
    while window.status() != Some(ServerStatus::Ready)
        && started.elapsed() < std::time::Duration::from_secs(50)
    {
        let status = stream_status(&mut window, server_id);
        if seen.last() != Some(&status) {
            seen.push(status);
        }
        window.pass(std::time::Duration::from_millis(100));
        window.settle();
    }
    assert_eq!(
        window.status(),
        Some(ServerStatus::Ready),
        "the stream is not live again after {:?}: the statuses of the stream {seen:?}; \
         {} connections of the stream reached the proxy after the cut; failures of the proxy \
         {:?}",
        started.elapsed(),
        connections(&proxy) - before,
        proxy.failures()
    );
}
