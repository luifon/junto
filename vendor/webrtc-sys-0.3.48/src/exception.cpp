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

#include <cstdio>
#include <cstdlib>
#include <vector>

#if !defined(_MSC_VER)
#include <cxxabi.h>
#endif

namespace livekit_ffi {
namespace {

std::string readable(const char* name) {
#if defined(_MSC_VER)
  return name;
#else
  int status = 0;
  char* text = abi::__cxa_demangle(name, nullptr, nullptr, &status);
  if (status != 0 || text == nullptr) {
    return name;
  }
  std::string result(text);
  std::free(text);
  return result;
#endif
}

std::string current_type(const char* windows_type) {
#if defined(_MSC_VER)
  return windows_type != nullptr ? windows_type : "not known";
#else
  static_cast<void>(windows_type);
  const std::type_info* type = abi::__cxa_current_exception_type();
  return type != nullptr ? readable(type->name()) : "not known";
#endif
}

}  // namespace

void end_at_foreign_exception(const char* bridge_function,
                              const char* windows_type,
                              const char* message) noexcept {
  std::string type = current_type(windows_type);
  std::fprintf(stderr,
               "livekit_ffi: the bridge function %s ended with a C++ exception that is not "
               "livekit_ffi::Exception, of type %s%s%s; the process ends\n",
               readable(bridge_function).c_str(), type.c_str(),
               message != nullptr ? ": " : "", message != nullptr ? message : "");
  std::fflush(stderr);
#if defined(_MSC_VER)
  // abort() of the C++ runtime would also show a message box and report the fault to Windows.
  _set_abort_behavior(0, _WRITE_ABORT_MSG | _CALL_REPORTFAULT);
#endif
  std::abort();
}

// std::vector::at throws std::out_of_range: on Windows from an out-of-line function of the C++
// runtime, also with _HAS_EXCEPTIONS=0, and on macOS from libc++. On Linux the libc++ inside the
// WebRTC library is built without exceptions and ends the process instead.
void throw_runtime_exception(std::size_t index) {
  std::vector<int> empty;
  static_cast<void>(empty.at(index));
}

void throw_value() {
  throw 1;
}

}  // namespace livekit_ffi
