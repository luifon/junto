//! Screen shares between engines (ADR 0502, ADR 0737): a share with its sound, two shares
//! and the choice of the viewer, the end of a share, and the blocks of the server. The
//! capture is a picture and a tone that the program makes; no screen and no sound of the
//! machine is read.

use std::time::Duration;

use junto_client_core::protocol::call::{BlockReason, ShareAudio};
use junto_media::engine::{MainPicture, MicrophoneState, ShareId, ShareInfo, ShareState};
use junto_media::share::synthetic::{Colour, SyntheticCapture};
use junto_media::share::{
    CaptureError, CaptureRequest, PictureSource, ShareKind, ShareQuality, SoundChoice,
};

use crate::limit::limited;
use crate::scene::{
    ABSENT_DBFS, PATIENCE, Person, Room, account, media_server_alone, media_server_shared,
    model_when,
};

/// The tones of all system sound and of the application "Player" of the synthetic capture,
/// on the left and on the right channel.
const SYSTEM: (f32, f32) = (700.0, 1_900.0);
const PLAYER: (f32, f32) = (1_100.0, 2_700.0);
/// A tone of a share that arrives is above this. The call library sends a sound track
/// with one channel, so each of the two tones of a share arrives at half its level on
/// both channels: about -18 dBFS.
const SHARE_HEARD_DBFS: f32 = -26.0;
/// The colours of the picture of the synthetic capture at its start.
const RED: Colour = (200, 30, 30);
const BLUE: Colour = (30, 30, 200);
const GREEN: Colour = (30, 160, 60);
const YELLOW: Colour = (210, 190, 40);

fn screen(sound: SoundChoice) -> CaptureRequest {
    CaptureRequest {
        picture: PictureSource::Display(1),
        sound,
        quality: ShareQuality::default(),
    }
}

fn window(sound: SoundChoice) -> CaptureRequest {
    CaptureRequest {
        picture: PictureSource::Window(11),
        sound,
        quality: ShareQuality::default(),
    }
}

fn player() -> SoundChoice {
    SoundChoice::Application(SyntheticCapture::player())
}

/// A colour after the video encoding is near the colour that was sent.
fn near(seen: Colour, sent: Colour) -> bool {
    let far = |one: u8, other: u8| one.abs_diff(other) > 45;
    !(far(seen.0, sent.0) || far(seen.1, sent.1) || far(seen.2, sent.2))
}

impl Person {
    /// Waits until the share of this device runs, and returns what it sends.
    async fn until_sharing(&self) -> ShareInfo {
        let view = self
            .until("the own share runs", |view| {
                view.call
                    .as_ref()
                    .is_some_and(|call| matches!(call.share, ShareState::Running(_)))
            })
            .await;
        match view.call.unwrap().share {
            ShareState::Running(info) => info,
            other => panic!("{other:?}"),
        }
    }

    /// Waits for a picture of the watched share with these colours in its halves.
    async fn until_shown(&self, share: ShareId, left: Colour, right: Colour) -> MainPicture {
        let mut pictures = self.calls.main_picture();
        let shown = tokio::time::timeout(
            PATIENCE,
            pictures.wait_for(|picture| {
                picture.as_ref().is_some_and(|picture| {
                    let frame = &picture.frame;
                    let at = |x: u32| frame.pixel(x, frame.height / 2);
                    picture.share == share
                        && at(frame.width / 4).is_some_and(|seen| near(seen, left))
                        && at(frame.width * 3 / 4).is_some_and(|seen| near(seen, right))
                })
            }),
        )
        .await;
        match shown {
            Ok(Ok(picture)) => picture.clone().unwrap(),
            _ => panic!(
                "no picture of {share:?} with the colours {left:?} and {right:?}: {:?}, and the \
                 view is {:?}",
                self.calls.main_picture().borrow().as_ref().map(|picture| (
                    picture.share,
                    picture
                        .frame
                        .pixel(picture.frame.width / 4, picture.frame.height / 2),
                    picture
                        .frame
                        .pixel(picture.frame.width * 3 / 4, picture.frame.height / 2),
                )),
                self.calls.view()
            ),
        }
    }

    async fn until_heard_tone(&self, hz: f32) -> f32 {
        self.until_level(hz, "the tone of the share arrives", |level| {
            level > SHARE_HEARD_DBFS
        })
        .await
    }

    async fn until_silent_tone(&self, hz: f32) -> f32 {
        self.until_level(hz, "the tone of the share has stopped", |level| {
            level < ABSENT_DBFS
        })
        .await
    }

    /// Waits until a tone of a share arrives at a level that does not change any more, and
    /// returns that level.
    async fn until_steady_tone(&self, hz: f32) -> f32 {
        let started = tokio::time::Instant::now();
        let mut before = f32::MIN;
        loop {
            let level = self.hears(hz);
            if level > SHARE_HEARD_DBFS && (level - before).abs() < 0.3 {
                return level;
            }
            assert!(
                started.elapsed() < PATIENCE,
                "after {PATIENCE:?} the tone of {hz} Hz is not steady: {level} dBFS"
            );
            before = level;
            tokio::time::sleep(Duration::from_millis(600)).await;
        }
    }

