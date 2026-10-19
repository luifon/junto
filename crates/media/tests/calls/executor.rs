//! The futures of the calls in a real call, polled by an executor without a Tokio runtime,
//! as the window polls them on the executor of gpui (ADR 0733).

use std::future::Future;

use junto_media::calls::Phase;
use junto_media::engine::{ShareId, ShareState};
use junto_media::share::{CaptureRequest, PictureSource, ShareQuality, SoundChoice};

use crate::limit::limited;
use crate::scene::{PATIENCE, Person, Room, account, media_server_shared};

/// Makes a future with `make` and polls it to its end on a new thread, which has no Tokio
/// runtime, with an executor that has none either. The test waits at most [`PATIENCE`].
async fn without_runtime<F, T>(make: impl FnOnce() -> F + Send + 'static) -> T
where
    F: Future<Output = T>,
    T: Send + 'static,
{
    let thread = std::thread::spawn(move || {
        assert!(tokio::runtime::Handle::try_current().is_err());
        futures_executor::block_on(make())
    });
    tokio::time::timeout(PATIENCE, tokio::task::spawn_blocking(move || thread.join()))
        .await
        .unwrap_or_else(|_| panic!("the future did not end within {PATIENCE:?}"))
        .unwrap()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
async fn the_futures_of_the_calls_end_on_an_executor_without_a_tokio_runtime() {
    limited(async {
        let _media = media_server_shared().await;
        let (ann, ben) = (Person::new(440.0), Person::new(880.0));
        let (room, ann_at) = Room::new("Acme", &ann, "Ann").await;
        let ben_at = room.member(&ann_at, &ben, "Ben").await;
        let (target, ben_id) = (room.target, account(&ben_at));
        ann.enter(target).await;

        let calls = ben.calls.clone();
        without_runtime(move || calls.join(target)).await.unwrap();
        ben.until("the media connection of the call", |view| {
            view.call
                .as_ref()
                .is_some_and(|call| call.target == target && call.phase == Phase::Connected)
        })
        .await;

        // The list of the capture, and a share of the first screen in it.
        let calls = ben.calls.clone();
        let sources = without_runtime(move || calls.share_sources())
            .await
            .unwrap();
        let display = sources.displays.first().expect("a listed screen").id;
        ben.calls.start_share(CaptureRequest {
            picture: PictureSource::Display(display),
            sound: SoundChoice::None,
            quality: ShareQuality::default(),
        });
        ben.until("the own share runs", |view| {
            view.call
                .as_ref()
                .is_some_and(|call| matches!(call.share, ShareState::Running(_)))
        })
        .await;
        ann.until("Ann watches the share of Ben", |view| {
            view.call
                .as_ref()
                .is_some_and(|call| call.watched == Some(ShareId::Of(ben_id)))
        })
        .await;

        let calls = ben.calls.clone();
        without_runtime(move || calls.statistics()).await.unwrap();
        let calls = ben.calls.clone();
        assert!(without_runtime(move || calls.sound_check()).await.is_some());

        let calls = ben.calls.clone();
        without_runtime(move || calls.leave()).await;
        ben.until("Ben has left the call", |view| view.call.is_none())
            .await;
        ann.until("the share of Ben has ended", |view| {
            view.call
                .as_ref()
                .is_some_and(|call| call.shares.is_empty())
        })
        .await;

        // Ann invites Ben back, and Ben accepts the invitation.
        let invitation = ann_at
            .api()
            .invite_to_call(room.voice, ben_id)
            .await
            .unwrap();
        let calls = ben.calls.clone();
        without_runtime(move || calls.join_invited(target, invitation.id))
            .await
            .unwrap();
        ben.until("the media connection of the invited call", |view| {
            view.call
                .as_ref()
                .is_some_and(|call| call.target == target && call.phase == Phase::Connected)
        })
        .await;
        let calls = ben.calls.clone();
        without_runtime(move || calls.leave()).await;
        ben.until("Ben has left the call again", |view| view.call.is_none())
            .await;
    })
    .await;
}
