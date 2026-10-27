//! Whether the system lets the application show notifications, and the question of macOS
//! (ADR 0708). macOS keeps an answer of the person per application; Windows keeps an on
//! and off state per application and for every application; Linux keeps none that a
//! program outside a sandbox can read.

use std::future::Future;
use std::pin::Pin;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationAccess {
    /// The system has not asked the person yet.
    NotAsked,
    Allowed,
    /// The person refused, or turned the notifications of the application off. Only the
    /// settings of the system change it.
    Refused,
}

pub type Answer = Pin<Box<dyn Future<Output = NotificationAccess> + Send + 'static>>;

/// The state now. It shows no question. On macOS the process must run as an application
/// bundle: the notification centre of a process without one stops the process.
pub fn read() -> Answer {
    #[cfg(target_os = "macos")]
    {
        macos::read()
    }
    #[cfg(windows)]
    {
        let state = windows::read();
        Box::pin(std::future::ready(state))
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        Box::pin(std::future::ready(NotificationAccess::Allowed))
    }
}

/// Makes macOS ask the person when it has not asked yet, and returns the state after the
/// answer. Windows and Linux ask nothing: the state now.
pub fn request() -> Answer {
    #[cfg(target_os = "macos")]
    {
        macos::request()
    }
    #[cfg(not(target_os = "macos"))]
    {
        read()
    }
}

// The answer of the notification centre is a pointer that the system gives to the block.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod macos {
    use std::ptr::NonNull;
    use std::sync::Mutex;

    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_foundation::NSError;
    use objc2_user_notifications::{
        UNAuthorizationOptions, UNAuthorizationStatus, UNNotificationSettings,
        UNUserNotificationCenter,
    };
    use tokio::sync::oneshot;

    use super::{Answer, NotificationAccess};

    fn of(status: UNAuthorizationStatus) -> NotificationAccess {
        if status == UNAuthorizationStatus::NotDetermined {
            NotificationAccess::NotAsked
        } else if status == UNAuthorizationStatus::Denied {
            NotificationAccess::Refused
        } else {
            NotificationAccess::Allowed
        }
    }

    pub(super) fn read() -> Answer {
        let (sender, answer) = oneshot::channel();
        let sender = Mutex::new(Some(sender));
        let handler = RcBlock::new(move |settings: NonNull<UNNotificationSettings>| {
            // SAFETY: the framework gives a valid settings object for the time of the call.
            let status = unsafe { settings.as_ref() }.authorizationStatus();
            if let Ok(mut sender) = sender.lock()
                && let Some(sender) = sender.take()
            {
                let _ = sender.send(of(status));
            }
        });
        UNUserNotificationCenter::currentNotificationCenter()
            .getNotificationSettingsWithCompletionHandler(&handler);
        Box::pin(async move { answer.await.unwrap_or(NotificationAccess::Refused) })
    }

    pub(super) fn request() -> Answer {
        let (sender, answer) = oneshot::channel();
        let sender = Mutex::new(Some(sender));
        let handler = RcBlock::new(move |_granted: Bool, error: *mut NSError| {
            if !error.is_null() {
                tracing::warn!("macOS did not answer the request for notifications");
            }
            if let Ok(mut sender) = sender.lock()
                && let Some(sender) = sender.take()
            {
                let _ = sender.send(());
            }
        });
        UNUserNotificationCenter::currentNotificationCenter()
            .requestAuthorizationWithOptions_completionHandler(
                UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
                &handler,
            );
        Box::pin(async move {
            let _ = answer.await;
            read().await
        })
    }
}

#[cfg(windows)]
mod windows {
    use ::windows::UI::Notifications::{NotificationSetting, ToastNotificationManager};
    use ::windows::core::HSTRING;

    use super::NotificationAccess;

    /// The setting of the identifier of the application (ADR 0742) and of every application
    /// together. A read that fails is "allowed": gpui then shows the notification, and
    /// Windows decides.
    pub(super) fn read() -> NotificationAccess {
        match ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(
            crate::app_identity::APP_ID,
        ))
        .and_then(|notifier| notifier.Setting())
        {
            Ok(NotificationSetting::Enabled) | Err(_) => NotificationAccess::Allowed,
            Ok(_) => NotificationAccess::Refused,
        }
    }
}
