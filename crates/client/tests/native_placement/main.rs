//! A native detached window driven by tools/check_window_placement.py.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use junto_client::{
    app::{self, WindowStart},
    assets::Assets,
    shell::detached,
};
use junto_client_core::{Client, ClientOptions};
use junto_media::{
    audio::synthetic::SyntheticHost,
    calls::{CallTiming, MediaOptions},
    camera::synthetic::SyntheticCamera,
    permission::FixedPermission,
    share::synthetic::SyntheticCapture,
};
use std::{cell::Cell, process::ExitCode, rc::Rc, sync::Arc, time::Duration};

fn main() -> ExitCode {
    let Some(directory) = std::env::var_os("JUNTO_TEST_PLACEMENT_DATA_DIR") else {
        println!("native_placement: not run, no isolated data directory supplied");
        return ExitCode::SUCCESS;
    };
    junto_client::panics::install();
    let timed_out = Rc::new(Cell::new(false));
    let failed = timed_out.clone();
    gpui_platform::application()
        .with_assets(Assets)
        .run(move |cx| {
            app::init(cx).unwrap();
            let client = Client::open(ClientOptions::new(directory)).unwrap();
            junto_client::panics::record_into(client.data_dir().root());
            let media = MediaOptions {
                camera: Arc::new(SyntheticCamera::new()),
                host: Arc::new(SyntheticHost::with_tone(440.0)),
                permission: Arc::new(FixedPermission::allowed()),
                capture: Arc::new(SyntheticCapture::new()),
                timing: CallTiming::default(),
            };
            let state = app::state_with_media(client, WindowStart::default(), media, cx);
            detached::open(state, cx);
            cx.spawn(async move |cx| {
                cx.background_executor()
                    .timer(Duration::from_secs(30))
                    .await;
                failed.set(true);
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    if timed_out.get() {
        eprintln!("native_placement: the driver did not finish within 30 seconds");
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
