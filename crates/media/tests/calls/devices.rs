//! The sound devices and the microphone permission of a person in a call, through the
//! interface of the devices with a fake device list.

use junto_media::audio::synthetic::Tone;
use junto_media::audio::{AudioError, DeviceId};
use junto_media::engine::{MicrophoneState, OutputState};
use junto_media::permission::{FixedPermission, MicrophoneAccess};

use crate::scene::{
    ABSENT_DBFS, HEARD_DBFS, MEDIA_SERVER, Person, Room, account, model_when, plain_voice, until,
};

/// The tones of a microphone that is opened again, apart from every tone of a person.
const AFTER_HZ: f32 = 550.0;
const RETRY_HZ: f32 = 1100.0;

fn id(text: &str) -> Option<DeviceId> {
    Some(DeviceId(text.to_owned()))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
async fn the_chosen_devices_are_opened_and_a_missing_one_gives_way_to_the_default() {
    let _media = MEDIA_SERVER.read().await;
    let (ann, ben) = (Person::new(440.0), Person::new(880.0));
    let (room, ann_at) = Room::new("Acme", &ann, "Ann").await;
    room.member(&ann_at, &ben, "Ben").await;
    ann.host
        .add_input("desk", "Desk microphone", Tone::new(660.0));
    ann.host.add_output("phones", "Headphones");
    let devices = ann.calls.devices().unwrap();
    assert_eq!(devices.inputs.len(), 2);
    assert_eq!(devices.outputs.len(), 2);
    assert!(devices.inputs[0].is_default && !devices.inputs[1].is_default);

    // The devices that were chosen before the call are the ones that the call opens.
    let mut voice = plain_voice();
    voice.input_device = id("desk");
    voice.output_device = id("phones");
    ann.calls.set_voice(voice.clone());
    ann.enter(room.target).await;
    ben.enter(room.target).await;
    let call = ann.calls.view().call.unwrap();
    assert_eq!(call.input_device.unwrap().name, "Desk microphone");
    assert_eq!(call.output_device.unwrap().name, "Headphones");
    assert_eq!(ann.host.opened_inputs(), [id("desk")]);
    assert_eq!(ann.host.opened_outputs(), [id("phones")]);
    ben.until_level(660.0, "the tone of the desk microphone", |level| {
        level > HEARD_DBFS
    })
    .await;

    // A change during the call takes effect in the call.
    voice.input_device = None;
    ann.calls.set_voice(voice.clone());
    until("the default microphone is opened", || {
        ann.host.opened_inputs().len() == 2
    })
    .await;
    ben.until_heard_afresh(&ann).await;
    assert_eq!(ann.host.opened_inputs(), [id("desk"), None]);
    assert_eq!(
        ann.calls.view().call.unwrap().input_device.unwrap().name,
        "Tone microphone"
    );

    // A chosen device that is not connected: the default is opened in its place. The
    // default delivers another tone from now on, so the sound of the stream that was open
    // before does not answer for the new one.
    ann.host.set_tone("tone", AFTER_HZ);
    voice.input_device = id("headset");
    voice.output_device = id("speakers");
    ann.calls.set_voice(voice.clone());
    until("both devices are opened again", || {
        ann.host.opened_inputs().len() == 3 && ann.host.opened_outputs().len() == 2
    })
    .await;
    assert_eq!(ann.host.opened_inputs(), [id("desk"), None, None]);
    assert_eq!(ann.host.opened_outputs(), [id("phones"), None]);
    let call = ann.calls.view().call.unwrap();
    assert_eq!(
        (call.microphone, call.output),
        (MicrophoneState::Running, OutputState::Running)
    );
    ben.until_level(
        AFTER_HZ,
        "the tone of the stream that was opened last",
        |level| level > HEARD_DBFS,
    )
    .await;
    ann.until_heard_afresh(&ben).await;

    // The open microphone is disconnected: the client opens the default again.
    voice.input_device = id("desk");
    ann.calls.set_voice(voice);
    ben.until_level(660.0, "the desk microphone again", |level| {
        level > HEARD_DBFS
    })
    .await;
    let opened = ann.host.opened_inputs().len();
    ann.host.remove_input("desk");
    ann.host.break_input(AudioError::NoDevice);
    until("the default microphone is opened again", || {
        ann.host.opened_inputs().len() == opened + 1
    })
    .await;
    ben.until_level(AFTER_HZ, "the tone of the default microphone", |level| {
        level > HEARD_DBFS
    })
    .await;
    assert_eq!(
        ann.calls.view().call.unwrap().input_device.unwrap().name,
        "Tone microphone"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
async fn a_device_that_fails_again_at_once_stays_closed_until_the_person_acts() {
    let _media = MEDIA_SERVER.read().await;
    let (ann, ben) = (Person::new(440.0), Person::new(880.0));
    let (room, ann_at) = Room::new("Acme", &ann, "Ann").await;
    let ben_at = room.member(&ann_at, &ben, "Ben").await;
    let ann_id = account(&ann_at);
    ann.host.add_output("phones", "Headphones");
    ann.enter(room.target).await;
    ben.enter(room.target).await;
    ben.until_heard(&ann).await;
    ann.until_heard(&ben).await;
    let failure = || AudioError::Failed("the device stopped".to_owned());

    // The microphone stops: the client opens it again, once. It stops again at once: the
    // client does not open it a third time.
    ann.host.set_tone("tone", AFTER_HZ);
    ann.host.break_input(failure());
    until("the microphone is opened again", || {
        ann.host.opened_inputs().len() == 2
    })
    .await;
    ann.host.break_input(failure());
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    assert_eq!(ann.host.opened_inputs().len(), 2);
    let view = ann
        .until("the failure of the microphone is shown", |view| {
            view.call
                .as_ref()
                .is_some_and(|call| matches!(call.microphone, MicrophoneState::Failed(_)))
        })
        .await;
    assert_eq!(
        view.call.unwrap().microphone,
        MicrophoneState::Failed(failure())
    );
    ben.until_silent(&ann).await;
    ben.until_level(AFTER_HZ, "the tone has stopped", |level| {
        level < ABSENT_DBFS
    })
    .await;
    model_when(&ben_at, "Ann is shown as muted", |model| {
        model
            .call_at(&room.target.place)
            .is_some_and(|call| call.participant(ann_id).is_some_and(|ann| ann.muted))
    })
    .await;
    // The person tries again. The microphone delivers a third tone from now on.
    ann.host.set_tone("tone", RETRY_HZ);
    ann.calls.retry_microphone();
    ann.until("the microphone runs", |view| {
        view.call
            .as_ref()
            .is_some_and(|call| call.microphone == MicrophoneState::Running)
    })
    .await;
    assert_eq!(ann.host.opened_inputs().len(), 3);
    ben.until_level(
        RETRY_HZ,
        "the tone of the microphone after the retry",
        |level| level > HEARD_DBFS,
    )
    .await;

    // The same rule for the output.
    ann.host.break_output(failure());
    until("the output is opened again", || {
        ann.host.opened_outputs().len() == 2
    })
    .await;
    ann.host.break_output(failure());
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    assert_eq!(ann.host.opened_outputs().len(), 2);
    let view = ann
        .until("the failure of the output is shown", |view| {
            view.call
                .as_ref()
                .is_some_and(|call| matches!(call.output, OutputState::Failed(_)))
        })
        .await;
    assert_eq!(view.call.unwrap().output_device, None);
    // The person chooses an output.
    let mut voice = plain_voice();
    voice.output_device = id("phones");
    ann.calls.set_voice(voice);
    ann.until("the output runs", |view| {
        view.call
            .as_ref()
            .is_some_and(|call| call.output == OutputState::Running)
    })
    .await;
    assert_eq!(ann.host.opened_outputs(), [None, None, id("phones")]);
    ann.until_heard_afresh(&ben).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
async fn a_person_without_the_microphone_permission_hears_the_call_and_is_shown_as_muted() {
    let _media = MEDIA_SERVER.read().await;
    let refused = FixedPermission::new(MicrophoneAccess::Refused, MicrophoneAccess::Refused);
    let (ann, ben) = (Person::new(440.0), Person::with_permission(880.0, refused));
    let (room, ann_at) = Room::new("Acme", &ann, "Ann").await;
    let ben_at = room.member(&ann_at, &ben, "Ben").await;
    let ben_id = account(&ben_at);
    assert_eq!(ben.calls.microphone_access(), MicrophoneAccess::Refused);
    ann.enter(room.target).await;
    ben.enter(room.target).await;

    let view = ben
        .until("the refusal is shown", |view| {
            view.call
                .as_ref()
                .is_some_and(|call| call.microphone == MicrophoneState::NoPermission)
        })
        .await;
    assert_eq!(view.call.unwrap().access, MicrophoneAccess::Refused);
    // No microphone was opened, and the system was not made to ask.
    assert!(ben.host.opened_inputs().is_empty());
    assert_eq!(ben.permission.asked(), 0);
    ben.until_heard(&ann).await;
    model_when(&ann_at, "Ben is shown as muted", |model| {
        model
            .call_at(&room.target.place)
            .is_some_and(|call| call.participant(ben_id).is_some_and(|ben| ben.muted))
    })
    .await;
    assert!(!ben.calls.view().muted);

    // The person allows the microphone in the settings of the system and tries again.
    ben.permission.set(MicrophoneAccess::Allowed);
    ben.calls.retry_microphone();
    ben.until("the microphone runs", |view| {
        view.call.as_ref().is_some_and(|call| {
            call.microphone == MicrophoneState::Running && call.access == MicrophoneAccess::Allowed
        })
    })
    .await;
    ann.until_heard(&ben).await;
    model_when(&ann_at, "Ben is not muted", |model| {
        model
            .call_at(&room.target.place)
            .is_some_and(|call| call.participant(ben_id).is_some_and(|ben| !ben.muted))
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
async fn the_first_use_of_the_microphone_asks_the_system_once() {
    let _media = MEDIA_SERVER.read().await;
    let asks = FixedPermission::new(MicrophoneAccess::NotAsked, MicrophoneAccess::Allowed);
    let (ann, ben) = (Person::new(440.0), Person::with_permission(880.0, asks));
    let (room, ann_at) = Room::new("Acme", &ann, "Ann").await;
    room.member(&ann_at, &ben, "Ben").await;
    assert_eq!(ben.calls.microphone_access(), MicrophoneAccess::NotAsked);
    assert_eq!(ben.permission.asked(), 0);
    ann.enter(room.target).await;
    ben.enter(room.target).await;
    ann.until_heard(&ben).await;
    assert_eq!(ben.permission.asked(), 1);
    assert_eq!(
        ben.calls.view().call.unwrap().access,
        MicrophoneAccess::Allowed
    );

    // A person who answers "no" is in the call without a microphone.
    let denies = FixedPermission::new(MicrophoneAccess::NotAsked, MicrophoneAccess::Refused);
    let cal = Person::with_permission(1320.0, denies);
    room.member(&ann_at, &cal, "Cal").await;
    cal.enter(room.target).await;
    cal.until("the refusal is shown", |view| {
        view.call
            .as_ref()
            .is_some_and(|call| call.microphone == MicrophoneState::NoPermission)
    })
    .await;
    assert_eq!(cal.permission.asked(), 1);
    assert!(cal.host.opened_inputs().is_empty());
    cal.until_heard(&ann).await;
}
