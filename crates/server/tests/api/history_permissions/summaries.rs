use super::*;
use junto_protocol::message::MessagePlace;

#[tokio::test]
async fn followed_thread_lists_require_history_permission() {
    let (server, _, reader, channel, _) = denied().await;
    server
        .get(&reader, &format!("/api/v1/channels/{}/threads", channel.id))
        .await
        .error(ErrorCode::Forbidden);
}

#[tokio::test]
async fn snapshots_do_not_expose_old_message_counts_without_history() {
    let (server, owner, reader, channel, _) = denied().await;
    crate::discovery_support::mention(&server, &owner, &channel, &reader).await;
    let snapshot = server.snapshot(&reader).await;
    assert!(
        snapshot
            .workspaces
            .iter()
            .any(|w| w.channels.iter().any(|c| c.id == channel.id))
    );
    assert!(!snapshot.messages.iter().any(
        |s| matches!(s.place, MessagePlace::Channel {channel_id,..} if channel_id==channel.id)
    ));
    let allowed = server.snapshot(&owner).await;
    assert!(allowed.messages.iter().any(
        |s| matches!(s.place, MessagePlace::Channel {channel_id,..} if channel_id==channel.id)
    ));
}
