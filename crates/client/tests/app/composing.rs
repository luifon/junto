//! The composer in the window against the real server (ADR 0740): the controls of the
//! formatting bar, a picture pasted from the clipboard, a file taken away before the send,
//! and an upload that goes on after its connection was cut.

use gpui::{ClipboardItem, Image, ImageFormat, TestAppContext};
use junto_client::shell::Pane;
use junto_client::text;
use junto_client_core::protocol::ids::MessageId;
use junto_client_core::protocol::message::{Block, Document, Inline, Mark};
use junto_client_core::transfers::UploadStage;

use crate::harness::{Admin, Window, World, member};
use crate::support::{Fault, FaultProxy};

impl Window {
    /// Types a text into the composer, selects all of it with the keyboard, and clicks a
    /// control of the formatting bar.
    fn format(&mut self, text: &str, tool: &'static str) {
        self.in_shell(|shell, window, cx| {
            shell.type_message(Pane::Main, text, window, cx);
            shell.focus_composer(Pane::Main, window, cx);
        });
        self.press("secondary-a");
        self.click(tool);
    }

    fn composer_text(&mut self) -> String {
        self.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx))
    }

    /// Sends the composer with Enter, and waits for the message that the server stored.
    /// Returns its identifier and its document as the window draws it.
    fn send_composer(&mut self) -> (MessageId, Document) {
        let before = self.messages().len();
        self.press("enter");
        self.until("the message is sent", |window| {
            window.messages().len() == before + 1
        });
        let row = self.messages().pop().unwrap();
        (row.message.id, row.message.document.clone().unwrap())
    }

    /// The draft files of the shown place.
    fn draft(&mut self) -> Vec<junto_client::state::view::DraftFile> {
        self.view()
            .lists()
            .map(|lists| lists.draft.clone())
            .unwrap_or_default()
    }
}

/// The document of a message as the server holds it.
fn stored(world: &World, admin: &Admin, message_id: MessageId) -> Document {
    world
        .run(admin.handle.api().message(message_id))
        .unwrap()
        .document
        .unwrap()
}

fn text(text: &str, marks: Vec<Mark>) -> Inline {
    Inline::Text {
        text: text.to_owned(),
        marks,
    }
}

#[gpui::test]
fn the_formatting_bar_marks_the_selected_text_of_the_message(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    window.until("the messages are read", |window| {
        window.feed().is_some_and(|feed| feed.loaded)
    });

    // Strikethrough around the selected words.
    window.format("old price", "format-strike");
    assert_eq!(window.composer_text(), "~~old price~~");
    let (id, drawn) = window.send_composer();
    let expected = Document {
        blocks: vec![Block::Paragraph {
            content: vec![text("old price", vec![Mark::Strikethrough])],
        }],
    };
    assert_eq!(drawn, expected);
    assert_eq!(stored(&world, &admin, id), expected);

    // A quote of the selected line.
    window.format("said before", "format-quote");
    assert_eq!(window.composer_text(), "> said before");
    let (id, drawn) = window.send_composer();
    let expected = Document {
        blocks: vec![Block::Quote {
            content: vec![text("said before", Vec::new())],
        }],
    };
    assert_eq!(drawn, expected);
    assert_eq!(stored(&world, &admin, id), expected);

    // A list of each selected line.
    window.format("milk\nbread", "format-list");
    assert_eq!(window.composer_text(), "- milk\n- bread");
    let (id, drawn) = window.send_composer();
    let expected = Document {
        blocks: vec![Block::List {
            ordered: false,
            items: vec![
                vec![text("milk", Vec::new())],
                vec![text("bread", Vec::new())],
            ],
        }],
    };
    assert_eq!(drawn, expected);
    assert_eq!(stored(&world, &admin, id), expected);

    // A code block around the selected code; the person names its language after the
    // opening marks, and the window draws the block in the colours of that language.
    let code = "fn main() { let total = 4; }";
    window.format(code, "format-code-block");
    assert_eq!(window.composer_text(), format!("```\n{code}\n```"));
    window.press("up end");
    window.cx.simulate_input("rust");
    assert_eq!(window.composer_text(), format!("```rust\n{code}\n```"));
    let (id, drawn) = window.send_composer();
    let expected = Document {
        blocks: vec![Block::Code {
            text: code.to_owned(),
            language: Some("rust".to_owned()),
        }],
    };
    assert_eq!(drawn, expected);
    assert_eq!(stored(&world, &admin, id), expected);
    let colours = window.in_shell(|shell, _, _| shell.code_colours(code, "rust"));
    assert!(
        colours.is_some_and(|parts| parts > 0),
        "the code block is drawn without colours: {colours:?}"
    );

    // A link with the selected words as its text; the person types the address in its
    // place.
    window.format("the guide", "format-link");
    assert_eq!(window.composer_text(), "[the guide](https://)");
    window.press("end left");
    window.cx.simulate_input("example.com/guide");
    assert_eq!(
        window.composer_text(),
        "[the guide](https://example.com/guide)"
    );
    let (id, drawn) = window.send_composer();
    let expected = Document {
        blocks: vec![Block::Paragraph {
            content: vec![Inline::Link {
                text: "the guide".to_owned(),
                url: "https://example.com/guide".to_owned(),
            }],
        }],
    };
    assert_eq!(drawn, expected);
    assert_eq!(stored(&world, &admin, id), expected);
}

