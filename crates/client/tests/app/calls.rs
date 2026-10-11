//! The controls of calls that need no media server: the person's own microphone and sound
//! settings, the sound settings of the application, and a join that the server refuses.

use gpui::TestAppContext;
use junto_client::state::view::{CallNotice, MicrophoneButton, Relation};
use junto_client_core::flow::FlowError;
use junto_client_core::protocol::error::ErrorCode;
use junto_client_core::protocol::structure::{ChannelKind, CreateChannel};
use junto_client_core::voice::{InputMode, TalkKey, VoiceSettings};

use crate::harness::{Admin, Window, World, member, open};

pub fn voice_channel(world: &World, admin: &Admin, window: &mut Window, name: &str) {
    let workspace_id = window.view().workspace().unwrap().id;
    world
        .run(admin.handle.api().create_channel(
            workspace_id,
            &CreateChannel {
                name: name.to_owned(),
                kind: ChannelKind::Voice,
                category_id: None,
                position: None,
            },
        ))
        .unwrap();
    let name = name.to_owned();
    window.until("the voice channel is listed", move |window| {
        window.shows_channel(&name)
    });
}

fn relation(window: &mut Window, name: &str) -> Relation {
    window
        .channels()
        .into_iter()
        .find(|row| row.name == name)
        .and_then(|row| row.voice)
        .map(|voice| voice.relation)
        .unwrap()
}

#[gpui::test]
fn a_join_that_the_server_refuses_is_told_and_leaves_the_person_outside(cx: &mut TestAppContext) {
    let world = World::new();
    // The server has no media server: it gives no place in a call.
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    voice_channel(&world, &admin, &mut window, "Daily");

    // A click on a voice channel shows it and joins nothing.
    window.click_channel("Daily");
    assert!(window.shows("join-call") && window.shows("voice-chat-not-available"));
    assert_eq!(relation(&mut window, "Daily"), Relation::Outside);
    assert!(!window.shows("call-strip") && !window.shows("call-stage"));

    window.click("join-call");
    window.until("the refusal is told", |window| window.shows("call-notice"));
    let view = window.view();
    assert!(
        matches!(
            &view.call_notice,
            Some(CallNotice::JoinFailed { channel, error: FlowError::Refused { code, .. } })
                if channel.as_deref() == Some("Daily") && *code == ErrorCode::MediaUnavailable
        ),
        "{:?}",
        view.call_notice
    );
    assert!(view.call.is_none());
    assert_eq!(relation(&mut window, "Daily"), Relation::Outside);
    assert!(window.shows("join-call") && !window.shows("call-strip"));
    // No microphone was opened for a call that did not begin.
    assert!(window.sound.opened_inputs().is_empty());
    assert!(window.sound.opened_outputs().is_empty());

    // The notice stays while another channel is shown, and goes with its button.
    window.click_channel("general");
    assert!(window.shows("call-notice"));
    window.click("dismiss-call-notice");
    window.until("the notice is gone", |window| !window.shows("call-notice"));
    assert_eq!(window.view().call_notice, None);
}

#[gpui::test]
fn the_microphone_and_the_sound_button_hold_the_setting_of_the_person_outside_a_call(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let controls = window.view().controls;
    assert_eq!(controls.microphone, MicrophoneButton::Open);
    assert!(!controls.sound_off && controls.warning.is_none());

    window.click("microphone");
    window.until("the microphone is closed", |window| {
        window.view().controls.microphone == MicrophoneButton::Muted
    });
    window.click("sound-output");
    window.until("the sound is off", |window| {
        window.view().controls.sound_off
    });
    // The keyboard reaches both buttons.
    window.click("microphone");
    window.until("the microphone is open", |window| {
        window.view().controls.microphone == MicrophoneButton::Open
    });
    assert!(window.view().controls.sound_off);
    assert!(window.sound.opened_inputs().is_empty());
}

fn voice_file(directory: &std::path::Path) -> std::path::PathBuf {
    directory.join("junto").join("voice.json")
}

#[gpui::test]
fn the_sound_settings_of_the_application_are_kept_over_a_restart(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let voice = |window: &mut Window| -> VoiceSettings {
        window
            .state
            .read_with(&window.cx, |state, _| state.voice().clone())
    };
    assert_eq!(voice(&mut window), VoiceSettings::default());
    assert!(!voice_file(directory.path()).exists());

    window.click("app-settings");
    assert!(window.shows("settings-dialog"));
    // Voice activation has no key to choose.
    assert!(window.shows("input-mode-0") && !window.shows("talk-key-0"));
    window.click("input-mode-1");
    window.until("the key of push-to-talk is offered", |window| {
        window.shows("talk-key-1")
    });
    window.click("talk-key-1");
    window.click("noise-suppression");
    window.click("echo-cancellation");
    window.click("echo-cancellation");
    window.until("the settings are used", |window| {
        let voice = voice(window);
        voice.input_mode == InputMode::PushToTalk
            && voice.talk_key == TalkKey::Control
            && !voice.noise_suppression
            && voice.echo_cancellation
            && voice.automatic_gain
    });
    assert!(!window.shows("voice-settings-error"));
    window.press("escape");
    let chosen = voice(&mut window);
    window.close();

    // The file holds what was chosen, and a new start reads it.
    let text = std::fs::read_to_string(voice_file(directory.path())).unwrap();
    assert!(text.contains("\"input_mode\": \"push_to_talk\""), "{text}");
    assert!(text.contains("\"talk_key\": \"control\""), "{text}");
    let mut window = open(cx, directory.path());
    assert_eq!(voice(&mut window), chosen);
    window.click("app-settings");
    assert!(window.shows("talk-key-1"));
}
