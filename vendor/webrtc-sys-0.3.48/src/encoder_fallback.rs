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

//! A function for the test of the software fallback of `CreateAdaptedEncoder`
//! (`src/video_encoder_factory.cpp`): an encoder of a hardware backend that does not start.

#[cxx::bridge(namespace = "livekit_ffi")]
pub mod ffi {
    /// What an adapted encoder did with one picture.
    #[derive(Debug)]
    pub struct FallbackEncoding {
        /// The result of `InitEncode`; 0 is success.
        pub init: i32,
        /// The result of `Encode` of one key picture; 0 is success.
        pub encode: i32,
        /// The encoded pictures that the callback received.
        pub images: u32,
        /// The codec of the last encoded picture, as its payload name.
        pub codec: String,
        /// The implementation name of the encoder after the picture.
        pub implementation: String,
    }

    unsafe extern "C++" {
        include!("livekit/encoder_fallback.h");

        /// Makes an encoder of `codec` ("H264" or "VP8") with the hardware backend
        /// `VideoToolbox` through `CreateAdaptedEncoder`, from a primary factory whose encoders
        /// do not start, and encodes one picture of 320 × 180.
        fn encode_with_a_failing_hardware_encoder(codec: &str) -> FallbackEncoding;
    }
}
