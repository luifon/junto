//! The members of a workspace who can see a channel (ADR 0790).

use crate::discovery_support::*;
use junto_protocol::structure::ChannelViewers;

#[tokio::test]
async fn the_viewers_of_a_channel_are_the_members_who_may_see_it() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let ann = server.join(&owner, "Ann").await;
    let ben = server.join(&owner, "Ben").await;
    let workspace = server.default_workspace(&owner).await;
    let plans = server.channel(&owner, workspace.id, "plans").await;
    let path = format!("/api/v1/channels/{}/viewers", plans.id);
    let mut every = vec![owner.account_id, ann.account_id, ben.account_id];
    every.sort();
    let open: ChannelViewers = server.get(&ann, &path).await.ok(StatusCode::OK);
    let mut listed = open.account_ids.clone();
    listed.sort();
    assert_eq!(listed, every);

    // A private channel: the "everyone" role does not see it, and a rule lets Ann see it.
    let everyone = server.everyone_role(&owner, workspace.id).await;
    server
        .put(
            &owner,
            &format!("/api/v1/channels/{}/rules/roles/{}", plans.id, everyone.id),
            &json!({ "deny": ["view_channel"] }),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    server
        .put(
            &owner,
            &format!(
                "/api/v1/channels/{}/rules/people/{}",
                plans.id, ann.account_id
            ),
            &json!({ "allow": ["view_channel"] }),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    let private: ChannelViewers = server.get(&ann, &path).await.ok(StatusCode::OK);
    let mut listed = private.account_ids.clone();
    listed.sort();
    let mut expected = vec![owner.account_id, ann.account_id];
    expected.sort();
    assert_eq!(listed, expected);

    // A member who does not see the channel does not learn who sees it.
    server.get(&ben, &path).await.error(ErrorCode::NotFound);
}
