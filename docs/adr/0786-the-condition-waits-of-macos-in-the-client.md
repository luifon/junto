# 0786. The condition waits of macOS in the client

## Status

Accepted

## Context

The test program of the media engine ended with a memory fault under processor load, in two of ten runs. In both reports a thread of the WebRTC library ran a task of `webrtc::SctpDataChannel::RegisterObserver` and read through a pointer into the stack of the thread that had called `RegisterObserver`. That thread was already in a later call, `AddTransceiver`, of the same connection set-up (`Room::connect` of the call library, ADR 0701). The blocking call had returned before its task ran.

What was measured, with a recorder loaded into the test program:

- In one of eight runs under load, `pthread_cond_timedwait` returned `EINVAL` twice. Each time it returned at once, with three seconds of its time left, and the system call below it, `__psynch_cvwait`, had returned the same error. Both calls came from `webrtc::Event::Wait`, called by `webrtc::Thread::BlockingCallImpl`, called by `RegisterObserver`, inside `Room::connect`.
- The condition and the mutex of that wait were valid. The system library checks, before it calls the kernel, the signature of the mutex and of the condition (a destroyed object fails here), the time, and that no thread waits on the condition with another mutex; each of these returns `EINVAL` without a call of the kernel. The recorder saw the call of the kernel in both cases, and the recorded condition was at its first wait. In the source of the WebRTC library the event of a blocking call is a variable of `BlockingCallImpl`, created three statements before the wait and destroyed when the function returns; no other thread has a reference to it before the task is posted.
- So the kernel refused a valid wait. The kernel part of Apple's `libpthread` returns `EINVAL` from `ksyn_wqfind` when the address of the condition or of the mutex is still held in the kernel for a synchronizer of another kind that is in use; the recorded sequence numbers exclude its other `EINVAL`. Which earlier object held the address was not recorded. A program that only repeats blocking calls, 57 million times, did not get the error.

The WebRTC library at the linked revision (`webrtc-89d790b`) turns the refusal into the fault:

- `Event::Wait` ends its loop on any error of the condition wait other than the time limit of its first wait, and returns "not signalled".
- `Thread::BlockingCallImpl` keeps the event on its stack, gives the task a reference to the caller's function and to that event, and does not read what `Wait` returns.

A check with the linked library shows this without chance (`tools/check-condition-wait.sh`): a refusal injected into the wait of one blocking call makes the call return before its task. The event waits with a time limit of three seconds first (`pthread_cond_timedwait`) and without one after that (`pthread_cond_wait`); the check shows the fault for each of the two. The refusal was measured for the first one only. The same error in the second one has the same result, so both are covered.

The project calls `Room::connect` and awaits it. The newest published versions of the call library (livekit 0.9.5, libwebrtc 0.3.52, webrtc-sys 0.3.49) link the same revision, and the main branch of WebRTC has the same code. No version that corrects it exists.

## Decision

On macOS the media crate defines `pthread_cond_wait` and `pthread_cond_timedwait` (`crates/media/src/condition_wait.rs`), with one rule for both.

- Each function calls the function of the system, which it finds with `dlsym(RTLD_NEXT, …)`: the search starts after the image of the program, so it cannot find these definitions; the program ends if it finds none or the definition itself.
- It changes one result. When the system returns `EINVAL`, the function releases the mutex, waits 200 microseconds, takes the mutex again and returns 0. For the caller this is a wake-up without a signal, which POSIX allows at any time: a caller tests its condition and waits again. `Event::Wait` does, and the blocking call returns after its task.
- Every other result is returned as it is: 0, the time limit, `EINVAL` for a time whose nanoseconds are out of range, and `EINVAL` when the release of the mutex fails.
- The module counts the refusals. The engine and the connection to the room of a move write a warning when the system refused a wait while a media connection was made.

What else returns `EINVAL`, measured on macOS 26.6 with a mutex of the default kind:

- A wait on a condition without a signature, which is a condition that was not initialized or was destroyed, returns `EINVAL`. The module cannot tell it from a refusal of the kernel, and returns it as a wake-up too.
- A wait with a mutex of the default kind that the calling thread does not hold does not return `EINVAL`: the system waits and returns with the mutex held, also when another thread held it. A time limit with a negative second returns the time limit. Neither reaches the rule of the module.
- The release of a mutex of the default kind does not fail, whoever holds it. After `EINVAL` from a wait with a valid condition, the system has returned with the mutex held by the caller, and the module releases that.
- Not measured: a mutex of the error-checking or of the recursive kind that the caller does not hold. The module returns the error of the system when the release of the mutex fails.
- Not covered: a wait on a condition without a signature with a mutex of the default kind that another thread holds. The system returns `EINVAL` without taking the mutex, and the module then releases the mutex of the other thread. Such a caller has two faults of its own.

What the definitions cover. The linker binds a reference to the definition in the program itself; a library of the system keeps its own binding.

- Covered: every call of the two functions in code that is linked statically into the program. In the WebRTC archive, `pthread_cond_timedwait` is called by three objects: `event.o` (the event of a blocking call and of a proxy call), `pthread_waiter.o` (the waiter of the abseil mutex) and `WelsThreadLib.o` (the H.264 encoder); `pthread_cond_wait` by twenty objects, `event.o` among them. The Rust standard library calls `pthread_cond_wait` for a wait without a time limit.
- Not covered: `pthread_cond_timedwait_relative_np`, which the Rust standard library and the abseil waiter use for a wait with a time limit; and all code in the libraries of the system, the C++ library among them. A refusal was measured for none of these.
- `tools/check-condition-wait.sh <program>` proves the binding for a built program: the program defines `_pthread_cond_wait` and `_pthread_cond_timedwait` and takes neither from the system library.

