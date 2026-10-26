//! The permissions of the operating system in the window: the section "Permissions" of the
//! settings, the question of the system, and the page of its settings (ADR 0708). The
//! permissions of the test are fixed ones that the test changes as a person does in the
//! settings of the system; the window decides with the rules of each system.

use gpui::TestAppContext;
use junto_client::permissions::{Access, Permission, System};
use junto_client::shell::SettingsSection;
use junto_client_core::notification_access::NotificationAccess;
use junto_media::camera::CameraAccess;
use junto_media::permission::MicrophoneAccess;
use junto_media::share::ScreenAccess;

use crate::harness::{Window, named};

fn rules(window: &mut Window, system: System) {
    window.state.update(&mut window.cx, |state, cx| {
        state.use_permission_rules_of(system, cx);
    });
}

fn access(window: &mut Window, permission: Permission) -> Access {
    window
        .state
        .read_with(&window.cx, |state, _| state.permission_access(permission))
}

fn open_permissions(window: &mut Window) {
    window.click("app-settings");
    window.click(SettingsSection::Permissions.selector());
    window.until("the permissions are read", |window| {
        access(window, Permission::Camera) != Access::Unread
    });
}

#[gpui::test]
fn macos_lists_each_permission_asks_for_one_not_asked_and_opens_the_page_of_a_refused_one(
    cx: &mut TestAppContext,
) {
    let (_directory, mut window) = named(cx, "Luca");
    rules(&mut window, System::MacOs);
    window.permission.set(MicrophoneAccess::NotAsked);
    window.camera.set_access(CameraAccess::Refused);
    window.capture.set_access(ScreenAccess::NotAllowed);
    let notifications = window
        .state
        .read_with(&window.cx, |state, _| state.fixed_notification_permission());
    notifications.set(NotificationAccess::NotAsked);
    open_permissions(&mut window);

    for row in [
        "permission-microphone",
        "permission-camera",
        "permission-screen",
        "permission-notifications",
        "permission-downloads",
        "permission-local-network",
    ] {
        assert!(window.shows(row), "{row}");
    }
    assert!(window.shows("check-permissions"));

    // A permission that macOS has not asked for: the button makes macOS ask.
    window.click_in_settings("permission-action-microphone");
    window.until("the microphone is allowed", |window| {
        access(window, Permission::Microphone) == Access::Allowed
    });
    assert!(!window.shows("permission-action-microphone"));
    window.click_in_settings("permission-action-notifications");
    window.until("the notifications are allowed", |window| {
        access(window, Permission::Notifications) == Access::Allowed
    });
    assert_eq!(notifications.asked(), 1);

    // A refused permission: the button opens its page of System Settings.
    window.click_in_settings("permission-action-camera");
    assert_eq!(
        window.cx.opened_url().as_deref(),
        Some("x-apple.systempreferences:com.apple.preference.security?Privacy_Camera")
    );
    window.click_in_settings("permission-action-downloads");
    assert_eq!(
        window.cx.opened_url().as_deref(),
        Some("x-apple.systempreferences:com.apple.preference.security?Privacy_FilesAndFolders")
    );
    window.click_in_settings("permission-action-local-network");
    assert_eq!(
        window.cx.opened_url().as_deref(),
        Some("x-apple.systempreferences:com.apple.preference.security?Privacy_LocalNetwork")
    );
    // The screen: macOS first lists Junto on its page, and asks when it has not asked.
    window.click_in_settings("permission-action-screen");
    assert_eq!(
        (window.capture.asked(), window.capture.settings_opened()),
        (1, 1)
    );

    // The person turned the camera on in System Settings and comes back: a new read shows
    // it without a button.
    window.camera.set_access(CameraAccess::Allowed);
    window.click_in_settings("check-permissions");
    window.until("the camera is allowed", |window| {
        access(window, Permission::Camera) == Access::Allowed
    });
    assert!(!window.shows("permission-action-camera"));

    // Input Monitoring is listed while push to talk reads the key while another
    // application is in front. Its button makes macOS list Junto and ask once, and opens
    // its page.
    assert!(!window.shows("permission-input-monitoring"));
    let input = window
        .state
        .read_with(&window.cx, |state, _| state.fixed_input_monitoring());
    input.set(false);
    window.state.update(&mut window.cx, |state, cx| {
        state.set_input_mode(junto_client_core::voice::InputMode::PushToTalk, cx);
        state.read_permissions(cx);
    });
    window.until("Input Monitoring is listed", |window| {
        window.shows("permission-input-monitoring")
    });
    window.click_in_settings("permission-action-input-monitoring");
    assert_eq!(input.asked(), 1);
    assert_eq!(
        window.cx.opened_url().as_deref(),
        Some("x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent")
    );
}