/// A PNG image of one colour.
fn png(side: u32) -> Vec<u8> {
    let picture = image::RgbaImage::from_pixel(side, side, image::Rgba([40, 90, 200, 255]));
    let mut bytes = Vec::new();
    picture
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    bytes
}

#[gpui::test]
fn a_pasted_picture_is_a_file_of_the_message_and_pasted_text_is_text(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    window.until("the messages are read", |window| {
        window.feed().is_some_and(|feed| feed.loaded)
    });
    window.in_shell(|shell, window, cx| shell.focus_composer(Pane::Main, window, cx));

    // Text in the clipboard is pasted into the composer, and no file is attached.
    window
        .cx
        .write_to_clipboard(ClipboardItem::new_string("Pasted words".to_owned()));
    window.press("secondary-v");
    assert_eq!(window.composer_text(), "Pasted words");
    assert!(window.draft().is_empty());

    // A picture in the clipboard becomes a file of the message; the text stays.
    let picture = png(24);
    window
        .cx
        .write_to_clipboard(ClipboardItem::new_image(&Image::from_bytes(
            ImageFormat::Png,
            picture.clone(),
        )));
    window.press("secondary-v");
    assert_eq!(window.composer_text(), "Pasted words");
    window.until("the pasted picture is uploaded", |window| {
        let draft = window.draft();
        draft.len() == 1
            && draft[0].name == text::PASTED_PICTURE
            && matches!(draft[0].progress.stage, UploadStage::Complete(_))
    });
    assert!(window.shows_named("draft-file:0".to_owned()));

    let (id, _) = window.send_composer();
    let message = world.run(admin.handle.api().message(id)).unwrap();
    assert_eq!(message.attachments.len(), 1);
    assert_eq!(message.attachments[0].name, text::PASTED_PICTURE);
    assert_eq!(message.attachments[0].bytes, picture.len() as u64);
    assert!(window.draft().is_empty());
}

