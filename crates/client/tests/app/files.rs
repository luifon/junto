//! The file of a message in the window against the real server (ADR 0740): the person
//! starts its download at its control, cancels a download that runs, and shows the saved
//! file in its folder.

use gpui::TestAppContext;
use junto_client_core::model::PlaceId;
use junto_client_core::protocol::message::SendMessage;
use junto_client_core::transfers::{DownloadStage, UploadSource};

use crate::harness::{World, member};
use crate::messages::document;
use crate::support::FaultProxy;

#[gpui::test]
fn the_controls_of_a_file_download_it_cancel_the_download_and_show_the_file(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let proxy = world.run(FaultProxy::start(&server));
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Luca");
    window.until("the messages are read", |window| {
        window.feed().is_some_and(|feed| feed.loaded)
    });
    let place = PlaceId::Channel(window.selected_channel());
    let content: Vec<u8> = (0..60_000u32).map(|number| (number % 241) as u8).collect();
    let attachment_id = world.run(async {
        let source = UploadSource::Bytes {
            name: "budget.ods".to_owned(),
            bytes: std::sync::Arc::new(content.clone()),
        };
        let attachment_id = admin
            .handle
            .transfers()
            .upload(place, source)
            .finished()
            .await
            .unwrap();
        let message = SendMessage {
            document: document("The budget"),
            reply_to: None,
            attachments: vec![attachment_id],
        };
        admin
            .handle
            .api()
            .send_message(place, &message)
            .await
            .unwrap();
        attachment_id
    });
    window.until_message("The budget");
    // The folder of the downloads of the system is a folder of the test.
    let downloads = tempfile::tempdir().unwrap();
    let target = downloads.path().to_path_buf();
    window.in_shell(move |shell, _, cx| {
        shell
            .state()
            .update(cx, |state, _| state.set_download_directory(target));
    });
    let start = crate::harness::selector(format!("download:{attachment_id}"));
    let cancel = crate::harness::selector(format!("cancel-download:{attachment_id}"));
    let show = crate::harness::selector(format!("show-file:{attachment_id}"));
    let stage = |window: &mut crate::harness::Window| {
        window
            .view()
            .downloads
            .iter()
            .find(|((_, id), _)| *id == attachment_id)
            .map(|(_, progress)| progress.stage.clone())
    };
    let file = format!("/api/v1/attachments/{attachment_id}/file");
    assert!(window.shows(start));

    // The file of the download waits at the proxy: the control shows the progress and
    // Cancel, and Cancel ends the download.
    proxy.hold(&file);
    window.click(start);
    window.until("the download runs", |window| {
        matches!(stage(window), Some(DownloadStage::Receiving)) && window.shows(cancel)
    });
    assert!(!window.shows(start));
    window.click(cancel);
    window.until("the download is cancelled", |window| {
        stage(window).is_none() && window.shows(start)
    });
    proxy.release();
    window.settle();
    let left: Vec<_> = std::fs::read_dir(downloads.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert!(left.is_empty(), "a cancelled download left {left:?}");

    // Download: the file is saved, and the control shows the folder of the file.
    window.click(start);
    let saved = downloads.path().join("budget.ods");
    window.until("the file is saved", |window| {
        matches!(stage(window), Some(DownloadStage::Saved(path)) if path == saved)
            && window.shows(show)
    });
    assert_eq!(std::fs::read(&saved).unwrap(), content);
    assert!(
        proxy.seen(&file).iter().any(|seen| seen
            .answer
            .as_ref()
            .is_some_and(|(status, _)| *status == 200)),
        "the server sent the file"
    );

    // Show in folder hands the saved file to the file manager of the system. The test
    // platform of gpui has no file manager, and the application keeps the path of a file
    // that it would show.
    window.click(show);
    let shown = window
        .state
        .read_with(&window.cx, |state, _| state.kept_shown().to_vec());
    assert_eq!(shown, [saved]);
}
