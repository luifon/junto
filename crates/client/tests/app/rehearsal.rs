//! The flows of the rehearsal (ADR 0902): two windows with their own profiles go through
//! the first use of a deployment. Window A claims the server with the owner code and
//! invites window B; both write, answer in a thread, call, share, record, read the
//! transcript, search, and start again on their profiles.
//!
//! The same steps run in two places. `tests/rehearsal/` runs them in native windows
//! against a deployment that was installed with `docs/install.md`. The test at the end of
//! this file runs the steps that need no recorder against the server of the tests, so that
//! a change of the window that breaks a step fails in the ordinary live tier.
//!
//! Every action of a person is a press of the pointer on a drawn control, given to the
//! window as a pointer event, or keys typed into a drawn field with gpui's simulation of a
//! key (ADR 0902). One setting is made on the application state: the download directory of
//! step 8, so that saved files go into the work directory of the run. The checks read the
//! view state from which the window draws, the drawn bounds of named elements and, for a
//! share, drawn pixels. Nothing is written into the server directly.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gpui::TestAppContext;
use junto_client::state::admin::view::people::{InviteLine, USES};
use junto_client::state::admin::view::{Scope, ServerPage};
use junto_client::state::feed::{FeedRow, MessageRow, plain_text};
use junto_client::state::recorded::{CardDetail, CardState, FileKind, JobShown, LineFrom};
use junto_client::state::search::{ResultTarget, SearchPanel, SearchShown};
use junto_client::state::view::{Occupant, OwnShare, ShareStage};
use junto_client::text;
use junto_client_core::protocol::error::ErrorCode;
use junto_client_core::protocol::ids::{AccountId, ChannelId, MessageId, RecordingId};
use junto_client_core::protocol::structure::ChannelKind;
use junto_client_core::protocol::transcript::{LineSource, TranscriptState};
use junto_client_core::transfers::DownloadStage;
use junto_media::calls::Phase;
use junto_media::share::synthetic::Colour;

use crate::harness::{ABSENT_DBFS, HEARD_DBFS, PATIENCE, WINDOW_TONE_HZ, Window, heard, open};

/// The installed server: its address as a person types it, and its one-time owner code.
pub struct Deployment {
    pub address: String,
    pub owner_code: String,
}

/// Known speech for the microphone of one window, with the text that it says.
pub struct Speech {
    /// Mono samples.
    pub samples: Vec<f32>,
    pub rate: u32,
    pub text: String,
}

/// The tone of the microphone of window B. Window A has the tone of the harness.
pub const B_TONE_HZ: f32 = 880.0;
/// The tone of the system sound of a synthetic screen share, on its left channel.
const SHARE_TONE_HZ: f32 = 700.0;
/// A share tone that arrives is above this.
const SHARE_HEARD_DBFS: f32 = -26.0;
/// The colours of the left and of the right half of a share picture.
type Halves = (Colour, Colour);
/// The two colours of the synthetic share before and after the change of its picture.
const FIRST_COLOURS: Halves = ((200, 30, 30), (30, 30, 200));
const SECOND_COLOURS: Halves = ((30, 190, 40), (220, 200, 30));
/// The largest difference of a drawn colour channel from the colour of the source, after
/// video coding and scaling.
const COLOUR_TOLERANCE: i32 = 40;
/// The time a recording may take from its stop to its files, and its transcript on the
/// processor of the machine with the default model.
const PROCESSING: Duration = Duration::from_secs(5 * 60);
const TRANSCRIBING: Duration = Duration::from_secs(30 * 60);

/// The names of the two people.
const A_NAME: &str = "Ada";
const B_NAME: &str = "Bea";

/// What the steps found, for the later steps and for the report.
#[derive(Default)]
pub struct Found {
    pub message: Option<MessageId>,
    pub reply: Option<MessageId>,
    pub recording: Option<RecordingId>,
    /// The accounts of A and B.
    pub accounts: Option<(AccountId, AccountId)>,
    pub downloads: Vec<(PathBuf, u64, FileKind, Option<String>)>,
}

/// Two windows on one deployment.
pub struct Rehearsal {
    cx: TestAppContext,
    pub a: Window,
    pub b: Option<Window>,
    a_profile: PathBuf,
    b_profile: PathBuf,
    deployment: Deployment,
    /// Native windows are drawn by the GPU, and their pictures are saved and checked.
    native: bool,
    evidence: Option<PathBuf>,
    /// A word of this run, so that every message and search refers to this run.
    tag: String,
    pub found: Found,
}

impl Rehearsal {
    /// Opens window A on an empty profile. Window B opens in the step of the invite.
    pub fn new(
        cx: &mut TestAppContext,
        profiles: &Path,
        deployment: Deployment,
        native: bool,
        evidence: Option<PathBuf>,
        tag: &str,
    ) -> Self {
        let (a_profile, b_profile) = (profiles.join("a"), profiles.join("b"));
        for profile in [&a_profile, &b_profile] {
            assert!(
                !profile.exists(),
                "{} must be a new profile",
                profile.display()
            );
        }
        std::fs::create_dir_all(&a_profile).unwrap();
        let a = open(cx, &a_profile);
        Self {
            cx: cx.clone(),
            a,
            b: None,
            a_profile,
            b_profile,
            deployment,
            native,
            evidence,
            tag: tag.to_owned(),
            found: Found::default(),
        }
    }