#[gpui::test]
fn windows_lists_its_three_settings_and_opens_the_privacy_page_of_one_that_is_off(
    cx: &mut TestAppContext,
) {
    let (_directory, mut window) = named(cx, "Luca");
    rules(&mut window, System::Windows);
    window.permission.set(MicrophoneAccess::Refused);
    window.camera.set_access(CameraAccess::Refused);
    let notifications = window
        .state
        .read_with(&window.cx, |state, _| state.fixed_notification_permission());
    notifications.set(NotificationAccess::Refused);
    open_permissions(&mut window);

    for row in [
        "permission-microphone",
        "permission-camera",
        "permission-notifications",
    ] {
        assert!(window.shows(row), "{row}");
    }
    for row in [
        "permission-screen",
        "permission-downloads",
        "permission-local-network",
    ] {
        assert!(!window.shows(row), "{row}");
    }
    window.click_in_settings("permission-action-microphone");
    assert_eq!(
        window.cx.opened_url().as_deref(),
        Some("ms-settings:privacy-microphone")
    );
    window.click_in_settings("permission-action-camera");
    assert_eq!(
        window.cx.opened_url().as_deref(),
        Some("ms-settings:privacy-webcam")
    );

    // The section of the notifications states the setting of the system with its button.
    window.click(SettingsSection::Notifications.selector());
    assert!(window.shows("notification-permission"));
    window.click("notifications-permission-action");
    assert_eq!(
        window.cx.opened_url().as_deref(),
        Some("ms-settings:notifications")
    );
    notifications.set(NotificationAccess::Allowed);
    window.click(SettingsSection::Permissions.selector());
    window.click(SettingsSection::Notifications.selector());
    window.until("the notifications are allowed", |window| {
        access(window, Permission::Notifications) == Access::Allowed
    });
    assert!(!window.shows("notification-permission"));
}

#[gpui::test]
fn linux_states_the_camera_devices_and_the_question_of_the_desktop_without_a_page(
    cx: &mut TestAppContext,
) {
    let (_directory, mut window) = named(cx, "Luca");
    rules(&mut window, System::Linux);
    window.camera.set_access(CameraAccess::Restricted);
    window.capture.set_access(ScreenAccess::AskOnStart);
    open_permissions(&mut window);

    assert!(window.shows("permission-camera") && window.shows("permission-screen"));
    assert!(!window.shows("permission-microphone"));
    assert_eq!(
        access(&mut window, Permission::Screen),
        Access::AskedAtEachUse
    );
    // Linux has no page of settings for a program outside a sandbox: the sentence says
    // what to do, and no button opens anything.
    assert!(!window.shows("permission-action-camera"));
    assert!(!window.shows("permission-action-screen"));
    assert_eq!(window.cx.opened_url(), None);
}

#[gpui::test]
fn the_question_for_the_notifications_is_asked_once_at_the_start(cx: &mut TestAppContext) {
    let (_directory, mut window) = named(cx, "Luca");
    let notifications = window
        .state
        .read_with(&window.cx, |state, _| state.fixed_notification_permission());
    notifications.set(NotificationAccess::NotAsked);
    window.state.update(&mut window.cx, |state, cx| {
        state.ask_for_notifications_once(cx);
    });
    window.until("the notifications are allowed", |window| {
        access(window, Permission::Notifications) == Access::Allowed
    });
    assert_eq!(notifications.asked(), 1);
    // A refusal is not asked again: macOS shows its question once.
    notifications.set(NotificationAccess::Refused);
    window.state.update(&mut window.cx, |state, cx| {
        state.ask_for_notifications_once(cx);
    });
    window.until("the refusal is read", |window| {
        access(window, Permission::Notifications) == Access::Refused
    });
    assert_eq!(notifications.asked(), 1);
}
