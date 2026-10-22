// Added to the vendored copy of webrtc-sys 0.3.48; this file is not in the published crate
// and contains no code of it.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#include "livekit/exception.h"

#include <cstddef>
#include <cstdio>
#include <cstdlib>
#include <exception>
#include <vector>

#if defined(_MSC_VER)
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#else
#include <cxxabi.h>
#include <unistd.h>

#include <cerrno>
#endif

namespace livekit_ffi {
namespace {

// One line of standard error in memory of the stack; text that does not fit is cut. It is
// written to the handle of standard error, not through the stream stderr of the C library, whose
// lock another thread can hold for ever, and it is formatted without an allocation.
class Line {
 public:
  void add(const char* text) noexcept {
    for (; text != nullptr && *text != '\0' && length_ < sizeof(text_) - 1; ++text) {
      text_[length_++] = *text;
    }
  }

  void write() noexcept {
    text_[length_++] = '\n';
    std::size_t sent = 0;
#if defined(_MSC_VER)
    HANDLE output = GetStdHandle(STD_ERROR_HANDLE);
    DWORD written = 0;
    while (output != nullptr && output != INVALID_HANDLE_VALUE && sent < length_ &&
           WriteFile(output, text_ + sent, static_cast<DWORD>(length_ - sent), &written,
                     nullptr) &&
           written > 0) {
      sent += written;
    }
#else
    while (sent < length_) {
      ssize_t written = ::write(STDERR_FILENO, text_ + sent, length_ - sent);
      if (written > 0) {
        sent += static_cast<std::size_t>(written);
      } else if (written < 0 && errno == EINTR) {
        continue;
      } else {
        break;
      }
    }
#endif
    length_ = 0;
  }

 private:
  char text_[2048];
  std::size_t length_ = 0;
};

// The names as the compiler wrote them, which need no allocation: on Windows the decorated
// name, because name() of MSVC allocates the undecorated one.
const char* written_name(const std::type_info& type) noexcept {
#if defined(_MSC_VER)
  return type.raw_name();
#else
  return type.name();
#endif
}

const std::type_info* current_type(const std::exception* exception) noexcept {
#if defined(_MSC_VER)
  return exception != nullptr ? &typeid(*exception) : nullptr;
#else
  static_cast<void>(exception);
  return abi::__cxa_current_exception_type();
#endif
}

// The handler of std::terminate while the process ends: the default handler of libc++abi writes
// through the stream stderr, whose lock another thread can hold, so a failure in the second line
// (the demangler calls std::terminate when it cannot allocate) would keep the process from
// ending.
[[noreturn]] void abort_only() noexcept {
  std::abort();
}

// The readable name of a type. It can allocate, and the demangler of libc++abi ends the process
// when an allocation fails, so it is read only after the first line.
const char* readable_name(const std::type_info& type) noexcept {
#if defined(_MSC_VER)
  return type.name();
#else
  int status = 0;
  const char* name = abi::__cxa_demangle(type.name(), nullptr, nullptr, &status);
  return name != nullptr ? name : type.name();
#endif
}

}  // namespace

void end_at_foreign_exception(const std::type_info& bridge_function,
                              const std::exception* exception) noexcept {
#if defined(_MSC_VER)
  // abort() of the C++ runtime would also show a message box and report the fault to Windows.
  // This comes first, so that a fault in the steps below does not show it either.
  _set_abort_behavior(0, _WRITE_ABORT_MSG | _CALL_REPORTFAULT);
#endif
  std::set_terminate(abort_only);
  const std::type_info* type = current_type(exception);
  const char* message = exception != nullptr ? exception->what() : nullptr;
  Line line;
  line.add("livekit_ffi: the bridge function ");
  line.add(written_name(bridge_function));
  line.add(" ended with a C++ exception that is not livekit_ffi::Exception, of type ");
  line.add(type != nullptr ? written_name(*type) : "not known");
  if (message != nullptr) {
    line.add(": ");
    line.add(message);
  }
  line.add("; the process ends");
  line.write();
  line.add("livekit_ffi: readable names: the bridge function ");
  line.add(readable_name(bridge_function));
  line.add(", the type ");
  line.add(type != nullptr ? readable_name(*type) : "not known");
  line.write();
  std::abort();
}

// std::vector::at throws std::out_of_range: on Windows from an out-of-line function of the C++
// runtime, also with _HAS_EXCEPTIONS=0, and on macOS from libc++. On Linux the libc++ inside the
// WebRTC library is built without exceptions and ends the process instead.
void throw_runtime_exception(std::size_t index) {
  std::vector<int> empty;
  static_cast<void>(empty.at(index));
}

class TestError final : public std::exception {
 public:
  const char* what() const noexcept override { return "a test error"; }
};

void throw_value() {
  throw 1;
}

void throw_test_error() {
  throw TestError();
}

void hold_standard_error() {
#if defined(_MSC_VER)
  _lock_file(stderr);
#else
  flockfile(stderr);
#endif
}

}  // namespace livekit_ffi
