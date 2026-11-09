//! The event stream connects again after a cut, in the set-up of the quote tests
//! (`reply_quotes::reply_behind_a_proxy`): a channel with a reply to an original that the
//! loaded messages do not hold, behind a proxy that loses the answers of the reads of that
//! original.

use gpui::TestAppContext;
use junto_client::state::view::ServerStatus;
use junto_client_core::live::StreamStatus;
use junto_client_core::protocol::ids::ServerId;

use crate::harness::Window;
use crate::support::FaultProxy;

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
    let (_world, _server, proxy, _directory, mut window, _original, read) =
        crate::reply_quotes::reply_behind_a_proxy(cx, 7);
    window.until("a read of the original was lost", |_| {
        proxy
            .all()
            .iter()
            .any(|seen| seen.method == "GET" && seen.path.ends_with(&read))
    });
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
