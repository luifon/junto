use crate::harness::{named, open};
use gpui::TestAppContext;

#[gpui::test]
fn the_clock_format_is_chosen_in_interface_and_kept_after_restart(cx: &mut TestAppContext) {
    let (directory, mut window) = named(cx, "Member");
    window.click("app-settings");
    window.click("settings-section-interface");
    assert!(window.shows("clock-format-0"));
    window.click("clock-format-1");
    window.run_tasks();
    let file = directory.path().join("junto/settings.json");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&std::fs::read(&file).unwrap()).unwrap()["clock_format"],
        "twelve_hour"
    );
    window.close();
    let mut window = open(cx, directory.path());
    window.click("app-settings");
    window.click("settings-section-interface");
    window.click("clock-format-2");
    window.run_tasks();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&std::fs::read(file).unwrap()).unwrap()["clock_format"],
        "twenty_four_hour"
    );
}

#[gpui::test]
fn the_newest_system_clock_reading_wins_and_an_explicit_choice_stays(cx: &mut TestAppContext) {
    use junto_client_core::clock::{ClockFormat, HourCycle};
    let (_directory, mut window) = named(cx, "Member");
    window.state.update(&mut window.cx, |state, cx| {
        state.set_zone_reader(|| chrono_tz::UTC);
        for reading in 0..40 {
            state.set_clock_reader(move || {
                if reading % 2 == 0 {
                    HourCycle::TwentyFour
                } else {
                    HourCycle::Twelve
                }
            });
            state.set_window_active(false, cx);
            state.set_window_active(true, cx);
        }
    });
    window.settle();
    assert_eq!(
        window
            .state
            .read_with(&window.cx, |state, _| state.hour_cycle()),
        HourCycle::Twelve
    );
    window.state.update(&mut window.cx, |state, cx| {
        state.set_clock_format(ClockFormat::TwentyFourHour, cx);
        state.set_window_active(true, cx);
    });
    window.settle();
    assert_eq!(
        window
            .state
            .read_with(&window.cx, |state, _| state.hour_cycle()),
        HourCycle::TwentyFour
    );
    cx.update(|cx| junto_client::shell::detached::open(window.state.clone(), cx));
    window.state.update(&mut window.cx, |state, cx| {
        state.set_clock_format(ClockFormat::System, cx);
        state.set_clock_reader(|| HourCycle::TwentyFour);
        state.set_call_window_active(true, cx);
    });
    window.until("the detached window refreshes the system clock", |window| {
        window
            .state
            .read_with(&window.cx, |state, _| state.hour_cycle())
            == HourCycle::TwentyFour
    });
}

#[gpui::test]
fn messages_notices_and_quiet_hour_entry_use_the_chosen_clock(cx: &mut TestAppContext) {
    use crate::harness::{World, member};
    use junto_client::state::notices::Agreement;
    use junto_client_core::clock::ClockFormat;
    let world = World::new();
    let server = world.server("Clock display");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
    let channel = window.selected_channel();
    let own = window.own_account();
    let message = admin.send(
        &world,
        channel,
        crate::notices::mention("clock example, ", own),
    );
    window.state.update(&mut window.cx, |state, cx| {
        state.set_clock_format(ClockFormat::TwelveHour, cx)
    });
    window.until("the message uses AM or PM", |window| {
        window.messages().iter().any(|row| {
            row.message.id == message.id && (row.time.ends_with(" AM") || row.time.ends_with(" PM"))
        })
    });
    world
        .run(admin.handle.api().pin_message(message.id))
        .unwrap();
    window.until("the system row uses the chosen clock", |window| {
        window.feed().is_some_and(|feed| feed.rows.iter().any(|row| {
            matches!(row, junto_client::state::feed::FeedRow::System(row) if row.time.ends_with(" AM") || row.time.ends_with(" PM"))
        }))
    });
    let system_id = window
        .feed()
        .unwrap()
        .rows
        .iter()
        .find_map(|row| match row {
            junto_client::state::feed::FeedRow::System(row) => Some(row.message.id),
            _ => None,
        })
        .unwrap();
    let time = window
        .cx
        .debug_bounds(crate::harness::selector(format!("system-time:{system_id}")))
        .unwrap();
    let body = window
        .cx
        .debug_bounds(crate::harness::selector(format!("system-text:{system_id}")))
        .unwrap();
    assert!(time.size.width > gpui::px(40.));
    assert!(time.right() <= body.left());
    window.open_notices();
    window.until("the notice uses AM or PM", |window| {
        window.notices().is_some_and(|panel| {
            !panel.rows.is_empty()
                && panel
                    .rows
                    .iter()
                    .all(|row| row.time.ends_with(" AM") || row.time.ends_with(" PM"))
        })
    });
    window.state.update(&mut window.cx, |state, cx| {
        state.set_clock_format(ClockFormat::TwentyFourHour, cx)
    });
    window.until("the notice changes to 24 hours", |window| {
        window.notices().is_some_and(|panel| {
            !panel.rows.is_empty() && panel.rows.iter().all(|row| row.time.len() == 5)
        })
    });
    window.state.update(&mut window.cx, |state, cx| {
        state.set_clock_format(ClockFormat::TwelveHour, cx)
    });
    window.click("app-settings");
    window.click("settings-section-quiet");
    window.state.update(&mut window.cx, |state, cx| {
        state.set_clock_format(ClockFormat::TwentyFourHour, cx)
    });
    window.settle();
    window.state.update(&mut window.cx, |state, cx| {
        state.set_clock_format(ClockFormat::TwelveHour, cx)
    });
    window.settle();
    window.click("quiet-from");
    window.press("secondary-a");
    window.press("secondary-c");
    assert_eq!(
        window
            .cx
            .update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text())),
        Some("10:00 PM".into())
    );
    window.simulate(|cx| cx.simulate_input("9:45 PM"));
    window.state.update(&mut window.cx, |state, cx| {
        state.set_clock_format(ClockFormat::TwentyFourHour, cx)
    });
    window.settle();
    window.click("quiet-until");
    window.press("secondary-a");
    window.simulate(|cx| cx.simulate_input("8:15 AM"));
    window.click("set-daily-quiet");
    window.until("AM and PM are sent as the correct minutes", |window| {
        matches!(window.view().quiet.daily, Some(Agreement::All(Some(daily))) if daily.start_minute == 21 * 60 + 45 && daily.end_minute == 8 * 60 + 15)
    });
}
