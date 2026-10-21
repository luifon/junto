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

//! Functions that throw through the bridge, for the tests of `livekit/exception.h`: an
//! exception of the C++ runtime and a value that is not an exception must each end the process
//! with a line that names the bridge function, also while another thread holds the stream
//! `stderr` of the C library.

#[cxx::bridge(namespace = "livekit_ffi")]
pub mod ffi {
    unsafe extern "C++" {
        include!("livekit/exception.h");

        /// Reads element `index` of an empty `std::vector` with `at`.
        fn throw_runtime_exception(index: usize) -> Result<()>;

        /// Throws an `int`.
        fn throw_value() -> Result<()>;

        /// Takes the lock of the stream `stderr` of the C library and keeps it.
        fn hold_standard_error();

        /// Throws a subclass of `std::exception` other than `livekit_ffi::Exception`, with the
        /// message "a test error".
        fn throw_test_error() -> Result<()>;
    }
}
