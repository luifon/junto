//! The time of a frame of the window with 10,000 loaded messages in the shown channel
//! (ADR 0740). The tool draws with the real window code and the headless renderer, and
//! prints the times. It writes no picture.

use std::sync::Arc;
use std::time::{Duration, Instant};

use junto_client::shell::{Pane, Shell};
use junto_client::state::view::ServerSnapshot;

use super::messages::{GENERAL, Made, acme_with, none, prepare};
use super::{Screen, Studio, harbor};

const MESSAGES: u32 = 10_000;
const AUTHORS: [&str; 5] = ["Ana", "Bruno", "Carla", "Diego", "Luca"];
const TEXTS: [&str; 5] = [
    "The export worker ran out of memory on large rooms. Now it drops muted tracks.",
    "Ran it on the 40-person room: the mixdown went from **96 s** to `11 s`.",
    "Should recordings keep one track per speaker, or only the mixdown?",
    "```Rust\nfn mix_tracks(tracks: &[Track]) -> Mixdown {\n    tracks.iter().collect()\n}\n```",
    "Reviewed. The default is 30 days, with a per-channel override. I merge after lunch.",
];

/// Adds the next message to the messages of `#general`.
fn add(made: &mut Made) {
    let index = made.messages.len();
    made.message(
        AUTHORS[index % AUTHORS.len()],
        index as i64 * 3,
        TEXTS[(index / 2) % TEXTS.len()],
    );
}

/// "Acme Studio" with these messages in `#general`. The state shares each message with
/// the state before, as the client core does.
fn servers(made: &Made) -> Vec<ServerSnapshot> {
    let held = Made {
        channel: made.channel,
        position: made.position,
        messages: made.messages.clone(),
    };
    vec![
        acme_with(|model| model.timelines = vec![Arc::new(held.timeline())]),
        harbor(),
    ]
}

struct Times(Vec<Duration>);

impl Times {
    fn line(mut self, what: &str) -> String {
        self.0.sort();
        let milliseconds = |time: Duration| time.as_secs_f64() * 1_000.0;
        let median = self.0[self.0.len() / 2];
        let longest = self.0[self.0.len() - 1];
        format!(
            "{what}: {} frames, median {:.2} ms, longest {:.2} ms",
            self.0.len(),
            milliseconds(median),
            milliseconds(longest)
        )
    }
}

pub(crate) fn measure(studio: &mut Studio) -> Result<(), String> {
    // The same window with few messages tells what the messages cost.
    for count in [50, MESSAGES] {
        measure_with(studio, count)?;
    }
    Ok(())
}

/// The frames of the audit log of the administration with 50 and with 2,000 entries
/// (ADR 0761): without a change, after a scroll by the wheel, after a jump of the scroll
/// bar to a distant place, and after a newer entry arrives at the top of the log.
pub(crate) fn measure_audit(studio: &mut Studio) -> Result<(), String> {
    const WARM_UP: usize = 5;
    const STEPS: usize = WARM_UP + 60;
    for count in [50, 2_000] {
        let (window, shell, _) = studio.open(
            Screen::new("audit-frames", super::admin::with_names()),
            Box::new(move |shell, window, cx| {
                super::admin::prepare_audit_rows(shell, count, window, cx);
            }),
        )?;
        // Each log of the change has one newer entry than the log before. They are built
        // before the frames are measured.
        let mut logs: Vec<_> = (1..=STEPS)
            .map(|newer| Some(super::admin::audit_rows(count, newer)))
            .collect();
        for case in ["unchanged", "scrolled", "jumped", "changed"] {
            let mut times = Vec::new();
            for (step, slot) in logs.iter_mut().enumerate() {
                let log = if case == "changed" { slot.take() } else { None };
                let started = Instant::now();
                studio
                    .cx
                    .update_window(window.into(), |_, window, cx| {
                        shell.update(cx, |shell, cx| match (case, log) {
                            ("scrolled", _) => {
                                shell.scroll_admin_page_by(gpui::px(if step % 2 == 0 {
                                    360.0
                                } else {
                                    -240.0
                                }))
                            }
                            // 101 places spread over the list, each once.
                            ("jumped", _) => {
                                shell.scroll_admin_page_to((step * 37 % 101) as f32 / 100.0)
                            }
                            (_, Some(log)) => super::admin::show_audit_rows(shell, log, cx),
                            _ => {}
                        });
                        window.refresh();
                    })
                    .map_err(|error| format!("{error:#}"))?;
                studio.cx.run_until_parked();
                if step >= WARM_UP {
                    times.push(started.elapsed());
                }
            }
            println!("audit with {count} rows: {}", Times(times).line(case));
        }
        studio
            .cx
            .update_window(window.into(), |_, window, _| window.remove_window())
            .map_err(|error| format!("{error:#}"))?;
        studio.cx.run_until_parked();
    }
    Ok(())
}

