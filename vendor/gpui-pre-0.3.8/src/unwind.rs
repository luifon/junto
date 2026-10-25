// Added to the vendored copy of gpui-pre 0.3.8; this file is not in the published crate.
//
// The platform calls gpui from functions that cannot unwind: the Objective-C methods and
// the dispatch queue functions of macOS, and the window procedure of Windows. A panic that
// reaches such a frame ends the process. Every function that gpui gives the platform runs
// its body through `catch`, and every gpui frame that takes a value out of the state of the
// application while it calls other code puts that value back when a panic passes through
// it, so that the application can continue after `catch` returns.

use std::panic::{self, AssertUnwindSafe};

/// Runs `body`. When it panics, the panic stops here and `fallback` gives the value that
/// the platform receives. The panic hook of the process has already recorded the panic.
pub(crate) fn catch<R>(
    site: &'static str,
    fallback: impl FnOnce() -> R,
    body: impl FnOnce() -> R,
) -> R {
    match panic::catch_unwind(AssertUnwindSafe(body)) {
        Ok(value) => value,
        Err(_) => {
            log::error!("a panic in the {site} callback was stopped; the application continues");
            fallback()
        }
    }
}

/// Runs `body` with `state`. When it panics, `restore` runs with `state` and the panic
/// continues.
pub(crate) fn restore_on_panic<S: ?Sized, R>(
    state: &mut S,
    body: impl FnOnce(&mut S) -> R,
    restore: impl FnOnce(&mut S),
) -> R {
    match panic::catch_unwind(AssertUnwindSafe(|| body(&mut *state))) {
        Ok(value) => value,
        Err(payload) => {
            restore(state);
            panic::resume_unwind(payload)
        }
    }
}

/// A callback of the platform without a result, whose body runs through `catch`.
pub(crate) fn callback<A>(site: &'static str, mut body: impl FnMut(A) + 'static) -> Box<dyn FnMut(A)> {
    Box::new(move |argument| catch(site, || (), || body(argument)))
}

/// A callback of the platform without arguments and without a result.
pub(crate) fn callback0(site: &'static str, mut body: impl FnMut() + 'static) -> Box<dyn FnMut()> {
    Box::new(move || catch(site, || (), &mut body))
}

/// A callback of the platform with a result: `fallback` gives the result after a panic.
pub(crate) fn callback_with<A, R: 'static>(
    site: &'static str,
    fallback: fn() -> R,
    mut body: impl FnMut(A) -> R + 'static,
) -> Box<dyn FnMut(A) -> R> {
    Box::new(move |argument| catch(site, fallback, || body(argument)))
}

/// A callback of the platform without arguments, with a result.
pub(crate) fn callback0_with<R: 'static>(
    site: &'static str,
    fallback: fn() -> R,
    mut body: impl FnMut() -> R + 'static,
) -> Box<dyn FnMut() -> R> {
    Box::new(move || catch(site, fallback, &mut body))
}

/// A callback of the platform with two arguments, without a result.
pub(crate) fn callback2<A, B>(
    site: &'static str,
    mut body: impl FnMut(A, B) + 'static,
) -> Box<dyn FnMut(A, B)> {
    Box::new(move |first, second| catch(site, || (), || body(first, second)))
}

/// A callback of the platform that runs once.
pub(crate) fn callback_once(site: &'static str, body: impl FnOnce() + 'static) -> Box<dyn FnOnce()> {
    Box::new(move || catch(site, || (), body))
}
