//! The condition waits of macOS for the code that is linked statically into the program
//! (ADR 0786).
//!
//! A condition wait of macOS can return `EINVAL` for a valid condition and a valid mutex:
//! the kernel refuses the wait. The WebRTC library ends the wait of its event on that
//! error, and its blocking call does not read the result: the call returns before its task
//! ran, and the task later uses the memory of a stack frame that has ended.
//!
//! This module defines `pthread_cond_wait` and `pthread_cond_timedwait` under the names
//! that the system gives them. The linker binds every caller that is linked statically into
//! the program to these definitions. Each function calls the function of the system and
//! changes one result: `EINVAL` is returned as 0, a wake-up without a signal, which the
//! caller of a condition wait handles by testing its condition again. Every other result
//! is returned as it is.

use std::ffi::{CStr, c_char, c_int, c_long, c_void};
use std::sync::atomic::{AtomicPtr, AtomicU64, Ordering};

const EINVAL: c_int = 22;
/// The handle of `dlsym` for "the next definition after the image of the caller".
const RTLD_NEXT: *mut c_void = -1isize as *mut c_void;
/// The mutex stays free for this time, so that the thread that sets the condition runs.
const PAUSE_NANOSECONDS: c_long = 200_000;

/// The time of the system: seconds and nanoseconds.
#[repr(C)]
pub struct Timespec {
    seconds: i64,
    nanoseconds: c_long,
}

type Wait = unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_int;
type TimedWait = unsafe extern "C" fn(*mut c_void, *mut c_void, *const Timespec) -> c_int;

unsafe extern "C" {
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn pthread_mutex_lock(mutex: *mut c_void) -> c_int;
    fn pthread_mutex_unlock(mutex: *mut c_void) -> c_int;
    fn nanosleep(time: *const Timespec, left: *mut Timespec) -> c_int;
}

static SYSTEM_WAIT: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static SYSTEM_TIMED_WAIT: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static REFUSED: AtomicU64 = AtomicU64::new(0);

/// How many condition waits the system refused with `EINVAL` since the program started,
/// and this module returned as a wake-up.
pub fn refused() -> u64 {
    REFUSED.load(Ordering::Relaxed)
}

/// The function of the system with this name. `RTLD_NEXT` names the definition in the
/// images that were loaded after the image of this code, so the search does not find this
/// module. A result that is absent or that is `own` would make the wait call itself: the
/// program ends.
fn system(held: &AtomicPtr<c_void>, name: &CStr, own: *mut c_void) -> *mut c_void {
    let mut found = held.load(Ordering::Relaxed);
    if found.is_null() {
        // SAFETY: the name ends with a zero byte, and `dlsym` only reads it.
        found = unsafe { dlsym(RTLD_NEXT, name.as_ptr()) };
        if found.is_null() || found == own {
            std::process::abort();
        }
        held.store(found, Ordering::Relaxed);
    }
    found
}

/// Returns a refusal of the system as a wake-up without a signal: 0, with the mutex
/// released for a moment and taken again. When the release fails, the caller gets `error`.
///
/// # Safety
///
/// `mutex` is the mutex of the condition wait that returned `error`.
unsafe fn woken_without_signal(mutex: *mut c_void, error: c_int) -> c_int {
    // SAFETY: the system returns from a condition wait with the mutex held by the caller.
    // For a mutex of the default kind this holds also when the caller did not hold it
    // before the wait: macOS takes the mutex before it returns from such a wait, and does
    // not return `EINVAL` for it. The release of a mutex of another kind that the caller
    // does not hold fails, and the error is returned.
    if unsafe { pthread_mutex_unlock(mutex) } != 0 {
        return error;
    }
    REFUSED.fetch_add(1, Ordering::Relaxed);
    let pause = Timespec {
        seconds: 0,
        nanoseconds: PAUSE_NANOSECONDS,
    };
    // SAFETY: `pause` lives during the call, and no remaining time is asked for.
    unsafe { nanosleep(&pause, std::ptr::null_mut()) };
    // SAFETY: the mutex is the one that was released above.
    if unsafe { pthread_mutex_lock(mutex) } != 0 {
        // The caller expects to hold the mutex, whatever this function returns.
        std::process::abort();
    }
    0
}