    /// What each window shows of its server and its dialogs, for the report of a failed
    /// step.
    pub fn describe(&mut self) -> String {
        let mut told = Vec::new();
        for (side, window) in [("A", Some(&mut self.a)), ("B", self.b.as_mut())] {
            let Some(window) = window else { continue };
            let view = window.view();
            let error = window.in_shell(|shell, _, _| shell.join_error().map(str::to_owned));
            told.push(format!(
                "{side}: servers {:?}; selected {:?}; join dialog {}; join error {error:?}",
                view.servers
                    .iter()
                    .map(|server| (&server.name, &server.status))
                    .collect::<Vec<_>>(),
                view.selected.as_ref().map(|server| &server.status),
                window.shows("join-dialog"),
            ));
        }
        told.join("\n")
    }

    pub fn b(&mut self) -> &mut Window {
        self.b.as_mut().expect("window B is open")
    }

    /// Saves what both windows drew, as the GPU draws it, under the name of a step.
    pub fn pictures(&mut self, step: &str) {
        let Some(evidence) = self.evidence.clone() else {
            return;
        };
        for (side, window) in [("a", Some(&mut self.a)), ("b", self.b.as_mut())] {
            let Some(window) = window else { continue };
            window.settle();
            let picture = window
                .cx
                .update(|window, _| window.render_to_image())
                .expect("a native window draws into a picture");
            picture
                .save(evidence.join(format!("{step}-{side}.png")))
                .unwrap();
        }
    }

    /// Step 1: window A starts on an empty profile, adds the server and claims it with the
    /// owner code.
    pub fn first_owner(&mut self) {
        let address = self.deployment.address.clone();
        let code = self.deployment.owner_code.clone();
        let a = &mut self.a;
        start_with_name(a, A_NAME);
        add_server(a, &address);
        type_into(a, "code-field", &code);
        a.click("join-with-code");
        a.until_ready();
        assert!(!a.shows("join-dialog"));
        let view = a.view();
        assert_eq!(view.servers.len(), 1);
        assert_eq!(view.person.name, A_NAME);
        let kinds: Vec<ChannelKind> = a.channels().iter().map(|row| row.kind).collect();
        assert_eq!(kinds, [ChannelKind::Text, ChannelKind::Voice]);
        // The owner has the administration in the menu of the server.
        a.click("server-menu-button");
        assert!(a.shows("menu-administration"));
        a.press("escape");
        assert!(a.shows("feed") && a.shows("composer"));
    }

    /// Step 2: window A makes an invite for one use in the administration; window B starts
    /// on its own profile, is refused with the used owner code, and joins with the invite.
    pub fn invitation(&mut self) {
        let a = &mut self.a;
        a.click("server-menu-button");
        a.click("menu-administration");
        assert!(a.shows("admin"));
        a.click("admin-nav-invites");
        a.until("the page of the invites is read", |window| {
            invites(window).is_some_and(|page| page.invites.loaded().is_some())
        });
        a.click("invite-new");
        a.until("the form of a new invite is open", |window| {
            window.shows("invite-form")
        });
        let one = USES.iter().position(|uses| *uses == 1).unwrap();
        a.click(crate::harness::selector(format!("invite-uses-{one}")));
        a.click("invite-create");
        let made = a.until_value("the code of the new invite is shown", |window| {
            invites(window)?
                .made
                .filter(|_| window.shows("invite-code"))
        });
        let code = made.code.expose().to_owned();
        a.click("invite-code-done");
        a.until("the code is put away", |window| {
            !window.shows("invite-code")
        });
        a.click("admin-close");
        assert!(!a.shows("admin"));
        self.pictures("2-invite-made");

        std::fs::create_dir_all(&self.b_profile).unwrap();
        self.b = Some(open(&mut self.cx, &self.b_profile));
        let (address, owner_code) = (
            self.deployment.address.clone(),
            self.deployment.owner_code.clone(),
        );
        let b = self.b();
        start_with_name(b, B_NAME);
        add_server(b, &address);
        // The owner code was used: it does not make a second owner.
        type_into(b, "code-field", &owner_code);
        b.click("join-with-code");
        b.until("the used owner code is refused", |window| {
            window.in_shell(|shell, _, _| shell.join_error().map(str::to_owned))
                == Some(text::refusal(ErrorCode::OwnerAlreadyClaimed))
        });
        replace_in(b, "code-field", &code);
        b.click("join-with-code");
        b.until_ready();
        assert_eq!(b.view().person.name, B_NAME);
        let b_account = own_account(b);
        let a_account = own_account(&mut self.a);
        assert_ne!(a_account, b_account);
        self.found.accounts = Some((a_account, b_account));

        // Each window lists the other person.
        for (window, other) in [(&mut self.a, B_NAME), (self.b.as_mut().unwrap(), A_NAME)] {
            window.click("members-button");
            window.until(&format!("the members panel lists {other}"), |window| {
                window.shows_member(other)
            });
            window.click("members-button");
        }

        // The row of the invite counts its one use.
        let a = &mut self.a;
        a.click("server-menu-button");
        a.click("menu-administration");
        a.click("admin-nav-invites");
        let row = a.until_value("the invite counts its use", |window| {
            invite_line(window, made.invite_id).filter(|row| row.uses == 1)
        });
        assert_eq!(row.max_uses, 1);
        a.click("admin-close");
        self.pictures("2-joined");
    }

