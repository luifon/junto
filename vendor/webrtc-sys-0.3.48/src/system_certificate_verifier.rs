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

use rustls::client::verify_server_cert_signed_by_trust_anchor;
use rustls::pki_types::{CertificateDer, UnixTime};
use rustls::server::ParsedCertificate;
use rustls::RootCertStore;

#[cxx::bridge(namespace = "livekit_ffi")]
mod ffi {
    struct PeerCertificate {
        der: Vec<u8>,
    }

    extern "Rust" {
        fn verify_system_chain(chain: Vec<PeerCertificate>) -> bool;
    }
}

fn verify_system_chain(chain: Vec<ffi::PeerCertificate>) -> bool {
    let certificates: Vec<CertificateDer<'_>> = chain
        .iter()
        .map(|certificate| CertificateDer::from(certificate.der.as_slice()))
        .collect();
    let Some((leaf, intermediates)) = certificates.split_first() else {
        return false;
    };
    let Ok(leaf) = ParsedCertificate::try_from(leaf) else {
        return false;
    };
    let native = rustls_native_certs::load_native_certs();
    if !native.errors.is_empty() {
        log::warn!("Some system TLS roots could not be loaded");
    }
    let mut roots = RootCertStore::empty();
    roots.add_parsable_certificates(native.certs);
    if roots.is_empty() {
        log::warn!("No usable system TLS roots were loaded");
        return false;
    }
    verify_server_cert_signed_by_trust_anchor(
        &leaf,
        &roots,
        intermediates,
        UnixTime::now(),
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .all,
    )
    .is_ok()
}
