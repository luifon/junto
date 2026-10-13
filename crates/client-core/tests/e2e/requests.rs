//! What the client asked of the server, in the client core against the real server
//! (ADR 0412, ADR 0413): the choice to follow a thread, the followed threads of a channel,
//! the first authors of a thread, every notice read in one request, and a notification
//! default that is not set.

use junto_client_core::api::ErrorCategory;
use junto_client_core::model::PlaceId;
use junto_client_core::protocol::discovery::NotificationLevel;

use crate::messages::{document, message_when, timeline_when};
use crate::stream::model_when;
use crate::support::logged_in;
use crate::threads::{open, scene_with_root};

#[tokio::test(flavor = "multi_thread")]
async fn an_account_leaves_a_thread_follows_it_again_and_reads_every_notice_in_one_request() {
    let (scene, place, root) = scene_with_root("Client requests").await;
    let (owner, member) = (&scene.owner, &scene.member);
    let PlaceId::Channel(channel_id) = place else {
        unreachable!()
    };
    let thread = PlaceId::Thread {
        channel_id,
        root_id: root.id,
    };
    open(member, thread).await;
    member
        .live()
        .send_message(thread, document("first reply"), None)
        .unwrap();
    let first = message_when(member, thread, "first reply").await;

    // The root names the first authors of its replies (ADR 0413).
    let member_id = logged_in(member);
    timeline_when(
        owner,
        place,
        "the author of the reply on the root",
        |timeline, _| {
            timeline
                .message(root.id)
                .is_some_and(|root| root.reply_authors == [member_id])
        },
    )
    .await;

    // The owner wrote the root and follows the thread by the rule: one unread reply and
    // its notice.
    let model = model_when(owner, "the unread thread and its notice", |model| {
        model
            .thread_state(root.id)
            .is_some_and(|state| state.unread == 1)
            && model.unread_notices().count() == 1
    })
    .await;
    let state = model.thread_state(root.id).unwrap();
    assert!(state.following);
    assert_eq!(state.follow_setting, None);
    let followed = owner
        .api()
        .followed_threads(channel_id, None, None)
        .await
        .unwrap();
    assert_eq!(followed.threads.len(), 1);
    assert_eq!(followed.threads[0].root_id, root.id);
    assert!(!followed.more);

    // One request marks every notice read, and the event reaches the state of the
    // client (ADR 0412).
    let newest = model.notices.first().unwrap().created_at;
    let read = owner
        .api()
        .mark_notifications_read(Some(newest))
        .await
        .unwrap();
    assert_eq!(read.marked, 1);
    model_when(owner, "no unread notice", |model| {
        model.unread_notices().count() == 0 && !model.notices.is_empty()
    })
    .await;
    // Nothing is left to mark.
    let again = owner.api().mark_notifications_read(None).await.unwrap();
    assert_eq!(again.marked, 0);

    // The owner chooses not to follow the thread: its replies are not unread any more,
    // and the state follows the event.
    let stopped = owner.api().set_thread_follow(root.id, false).await.unwrap();
    assert!(!stopped.following);
    assert_eq!(stopped.follow_setting, Some(false));
    model_when(owner, "the thread is no unread thread", |model| {
        model.thread_state(root.id).is_none()
    })
    .await;
    member
        .live()
        .send_message(thread, document("second reply"), None)
        .unwrap();
    message_when(member, thread, "second reply").await;
    let followed = owner
        .api()
        .followed_threads(channel_id, None, None)
        .await
        .unwrap();
    assert!(followed.threads.is_empty());
    assert!(
        owner
            .live()
            .view()
            .model
            .unwrap()
            .thread_state(root.id)
            .is_none()
    );

    // Without a choice the rule decides again: the author of the root follows.
    owner.api().clear_thread_follow(root.id).await.unwrap();
    member
        .live()
        .send_message(thread, document("third reply"), None)
        .unwrap();
    message_when(member, thread, "third reply").await;
    let model = model_when(owner, "the thread is followed again", |model| {
        model
            .thread_state(root.id)
            .is_some_and(|state| state.following && state.unread >= 1)
    })
    .await;
    assert_eq!(model.thread_state(root.id).unwrap().follow_setting, None);
    let _ = first;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_member_follows_the_notification_default_of_a_workspace_and_its_removal() {
    let (scene, _, _) = scene_with_root("Notification default").await;
    let (owner, member, workspace_id) = (&scene.owner, &scene.member, scene.workspace_id);
    let level = |model: &junto_client_core::model::ServerModel| {
        model
            .workspace(workspace_id)
            .and_then(|workspace| workspace.notification_default)
    };
    // A workspace sets no level until an administrator stores one.
    assert_eq!(level(&member.live().view().model.unwrap()), None);
    assert_eq!(
        member
            .api()
            .notification_default(workspace_id)
            .await
            .unwrap(),
        None
    );
    let stored = owner
        .api()
        .set_notification_default(workspace_id, NotificationLevel::All)
        .await
        .unwrap();
    assert_eq!(stored, Some(NotificationLevel::All));
    model_when(member, "the level of the workspace", |model| {
        level(model) == Some(NotificationLevel::All)
    })
    .await;

    // Only an administrator removes it; the member follows the removal.
    let refused = member
        .api()
        .remove_notification_default(workspace_id)
        .await
        .expect_err("a member removed the default of a workspace");
    assert_eq!(refused.category(), Some(ErrorCategory::NotAllowed));
    owner
        .api()
        .remove_notification_default(workspace_id)
        .await
        .unwrap();
    model_when(member, "no level of the workspace", |model| {
        level(model).is_none()
    })
    .await;
    assert_eq!(
        owner
            .api()
            .notification_default(workspace_id)
            .await
            .unwrap(),
        None
    );
}