    /// Step 3: messages. A writes, B reads and answers, A changes its message and deletes
    /// another one; B starts again on its profile and reads the same history.
    pub fn text(&mut self) {
        let tag = self.tag.clone();
        let first = format!("Planning note kestrel{tag}");
        let changed = format!("Planning note kestrel{tag} revised");
        let answer = format!("Answer from Bea heron{tag}");
        let passing = format!("Passing note {tag}");
        for window in [&mut self.a, self.b.as_mut().unwrap()] {
            window.until("the messages of the text channel are read", |window| {
                window
                    .view()
                    .lists()
                    .and_then(|lists| lists.feed.as_ref())
                    .is_some_and(|feed| feed.loaded)
            });
        }
        write(&mut self.a, "composer-input", &first);
        let sent = until_message(&mut self.a, &first);
        assert!(sent.own && sent.can_edit && sent.can_delete);
        let read = until_message(self.b(), &first);
        assert_eq!(read.author, A_NAME);
        assert_eq!(read.message.id, sent.message.id);
        assert!(!read.own);
        self.found.message = Some(sent.message.id);

        write(self.b(), "composer-input", &answer);
        let answered = until_message(&mut self.a, &answer);
        assert_eq!(answered.author, B_NAME);
        self.found.reply = Some(answered.message.id);

        // A changes its message through the menu of the message.
        let a = &mut self.a;
        a.right_click_named(format!("message:{}", sent.message.id));
        assert!(a.shows("message-menu"));
        a.click("message-edit");
        assert_eq!(
            a.in_shell(|shell, _, cx| shell.composer_text(junto_client::shell::Pane::Main, cx)),
            first
        );
        replace_in(a, "composer-input", &changed);
        a.press("enter");
        let edited = until_message(a, &changed);
        assert_eq!(edited.message.id, sent.message.id);
        assert!(edited.message.edited_at.is_some());
        let b = self.b();
        let seen = until_message(b, &changed);
        assert!(seen.message.edited_at.is_some());

        // A writes a passing note and deletes it; B sees it come and go.
        let a = &mut self.a;
        write(a, "composer-input", &passing);
        let note = until_message(a, &passing);
        until_message(self.b(), &passing);
        let a = &mut self.a;
        a.right_click_named(format!("message:{}", note.message.id));
        a.click("message-delete");
        assert!(a.shows("confirm-delete-message"));
        a.click("delete-message");
        for window in [&mut self.a, self.b.as_mut().unwrap()] {
            window.until("the deleted note is gone", |window| {
                waiting(window) == 0
                    && messages(window)
                        .iter()
                        .all(|row| row.message.id != note.message.id)
            });
        }
        let order = [changed.clone(), answer.clone()];
        assert_eq!(texts(&mut self.a), order);
        assert_eq!(texts(self.b()), order);
        self.pictures("3-messages");

        // B starts again on its profile: it logs in without a question and reads the
        // same history.
        self.restart_b();
        let b = self.b();
        b.until("the history after the restart", |window| {
            texts(window) == order
        });
    }

    /// Ends window B and starts it again on its profile.
    fn restart_b(&mut self) {
        self.b.take().unwrap().close();
        let mut b = open(&mut self.cx, &self.b_profile);
        assert!(!b.shows("welcome-dialog"));
        b.until_ready();
        self.b = Some(b);
    }

    /// Step 4: B answers A's message in its thread, and A answers there too.
    pub fn thread(&mut self) {
        let root = self.found.message.unwrap();
        let tag = self.tag.clone();
        let (one, two) = (
            format!("Thread answer one {tag}"),
            format!("Thread answer two {tag}"),
        );
        open_thread(self.b(), root);
        write(self.b(), "thread-composer-input", &one);
        self.b()
            .until("B draws its answer in the thread", |window| {
                replies(window) == [one.clone()]
            });
        open_thread(&mut self.a, root);
        self.a.until("A reads B's answer in the thread", |window| {
            replies(window) == [one.clone()]
        });
        write(&mut self.a, "thread-composer-input", &two);
        let both = [one.clone(), two.clone()];
        for window in [&mut self.a, self.b.as_mut().unwrap()] {
            window.until("both answers in the thread", |window| {
                replies(window) == both
            });
            // The answers stay in the thread: the list of the channel has the same two
            // messages, and the root counts two answers.
            window.until("the root counts the answers", |window| {
                messages(window).iter().any(|row| {
                    row.message.id == root
                        && row.thread.as_ref().is_some_and(|mark| mark.replies == 2)
                })
            });
            assert_eq!(texts(window).len(), 2);
        }
        let authors = reply_authors(&mut self.a);
        assert_eq!(authors, [B_NAME, A_NAME]);
        self.pictures("4-thread");
        for window in [&mut self.a, self.b.as_mut().unwrap()] {
            window.click("close-thread");
        }
    }

    /// Step 5: both enter the voice channel. Each hears only the tone of the other; a
    /// closed microphone is not heard, and is heard again when it is opened.
    pub fn call(&mut self) {
        self.b().sound.set_tone("tone", B_TONE_HZ);
        for window in [&mut self.a, self.b.as_mut().unwrap()] {
            turn_processing_off(window);
            window.click_channel("voice");
            assert!(window.shows("join-call"));
            window.click("join-call");
            window.until("the call is connected", |window| {
                window
                    .view()
                    .call
                    .is_some_and(|call| call.phase == Phase::Connected)
            });
        }
        for window in [&mut self.a, self.b.as_mut().unwrap()] {
            window.until("both people are in the call", |window| {
                let people = occupants(window);
                people.len() == 2
                    && people.iter().any(|person| person.is_self)
                    && people.iter().all(|person| {
                        person
                            .name
                            .as_deref()
                            .is_some_and(|name| [A_NAME, B_NAME].contains(&name))
                    })
            });
        }
        self.a.until("A hears B and not itself", |window| {
            heard(&window.sound, B_TONE_HZ) > HEARD_DBFS
                && heard(&window.sound, WINDOW_TONE_HZ) < ABSENT_DBFS
        });
        self.b().until("B hears A and not itself", |window| {
            heard(&window.sound, WINDOW_TONE_HZ) > HEARD_DBFS
                && heard(&window.sound, B_TONE_HZ) < ABSENT_DBFS
        });
        self.a.click("microphone");
        self.b()
            .until("B does not hear A's closed microphone", |window| {
                heard(&window.sound, WINDOW_TONE_HZ) < ABSENT_DBFS
                    && heard(&window.sound, B_TONE_HZ) < ABSENT_DBFS
            });
        self.a.until("A still hears B", |window| {
            heard(&window.sound, B_TONE_HZ) > HEARD_DBFS
        });
        self.a.click("microphone");
        self.b().until("B hears A again", |window| {
            heard(&window.sound, WINDOW_TONE_HZ) > HEARD_DBFS
        });
        self.pictures("5-call-connected");
    }

