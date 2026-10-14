// For tools/check-condition-wait.sh: one blocking call of the WebRTC library whose wait the
// system refuses (ADR 0786). The program is linked against the library that the build of
// this checkout links, once as it is and once with crates/media/src/condition_wait.rs.
//
// A blocking call must not return before its task ran. The target thread is held until
// the check has looked, so a call that returns while it is held returned too early.
//
// The event of a blocking call waits with a time limit of three seconds first and then
// without one. The argument "timed" refuses the first wait, "untimed" the second.

#include <atomic>
#include <chrono>
#include <cstdio>
#include <cstring>
#include <memory>
#include <thread>
#include <unistd.h>

#include "rtc_base/thread.h"

extern "C" {
void junto_check_refuse_next(int timed, int untimed);
int junto_check_refused(void);
}

int main(int argc, char** argv) {
  const bool untimed = argc > 1 && std::strcmp(argv[1], "untimed") == 0;
  const int refusals = 3;
  std::unique_ptr<webrtc::Thread> target = webrtc::Thread::Create();
  target->Start();

  auto release = std::make_shared<std::atomic<bool>>(false);
  target->PostTask([release] {
    while (!release->load()) {
      std::this_thread::sleep_for(std::chrono::milliseconds(1));
    }
  });
  // The target is released only after every refusal was given and some time has passed:
  // a wait that is repeated after the refusals is then still a wait.
  std::thread releaser([release] {
    while (junto_check_refused() < refusals) {
      std::this_thread::sleep_for(std::chrono::milliseconds(1));
    }
    std::this_thread::sleep_for(std::chrono::milliseconds(200));
    release->store(true);
  });

  auto ran = std::make_shared<std::atomic<bool>>(false);
  junto_check_refuse_next(untimed ? 0 : refusals, untimed ? refusals : 0);
  target->BlockingCall([ran] { ran->store(true); });

  if (!ran->load()) {
    std::fputs("FAIL: webrtc::Thread::BlockingCall returned before its task ran\n", stderr);
    // The task still waits in the queue of the target with a reference to the stack of
    // this call. The program ends before the target can run it.
    _exit(1);
  }
  releaser.join();
  target->Stop();
  std::printf("PASS: the blocking call returned after its task; the system refused %d waits\n",
              junto_check_refused());
  return 0;
}
