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
            comment: None,
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
    // Cancel is pressed while the request of the file waits at the proxy and the unfinished
    // file exists, so that the check below shows that the cancel removes it.
    window.until("the request of the file waits at the proxy", |_| {
        proxy.seen(&file).iter().any(|seen| seen.answer.is_none())
    });
    window.until("the unfinished file exists", |_| {
        std::fs::read_dir(downloads.path())
            .unwrap()
            .next()
            .is_some()
    });
    window.click(cancel);
    window.until("the download is cancelled", |window| {
        stage(window).is_none() && window.shows(start)
    });
    // The unfinished file is removed by the download task of the client core, after the
    // window has taken the download away. The request that waited is passed on: a download
    // that was not ended would now save the file.
    proxy.release();
    window.until("the cancelled download left no file", |_| {
        std::fs::read_dir(downloads.path())
            .unwrap()
            .next()
            .is_none()
    });

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

/// A download that failed shows its reason as text, and "Try again" downloads the file
/// again; it did only clear the failure.
#[gpui::test]
fn try_again_after_a_failed_download_downloads_the_file_again(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    window.until("the messages are read", |window| {
        window.feed().is_some_and(|feed| feed.loaded)
    });
    let place = PlaceId::Channel(window.selected_channel());
    let attachment_id = world.run(async {
        let source = UploadSource::Bytes {
            name: "notes.txt".to_owned(),
            bytes: std::sync::Arc::new(b"plan".to_vec()),
        };
        let attachment_id = admin
            .handle
            .transfers()
            .upload(place, source)
            .finished()
            .await
            .unwrap();
        let message = SendMessage {
            document: document("The notes"),
            comment: None,
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
    window.until_message("The notes");
    // A file in place of the folder of the downloads: the file cannot be written.
    let downloads = tempfile::tempdir().unwrap();
    let blocked = downloads.path().join("not-a-folder");
    std::fs::write(&blocked, b"").unwrap();
    let target = blocked.clone();
    window.in_shell(move |shell, _, cx| {
        shell
            .state()
            .update(cx, |state, _| state.set_download_directory(target));
    });
    let stage = |window: &mut crate::harness::Window| {
        window
            .view()
            .downloads
            .iter()
            .find(|((_, id), _)| *id == attachment_id)
            .map(|(_, progress)| progress.stage.clone())
    };
    window.click(crate::harness::selector(format!(
        "download:{attachment_id}"
    )));
    window.until("the download failed", |window| {
        matches!(stage(window), Some(DownloadStage::Failed(_)))
    });
    let retry = crate::harness::selector(format!("retry-download:{attachment_id}"));
    assert!(window.shows(retry));

    let target = downloads.path().to_path_buf();
    window.in_shell(move |shell, _, cx| {
        shell
            .state()
            .update(cx, |state, _| state.set_download_directory(target));
    });
    window.click(retry);
    let saved = downloads.path().join("notes.txt");
    window.until(
        "the file is saved at the second try",
        |window| matches!(stage(window), Some(DownloadStage::Saved(path)) if path == saved),
    );
    assert_eq!(std::fs::read(&saved).unwrap(), b"plan");
}