The condition for removing the module: a published version of the call library that links a WebRTC library in which `Event::Wait` waits again after an error, or in which a blocking call does not return before its event was set. The module and its check are then deleted.

Two more corrections of the engine's own handling of a media connection, found while the fault was read:

- The leave of a room goes on for up to 30 seconds after the engine stops waiting for it at its limit of 3 seconds. Before, the limit cut the leave off between two of its steps.
- A media connection that was made while the engine ended, and whose result was in the queue of the engine, is left. Before, it was dropped, and a connection that is only dropped stays in the room.

## Consequences

- While the system refuses a wait, the caller asks again every 200 microseconds until its condition holds or its time limit passes.
- A wait on a condition with another mutex than the one that a thread waits on it with, and a wait on a condition that was not initialized or was destroyed, are faults of the caller that the system reports with `EINVAL`. With this module such a caller gets a wake-up every 200 microseconds instead of the error. The code of the project does neither; the test of the module uses the first to get a refusal of the system. The code of the linked libraries was not read for them.
- No workflow of the repository runs `tools/check-condition-wait.sh`: the check needs the WebRTC library of a finished build and the compilers of macOS, and it was run on the machine of the work only.
- Every wait without a time limit of the Rust standard library in the program passes through the definition, the parked threads of the asynchronous runtime among them. The definition adds one call and one comparison to each.
- The definition exists only on macOS and in every program that links the media crate.

## Report for the maintainers of the call library

The text below is for `livekit/rust-sdks`, whose crate `webrtc-sys` links the WebRTC build `webrtc-89d790b` of `webrtc-sdk/webrtc`. The same code is in the main branch of WebRTC (`rtc_base/event.cc`, `rtc_base/thread.cc`). It was not sent.

> **macOS: `Room::connect` can crash in `SctpDataChannel::RegisterObserver`: `webrtc::Event::Wait` returns on `EINVAL` from `pthread_cond_timedwait`, and `Thread::BlockingCallImpl` ignores the result**
>
> Versions: livekit 0.9.4, libwebrtc 0.3.51, webrtc-sys 0.3.48, webrtc-sys-build 0.3.19 (`WEBRTC_TAG = "webrtc-89d790b"`), macOS 26 (Darwin 25.6), arm64. livekit 0.9.5 with webrtc-sys 0.3.49 links the same WebRTC build.
>
> What happens: a process makes several `Room::connect` calls while all processor cores are busy. In 2 of 10 runs of a test program the process ended with `EXC_BAD_ACCESS` on the signaling thread:
>
> ```
> webrtc::FunctionView<void ()>::CallVoidPtr<… webrtc::Thread::BlockingCall<webrtc::SctpDataChannel::RegisterObserver(webrtc::DataChannelObserver*)::$_0, …>::'lambda'()> +32
> webrtc::Thread::Dispatch(absl::AnyInvocable<void () &&>) +160
> webrtc::Thread::ProcessMessages(int) +224
> webrtc::Thread::PreRun(void*) +96
> ```
>
> The function object that the task reads lies in the stack of the thread that called `RegisterObserver` (from `RtcSession::create_data_channels`), and that thread is already in `PeerConnection::AddTransceiver` (from `RtcSession::add_recv_media_sections`). In one report the network thread runs the second task of the same `RegisterObserver` call at the same time.
>
> Cause: with `pthread_cond_timedwait` and `__psynch_cvwait` wrapped by an inserted library, `pthread_cond_timedwait` returned `EINVAL` at once (3.000 s of its time left; `__psynch_cvwait` returned -1 with `errno` `EINVAL`) in
>
> ```
> webrtc::Event::Wait(TimeDelta, TimeDelta) +308
> webrtc::Thread::BlockingCallImpl(FunctionView<void ()>, Location const&) +380
> webrtc::SctpDataChannel::RegisterObserver(DataChannelObserver*) +176
> livekit_ffi::DataChannel::register_observer(…)
> … RtcSession::create_data_channels … RtcSession::connect … Room::connect
> ```
>
> twice in one of eight runs under load. The condition and the mutex are the members of the `Event done` on the stack of `BlockingCallImpl` and were valid: the system library reached its system call. In the kernel part of Apple's libpthread (`kern/kern_synch.c`), `ksyn_wqfind` returns `EINVAL` when the address is still known to the kernel as a synchronizer of another kind that is in use; `_pthread_psynch_cond_wait` returns every such error as `EINVAL`, with the mutex held.
>
> Why it becomes a crash: `Event::Wait` (`rtc_base/event.cc`) loops `while (!event_status_ && error == 0)`, so an error other than `ETIMEDOUT` of the first wait ends the function with "not signalled". `Thread::BlockingCallImpl` (`rtc_base/thread.cc`) calls `done.Wait(Event::kForever);` without a check. The posted task holds a `FunctionView` of the caller's function object and a reference to `done`; both are on a stack frame that ends when `Wait` returns early.
>
> It can be shown without load: a program that links `libwebrtc.a`, holds the target thread, makes `pthread_cond_timedwait` return `EINVAL` once for the calling thread and calls `Thread::BlockingCall` gets the call back before its task ran. The same holds for `pthread_cond_wait` after the three-second warning.
>
> Request: `Event::Wait` should wait again after an error that is neither its time limit nor a set event, or `BlockingCallImpl` should wait until `done` was set.
>
> Workaround in an application: define `pthread_cond_wait` and `pthread_cond_timedwait` in the executable, call the system functions through `dlsym(RTLD_NEXT, …)`, and return `EINVAL` as a spurious wake-up (unlock the mutex, sleep 200 µs, lock, return 0). `Event::Wait` then waits again.
