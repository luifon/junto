//! The permissions of the operating system in the state of the application: what is known
//! of each, the question of the system, and the page of its settings (ADR 0708).
use std::{collections::HashMap, rc::Rc};

use gpui::Context;
use junto_client_core::transfers::{DownloadStage, TransferFailure};

use super::AppState;
use crate::notify::NotificationPermission;
use crate::permissions::{self, Access, Action, Permission, PermissionRow, System};

pub(super) struct PermissionsState {
    notifications: Rc<dyn NotificationPermission>,
    /// The fixed notification permission of a start that does not use the system.
    fixed: crate::notify::FixedNotificationPermission,
    input_source: Rc<dyn permissions::InputMonitoring>,
    /// The fixed Input Monitoring permission of a start that does not use the system.
    fixed_input: permissions::FixedInputMonitoring,
    camera: Option<Access>,
    screen: Option<Access>,
    notification: Option<Access>,
    ask_notifications_once: bool,
    input: Option<bool>,
    order: HashMap<Permission, PermissionOrder>,
    /// The system of the rules. A test of the window names another one.
    system: System,
}

#[derive(Default)]
pub(super) struct PermissionOrder {
    generation: u64,
    asking: bool,
}

impl PermissionOrder {
    pub(super) fn read(&mut self) -> Option<u64> {
        if self.asking {
            return None;
        }
        self.generation += 1;
        Some(self.generation)
    }

    fn request(&mut self) -> u64 {
        self.generation += 1;
        self.asking = true;
        self.generation
    }

    pub(super) fn accepts(&self, read: Option<u64>) -> bool {
        !self.asking && read == Some(self.generation)
    }

    fn finish_request(&mut self, request: u64) -> bool {
        if self.generation != request {
            return false;
        }
        self.asking = false;
        true
    }
}

impl PermissionsState {
    pub(super) fn order(&mut self, permission: Permission) -> &mut PermissionOrder {
        self.order.entry(permission).or_default()
    }

    pub fn new() -> Self {
        let fixed = crate::notify::FixedNotificationPermission::default();
        let fixed_input = permissions::FixedInputMonitoring::default();
        fixed_input.set(true);
        Self {
            notifications: Rc::new(fixed.clone()),
            fixed,
            input_source: Rc::new(fixed_input.clone()),
            fixed_input,
            camera: None,
            screen: None,
            notification: None,
            ask_notifications_once: false,
            input: None,
            order: HashMap::new(),
            system: System::this(),
        }
    }
}

impl AppState {
    /// Uses the notification and Input Monitoring permissions of the system. Only the
    /// start of the application does: a test and the screen tool keep fixed ones.
    pub fn use_system_permissions(&mut self, cx: &mut Context<Self>) {
        self.permissions.notifications = Rc::new(crate::notify::PlatformPermission);
        self.permissions.input_source = Rc::new(permissions::SystemInputMonitoring);
        self.read_permissions(cx);
    }

    /// The fixed Input Monitoring permission of a start that does not use the system.
    pub fn fixed_input_monitoring(&self) -> permissions::FixedInputMonitoring {
        self.permissions.fixed_input.clone()
    }

    /// Whether the client reads keys while another application is in front: the key of
    /// push to talk. macOS needs Input Monitoring for it.
    pub fn uses_keys_in_background(&self) -> bool {
        self.known.media.voice.input_mode == junto_client_core::voice::InputMode::PushToTalk
    }

    /// The fixed notification permission of a start that does not use the system.
    pub fn fixed_notification_permission(&self) -> crate::notify::FixedNotificationPermission {
        self.permissions.fixed.clone()
    }

    /// Draws and decides with the rules of another system. A test of the window uses it.
    #[cfg(feature = "test-support")]
    pub fn use_permission_rules_of(&mut self, system: System, cx: &mut Context<Self>) {
        self.permissions.system = system;
        cx.notify();
    }

    pub fn permission_system(&self) -> System {
        self.permissions.system
    }