    /// Step 6: A shares a screen with its sound and B watches it; the picture changes and
    /// the share stops. Then B shares and A watches.
    pub fn share(&mut self) {
        let (a, b) = (&mut self.a, self.b.as_mut().unwrap());
        share_and_watch(a, b, self.native, true);
        if self.native {
            self.pictures("6-share-a");
        }
        let (a, b) = (&mut self.a, self.b.as_mut().unwrap());
        stop_share(a, b);
        share_and_watch(b, a, self.native, false);
        stop_share(b, a);
    }

    /// Both windows share the synthetic screen and turn on the synthetic camera, and each
    /// watches the share of the other. The windows then draw every kind of picture of a
    /// call: the watched share, the small pictures of both shares, and both cameras. The
    /// frame test of the windows uses this call (ADR 0774).
    pub fn video(&mut self) {
        let (a, b) = (&mut self.a, self.b.as_mut().unwrap());
        start_share(a, false);
        start_share(b, false);
        let people = [&mut self.a, self.b.as_mut().unwrap()].map(|window| {
            window
                .view()
                .person
                .account_id
                .expect("the window knows its account")
        });
        for (window, other) in [
            (&mut self.a, people[1]),
            (self.b.as_mut().unwrap(), people[0]),
        ] {
            window.click("call-camera");
            window.state.update(&mut window.cx, |state, _| {
                state.watch_share(junto_media::engine::ShareId::Of(other));
            });
        }
        for (window, other) in [
            (&mut self.a, people[1]),
            (self.b.as_mut().unwrap(), people[0]),
        ] {
            window.until(
                "the window watches the other share and both cameras run",
                |window| {
                    let cameras = window.state.read_with(&window.cx, |state, _| {
                        let calls = state.calls();
                        let running = calls.view().call.is_some_and(|call| {
                            call.camera == junto_media::camera::CameraState::Running
                        });
                        running && calls.camera_pictures().borrow().len() == 2
                    });
                    cameras
                        && window.shows("share-picture")
                        && stage(window).is_some_and(|stage| {
                            stage.shares.len() == 2
                                && stage.watched().is_some_and(|share| {
                                    share.account_id == other && share.size.is_some()
                                })
                        })
                },
            );
        }
    }

    /// Step 7: A records the call. Both share a screen; A and then B speak known
    /// sentences, with silence between them; A stops the recording and both leave.
    pub fn recording(&mut self, a_speech: &Speech, b_speech: &Speech) {
        // The speech of the rehearsal is in one language, and each sentence is detected
        // (ADR 0616).
        self.a.click("start-recording");
        self.a.click("record-language:auto");
        for window in [&mut self.a, self.b.as_mut().unwrap()] {
            window.until("the call shows that it is recorded", |window| {
                window.shows("recording-indicator")
            });
        }
        let (a, b) = (&mut self.a, self.b.as_mut().unwrap());
        start_share(a, false);
        start_share(b, false);
        for window in [&mut self.a, self.b.as_mut().unwrap()] {
            window.until("the window watches a share", |window| {
                stage(window).is_some_and(|stage| stage.shares.len() == 2)
            });
            window.sound.hush("tone", true);
        }
        self.pictures("7-recording-runs");
        let quiet = Duration::from_secs(2);
        pause(&mut self.a, quiet);
        speak(&mut self.a, a_speech);
        pause(&mut self.a, quiet);
        speak(self.b.as_mut().unwrap(), b_speech);
        pause(&mut self.a, quiet);
        self.a.click("stop-recording");
        for window in [&mut self.a, self.b.as_mut().unwrap()] {
            window.until("the recording has stopped", |window| {
                !window.shows("recording-indicator")
            });
        }
        for window in [&mut self.a, self.b.as_mut().unwrap()] {
            window.click("call-share");
            window.click("leave-call");
            window.until("the window left the call", |window| {
                window.view().call.is_none() && !window.shows("call-strip")
            });
            window.sound.hush("tone", false);
        }
    }

