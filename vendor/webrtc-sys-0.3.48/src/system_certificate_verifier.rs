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