    /// What the client knows of a permission now.
    pub fn permission_access(&self, permission: Permission) -> Access {
        let known = &self.permissions;
        match permission {
            // The engine reads the permission when it opens the microphone of a call.
            Permission::Microphone => self
                .media_calls()
                .call
                .as_ref()
                .map(|call| call.access)
                .or(self.known.media.access)
                .map(Access::from)
                .unwrap_or(Access::Unread),
            // A camera that the system refused in the call is the newest answer.
            Permission::Camera => self
                .media_calls()
                .call
                .as_ref()
                .and_then(|call| match call.camera {
                    junto_media::camera::CameraState::Failed(
                        junto_media::camera::CameraError::NoPermission(access),
                    ) => Some(Access::from(access)),
                    _ => None,
                })
                .or(known.camera)
                .unwrap_or(Access::Unread),
            Permission::Screen => known.screen.unwrap_or(Access::Unread),
            Permission::Notifications => known.notification.unwrap_or(Access::Unread),
            Permission::Downloads => {
                if self.downloads_refused() {
                    Access::Refused
                } else {
                    Access::Unread
                }
            }
            Permission::LocalNetwork => {
                if self.local_network_refused() {
                    Access::Refused
                } else {
                    Access::Unread
                }
            }
            // macOS does not tell a permission that it never asked for from a refused one.
            Permission::InputMonitoring => match known.input {
                Some(true) => Access::Allowed,
                Some(false) => Access::Refused,
                None => Access::Unread,
            },
        }
    }

    /// What a permission needs from the person, when it needs something.
    pub fn permission_action(&self, permission: Permission) -> Option<Action> {
        permissions::action(
            self.permissions.system,
            permission,
            self.permission_access(permission),
        )
    }

    /// Every permission that this system keeps for the client, as the settings show them.
    pub fn permission_rows(&self) -> Vec<PermissionRow> {
        self.permissions
            .system
            .permissions()
            .iter()
            .filter(|permission| {
                **permission != Permission::InputMonitoring || self.uses_keys_in_background()
            })
            .map(|permission| PermissionRow {
                permission: *permission,
                access: self.permission_access(*permission),
                action: self.permission_action(*permission),
            })
            .collect()
    }

    /// A file was not saved into the download directory because the system refused the
    /// write. macOS asks for this directory at the first write (ADR 0708).
    pub fn downloads_refused(&self) -> bool {
        self.known.downloads.values().any(|progress| {
            matches!(
                progress.stage,
                DownloadStage::Failed(TransferFailure::File(std::io::ErrorKind::PermissionDenied))
            )
        })
    }

    /// A server on the local network was not reached because macOS refused the connection
    /// (ADR 0708).
    pub fn local_network_refused(&self) -> bool {
        self.client
            .servers()
            .iter()
            .any(|server| server.local_network_refused())
    }

