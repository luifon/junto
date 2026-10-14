// For tools/check-condition-wait.sh: a library that stands between a program and the
// condition waits of the system, and refuses the next waits of one thread with EINVAL, as
// the kernel of macOS was measured to do (ADR 0786).

#include <dlfcn.h>
#include <errno.h>
#include <pthread.h>
#include <stdatomic.h>

static _Thread_local int timed_to_refuse;
static _Thread_local int untimed_to_refuse;
static atomic_int refused;

// The next `timed` waits with a time limit and the next `untimed` waits without one, of the
// calling thread, return EINVAL at once.
void junto_check_refuse_next(int timed, int untimed) {
  timed_to_refuse = timed;
  untimed_to_refuse = untimed;
}

// How many waits were refused, by all threads.
int junto_check_refused(void) { return atomic_load(&refused); }

int pthread_cond_timedwait(pthread_cond_t* cond, pthread_mutex_t* mutex,
                           const struct timespec* until) {
  typedef int (*wait_fn)(pthread_cond_t*, pthread_mutex_t*, const struct timespec*);
  static _Atomic(wait_fn) system_wait;
  if (timed_to_refuse > 0) {
    timed_to_refuse--;
    atomic_fetch_add(&refused, 1);
    return EINVAL;
  }
  wait_fn wait = atomic_load(&system_wait);
  if (!wait) {
    wait = (wait_fn)dlsym(RTLD_NEXT, "pthread_cond_timedwait");
    atomic_store(&system_wait, wait);
  }
  return wait(cond, mutex, until);
}

int pthread_cond_wait(pthread_cond_t* cond, pthread_mutex_t* mutex) {
  typedef int (*wait_fn)(pthread_cond_t*, pthread_mutex_t*);
  static _Atomic(wait_fn) system_wait;
  if (untimed_to_refuse > 0) {
    untimed_to_refuse--;
    atomic_fetch_add(&refused, 1);
    return EINVAL;
  }
  wait_fn wait = atomic_load(&system_wait);
  if (!wait) {
    wait = (wait_fn)dlsym(RTLD_NEXT, "pthread_cond_wait");
    atomic_store(&system_wait, wait);
  }
  return wait(cond, mutex);
}