    /// Step 8: both windows show the card of the recording; B saves every file through
    /// the card. The sizes of the saved files are the sizes that the server states.
    pub fn files(&mut self, downloads: &Path) -> CardDetail {
        for window in [&mut self.a, self.b.as_mut().unwrap()] {
            window.click_channel("voice");
        }
        let recording = until_within(
            &mut self.a,
            "the card of the recording is ready",
            PROCESSING,
            |window| recording_card(window).filter(|(_, detail)| detail.job == JobShown::Ready),
        )
        .0;
        self.found.recording = Some(recording);
        let b = self.b.as_mut().unwrap();
        let detail = until_within(b, "B shows the same card", PROCESSING, |window| {
            recording_card(window)
                .filter(|(id, detail)| *id == recording && detail.job == JobShown::Ready)
                .map(|(_, detail)| detail)
        });
        let kinds: Vec<(FileKind, Option<&str>)> = detail
            .files
            .iter()
            .map(|file| (file.kind, file.person.as_deref()))
            .collect();
        assert!(kinds.contains(&(FileKind::Video, None)), "{kinds:?}");
        assert!(
            kinds.contains(&(FileKind::Microphone, Some(A_NAME))),
            "{kinds:?}"
        );
        assert!(
            kinds.contains(&(FileKind::Microphone, Some(B_NAME))),
            "{kinds:?}"
        );
        assert!(
            detail.files.iter().all(|file| file.problem.is_none()),
            "{:?}",
            detail.files
        );
        assert!(detail.picture.is_some());

        std::fs::create_dir_all(downloads).unwrap();
        let target = downloads.to_path_buf();
        b.state
            .update(&mut b.cx, |state, _| state.set_download_directory(target));
        for (position, file) in detail.files.iter().enumerate() {
            b.click("download-recording");
            assert!(b.shows("recording-files"));
            b.click(crate::harness::selector(format!(
                "recorded-file-{position}"
            )));
            let segment = file.segment_id;
            let saved = until_within(b, "the file is saved", PROCESSING, |window| {
                let detail = recording_card(window)?.1;
                let progress = detail
                    .files
                    .iter()
                    .find(|held| held.segment_id == segment)?
                    .download
                    .clone()?;
                match progress.stage {
                    DownloadStage::Saved(path) => Some(path),
                    DownloadStage::Failed(failure) => panic!("the download failed: {failure:?}"),
                    _ => None,
                }
            });
            assert!(saved.starts_with(downloads));
            #[allow(clippy::disallowed_methods)]
            let saved_length = std::fs::metadata(&saved).unwrap().len();
            assert_eq!(saved_length, file.bytes);
            self.found
                .downloads
                .push((saved, file.bytes, file.kind, file.person.clone()));
        }
        self.pictures("8-card");
        detail
    }

    /// Step 9: the transcript. The worker reaches ready; the panel shows the words of each
    /// person under that person's name, in the order of time; the saved text file has the
    /// lines of the panel. Returns the saved file, and the words that the transcript has
    /// for B.
    pub fn transcript(&mut self, a_speech: &Speech, b_speech: &Speech) -> (PathBuf, String) {
        let recording = self.found.recording.unwrap();
        let b = self.b.as_mut().unwrap();
        let detail = until_within(b, "the transcript is ready", TRANSCRIBING, |window| {
            recording_card(window)
                .map(|(_, detail)| detail)
                .filter(|detail| detail.transcript.state == TranscriptState::Ready)
        });
        assert!(detail.transcript.total > 0);
        b.click_named(format!("show-transcript:{recording}"));
        let lines = until_within(b, "the panel has read every line", PATIENCE, |window| {
            let panel = window.view().workspace()?.transcript.clone()?;
            assert_eq!(panel.recording_id, recording);
            match panel.state {
                junto_client::state::view::TranscriptPanelState::Shown { open, .. }
                    if open.lines.loaded
                        && !open.lines.more
                        && open.lines.lines.len() as u32 == detail.transcript.total =>
                {
                    Some(open.lines.lines.clone())
                }
                _ => None,
            }
        });
        assert!(b.shows("transcript-panel") && b.shows("transcript-lines"));
        let (a_account, b_account) = self.found.accounts.unwrap();
        assert!(
            lines
                .iter()
                .all(|line| line.source == LineSource::Microphone),
            "{lines:?}"
        );
        assert!(
            lines
                .iter()
                .all(|line| line.account_id == a_account || line.account_id == b_account),
            "every line belongs to one of the two people: {lines:?}"
        );
        let said = |account| -> String {
            lines
                .iter()
                .filter(|line| line.account_id == account)
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        };
        let (by_a, by_b) = (said(a_account), said(b_account));
        let (a_recall, b_recall) = (recall(&a_speech.text, &by_a), recall(&b_speech.text, &by_b));
        println!("rehearsal: transcript recall A {a_recall:.3}, B {b_recall:.3}");
        assert!(
            a_recall >= 0.9,
            "A said {:?}; the transcript has {by_a:?}",
            a_speech.text
        );
        assert!(
            b_recall >= 0.9,
            "B said {:?}; the transcript has {by_b:?}",
            b_speech.text
        );
        // A spoke first: every line of A comes before every line of B, and the times rise.
        let last_a = lines
            .iter()
            .rposition(|line| line.account_id == a_account)
            .unwrap();
        let first_b = lines
            .iter()
            .position(|line| line.account_id == b_account)
            .unwrap();
        assert!(last_a < first_b, "{lines:?}");
        assert!(
            lines
                .windows(2)
                .all(|pair| pair[0].start_ms <= pair[1].start_ms),
            "{lines:?}"
        );
        // The card names the speaker of each of its lines.
        for row in &detail.transcript.lines {
            let held = lines.iter().find(|line| line.line == row.line).unwrap();
            let name = if held.account_id == a_account {
                A_NAME
            } else {
                B_NAME
            };
            assert_eq!(
                (row.name.as_str(), &row.source),
                (name, &LineFrom::Microphone)
            );
        }
        self.pictures("9-transcript-panel");

        let b = self.b.as_mut().unwrap();
        b.click("close-transcript");
        b.click("download-recording");
        b.click("recorded-text");
        let saved = until_within(
            b,
            "the text of the transcript is saved",
            PATIENCE,
            |window| match recording_card(window)?.1.text_download?.stage {
                DownloadStage::Saved(path) => Some(path),
                DownloadStage::Failed(failure) => panic!("the download failed: {failure:?}"),
                _ => None,
            },
        );
        let written = std::fs::read_to_string(&saved).unwrap();
        for line in &lines {
            let name = if line.account_id == a_account {
                A_NAME
            } else {
                B_NAME
            };
            assert!(
                written.contains(&format!("{name}: {}", line.text)),
                "{written}"
            );
        }
        (saved, by_b)
    }

