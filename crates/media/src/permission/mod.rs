//! The permission of the operating system to use the microphone.
//!
//! macOS asks the person once, for the application that it holds responsible for the
//! process, and remembers the answer. The engine reads the state before it opens the
//! microphone and never opens a microphone that the person refused; the interface shows the
//! state. A test gives a fixed state, because an automated run cannot answer the question
//! of the system.

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod macos;

use std::future::Future;
use std::pin::Pin;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MicrophoneAccess {
    /// The system has not asked the person yet. The first use of the microphone asks.
    NotAsked,
    /// The person refused, or a rule of the machine forbids it. Only the settings of the
    /// system change this.
    Refused,
    Allowed,
}

pub type Answer = Pin<Box<dyn Future<Output = MicrophoneAccess> + Send + 'static>>;

pub trait MicrophonePermission: Send + Sync + 'static {
    /// The state now. The call shows no question.
    fn state(&self) -> MicrophoneAccess;

    /// Makes the system ask the person when it has not asked yet, and returns the state
    /// after the answer.
    fn request(&self) -> Answer;
}

/// The permission as the operating system holds it. A system without such a permission
/// allows the microphone.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemPermission;

#[cfg(target_os = "macos")]
impl MicrophonePermission for SystemPermission {
    fn state(&self) -> MicrophoneAccess {
        macos::state()
    }

    fn request(&self) -> Answer {
        Box::pin(macos::request())
    }
}

#[cfg(not(target_os = "macos"))]
impl MicrophonePermission for SystemPermission {
    fn state(&self) -> MicrophoneAccess {
        MicrophoneAccess::Allowed
    }

    fn request(&self) -> Answer {
        Box::pin(std::future::ready(MicrophoneAccess::Allowed))
    }
}

/// A permission with a fixed state and a fixed answer to the question.
#[cfg(feature = "test-support")]
#[derive(Debug, Clone)]
pub struct FixedPermission {
    state: std::sync::Arc<std::sync::Mutex<(MicrophoneAccess, MicrophoneAccess, u32)>>,
}

#[cfg(feature = "test-support")]
impl FixedPermission {
    pub fn allowed() -> Self {
        Self::new(MicrophoneAccess::Allowed, MicrophoneAccess::Allowed)
    }

    /// `state` is what the system holds, and `answer` is what the person answers when the
    /// system asks.
    pub fn new(state: MicrophoneAccess, answer: MicrophoneAccess) -> Self {
        Self {
            state: std::sync::Arc::new(std::sync::Mutex::new((state, answer, 0))),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, (MicrophoneAccess, MicrophoneAccess, u32)> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The person changed the permission in the settings of the system.
    pub fn set(&self, state: MicrophoneAccess) {
        self.lock().0 = state;
    }

    /// How often the system was made to ask.
    pub fn asked(&self) -> u32 {
        self.lock().2
    }
}

#[cfg(feature = "test-support")]
impl MicrophonePermission for FixedPermission {
    fn state(&self) -> MicrophoneAccess {
        self.lock().0
    }

    fn request(&self) -> Answer {
        let mut held = self.lock();
        if held.0 == MicrophoneAccess::NotAsked {
            held.2 += 1;
            held.0 = held.1;
        }
        Box::pin(std::future::ready(held.0))
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_system_asks_once_and_keeps_the_answer() {
        let permission =
            FixedPermission::new(MicrophoneAccess::NotAsked, MicrophoneAccess::Refused);
        assert_eq!(permission.state(), MicrophoneAccess::NotAsked);
        assert_eq!(permission.request().await, MicrophoneAccess::Refused);
        assert_eq!(permission.request().await, MicrophoneAccess::Refused);
        assert_eq!(permission.asked(), 1);
        permission.set(MicrophoneAccess::Allowed);
        assert_eq!(permission.request().await, MicrophoneAccess::Allowed);
        assert_eq!(permission.asked(), 1);
        assert_eq!(
            FixedPermission::allowed().state(),
            MicrophoneAccess::Allowed
        );
    }
}