fn measure_with(studio: &mut Studio, count: u32) -> Result<(), String> {
    let prepare_messages: super::Prepare = Box::new(|shell: &mut Shell, _, cx| {
        prepare(shell, none(), cx);
    });
    let mut made = Made::new(GENERAL);
    for _ in 0..count {
        add(&mut made);
    }
    let started = Instant::now();
    let (window, shell, state) =
        studio.open(Screen::new("frames", servers(&made)), prepare_messages)?;
    let frame = |studio: &mut Studio| -> Result<Duration, String> {
        let started = Instant::now();
        studio
            .cx
            .update_window(window.into(), |_, window, _| window.refresh())
            .map_err(|error| format!("{error:#}"))?;
        studio.cx.run_until_parked();
        Ok(started.elapsed())
    };
    frame(studio)?;
    println!(
        "the window with {count} messages: first frame after {:.0} ms",
        started.elapsed().as_secs_f64() * 1_000.0
    );

    // A frame in which nothing changed.
    let mut same = Vec::new();
    for _ in 0..60 {
        same.push(frame(studio)?);
    }
    println!("{}", Times(same).line("a frame without a change"));

    // A frame after the list moved by 40 rows.
    let mut moved = Vec::new();
    let rows = count as usize;
    for step in 0..60usize {
        let row = (rows - 1).saturating_sub((step * 40) % rows);
        studio.cx.update(|cx| {
            shell.update(cx, |shell, cx| shell.show_feed_row(Pane::Main, row, cx));
        });
        moved.push(frame(studio)?);
    }
    println!("{}", Times(moved).line("a frame after a scroll of 40 rows"));

    // A frame after a message arrived: the state changes, the rows are derived again and
    // the list takes them.
    let mut arrived = Vec::new();
    for _ in 0..20u32 {
        add(&mut made);
        let next = servers(&made);
        let started = Instant::now();
        studio.cx.update(|cx| {
            state.update(cx, |state, cx| state.show_fixture(next, cx));
        });
        studio
            .cx
            .update_window(window.into(), |_, window, _| window.refresh())
            .map_err(|error| format!("{error:#}"))?;
        studio.cx.run_until_parked();
        arrived.push(started.elapsed());
    }
    println!(
        "{}",
        Times(arrived).line("a frame after a new message, with the derivation of the rows")
    );
    studio
        .cx
        .update_window(window.into(), |_, window, _| window.remove_window())
        .map_err(|error| format!("{error:#}"))?;
    studio.cx.run_until_parked();
    Ok(())
}

/// The frames of the window with 601 members in the shown workspace and the People page of
/// the administration with 2,001 accounts open (open item CD8): each answer of a profile
/// that the window reads makes the state change, and the next frame derives the whole view
/// again. The time of a frame without a change is the cost of one such answer.
pub(crate) fn measure_people(studio: &mut Studio) -> Result<(), String> {
    use junto_client::state::admin::AdminPage;
    use junto_client_core::model::{Collection, Loadable};
    use junto_client_core::protocol::account::{AccountStatus, AccountSummary};
    use junto_client_core::protocol::structure::WorkspaceMember;

    const WARM_UP: usize = 5;
    for (members, accounts, panel) in [
        (7usize, 7usize, true),
        (601, 7, false),
        (601, 7, true),
        (7, 2_001, true),
        (601, 2_001, true),
    ] {
        let people = super::admin::servers(move |model| {
            let listed: Vec<AccountSummary> = (0..accounts)
                .map(|index| AccountSummary {
                    id: super::id(5_000 + index as u32),
                    name: format!("Person {index:04}"),
                    status: AccountStatus::Active,
                    is_owner: index == 0,
                    server_role_ids: Vec::new(),
                    created_at: super::at(),
                })
                .collect();
            super::admin::collections(model).accounts = Collection::Loaded(listed);
            let workspace = Arc::make_mut(&mut model.workspaces[0]);
            workspace.members = Loadable::Loaded(
                (0..members)
                    .map(|index| WorkspaceMember {
                        avatar: None,
                        account_id: super::id(5_000 + index as u32),
                        name: format!("Person {index:04}"),
                        role_ids: Vec::new(),
                    })
                    .collect(),
            );
        });
        let (window, _, state) = studio.open(
            Screen::new("people-frames", people),
            Box::new(move |shell: &mut Shell, window, cx| {
                if panel {
                    shell.open(junto_client::shell::Open::Members, window, cx);
                }
                super::admin::open_page(shell, AdminPage::People, window, cx);
            }),
        )?;
        let mut times = Vec::new();
        for step in 0..WARM_UP + 60 {
            let started = Instant::now();
            studio
                .cx
                .update_window(window.into(), |_, window, _| window.refresh())
                .map_err(|error| format!("{error:#}"))?;
            studio.cx.run_until_parked();
            if step >= WARM_UP {
                times.push(started.elapsed());
            }
        }
        println!(
            "{}",
            Times(times).line(&format!(
                "{members} members (member list {}) and the People page of {accounts} accounts",
                if panel { "open" } else { "closed" }
            ))
        );
        // A burst of answers of profiles: each changes the state, as an answer does, and
        // one frame follows (review M8).
        const ANSWERS: u32 = 500;
        let started = Instant::now();
        for _ in 0..ANSWERS {
            studio.cx.update(|cx| state.update(cx, |_, cx| cx.notify()));
        }
        studio
            .cx
            .update_window(window.into(), |_, window, _| window.refresh())
            .map_err(|error| format!("{error:#}"))?;
        studio.cx.run_until_parked();
        println!(
            "{members} members and {accounts} accounts: {ANSWERS} changes of the state and one \
             frame in {:.0} ms",
            started.elapsed().as_secs_f64() * 1_000.0
        );
        studio
            .cx
            .update_window(window.into(), |_, window, _| window.remove_window())
            .map_err(|error| format!("{error:#}"))?;
        studio.cx.run_until_parked();
    }
    Ok(())
}