#[gpui::test]
fn a_file_taken_away_before_the_send_is_not_in_the_message(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let proxy = world.run(FaultProxy::start(&server));
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Luca");
    window.until("the messages are read", |window| {
        window.feed().is_some_and(|feed| feed.loaded)
    });
    let files = tempfile::tempdir().unwrap();
    let paths: Vec<std::path::PathBuf> = ["keep.txt", "remove.txt"]
        .iter()
        .map(|name| {
            let path = files.path().join(name);
            std::fs::write(&path, name.as_bytes()).unwrap();
            path
        })
        .collect();

    // The person chooses two files in the file dialog.
    window.click("attach-file");
    assert!(window.cx.did_prompt_for_paths());
    let chosen = paths.clone();
    window
        .cx
        .simulate_path_prompt_response(move |_| Some(chosen));
    window.until("both files are uploaded", |window| {
        let draft = window.draft();
        draft.len() == 2
            && draft
                .iter()
                .all(|file| matches!(file.progress.stage, UploadStage::Complete(_)))
    });
    let removed = window
        .draft()
        .into_iter()
        .find(|file| file.name == "remove.txt")
        .unwrap();
    let removed_id = match removed.progress.stage {
        UploadStage::Complete(attachment_id) => attachment_id,
        _ => unreachable!(),
    };
    let index = window
        .draft()
        .iter()
        .position(|file| file.name == "remove.txt")
        .unwrap();

    // Its control takes the second file away from the draft.
    window.click_named(format!("remove-file:{index}"));
    window.until("one file stays in the draft", |window| {
        let draft = window.draft();
        draft.len() == 1 && draft[0].name == "keep.txt"
    });
    assert!(!window.shows_named("draft-file:1".to_owned()));

    window.in_shell(|shell, window, cx| {
        shell.type_message(Pane::Main, "One file", window, cx);
        shell.focus_composer(Pane::Main, window, cx);
    });
    let (id, _) = window.send_composer();
    let message = world.run(admin.handle.api().message(id)).unwrap();
    let names: Vec<&str> = message
        .attachments
        .iter()
        .map(|attachment| attachment.name.as_str())
        .collect();
    assert_eq!(names, ["keep.txt"]);
    // The upload that was taken away is deleted on the server.
    let deletion = format!("/api/v1/attachments/{removed_id}/upload");
    window.until("the server deleted the removed upload", |_| {
        proxy.seen(&deletion).iter().any(|seen| {
            seen.method == "DELETE"
                && seen
                    .answer
                    .as_ref()
                    .is_some_and(|(status, _)| (200..300).contains(status))
        })
    });
}

#[gpui::test]
fn an_upload_goes_on_after_its_connection_was_cut_and_the_draft_says_so(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let proxy = world.run(FaultProxy::start(&server));
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Luca");
    window.until("the messages are read", |window| {
        window.feed().is_some_and(|feed| feed.loaded)
    });

    // Two parts and a rest. The second part does not reach the server at first, and the
    // read of the state of the upload that follows waits at the proxy, so that the draft
    // shows the interruption.
    let megabyte = 1024 * 1024;
    let content: Vec<u8> = (0..2 * megabyte + 777)
        .map(|number: usize| (number % 251) as u8)
        .collect();
    let files = tempfile::tempdir().unwrap();
    let path = files.path().join("recording.wav");
    std::fs::write(&path, &content).unwrap();
    proxy.fail_ending(&format!("/chunks/{megabyte}"), [Fault::Cut]);
    proxy.hold_ending("/upload");
    window.click("attach-file");
    let chosen = path.clone();
    window
        .cx
        .simulate_path_prompt_response(move |_| Some(vec![chosen]));
    window.until("the draft shows the interruption", |window| {
        let draft = window.draft();
        draft.len() == 1
            && draft[0].progress.interrupted
            && matches!(draft[0].progress.stage, UploadStage::Sending)
    });
    assert!(window.shows_named("draft-file:0".to_owned()));

    // The connection works again: the upload goes on without an action of the person.
    proxy.release();
    window.until("the upload is complete", |window| {
        let draft = window.draft();
        draft.len() == 1
            && !draft[0].progress.interrupted
            && matches!(draft[0].progress.stage, UploadStage::Complete(_))
    });
    // The upload went on at the part that did not arrive: the first part was sent once,
    // the cut part twice, the rest once.
    let sent = |offset: usize| {
        let end = format!("/chunks/{offset}");
        proxy
            .all()
            .iter()
            .filter(|seen| seen.path.ends_with(&end))
            .count()
    };
    assert_eq!([sent(0), sent(megabyte), sent(2 * megabyte)], [1, 2, 1]);
    window.in_shell(|shell, window, cx| {
        shell.type_message(Pane::Main, "The recording", window, cx);
        shell.focus_composer(Pane::Main, window, cx);
    });
    let (id, _) = window.send_composer();
    let message = world.run(admin.handle.api().message(id)).unwrap();
    assert_eq!(message.attachments.len(), 1);
    assert_eq!(message.attachments[0].bytes, content.len() as u64);
    let target = tempfile::tempdir().unwrap();
    let saved = world
        .run(
            admin
                .handle
                .transfers()
                .download(&message.attachments[0], target.path().to_path_buf())
                .finished(),
        )
        .unwrap();
    assert_eq!(std::fs::read(saved).unwrap(), content);
}