    /// Step 10: B searches for A's message and for a word that only the recording has, and
    /// opens each result; a word that nobody wrote or said finds nothing.
    pub fn search(&mut self, spoken_word: Option<&str>) {
        let message = self.found.message.unwrap();
        let tag = self.tag.clone();
        let b = self.b.as_mut().unwrap();
        b.click_channel("voice");
        search_for(b, &format!("kestrel{tag}"));
        let panel = until_searched(b);
        assert_eq!(panel.state, SearchShown::Results);
        let targets: Vec<ResultTarget> = panel.messages().iter().map(|row| row.target).collect();
        assert_eq!(targets, [ResultTarget::Message(message)]);
        b.click_named(format!("search-result:{message}"));
        let text_channel = b.until_value("the result leads to its message", |window| {
            let channel = selected_channel(window)?;
            messages(window)
                .iter()
                .any(|row| row.message.id == message && row.target)
                .then_some(channel)
        });
        assert_ne!(Some(text_channel), voice_channel(b));

        if let Some(word) = spoken_word {
            search_for(b, word);
            let panel = until_searched(b);
            assert_eq!(panel.messages().len(), 0);
            assert!(!panel.transcripts().is_empty(), "{panel:?}");
            assert!(panel.transcripts().iter().all(|line| line.author == B_NAME));
            b.click("search-transcript-0");
            b.until("the result opens its recording", |window| {
                selected_channel(window) == voice_channel(window)
                    && window
                        .view()
                        .workspace()
                        .and_then(|workspace| workspace.transcript.clone())
                        .is_some_and(|panel| Some(panel.recording_id) == self.found.recording)
            });
            self.pictures("10-search-transcript");
        }

        let b = self.b.as_mut().unwrap();
        search_for(b, &format!("zirconium{tag}"));
        assert_eq!(until_searched(b).state, SearchShown::Nothing);
        b.click("close-search");
    }

    /// Step 11: both windows end and start again on their profiles, and show the same
    /// history without a claim or an invite. Then both end.
    pub fn persistence(mut self) {
        let recording = self.found.recording;
        let message = self.found.message.unwrap();
        self.a.close();
        self.b.take().unwrap().close();
        for profile in [&self.a_profile, &self.b_profile] {
            let mut window = open(&mut self.cx, profile);
            assert!(!window.shows("welcome-dialog"));
            window.until_ready();
            window.until("the message is there after the start", |window| {
                messages(window).iter().any(|row| row.message.id == message)
            });
            if let Some(recording) = recording {
                window.click_channel("voice");
                window.until("the card is there after the start", |window| {
                    recording_card(window).is_some_and(|(id, detail)| {
                        id == recording && detail.transcript.state == TranscriptState::Ready
                    })
                });
            }
            assert!(window.view().call.is_none());
            window.close();
        }
    }
}

/// The first start: the person types a name into the welcome dialog and continues.
fn start_with_name(window: &mut Window, name: &str) {
    assert!(window.shows("welcome-dialog"));
    type_into(window, "name-field", name);
    window.click("continue");
    assert!(!window.shows("welcome-dialog"));
    assert_eq!(window.view().person.name, name);
}

/// Types the address of the server into the server book and waits for its dialog.
fn add_server(window: &mut Window, address: &str) {
    type_into(window, "address-field", address);
    window.click("add-server");
    window.until("the join dialog opens", |window| {
        window.shows("join-dialog")
    });
}

/// Presses the pointer on a field and types a text into it, one key at a time, as a
/// keyboard does.
pub fn type_into(window: &mut Window, field: &'static str, text: &str) {
    window.click(field);
    type_keys(window, text);
}

/// Types a text into the field that has the keyboard focus, one key at a time.
fn type_keys(window: &mut Window, text: &str) {
    for character in text.chars() {
        let key = match character {
            ' ' => "space".to_owned(),
            other => other.to_lowercase().to_string(),
        };
        let keystroke = gpui::Keystroke {
            modifiers: gpui::Modifiers {
                shift: character.is_uppercase(),
                ..gpui::Modifiers::default()
            },
            key,
            key_char: Some(character.to_string()),
        };
        window.cx.update(|window, cx| {
            window.dispatch_keystroke(keystroke, cx);
        });
    }
    window.settle();
}

/// Selects the whole text of a field and types another text in its place.
fn replace_in(window: &mut Window, field: &'static str, text: &str) {
    window.click(field);
    window.press("secondary-a");
    type_keys(window, text);
}

/// Types a message into a composer and sends it with Enter.
fn write(window: &mut Window, composer: &'static str, text: &str) {
    type_into(window, composer, text);
    window.press("enter");
}

fn own_account(window: &mut Window) -> AccountId {
    window.view().person.account_id.unwrap()
}

fn invites(window: &mut Window) -> Option<junto_client::state::admin::view::people::InvitesPage> {
    window.run_tasks();
    let admin = window
        .state
        .read_with(&window.cx, |state, _| state.admin_view())?;
    match admin.scope {
        Scope::Server {
            page: ServerPage::Invites(page),
            ..
        } => Some(page),
        _ => None,
    }
}

fn invite_line(
    window: &mut Window,
    invite: junto_client_core::protocol::ids::InviteId,
) -> Option<InviteLine> {
    invites(window)?
        .invites
        .loaded()?
        .iter()
        .find(|row| row.id == invite)
        .cloned()
}

fn feed_rows(window: &mut Window) -> Option<junto_client::state::feed::Feed> {
    window.view().lists()?.feed.clone()
}

