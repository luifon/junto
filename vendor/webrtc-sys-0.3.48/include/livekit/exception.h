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

#pragma once

#include <cstddef>
#include <exception>
#include <string>
#include <typeinfo>
#include <utility>

namespace livekit_ffi {

// The exception that every function of this crate throws to its bridge. The crate is built
// with the defines of the WebRTC library, which has _HAS_EXCEPTIONS=0 on Windows; with that
// value std::runtime_error keeps only a pointer to its message. This class keeps a copy, which
// exists until the bridge has read it.
class Exception final : public std::exception {
 public:
  explicit Exception(std::string message) : message_(std::move(message)) {}

  const char* what() const noexcept override { return message_.c_str(); }

 private:
  std::string message_;
};

// Writes one line that names the bridge function, the type of the current exception and its
// message to standard error, then a line with the readable names, and ends the process with
// std::abort(). Called only inside a catch clause. Its arguments need no allocation, so that the
// catch can call it when the memory of the process is exhausted: `bridge_function` and
// `windows_type` are type_info objects, whose names it reads itself. `windows_type` and
// `message` are null when they are not known: on macOS and Linux the C++ ABI gives the type of
// the current exception, on Windows only the type of a std::exception is known.
[[noreturn]] void end_at_foreign_exception(const std::type_info& bridge_function,
                                           const std::type_info* windows_type,
                                           const char* message) noexcept;

inline const std::type_info* windows_type(const std::exception& e) {
#if defined(_MSC_VER)
  return &typeid(e);
#else
  static_cast<void>(e);
  return nullptr;
#endif
}

// Test functions of the bridge.
void throw_runtime_exception(std::size_t index);
void throw_value();
// Takes the lock of the stream stderr of the C library and keeps it: another thread that writes
// to that stream waits for ever.
void hold_standard_error();

}  // namespace livekit_ffi

namespace rust {
namespace behavior {

// The catch of every bridge that includes this file, in place of the one of cxx. Only an
// exception of this crate becomes a Rust error: the crate throws it from its own code, with no
// frame of the WebRTC library between the throw and this catch. Any other exception can have
// passed frames of the WebRTC library, which is built without exception handling, so their
// destructors did not run and their locks were not released; the process ends. With
// _HAS_EXCEPTIONS=0, std::exception is stdext::exception, from which the exceptions of the C++
// runtime do not derive, so they reach the last clause.
template <typename Try, typename Fail>
static void trycatch(Try&& func, Fail&& fail) noexcept try {
  func();
} catch (const ::livekit_ffi::Exception& e) {
  fail(e.what());
} catch (const ::std::exception& e) {
  ::livekit_ffi::end_at_foreign_exception(typeid(Try), ::livekit_ffi::windows_type(e), e.what());
} catch (...) {
  ::livekit_ffi::end_at_foreign_exception(typeid(Try), nullptr, nullptr);
}

}  // namespace behavior
}  // namespace rust
