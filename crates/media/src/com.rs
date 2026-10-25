//! The apartments of COM on Windows (ADR 0706, ADR 0733).
//!
//! No code of the crate, and no code of `cpal` (whose copy in `vendor/` is corrected for it),
//! leaves a thread that it did not create in an apartment. A thread of the system pool, of an
//! asynchronous runtime or of the window would keep that apartment for the next code that
//! runs on it: the activation of the sound of a share fails with `RPC_E_CHANGED_MODE` while
//! a thread of the system pool is in the single-threaded apartment. Code that needs COM uses
//! the multithreaded apartment that `keep_multithreaded` holds for the process, without
//! entering it, or enters an apartment only on a thread that it created and leaves it there
//! after an entry that succeeded.

use std::sync::OnceLock;

use windows::Win32::System::Com::CoIncrementMTAUsage;

/// Keeps the multithreaded apartment for the life of the process. A thread that has not
/// entered an apartment uses it, and the objects made in it stay usable after the thread that
/// made them has ended.
pub(crate) fn keep_multithreaded() -> Result<(), String> {
    static KEPT: OnceLock<Result<(), String>> = OnceLock::new();
    KEPT.get_or_init(|| {
        // SAFETY: the cookie is never given back, so the apartment lasts as long as the
        // process.
        unsafe { CoIncrementMTAUsage() }
            .map(|_| ())
            .map_err(|error| error.message())
    })
    .clone()
}

/// The apartment of the calling thread, for the tests.
#[cfg(any(test, feature = "test-support"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Apartment {
    /// The thread entered no apartment, and the process has no multithreaded apartment.
    None,
    /// The thread entered no apartment, and uses the multithreaded apartment of the process.
    ImplicitMultithreaded,
    Multithreaded,
    SingleThreaded,
    /// Another answer of the system, with its code.
    Other(i32),
}

#[cfg(any(test, feature = "test-support"))]
impl Apartment {
    /// Whether the thread has entered an apartment.
    pub fn entered(self) -> bool {
        !matches!(self, Self::None | Self::ImplicitMultithreaded)
    }
}

/// The apartment of the calling thread.
#[cfg(any(test, feature = "test-support"))]
pub fn apartment() -> Apartment {
    use windows::Win32::Foundation::CO_E_NOTINITIALIZED;
    use windows::Win32::System::Com::{
        APTTYPE, APTTYPE_MAINSTA, APTTYPE_MTA, APTTYPE_STA, APTTYPEQUALIFIER,
        APTTYPEQUALIFIER_IMPLICIT_MTA, CoGetApartmentType,
    };
    let mut kind = APTTYPE::default();
    let mut qualifier = APTTYPEQUALIFIER::default();
    // SAFETY: the call writes the two values.
    match unsafe { CoGetApartmentType(&mut kind, &mut qualifier) } {
        Err(error) if error.code() == CO_E_NOTINITIALIZED => Apartment::None,
        Ok(()) if kind == APTTYPE_MTA && qualifier == APTTYPEQUALIFIER_IMPLICIT_MTA => {
            Apartment::ImplicitMultithreaded
        }
        Ok(()) if kind == APTTYPE_MTA => Apartment::Multithreaded,
        Ok(()) if kind == APTTYPE_STA || kind == APTTYPE_MAINSTA => Apartment::SingleThreaded,
        Ok(()) => Apartment::Other(kind.0),
        Err(error) => Apartment::Other(error.code().0),
    }
}