/// Replaces `pthread_cond_wait` of the system for statically linked code.
///
/// # Safety
///
/// As for the function of the system: `cond` and `mutex` are an initialized condition and
/// its mutex, and the calling thread holds the mutex.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pthread_cond_wait(cond: *mut c_void, mutex: *mut c_void) -> c_int {
    let own = pthread_cond_wait as Wait as *mut c_void;
    let found = system(&SYSTEM_WAIT, c"pthread_cond_wait", own);
    // SAFETY: `dlsym` returned the function of the system with this name and signature.
    let wait = unsafe { std::mem::transmute::<*mut c_void, Wait>(found) };
    // SAFETY: the arguments are the ones of the caller.
    let result = unsafe { wait(cond, mutex) };
    if result != EINVAL {
        return result;
    }
    // SAFETY: `mutex` is the mutex of this wait.
    unsafe { woken_without_signal(mutex, result) }
}

/// Replaces `pthread_cond_timedwait` of the system for statically linked code.
///
/// # Safety
///
/// As for the function of the system: `cond` and `mutex` are an initialized condition and
/// its mutex, the calling thread holds the mutex, and `until` is a time.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pthread_cond_timedwait(
    cond: *mut c_void,
    mutex: *mut c_void,
    until: *const Timespec,
) -> c_int {
    let own = pthread_cond_timedwait as TimedWait as *mut c_void;
    let found = system(&SYSTEM_TIMED_WAIT, c"pthread_cond_timedwait", own);
    // SAFETY: `dlsym` returned the function of the system with this name and signature.
    let wait = unsafe { std::mem::transmute::<*mut c_void, TimedWait>(found) };
    // SAFETY: the arguments are the ones of the caller.
    let result = unsafe { wait(cond, mutex, until) };
    if result != EINVAL {
        return result;
    }
    // A time that is not a time is an error of the caller: the system returns it for
    // every wait with that time.
    // SAFETY: the caller gave null or a time.
    let time = unsafe { until.as_ref() };
    if !time.is_some_and(|time| (0..1_000_000_000).contains(&time.nanoseconds)) {
        return result;
    }
    // SAFETY: `mutex` is the mutex of this wait.
    unsafe { woken_without_signal(mutex, result) }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    const ETIMEDOUT: c_int = 60;

    unsafe extern "C" {
        fn pthread_mutex_init(mutex: *mut c_void, attributes: *const c_void) -> c_int;
        fn pthread_cond_init(cond: *mut c_void, attributes: *const c_void) -> c_int;
        fn pthread_cond_broadcast(cond: *mut c_void) -> c_int;
        fn gettimeofday(now: *mut [i64; 2], zone: *mut c_void) -> c_int;
    }

    /// The tests that count refusals run one at a time.
    static REFUSALS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// The memory of a mutex or of a condition of the system, which are 64 and 48 bytes.
    #[repr(C, align(8))]
    struct Memory([u8; 64]);

    struct Shared {
        first: Memory,
        second: Memory,
        condition: Memory,
        waiting: bool,
        released: bool,
    }

    struct Pointer(*mut Shared);
    // SAFETY: the test gives the pointer to one thread and joins it before the memory ends.
    unsafe impl Send for Pointer {}

    /// A time of the wall clock, `seconds` from now.
    fn in_seconds(seconds: i64) -> Timespec {
        let mut now = [0i64; 2];
        // SAFETY: the buffer holds the two fields of a `timeval` of this system.
        unsafe { gettimeofday(&mut now, std::ptr::null_mut()) };
        Timespec {
            seconds: now[0] + seconds,
            nanoseconds: 0,
        }
    }

    fn shared() -> Box<Shared> {
        let mut shared = Box::new(Shared {
            first: Memory([0; 64]),
            second: Memory([0; 64]),
            condition: Memory([0; 64]),
            waiting: false,
            released: false,
        });
        // SAFETY: the memory is large enough for each object and is not used before.
        unsafe {
            let first = (&raw mut shared.first).cast::<c_void>();
            let second = (&raw mut shared.second).cast::<c_void>();
            let condition = (&raw mut shared.condition).cast::<c_void>();
            assert_eq!(pthread_mutex_init(first, std::ptr::null()), 0);
            assert_eq!(pthread_mutex_init(second, std::ptr::null()), 0);
            assert_eq!(pthread_cond_init(condition, std::ptr::null()), 0);
        }
        shared
    }

    /// macOS refuses, with `EINVAL` and at once, a wait on a condition with another mutex
    /// than the one that a thread waits on it with. This gives one refusal of the system
    /// for valid arguments. `wait` is the wait under test, on the condition with the
    /// second mutex, which the calling thread holds. Returns what the wait returned and
    /// how many refusals the module counted for it.
    fn refused_once(wait: impl FnOnce(*mut c_void, *mut c_void) -> c_int) -> (c_int, u64) {
        let mut held = shared();
        let shared: *mut Shared = &mut *held;
        // SAFETY: the three objects are used only through the functions of the system and
        // live until the thread below is joined. The two flags are read and written only
        // under the first mutex.
        unsafe {
            let first = (&raw mut (*shared).first).cast::<c_void>();
            let second = (&raw mut (*shared).second).cast::<c_void>();
            let condition = (&raw mut (*shared).condition).cast::<c_void>();
            let given = Pointer(shared);
            let thread = std::thread::spawn(move || {
                let given = given;
                let shared = given.0;
                let first = (&raw mut (*shared).first).cast::<c_void>();
                let condition = (&raw mut (*shared).condition).cast::<c_void>();
                pthread_mutex_lock(first);
                (*shared).waiting = true;
                // The main thread sets the flag under the first mutex.
                loop {
                    if (*shared).released {
                        break;
                    }
                    pthread_cond_wait(condition, first);
                }
                pthread_mutex_unlock(first);
            });
            // The first mutex is free again only when the thread is inside its wait.
            loop {
                pthread_mutex_lock(first);
                let waiting = (*shared).waiting;
                pthread_mutex_unlock(first);
                if waiting {
                    break;
                }
                std::thread::yield_now();
            }
            let serial = REFUSALS.lock().unwrap();
            let before = refused();
            pthread_mutex_lock(second);
            let result = wait(condition, second);
            // The mutex is held again: the release succeeds.
            assert_eq!(pthread_mutex_unlock(second), 0);
            let counted = refused() - before;
            drop(serial);
            pthread_mutex_lock(first);
            (*shared).released = true;
            pthread_cond_broadcast(condition);
            pthread_mutex_unlock(first);
            thread.join().unwrap();
            (result, counted)
        }
    }

    #[test]
    fn a_wait_that_the_system_refuses_returns_as_a_wake_up_with_the_mutex_held() {
        // SAFETY: `refused_once` gives a condition and its mutex, held by this thread.
        let result =
            refused_once(|condition, mutex| unsafe { pthread_cond_wait(condition, mutex) });
        assert_eq!(
            result,
            (0, 1),
            "the refusal of the system reached the caller"
        );
    }

    #[test]
    fn a_timed_wait_that_the_system_refuses_returns_as_a_wake_up_with_the_mutex_held() {
        let until = in_seconds(3600);
        // SAFETY: `refused_once` gives a condition and its mutex, held by this thread.
        let result = refused_once(|condition, mutex| unsafe {
            pthread_cond_timedwait(condition, mutex, &until)
        });
        assert_eq!(
            result,
            (0, 1),
            "the refusal of the system reached the caller"
        );
    }

    #[test]
    fn every_other_result_of_the_system_is_returned_as_it_is() {
        let mut held = shared();
        // SAFETY: the objects are initialized, and this thread holds the mutex at each wait.
        unsafe {
            let mutex = (&raw mut held.first).cast::<c_void>();
            let condition = (&raw mut held.condition).cast::<c_void>();
            pthread_mutex_lock(mutex);
            // The time limit.
            let past = in_seconds(-1);
            assert_eq!(pthread_cond_timedwait(condition, mutex, &past), ETIMEDOUT);
            // A time that is not a time: the error of the caller stays an error.
            let mut invalid = in_seconds(3600);
            invalid.nanoseconds = 2_000_000_000;
            assert_eq!(pthread_cond_timedwait(condition, mutex, &invalid), EINVAL);
            assert_eq!(pthread_mutex_unlock(mutex), 0);
        }
    }
}
