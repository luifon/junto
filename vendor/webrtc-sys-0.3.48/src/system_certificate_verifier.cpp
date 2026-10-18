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

#include "livekit/system_certificate_verifier.h"

#include <memory>
#include <utility>

#include "rtc_base/buffer.h"
#include "rtc_base/platform_certificate_verifier.h"
#include "webrtc-sys/src/system_certificate_verifier.rs.h"

namespace livekit_ffi {
namespace {

class SystemCertificateVerifier final : public webrtc::SSLCertificateVerifier {
 public:
  bool VerifyChain(const webrtc::SSLCertChain& chain) override {
    rust::Vec<PeerCertificate> certificates;
    certificates.reserve(chain.GetSize());
    for (size_t i = 0; i < chain.GetSize(); ++i) {
      webrtc::Buffer der;
      chain.Get(i).ToDER(&der);
      PeerCertificate certificate;
      certificate.der.reserve(der.size());
      for (uint8_t byte : der) {
        certificate.der.push_back(byte);
      }
      certificates.push_back(std::move(certificate));
    }
    return verify_system_chain(std::move(certificates));
  }
};

std::unique_ptr<webrtc::SSLCertificateVerifier> CreateSystemCertificateVerifier() {
  return std::make_unique<SystemCertificateVerifier>();
}

}  // namespace

void initialize_system_certificate_verifier() {
  webrtc::SetPlatformCertificateVerifierFactory(CreateSystemCertificateVerifier);
}

}  // namespace livekit_ffi
