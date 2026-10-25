//! The bodies of the functions that code of the system calls: C, C++ and Objective-C, which a
//! Rust panic cannot unwind through. A panic in such a body stops here, and the function
//! returns a fixed value to the system (ADR 0757).

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Mutex, PoisonError};

static STOPPED: Mutex<Option<HashMap<&'static str, usize>>> = Mutex::new(None);

/// Runs `body`; after a panic, logs it and returns `fallback()`.
pub(crate) fn stop_panic<R>(
    site: &'static str,
    fallback: impl FnOnce() -> R,
    body: impl FnOnce() -> R,
) -> R {
    match catch_unwind(AssertUnwindSafe(body)) {
        Ok(value) => value,
        Err(_) => {
            let count = {
                let mut stopped = STOPPED.lock().unwrap_or_else(PoisonError::into_inner);
                let count = stopped
                    .get_or_insert_with(HashMap::new)
                    .entry(site)
                    .or_insert(0);
                *count += 1;
                *count
            };
            // The 1st, 10th, 100th, ... panic of each site: a callback of a device runs
            // many times a second.
            if is_a_power_of_ten(count) {
                tracing::error!(
                    site,
                    count,
                    "a panic in a callback of the system was stopped"
                );
            }
            fallback()
        }
    }
}

/// Runs the work that follows a stopped panic, such as the end of a share, under a stop of its
/// own: a fallback of [`stop_panic`] runs outside the stop of its body.
pub(crate) fn after_a_panic(site: &'static str, work: impl FnOnce()) {
    stop_panic(site, || (), work);
}

fn is_a_power_of_ten(mut count: usize) -> bool {
    while count >= 10 && count.is_multiple_of(10) {
        count /= 10;
    }
    count == 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panic_in_the_work_after_a_panic_is_stopped() {
        let fallback = stop_panic(
            "test",
            || {
                after_a_panic("test after", || panic!("a panic of the test after a panic"));
                2
            },
            || panic!("a panic of the test"),
        );
        assert_eq!(fallback, 2);
    }

    #[test]
    fn a_panic_in_a_body_gives_the_fallback() {
        assert_eq!(stop_panic("test", || 0, || 1), 1);
        assert_eq!(
            stop_panic("test", || 0, || panic!("a panic of the test")),
            0
        );
    }
}