    /// Reads every permission again without a question of the system: at the start, when a
    /// window of the application becomes active, and when the settings show them. A person
    /// changes them in the settings of the system and comes back.
    pub fn read_permissions(&mut self, cx: &mut Context<Self>) {
        let microphone_read = self.permissions.order(Permission::Microphone).read();
        let camera_read = self.permissions.order(Permission::Camera).read();
        let screen_read = self.permissions.order(Permission::Screen).read();
        let notification_read = self.permissions.order(Permission::Notifications).read();
        let calls = self.calls.clone();
        let reading = cx.background_executor().spawn(async move {
            (
                calls.microphone_access(),
                calls.camera_access(),
                calls.screen_access(),
            )
        });
        let notification = self.permissions.notifications.read();
        // The read of macOS is one call without a wait.
        let input = Some(self.permissions.input_source.allowed());
        if self.permissions.input != input {
            self.permissions.input = input;
            cx.notify();
        }
        cx.spawn(async move |this, cx| {
            let (microphone, camera, screen) = reading.await;
            let notification = notification.await;
            let _ = this.update(cx, |this, cx| {
                let before = (
                    this.known.media.access,
                    this.permissions.camera,
                    this.permissions.screen,
                    this.permissions.notification,
                );
                if this
                    .permissions
                    .order(Permission::Microphone)
                    .accepts(microphone_read)
                {
                    this.known.media.access = Some(microphone);
                }
                if this
                    .permissions
                    .order(Permission::Camera)
                    .accepts(camera_read)
                {
                    this.permissions.camera = Some(camera.into());
                }
                if this
                    .permissions
                    .order(Permission::Screen)
                    .accepts(screen_read)
                {
                    this.permissions.screen = Some(permissions::screen_access(screen));
                }
                if this
                    .permissions
                    .order(Permission::Notifications)
                    .accepts(notification_read)
                {
                    this.permissions.notification = Some(notification.into());
                    if std::mem::take(&mut this.permissions.ask_notifications_once)
                        && notification
                            == junto_client_core::notification_access::NotificationAccess::NotAsked
                    {
                        this.ask_permission(Permission::Notifications, cx);
                    }
                }
                // A read that finds what the state holds changes nothing, so the window is
                // not drawn again for it.
                if before
                    != (
                        this.known.media.access,
                        this.permissions.camera,
                        this.permissions.screen,
                        this.permissions.notification,
                    )
                {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Makes the system ask the person for a permission. The question of macOS is shown
    /// once; after an answer the person changes it in the settings of the system.
    pub fn ask_permission(&mut self, permission: Permission, cx: &mut Context<Self>) {
        match permission {
            Permission::Microphone => {
                let request = self.permissions.order(permission).request();
                let asking = self.calls.request_microphone_access();
                cx.spawn(async move |this, cx| {
                    let access = asking.await;
                    let _ = this.update(cx, |this, cx| {
                        if !this.permissions.order(permission).finish_request(request) {
                            return;
                        }
                        this.known.media.access = Some(access);
                        cx.notify();
                    });
                })
                .detach();
            }
            Permission::Camera => {
                let request = self.permissions.order(permission).request();
                let asking = self.calls.request_camera_access();
                cx.spawn(async move |this, cx| {
                    let access = asking.await;
                    let _ = this.update(cx, |this, cx| {
                        if !this.permissions.order(permission).finish_request(request) {
                            return;
                        }
                        this.permissions.camera = Some(access.into());
                        cx.notify();
                    });
                })
                .detach();
            }
            Permission::Screen => {
                self.calls.request_screen_access();
                self.read_permissions(cx);
            }
            Permission::Notifications => {
                self.permissions.ask_notifications_once = false;
                let request = self.permissions.order(permission).request();
                let asking = self.permissions.notifications.request();
                cx.spawn(async move |this, cx| {
                    let access = asking.await;
                    let _ = this.update(cx, |this, cx| {
                        if !this.permissions.order(permission).finish_request(request) {
                            return;
                        }
                        this.permissions.notification = Some(access.into());
                        cx.notify();
                    });
                })
                .detach();
            }
            Permission::InputMonitoring => {
                self.permissions.input = Some(self.permissions.input_source.request());
                cx.notify();
            }
            // The system asks at the first use and has no question of its own.
            Permission::Downloads | Permission::LocalNetwork => {}
        }
    }

    /// At the start: makes the system ask for the notifications when it has not asked yet,
    /// as Discord and Slack do at their first start. The other permissions are asked for
    /// at their first use.
    pub fn ask_for_notifications_once(&mut self, cx: &mut Context<Self>) {
        self.permissions.ask_notifications_once = true;
        self.read_permissions(cx);
    }

    /// Opens the page of the settings of the system that holds a permission. For the
    /// screen and Input Monitoring, macOS first lists the client on that page and asks
    /// once: it does not tell a permission that it never asked for from a refused one.
    pub fn open_permission_settings(&mut self, permission: Permission, cx: &mut Context<Self>) {
        if permission == Permission::Screen && self.permissions.system == System::MacOs {
            self.calls.ask_for_screen_access();
            return;
        }
        if permission == Permission::InputMonitoring {
            self.permissions.input = Some(self.permissions.input_source.request());
        }
        if let Some(page) = self.permissions.system.settings_page(permission) {
            cx.open_url(page);
        }
    }

    /// The action of a permission: the question of the system, or its settings.
    pub fn act_on_permission(&mut self, permission: Permission, cx: &mut Context<Self>) {
        match self.permission_action(permission) {
            Some(Action::Ask) => self.ask_permission(permission, cx),
            Some(Action::OpenSettings) => self.open_permission_settings(permission, cx),
            None => {}
        }
    }
}

#[cfg(feature = "test-support")]
impl AppState {
    pub fn use_notification_permission_for_test(&mut self, source: Rc<dyn NotificationPermission>) {
        self.permissions.notifications = source;
    }

    /// Draws these states of the permissions with the rules of a system, in place of the
    /// ones that a read gives. The screen tool uses it.
    pub fn show_permissions_fixture(
        &mut self,
        system: System,
        microphone: junto_media::permission::MicrophoneAccess,
        camera: Access,
        screen: Access,
        notifications: Access,
        cx: &mut Context<Self>,
    ) {
        // A read that is on its way does not replace the fixture.
        for order in self.permissions.order.values_mut() {
            order.generation += 1;
            order.asking = false;
        }
        self.permissions.system = system;
        self.known.media.access = Some(microphone);
        self.permissions.camera = Some(camera);
        self.permissions.screen = Some(screen);
        self.permissions.notification = Some(notifications);
        self.permissions.input = Some(false);
        cx.notify();
    }
}