    /// Waits until the picture of the share of a person arrives with this width.
    async fn until_width(&self, of: junto_client_core::protocol::ids::AccountId, width: u32) {
        self.until("the layer of the share", |view| {
            view.call.as_ref().is_some_and(|call| {
                call.shares.iter().any(|share| {
                    share.account_id == of && share.size.is_some_and(|size| size.0 == width)
                })
            })
        })
        .await;
    }

    /// Waits until the picture of the share of a person arrives with this width and keeps
    /// it for a second. A width that the picture has only on its way to another one does
    /// not end the wait.
    async fn until_width_stays(&self, of: junto_client_core::protocol::ids::AccountId, width: u32) {
        let started = tokio::time::Instant::now();
        loop {
            self.until_width(of, width).await;
            let mut views = self.calls.watch();
            let other = views.wait_for(|view| {
                !view.call.as_ref().is_some_and(|call| {
                    call.shares.iter().any(|share| {
                        share.account_id == of && share.size.is_some_and(|size| size.0 == width)
                    })
                })
            });
            if tokio::time::timeout(Duration::from_secs(1), other)
                .await
                .is_err()
            {
                return;
            }
            assert!(
                started.elapsed() < PATIENCE,
                "after {PATIENCE:?} the picture does not keep the width {width}; the view is {:?}",
                self.calls.view()
            );
        }
    }