fn messages(window: &mut Window) -> Vec<MessageRow> {
    feed_rows(window)
        .map(|feed| {
            feed.rows
                .iter()
                .filter_map(|row| match row {
                    FeedRow::Message(row) => Some(row.clone()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn waiting(window: &mut Window) -> usize {
    feed_rows(window).map_or(0, |feed| {
        feed.rows
            .iter()
            .filter(|row| matches!(row, FeedRow::Waiting(_)))
            .count()
    })
}

fn texts(window: &mut Window) -> Vec<String> {
    let Some(feed) = feed_rows(window) else {
        return Vec::new();
    };
    feed.rows
        .iter()
        .filter_map(|row| match row {
            FeedRow::Message(row) => row
                .message
                .document
                .as_ref()
                .map(|document| plain_text(document, &feed.names, 200)),
            _ => None,
        })
        .collect()
}

fn until_message(window: &mut Window, text: &str) -> MessageRow {
    window.until_value(&format!("the message {text:?} is drawn"), |window| {
        let feed = feed_rows(window)?;
        if waiting(window) > 0 {
            return None;
        }
        messages(window).into_iter().find(|row| {
            row.message
                .document
                .as_ref()
                .is_some_and(|document| plain_text(document, &feed.names, 200) == text)
        })
    })
}

fn thread_rows(window: &mut Window) -> Vec<MessageRow> {
    let view = window.view();
    let Some(feed) = view
        .workspace()
        .and_then(|workspace| workspace.thread.as_ref())
        .and_then(|thread| thread.feed.as_ref())
    else {
        return Vec::new();
    };
    feed.rows
        .iter()
        .filter_map(|row| match row {
            FeedRow::Message(row) => Some(row.clone()),
            _ => None,
        })
        .collect()
}

fn replies(window: &mut Window) -> Vec<String> {
    let names = window
        .view()
        .workspace()
        .and_then(|workspace| workspace.thread.as_ref())
        .and_then(|thread| thread.feed.as_ref())
        .map(|feed| feed.names.clone());
    let Some(names) = names else {
        return Vec::new();
    };
    thread_rows(window)
        .iter()
        .filter_map(|row| {
            row.message
                .document
                .as_ref()
                .map(|document| plain_text(document, &names, 200))
        })
        .collect()
}

fn reply_authors(window: &mut Window) -> Vec<String> {
    thread_rows(window)
        .into_iter()
        .map(|row| row.author)
        .collect()
}

/// Opens the thread of a message through the menu of the message.
fn open_thread(window: &mut Window, root: MessageId) {
    window.right_click_named(format!("message:{root}"));
    window.click("message-thread");
    window.until("the thread is read", |window| {
        window
            .view()
            .workspace()
            .and_then(|workspace| workspace.thread.as_ref())
            .is_some_and(|thread| {
                thread.root.is_some() && thread.feed.as_ref().is_some_and(|feed| feed.loaded)
            })
    });
}

/// A steady tone passes unchanged only without the processing of a voice.
fn turn_processing_off(window: &mut Window) {
    window.click("app-settings");
    for part in ["echo-cancellation", "noise-suppression", "automatic-gain"] {
        window.click(part);
    }
    window.until("the processing is off", |window| {
        let voice = window
            .state
            .read_with(&window.cx, |state, _| state.voice().clone());
        !voice.echo_cancellation && !voice.noise_suppression && !voice.automatic_gain
    });
    window.press("escape");
}

fn voice_channel(window: &mut Window) -> Option<ChannelId> {
    window
        .channels()
        .into_iter()
        .find(|row| row.kind == ChannelKind::Voice)
        .map(|row| row.id)
}

fn selected_channel(window: &mut Window) -> Option<ChannelId> {
    window
        .view()
        .workspace()
        .and_then(|workspace| workspace.channel.as_ref().map(|channel| channel.id))
}

fn occupants(window: &mut Window) -> Vec<Occupant> {
    window
        .channels()
        .into_iter()
        .find(|row| row.kind == ChannelKind::Voice)
        .and_then(|row| row.voice)
        .map(|voice| voice.occupants)
        .unwrap_or_default()
}

fn own_share(window: &mut Window) -> OwnShare {
    window.view().call.map_or(OwnShare::Off, |call| call.share)
}

fn stage(window: &mut Window) -> Option<ShareStage> {
    window
        .view()
        .lists()
        .and_then(|lists| lists.call.clone())
        .and_then(|panel| panel.stage)
}

/// Starts a share of the synthetic screen through the picker, with its sound or without.
fn start_share(window: &mut Window, sound: bool) {
    window.click("call-share");
    window.until("the screens are listed", |window| {
        window.shows("share-sources")
    });
    window.click("share-display:1");
    window.click(if sound {
        "share-sound-1"
    } else {
        "share-sound-0"
    });
    window.click("start-share");
    window.until("the share runs", |window| {
        own_share(window) == OwnShare::Running
    });
}

/// One window shares its screen and the other one watches it: the picture that the
/// watching window draws has the colours of the source, and changes when they change.
fn share_and_watch(sharing: &mut Window, watching: &mut Window, native: bool, sound: bool) {
    sharing
        .capture
        .set_colours(FIRST_COLOURS.0, FIRST_COLOURS.1);
    start_share(sharing, sound);
    let sharer = sharing.view().person.account_id;
    watching.until("the other window watches the share", |window| {
        stage(window).is_some_and(|stage| {
            stage
                .watched()
                .is_some_and(|share| Some(share.account_id) == sharer && share.size.is_some())
        }) && window.shows("share-picture")
    });
    if sound {
        watching.until("the other window plays the sound of the share", |window| {
            heard(&window.sound, SHARE_TONE_HZ) > SHARE_HEARD_DBFS
        });
    }
    if native {
        watching.until("the drawn share has the first colours", |window| {
            drawn_colours(window).is_some_and(|drawn| near(drawn, FIRST_COLOURS))
        });
        sharing
            .capture
            .set_colours(SECOND_COLOURS.0, SECOND_COLOURS.1);
        watching.until("the drawn share has the second colours", |window| {
            drawn_colours(window).is_some_and(|drawn| near(drawn, SECOND_COLOURS))
        });
    }
}

/// Stops the share of a window, and waits until the other window no longer shows it.
fn stop_share(sharing: &mut Window, watching: &mut Window) {
    sharing.click("call-share");
    sharing.until("the own share is off", |window| {
        own_share(window) == OwnShare::Off
    });
    watching.until("the share is gone in the other window", |window| {
        stage(window).is_none_or(|stage| stage.shares.is_empty())
    });
}

/// The colours that the window drew at the middle of the left and of the right half of
/// the picture of the watched share.
fn drawn_colours(window: &mut Window) -> Option<Halves> {
    let bounds = window.cx.debug_bounds("share-picture")?;
    let (picture, scale) = window
        .cx
        .update(|window, _| (window.render_to_image(), window.scale_factor()));
    let picture = picture.ok()?;
    let at = |fraction: f32| {
        let x = (f32::from(bounds.left()) + f32::from(bounds.size.width) * fraction) * scale;
        let y = f32::from(bounds.center().y) * scale;
        let pixel = picture.get_pixel(x as u32, y as u32).0;
        (pixel[0], pixel[1], pixel[2])
    };
    Some((at(0.4), at(0.6)))
}

fn near(drawn: Halves, wanted: Halves) -> bool {
    let close = |a: Colour, b: Colour| {
        [(a.0, b.0), (a.1, b.1), (a.2, b.2)]
            .iter()
            .all(|(x, y)| (i32::from(*x) - i32::from(*y)).abs() <= COLOUR_TOLERANCE)
    };
    close(drawn.0, wanted.0) && close(drawn.1, wanted.1)
}

/// Lets the application run for a time without an action, as a person who waits.
fn pause(window: &mut Window, time: Duration) {
    let end = Instant::now() + time;
    until_within(window, "the pause ends", time + PATIENCE, |_| {
        (Instant::now() >= end).then_some(())
    });
}

/// The microphone of the window says the speech; returns when it has been said.
fn speak(window: &mut Window, speech: &Speech) {
    window.sound.say("tone", &speech.samples, speech.rate);
    let length = Duration::from_secs_f64(speech.samples.len() as f64 / f64::from(speech.rate));
    until_within(
        window,
        "the speech has been said",
        length + PATIENCE,
        |window| (window.sound.speaking("tone") == Duration::ZERO).then_some(()),
    );
}

/// The recording card of the shown voice channel, when the channel has one.
fn recording_card(window: &mut Window) -> Option<(RecordingId, CardDetail)> {
    feed_rows(window)?.rows.iter().find_map(|row| match row {
        FeedRow::Recording(row) => match &row.card.state {
            CardState::Shown(detail) => Some((row.card.recording_id, (**detail).clone())),
            _ => None,
        },
        _ => None,
    })
}

fn search_for(window: &mut Window, text: &str) {
    replace_in(window, "search-field", text);
    window.press("enter");
}

fn until_searched(window: &mut Window) -> SearchPanel {
    window.until_value("the search has its answer", |window| {
        window
            .view()
            .workspace()?
            .search
            .clone()
            .filter(|panel| panel.state != SearchShown::Searching && !panel.reading_more)
    })
}

/// Waits like `Window::until`, with a limit of its own for work of the deployment that
/// takes minutes: the processing of a recording and its transcription.
#[track_caller]
fn until_within<T>(
    window: &mut Window,
    what: &str,
    limit: Duration,
    mut value: impl FnMut(&mut Window) -> Option<T>,
) -> T {
    let deadline = Instant::now() + limit;
    loop {
        window.settle();
        if let Some(found) = value(window) {
            return found;
        }
        assert!(
            Instant::now() < deadline,
            "after {limit:?} this did not hold: {what}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The words of a text in lower case, without punctuation.
pub fn words(text: &str) -> Vec<String> {
    text.split(|character: char| !character.is_alphanumeric() && character != '\'')
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// The share of the words of `said` that the transcript has.
fn recall(said: &str, transcript: &str) -> f64 {
    let heard: std::collections::HashSet<String> = words(transcript).into_iter().collect();
    let said = words(said);
    let found = said.iter().filter(|word| heard.contains(*word)).count();
    found as f64 / said.len().max(1) as f64
}

/// The steps that need no recorder, against the server of the tests with a media server.
#[gpui::test]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
fn the_rehearsal_steps_pass_in_two_windows_against_the_server_of_the_tests(
    cx: &mut TestAppContext,
) {
    let world = crate::harness::World::new();
    let server = world.server_with_media("Rehearsal");
    let profiles = tempfile::tempdir().unwrap();
    let deployment = Deployment {
        address: server.address(),
        owner_code: server.owner_code().unwrap(),
    };
    let mut run = Rehearsal::new(cx, profiles.path(), deployment, false, None, "t1");
    run.first_owner();
    run.invitation();
    run.text();
    run.thread();
    run.call();
    run.share();
    for window in [&mut run.a, run.b.as_mut().unwrap()] {
        window.click("leave-call");
        window.until("the window left the call", |window| {
            window.view().call.is_none()
        });
    }
    run.search(None);
    run.persistence();
}