    /// Waits for half a second of sound that was all played after this call, and returns
    /// the level of a tone in it.
    async fn hears_afresh(&self, hz: f32) -> f32 {
        self.host.clear_played();
        let started = tokio::time::Instant::now();
        loop {
            let (played, _) = self.host.played(Duration::from_millis(500));
            if !played.is_empty() {
                return self.hears(hz);
            }
            assert!(started.elapsed() < PATIENCE, "no sound was played");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
async fn a_share_with_sound_arrives_with_its_picture_and_its_sound() {
    limited(async {
        let _media = media_server_shared().await;
        let (ann, ben, cal) = (Person::new(440.0), Person::new(880.0), Person::new(1320.0));
        let (room, ann_at) = Room::new("Acme", &ann, "Ann").await;
        let ben_at = room.member(&ann_at, &ben, "Ben").await;
        let cal_at = room.member(&ann_at, &cal, "Cal").await;
        let (ann_id, place) = (account(&ann_at), room.target.place);
        let _ = &ben_at;
        ann.enter(room.target).await;
        ben.enter(room.target).await;
        ben.until_heard(&ann).await;
        // Without a call nothing is shared.
        cal.calls.start_share(screen(SoundChoice::System));
        assert_eq!(cal.capture.started(), []);

        // Ann shares her screen with the sound of one application.
        ann.calls.start_share(screen(player()));
        ann.until_sharing().await;
        // The sound track is published by itself, a moment after the picture track.
        let view = ann
            .until("the sound of the share is sent", |view| {
                view.call.as_ref().is_some_and(
                    |call| matches!(&call.share, ShareState::Running(info) if !info.sound_held),
                )
            })
            .await;
        let ShareState::Running(info) = view.call.unwrap().share else {
            panic!("the share runs");
        };
        assert_eq!(
            info,
            ShareInfo {
                kind: ShareKind::Screen,
                sound: ShareAudio::Application,
                sound_held: false,
                size: (1280, 720),
                frame_rate: 30,
            }
        );
        assert_eq!(ann.capture.started(), [screen(player())]);
        assert_eq!(ann.capture.running(), 1);

        // The server shows the share with its sound choice to a person outside the call.
        model_when(&cal_at, "the share of Ann", |model| {
            model.call_at(&place).is_some_and(|call| {
                call.participant(ann_id).is_some_and(|ann| {
                    ann.screen_share
                        .as_ref()
                        .is_some_and(|share| share.audio == ShareAudio::Application)
                })
            })
        })
        .await;

        // Ben gets the share: what it is, its picture, and its sound on both channels.
        let view = ben
            .until("Ben watches the share of Ann", |view| {
                view.call.as_ref().is_some_and(|call| {
                    call.watched == Some(ShareId::Of(ann_id))
                        && call.shares.len() == 1
                        && call.shares[0].size.is_some()
                })
            })
            .await;
        let seen = &view.call.unwrap().shares[0];
        assert_eq!(seen.account_id, ann_id);
        assert_eq!(
            (seen.kind, seen.frame_rate),
            (Some(ShareKind::Screen), Some(30))
        );
        let picture = ben.until_shown(ShareId::Of(ann_id), RED, BLUE).await;
        assert_eq!(picture.frame.source.0 * 9, picture.frame.source.1 * 16);
        let left = ben.until_heard_tone(PLAYER.0).await;
        assert!(left < -8.0, "{left}");
        // The sound of both channels of the capture arrives, on both channels of the output.
        ben.until_heard_tone(PLAYER.1).await;
        ben.until_level(PLAYER.1, "the right channel of the output", |_| {
            ben.hears_right(PLAYER.1) > SHARE_HEARD_DBFS
                && ben.hears_right(PLAYER.0) > SHARE_HEARD_DBFS
        })
        .await;
        // The sound of the share is the chosen application and not all system sound, and the
        // voice of Ann arrives beside it.
        assert!(ben.hears(SYSTEM.0) < ABSENT_DBFS, "{}", ben.hears(SYSTEM.0));
        ben.until_heard(&ann).await;

        // A small picture of the share exists for the bar of shares.
        let mut small = ben.calls.small_pictures();
        let has_small = tokio::time::timeout(
            PATIENCE,
            small.wait_for(|small| small.contains_key(&ShareId::Of(ann_id))),
        )
        .await
        .is_ok();
        assert!(has_small);

        // Ann sees her own share and does not hear it.
        assert_eq!(ann.calls.view().call.unwrap().watched, Some(ShareId::Own));
        ann.until_shown(ShareId::Own, RED, BLUE).await;
        let own = ann.hears_afresh(PLAYER.0).await;
        assert!(own < ABSENT_DBFS, "{own}");

        // A change of what the screen shows arrives.
        ann.capture.set_colours(GREEN, YELLOW);
        ben.until_shown(ShareId::Of(ann_id), GREEN, YELLOW).await;

        // Ann stops: the picture and the sound end for Ben, and the server shows no share.
        ann.calls.stop_share();
        ann.until("the own share is off", |view| {
            view.call
                .as_ref()
                .is_some_and(|call| call.share == ShareState::Off && call.watched.is_none())
        })
        .await;
        assert_eq!(ann.capture.running(), 0);
        ben.until("the share of Ann has ended", |view| {
            view.call
                .as_ref()
                .is_some_and(|call| call.shares.is_empty() && call.watched.is_none())
        })
        .await;
        ben.until_silent_tone(PLAYER.0).await;
        assert_eq!(*ben.calls.main_picture().borrow(), None);
        assert!(ben.calls.small_pictures().borrow().is_empty());
        model_when(&cal_at, "no share of Ann", |model| {
            model.call_at(&place).is_some_and(|call| {
                call.participant(ann_id)
                    .is_some_and(|ann| ann.screen_share.is_none())
            })
        })
        .await;
        // The voice of Ann goes on.
        ben.until_heard_afresh(&ann).await;
    })
    .await;
}

/// The longest time, in seconds, from a change of the watched share to the first of three
/// seconds in a row with the full frame rate (ADR 0737).
const FULL_RATE_WITHIN_SECONDS: usize = 20;

/// The last three seconds hold at least 66 pictures of the full size: more than the 15 of
/// the defaults of the call library, and more than the 45 that a layer of half the size
/// gives at its 15 per second when a peer publishes one (this client publishes none,
/// ADR 0773).
fn full_rate(seconds: &[usize]) -> bool {
    seconds.len() >= 3 && seconds[seconds.len() - 3..].iter().sum::<usize>() >= 66
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
async fn the_sound_follows_the_share_that_the_viewer_watches_and_a_share_ends_with_its_sharer() {
    limited(async {
        // Keep the measured room isolated; whether contention causes the low counts is unproven.
        let _media = media_server_alone().await;
        let (ann, ben, cal) = (Person::new(440.0), Person::new(880.0), Person::new(1320.0));
        let (room, ann_at) = Room::new("Acme", &ann, "Ann").await;
        let ben_at = room.member(&ann_at, &ben, "Ben").await;
        let cal_at = room.member(&ann_at, &cal, "Cal").await;
        let (ann_id, ben_id) = (account(&ann_at), account(&ben_at));
        let place = room.target.place;
        for person in [&ann, &ben, &cal] {
            person.enter(room.target).await;
            // The voices are closed, so that only the sound of the shares is measured.
            person.calls.set_muted(true);
        }
        ben.capture.set_colours(GREEN, YELLOW);

        // Ann shares her screen with all system sound. Cal watches it.
        ann.calls.start_share(screen(SoundChoice::System));
        ann.until_sharing().await;
        cal.until("Cal watches the share of Ann", |view| {
            view.call
                .as_ref()
                .is_some_and(|call| call.watched == Some(ShareId::Of(ann_id)))
        })
        .await;
        cal.until_heard_tone(SYSTEM.0).await;

        // Ben shares a window with the sound of its application. Cal goes on watching Ann:
        // a share that starts does not replace what a person watches.
        let share_started = tokio::time::Instant::now();
        ben.calls.start_share(window(player()));
        let info = ben.until_sharing().await;
        assert_eq!(
            (info.kind, info.sound, info.size),
            (ShareKind::Window, ShareAudio::Application, (800, 600))
        );
        let view = cal
            .until("Cal sees two shares", |view| {
                view.call
                    .as_ref()
                    .is_some_and(|call| call.shares.len() == 2)
            })
            .await;
        let call = view.call.unwrap();
        assert_eq!(
            call.shares
                .iter()
                .map(|share| (share.account_id, share.kind))
                .collect::<Vec<_>>(),
            [
                (ann_id, Some(ShareKind::Screen)),
                (ben_id, Some(ShareKind::Window))
            ]
        );
        assert_eq!(call.watched, Some(ShareId::Of(ann_id)));
        cal.until_shown(ShareId::Of(ann_id), RED, BLUE).await;
        let other = cal.hears_afresh(PLAYER.0).await;
        assert!(other < ABSENT_DBFS, "{other}");
        // The sound of the share of Ann goes on. One half second can hold a gap while the
        // processor is busy, so the test waits for a half second that holds the tone.
        let level = cal.until_heard_tone(SYSTEM.0).await;
        assert!(level > SHARE_HEARD_DBFS, "{level}");
        // Each sharer watches the share of the other one, and hears it. The client of Ann
        // learns of the share of Ben by itself: nothing orders that with the view of Cal.
        ann.until("Ann watches the share of Ben", |view| {
            view.call
                .as_ref()
                .is_some_and(|call| call.watched == Some(ShareId::Of(ben_id)))
        })
        .await;
        assert_eq!(
            ann.calls.view().call.unwrap().watched,
            Some(ShareId::Of(ben_id))
        );
        ann.until_heard_tone(PLAYER.0).await;
        ben.until_heard_tone(SYSTEM.0).await;

        // Cal chooses the share of Ben: its picture, and its sound in place of the other one.
        // The pictures of the share of Ben in its full size are counted from the change, in
        // seconds, so that a small layer or a low rate in the first seconds after the change
        // is measured and not waited out.
        let before_switch = crate::statistics::snapshot(&ben, &cal, share_started).await;
        let mut pictures = cal.calls.main_picture();
        pictures.borrow_and_update();
        cal.calls.watch_share(ShareId::Of(ben_id));
        let counted = tokio::spawn(async move {
            let started = tokio::time::Instant::now();
            let mut seconds: Vec<usize> = Vec::new();
            while !full_rate(&seconds) && started.elapsed() < PATIENCE {
                let end = started + Duration::from_secs(seconds.len() as u64 + 1);
                let mut arrived = 0;
                loop {
                    match tokio::time::timeout_at(end, pictures.changed()).await {
                        Ok(Ok(())) => {
                            let full = pictures.borrow_and_update().as_ref().is_some_and(|shown| {
                                shown.share == ShareId::Of(ben_id)
                                    && shown.frame.source == (800, 600)
                            });
                            arrived += usize::from(full);
                        }
                        Ok(Err(_)) => {
                            seconds.push(arrived);
                            return seconds;
                        }
                        Err(_) => break,
                    }
                }
                seconds.push(arrived);
            }
            seconds
        });
        cal.until("Cal watches the share of Ben", |view| {
            view.call
                .as_ref()
                .is_some_and(|call| call.watched == Some(ShareId::Of(ben_id)))
        })
        .await;
        let picture = cal.until_shown(ShareId::Of(ben_id), GREEN, YELLOW).await;
        assert_eq!(picture.frame.source.0 * 3, picture.frame.source.1 * 4);
        // The share is sent at the frame rate that its sharer chose, 30 pictures per second.
        // The defaults of the call library send a picture of 800 pixels at 5 per second: in
        // three seconds 15 pictures arrive with them. A share has one encoding of the full
        // size (ADR 0773), and a sender lowers its cadence while its estimate of the bandwidth
        // is low, as in the first seconds of a stream on a busy machine. The time until three
        // seconds in a row hold 66 pictures of the full size is printed in every run and has
        // a bound (ADR 0737).
        let seconds = counted.await.unwrap();
        let delay = seconds.len().saturating_sub(3);
        eprintln!(
            "pictures of the full size in each second after the change of the watched share: \
         {seconds:?}; the full rate began after {delay} s"
        );
        if !full_rate(&seconds) || delay > FULL_RATE_WITHIN_SECONDS {
            let after = crate::statistics::snapshot(&ben, &cal, share_started).await;
            crate::statistics::frame_rate(
                &cal,
                seconds.iter().rev().take(3).sum(),
                &before_switch,
                &after,
            );
            panic!(
                "the full frame rate began {delay} s after the change of the watched share, \
             later than {FULL_RATE_WITHIN_SECONDS} s: {seconds:?}"
            );
        }
        cal.until_heard_tone(PLAYER.0).await;
        cal.until_silent_tone(SYSTEM.0).await;

        // The sound of the share off, and the picture stays.
        cal.calls.set_share_volume(0.0);
        cal.until_silent_tone(PLAYER.0).await;
        ben.capture.set_colours(BLUE, RED);
        cal.until_shown(ShareId::Of(ben_id), BLUE, RED).await;
        cal.calls.set_share_volume(1.0);
        cal.until_heard_tone(PLAYER.0).await;
        let full = cal.until_steady_tone(PLAYER.0).await;
        cal.calls.set_share_volume(0.5);
        cal.until_level(PLAYER.0, "half the volume of the share", |level| {
            (full - level - 6.0).abs() < 1.5
        })
        .await;
        cal.calls.set_share_volume(1.0);

        // Full-resolution publication preserves text even when a share is drawn small.
        cal.until_width(ann_id, 1280).await;
        cal.until_width(ben_id, 800).await;
        cal.calls.set_view_size(320, 240);
        cal.until_width(ben_id, 800).await;
        cal.calls.set_view_size(1280, 800);
        cal.until_width(ben_id, 800).await;

        // Ann chooses her own share: its preview, and no sound of the share of Ben.
        ann.calls.watch_share(ShareId::Own);
        ann.until_shown(ShareId::Own, RED, BLUE).await;
        ann.until_silent_tone(PLAYER.0).await;

        // Ben leaves the call. His share ends for everyone, and Cal watches the share that is
        // left, with its sound.
        ben.calls.leave().await;
        assert_eq!(ben.capture.running(), 0);
        cal.until("the share of Ben has ended", |view| {
            view.call.as_ref().is_some_and(|call| {
                call.shares.len() == 1 && call.watched == Some(ShareId::Of(ann_id))
            })
        })
        .await;
        cal.until_shown(ShareId::Of(ann_id), RED, BLUE).await;
        cal.until_heard_tone(SYSTEM.0).await;
        cal.until_silent_tone(PLAYER.0).await;
        cal.until_width(ann_id, 1280).await;
        model_when(&cal_at, "Ben has left with his share", |model| {
            model.call_at(&place).is_some_and(|call| {
                call.participant(ben_id).is_none()
                    && call
                        .participant(ann_id)
                        .is_some_and(|ann| ann.screen_share.is_some())
            })
        })
        .await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
async fn the_last_size_decides_drawing_bounds_while_the_share_keeps_full_resolution() {
    limited(async {
        let _media = media_server_shared().await;
        let (ann, ben) = (Person::new(440.0), Person::new(880.0));
        let (room, ann_at) = Room::new("Acme", &ann, "Ann").await;
        let ben_at = room.member(&ann_at, &ben, "Ben").await;
        let _ = &ben_at;
        let ann_id = account(&ann_at);
        for person in [&ann, &ben] {
            person.enter(room.target).await;
            person.calls.set_muted(false);
        }
        ann.calls.start_share(screen(SoundChoice::None));
        ann.until_sharing().await;
        ben.until_width(ann_id, 1280).await;

        ben.until_heard(&ann).await;
        ann.until_heard(&ben).await;
        // Drawing size follows the last request; the received stream remains full size.
        const SMALL: (u32, u32) = (320, 240);
        const LARGE: (u32, u32) = (1280, 800);
        for _ in 0..6 {
            for (sizes, drawn) in [([SMALL, LARGE, SMALL], 320), ([LARGE, SMALL, LARGE], 1280)] {
                for (width, height) in sizes {
                    ben.calls.set_view_size(width, height);
                }
                ben.until_width_stays(ann_id, 1280).await;
                let mut pictures = ben.calls.main_picture();
                tokio::time::timeout(
                    PATIENCE,
                    pictures.wait_for(|picture| {
                        picture
                            .as_ref()
                            .is_some_and(|picture| picture.frame.width == drawn)
                    }),
                )
                .await
                .unwrap()
                .unwrap();
            }
        }

        // A change of the volume of the share and a new size at the same moment: the volume
        // asks the media server for nothing, so the new size is the last request.
        for (size, drawn) in [(SMALL, 320), (LARGE, 1280), (SMALL, 320)] {
            ben.calls.set_share_volume(0.5);
            ben.calls.set_share_volume(1.0);
            ben.calls.set_view_size(size.0, size.1);
            ben.until_width_stays(ann_id, 1280).await;
            let mut pictures = ben.calls.main_picture();
            tokio::time::timeout(
                PATIENCE,
                pictures.wait_for(|picture| {
                    picture
                        .as_ref()
                        .is_some_and(|picture| picture.frame.width == drawn)
                }),
            )
            .await
            .unwrap()
            .unwrap();
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
async fn a_share_that_ends_by_itself_is_told_and_a_still_screen_reaches_a_late_viewer() {
    limited(async {
        let _media = media_server_shared().await;
        let (ann, ben) = (Person::new(440.0), Person::new(880.0));
        let (room, ann_at) = Room::new("Acme", &ann, "Ann").await;
        let ben_at = room.member(&ann_at, &ben, "Ben").await;
        let (ann_id, place) = (account(&ann_at), room.target.place);
        ann.enter(room.target).await;

        // A capture that the system refuses starts no share, and the person is told why.
        ann.capture.fail_next(CaptureError::NotAllowed);
        ann.calls.start_share(screen(SoundChoice::None));
        ann.until("the share did not start", |view| {
            view.call
                .as_ref()
                .is_some_and(|call| call.share == ShareState::Ended(CaptureError::NotAllowed))
        })
        .await;
        assert_eq!(ann.capture.running(), 0);
        // The statement is put away.
        ann.calls.stop_share();
        ann.until("the statement is put away", |view| {
            view.call
                .as_ref()
                .is_some_and(|call| call.share == ShareState::Off)
        })
        .await;

        // A share that the person stops while the system still prepares its capture never
        // captures anything.
        ann.capture.set_start_delay(Duration::from_millis(700));
        ann.calls.start_share(screen(SoundChoice::System));
        ann.until("the share starts", |view| {
            view.call
                .as_ref()
                .is_some_and(|call| call.share == ShareState::Starting)
        })
        .await;
        ann.calls.stop_share();
        ann.until("the share is off", |view| {
            view.call
                .as_ref()
                .is_some_and(|call| call.share == ShareState::Off)
        })
        .await;
        tokio::time::sleep(Duration::from_millis(1_500)).await;
        assert_eq!(
            ann.capture.started(),
            [],
            "a capture started after the stop"
        );
        assert_eq!(ann.capture.running(), 0);
        ann.capture.set_start_delay(Duration::ZERO);

        // A screen that does not change gives one picture. A person who enters the call
        // later still gets it, and no sound: this share has none.
        ann.capture.set_still(true);
        ann.calls.start_share(screen(SoundChoice::None));
        let info = ann.until_sharing().await;
        assert_eq!((info.sound, info.sound_held), (ShareAudio::None, false));
        tokio::time::sleep(Duration::from_secs(3)).await;
        ben.enter(room.target).await;
        ben.until_shown(ShareId::Of(ann_id), RED, BLUE).await;
        let none = ben.hears_afresh(SYSTEM.0).await;
        assert!(none < ABSENT_DBFS, "{none}");
        model_when(&ben_at, "the share of Ann without sound", |model| {
            model.call_at(&place).is_some_and(|call| {
                call.participant(ann_id).is_some_and(|ann| {
                    ann.screen_share
                        .as_ref()
                        .is_some_and(|share| share.audio == ShareAudio::None)
                })
            })
        })
        .await;

        // The shared window is closed: the share ends, both sides see it, and the server is
        // told.
        ann.capture.end_all(&CaptureError::SourceGone);
        ann.until("the share ended by itself", |view| {
            view.call
                .as_ref()
                .is_some_and(|call| call.share == ShareState::Ended(CaptureError::SourceGone))
        })
        .await;
        ben.until("the share of Ann has ended", |view| {
            view.call
                .as_ref()
                .is_some_and(|call| call.shares.is_empty())
        })
        .await;
        model_when(&ben_at, "no share of Ann", |model| {
            model.call_at(&place).is_some_and(|call| {
                call.participant(ann_id)
                    .is_some_and(|ann| ann.screen_share.is_none())
            })
        })
        .await;

        // A new share replaces the statement, and a second start replaces the first share.
        ann.capture.set_still(false);
        ann.calls.start_share(screen(SoundChoice::None));
        ann.until_sharing().await;
        ann.calls.start_share(window(SoundChoice::None));
        ann.until("the window is shared", |view| {
            view.call.as_ref().is_some_and(|call| {
            matches!(&call.share, ShareState::Running(info) if info.kind == ShareKind::Window)
        })
        })
        .await;
        assert_eq!(ann.capture.running(), 1);
        ben.until("Ben sees the window", |view| {
            view.call.as_ref().is_some_and(|call| {
                call.shares.len() == 1 && call.shares[0].kind == Some(ShareKind::Window)
            })
        })
        .await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
async fn the_blocks_of_the_server_hold_for_a_share_and_from_the_first_moment_of_a_new_place() {
    limited(async {
        let _media = media_server_shared().await;
        let (ann, ben) = (Person::new(440.0), Person::new(880.0));
        let (room, ann_at) = Room::new("Acme", &ann, "Ann").await;
        let ben_at = room.member(&ann_at, &ben, "Ben").await;
        let (ann_id, ben_id) = (account(&ann_at), account(&ben_at));
        ann.enter(room.target).await;
        ben.enter(room.target).await;
        ann.until_heard(&ben).await;

        // A microphone block closes every sound source of the person (ADR 0510): the share of
        // Ben runs with its picture and without its sound, and Ben is told so.
        ann_at
            .api()
            .block_microphone(room.voice, ben_id)
            .await
            .unwrap();
        ben.until("the microphone of Ben is blocked", |view| {
            view.call.as_ref().is_some_and(|call| {
                call.microphone_block
                    .as_ref()
                    .is_some_and(|block| block.reason == BlockReason::Moderator)
            })
        })
        .await;
        ben.calls.start_share(screen(SoundChoice::System));
        ben.until("the share runs and its sound is held", |view| {
            view.call.as_ref().is_some_and(
                |call| matches!(&call.share, ShareState::Running(info) if info.sound_held),
            )
        })
        .await;
        ann.until_shown(ShareId::Of(ben_id), RED, BLUE).await;
        let held = ann.hears_afresh(SYSTEM.0).await;
        assert!(held < ABSENT_DBFS, "{held}");

        // Without the block the sound of the share is sent.
        ann_at
            .api()
            .unblock_microphone(room.voice, ben_id)
            .await
            .unwrap();
        ben.until("the sound of the share is sent", |view| {
            view.call.as_ref().is_some_and(
                |call| matches!(&call.share, ShareState::Running(info) if !info.sound_held),
            )
        })
        .await;
        ann.until_heard_tone(SYSTEM.0).await;
        ben.calls.stop_share();

        // Ben gets both blocks, leaves and enters again. The blocks belong to the call, and
        // the answer of the server to the join states them: the first view of the new place
        // holds them, before the state of the server lists Ben, so no microphone is opened
        // and nothing is played at any moment.
        ann_at
            .api()
            .block_microphone(room.voice, ben_id)
            .await
            .unwrap();
        ann_at.api().block_sound(room.voice, ben_id).await.unwrap();
        ben.until("Ben has both blocks", |view| {
            view.call
                .as_ref()
                .is_some_and(|call| call.microphone_block.is_some() && call.sound_block.is_some())
        })
        .await;
        ben.calls.leave().await;
        ben.host.clear_played();
        let cleared = tokio::time::Instant::now();
        let mut views = ben.calls.watch();
        let joining = ben.calls.join(room.target);
        let first = tokio::time::timeout(PATIENCE, views.wait_for(|view| view.call.is_some()))
            .await
            .unwrap()
            .unwrap()
            .call
            .clone()
            .unwrap();
        joining.await.unwrap();
        assert_eq!(first.sound_block.map(|block| block.by), Some(ann_id));
        assert_eq!(
            first.microphone_block.map(|block| block.reason),
            Some(BlockReason::Moderator)
        );
        let view = ben
            .until("Ben is connected again", |view| {
                view.call
                    .as_ref()
                    .is_some_and(|call| call.phase == junto_media::calls::Phase::Connected)
            })
            .await;
        assert_eq!(view.call.unwrap().microphone, MicrophoneState::Closed);
        // Two seconds of the call: the tone of Ann is in none of the sound that was played
        // since the join, from its first sample.
        tokio::time::sleep(Duration::from_secs(2)).await;
        let (played, rate) = ben.host.played_since_clear();
        assert!(played.len() > rate as usize, "{}", played.len());
        // The sink keeps the last 12 seconds: everything since the join is examined.
        assert!(cleared.elapsed() < Duration::from_secs(10));
        for part in played.chunks(rate as usize / 4) {
            let heard = junto_media::audio::synthetic::tone_level(part, rate, ann.hz);
            assert!(heard < ABSENT_DBFS, "{heard}");
        }
        let silent = ann.hears_afresh(ben.hz).await;
        assert!(silent < ABSENT_DBFS, "{silent}");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
async fn a_share_goes_on_when_the_call_library_connects_anew_and_publishes_its_tracks_again() {
    limited(async {
        let _media = media_server_shared().await;
        let (ann, ben) = (Person::new(440.0), Person::new(880.0));
        let (room, ann_at) = Room::new("Acme", &ann, "Ann").await;
        let ben_at = room.member(&ann_at, &ben, "Ben").await;
        let _ = &ben_at;
        let ann_id = account(&ann_at);
        ann.enter(room.target).await;
        ben.enter(room.target).await;
        ann.calls.start_share(screen(SoundChoice::System));
        ann.until_sharing().await;
        ben.until_shown(ShareId::Of(ann_id), RED, BLUE).await;
        ben.until_heard_tone(SYSTEM.0).await;
        ben.until_heard(&ann).await;

        // The call library connects anew. It withdraws each track of this device and
        // publishes it again under another identifier. The share is the same share: its
        // capture goes on, and the viewer gets the pictures and the sound after it.
        ann.calls.simulate_full_reconnect();
        ben.until("the picture track of Ann was withdrawn", |view| {
            view.call
                .as_ref()
                .is_some_and(|call| call.shares.is_empty())
        })
        .await;
        ben.until("the picture track of Ann is back", |view| {
            view.call
                .as_ref()
                .is_some_and(|call| call.shares.len() == 1)
        })
        .await;
        ann.until_sharing().await;
        assert_eq!(ann.capture.running(), 1);
        assert_eq!(ann.capture.started().len(), 1);
        ann.capture.set_colours(GREEN, YELLOW);
        ben.until_shown(ShareId::Of(ann_id), GREEN, YELLOW).await;
        ben.host.clear_played();
        ben.until_heard_tone(SYSTEM.0).await;
        // One picture track and one sound track of Ann: no track is published twice.
        tokio::time::sleep(Duration::from_secs(3)).await;
        let view = ben.calls.view().call.unwrap();
        assert_eq!(view.shares.len(), 1);
        assert!(matches!(
            ann.calls.view().call.unwrap().share,
            ShareState::Running(_)
        ));

        // The microphone of Ann is the same microphone after it: her voice arrives, and her
        // mute stops it.
        ben.until_heard_afresh(&ann).await;
        ann.calls.set_muted(true);
        ben.until_silent(&ann).await;
        ann.calls.set_muted(false);
        ben.until_heard_afresh(&ann).await;

        // The person stops the share while the call library connects anew. The tracks that
        // the call library publishes again belong to no share and are withdrawn: the viewer
        // gets no share and no sound of it, also some seconds later.
        ann.calls.simulate_full_reconnect();
        ben.until("the picture track of Ann was withdrawn", |view| {
            view.call
                .as_ref()
                .is_some_and(|call| call.shares.is_empty())
        })
        .await;
        ann.calls.stop_share();
        ann.until("the share is off", |view| {
            view.call
                .as_ref()
                .is_some_and(|call| call.share == ShareState::Off)
        })
        .await;
        assert_eq!(ann.capture.running(), 0);
        ben.until_heard_afresh(&ann).await;
        for _ in 0..12 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            assert_eq!(ben.calls.view().call.unwrap().shares, []);
        }
        let orphan = ben.hears_afresh(SYSTEM.0).await;
        assert!(orphan < ABSENT_DBFS, "{orphan}");
    })
    .await;
}

#[tokio::test]
#[ignore = "needs PostgreSQL and LiveKit"]
async fn the_local_admission_gate_withdraws_share_sound_with_a_permissive_token() {
    limited(async {
        use junto_media::engine::{Controls, Engine, EngineOptions, MediaJoin, Watching};
        use std::sync::Arc;
        let _media = media_server_shared().await;
        let one = Person::new(440.0);
        let two = Person::new(880.0);
        let (room, owner) = Room::new("Share admission", &one, "One").await;
        let peer = room.member(&owner, &two, "Two").await;
        one.enter(room.target).await;
        let answer = peer.api().join_channel_call(room.voice).await.unwrap();
        let engine = Engine::start(
            &tokio::runtime::Handle::current(),
            EngineOptions {
                camera: Arc::new(two.camera.clone()),
                host: Arc::new(two.host.clone()),
                permission: Arc::new(two.permission.clone()),
                capture: Arc::new(two.capture.clone()),
            },
            MediaJoin {
                url: answer.media_url,
                token: answer.token,
            },
            crate::scene::plain_voice(),
            Controls::default(),
            Default::default(),
            Watching::default(),
        );
        let mut state = engine.state();
        tokio::time::timeout(
            PATIENCE,
            state.wait_for(|state| state.connection == junto_media::engine::Connection::Connected),
        )
        .await
        .unwrap()
        .unwrap();
        engine.start_share(screen(SoundChoice::System));
        let id = ShareId::Of(account(&peer));
        one.calls.watch_share(id);
        one.until_shown(id, RED, BLUE).await;
        one.until_heard_tone(SYSTEM.0).await;
        engine.set_controls(Controls {
            microphone_blocked: true,
            ..Controls::default()
        });
        one.until_silent_tone(SYSTEM.0).await;
        two.capture.set_colours(GREEN, YELLOW);
        one.until_shown(id, GREEN, YELLOW).await;
        engine.set_controls(Controls::default());
        one.until_heard_tone(SYSTEM.0).await;
        engine.close().await;
        peer.api().leave_call_as(&answer.identity).await.unwrap();
        one.calls.leave().await;
    })
    .await;
}

/// On macOS the own share and the own camera are encoded in H.264 by the encoder of the
/// system hardware, and the other person decodes both (ADR 0737).
#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
async fn on_macos_the_own_share_and_camera_are_encoded_by_the_hardware_of_the_system() {
    use livekit::webrtc::stats::RtcStats;

    limited(async {
        let _media = media_server_shared().await;
        let (ann, ben) = (Person::new(440.0), Person::new(880.0));
        let (room, ann_at) = Room::new("Acme", &ann, "Ann").await;
        let _ben_at = room.member(&ann_at, &ben, "Ben").await;
        let ann_id = account(&ann_at);
        ann.enter(room.target).await;
        ben.enter(room.target).await;
        ann.calls.start_share(screen(SoundChoice::None));
        ann.until_sharing().await;
        ann.calls.start_camera(None);
        ben.until_shown(ShareId::Of(ann_id), RED, BLUE).await;
        let mut cameras = ben.calls.camera_pictures();
        let camera = tokio::time::timeout(
            PATIENCE,
            cameras.wait_for(|pictures| pictures.contains_key(&ShareId::Of(ann_id))),
        )
        .await;
        assert!(camera.is_ok(), "no camera picture of Ann arrived");

        // Each encoding that sends pictures: the share and at least one layer of the camera.
        let started = tokio::time::Instant::now();
        let encoders = loop {
            let stats = ann.calls.statistics().await.unwrap();
            let encoders: Vec<(String, bool)> = (stats.publisher_stats.iter())
                .filter_map(|stat| match stat {
                    RtcStats::OutboundRtp(stat)
                        if stat.stream.kind == "video" && stat.outbound.frames_encoded > 0 =>
                    {
                        Some((
                            stat.outbound.encoder_implementation.clone(),
                            stat.outbound.power_efficient_encoder,
                        ))
                    }
                    _ => None,
                })
                .collect();
            if encoders.len() >= 2 || started.elapsed() > PATIENCE {
                break encoders;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        };
        assert!(encoders.len() >= 2, "{encoders:?}");
        assert!(
            encoders
                .iter()
                .all(|(name, hardware)| name.contains("VideoToolbox") && *hardware),
            "{encoders:?}"
        );
        let codecs: Vec<String> = (ann.calls.statistics().await.unwrap().publisher_stats)
            .iter()
            .filter_map(|stat| match stat {
                RtcStats::Codec(codec) if codec.codec.mime_type.starts_with("video/") => {
                    Some(codec.codec.mime_type.to_lowercase())
                }
                _ => None,
            })
            .collect();
        assert!(
            !codecs.is_empty() && codecs.iter().all(|codec| codec == "video/h264"),
            "{codecs:?}"
        );
    })
    .await;
}
