//! Identity keys, self-signed certificates and TLS configuration.
//!
//! Mirrors `RsaHelper` and `SslHelper` from upstream:
//!   * EC P-256 (secp256r1) key pair by default, RSA kept only for legacy data.
//!   * self-signed X.509 v3 certificate, DN `CN=<deviceId>, OU=KDE Connect, O=KDE`.
//!   * TLS 1.2 only.
//!   * trusted peers are certificate-pinned (TOFU), untrusted peers use a
//!     trust-all verifier so that the `kdeconnect.pair` packet can be exchanged
//!     before a certificate is stored.

use std::sync::{Arc, Mutex};

use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair, PKCS_ECDSA_P256_SHA256};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{
    crypto, CertificateError, ClientConfig, DigitallySignedStruct, DistinguishedName as RustlsDn,
    Error as TlsError, ServerConfig, SignatureScheme,
};
use x509_parser::prelude::{FromDer, X509Certificate};

use crate::ProtocolError;

/// Material loaded from (or persisted to) the app settings.
#[derive(Clone, Debug)]
pub struct Identity {
    pub device_id: String,
    /// DER bytes of the self-signed leaf certificate.
    pub cert_der: Vec<u8>,
    /// PEM form of `cert_der` (what the Bluetooth path puts in the identity packet).
    pub cert_pem: String,
    /// PKCS#8 DER private key.
    pub private_key_der: Vec<u8>,
    /// PEM private key for debugging / migration from the Kotlin store.
    pub private_key_pem: String,
    /// SPKI DER public key; used to compute the pairing verification code.
    pub public_key_der: Vec<u8>,
}

impl Identity {
    /// Restore an identity from persisted material, validating the certificate.
    pub fn from_parts(
        device_id: String,
        cert_pem: &str,
        private_key_der: Vec<u8>,
    ) -> Result<Self, ProtocolError> {
        let cert_der = pem_decode("CERTIFICATE", cert_pem)?;
        let parsed = parse_certificate_der(&cert_der)?;
        Ok(Self {
            device_id,
            cert_der,
            cert_pem: cert_pem.to_owned(),
            private_key_der,
            private_key_pem: String::new(),
            public_key_der: parsed.spki_der,
        })
    }
}

/// Generate a fresh EC P-256 identity for a device ID.
pub fn generate_identity(device_id: &str) -> Result<Identity, ProtocolError> {
    let key_pair = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
        .map_err(|e| ProtocolError::Certificate(format!("key generation failed: {e}")))?;

    let mut params = CertificateParams::new(vec![device_id.to_owned()])
        .map_err(|e| ProtocolError::Certificate(format!("bad params: {e}")))?;
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, device_id);
    dn.push(DnType::OrganizationalUnitName, "KDE Connect");
    dn.push(DnType::OrganizationName, "KDE");
    params.distinguished_name = dn;

    let cert = params
        .self_signed(&key_pair)
        .map_err(|e| ProtocolError::Certificate(format!("certificate generation failed: {e}")))?;
    let public_key_der = pem_decode("PUBLIC KEY", &key_pair.public_key_pem())?;

    Ok(Identity {
        device_id: device_id.to_owned(),
        cert_der: cert.der().as_ref().to_vec(),
        cert_pem: cert.pem(),
        private_key_der: key_pair.serialize_der(),
        private_key_pem: key_pair.serialize_pem(),
        public_key_der,
    })
}

/// Extract the base64 payload of the first PEM block with `label`.
fn pem_decode(label: &str, pem: &str) -> Result<Vec<u8>, ProtocolError> {
    let begin = format!("-----BEGIN {label}-----");
    let end = format!("-----END {label}-----");
    let mut b64 = String::new();
    let mut inside = false;
    for line in pem.lines() {
        let line = line.trim();
        if line.starts_with(&begin) {
            inside = true;
        } else if line.starts_with(&end) {
            break;
        } else if inside {
            b64.push_str(line);
        }
    }
    if b64.is_empty() {
        return Err(ProtocolError::Certificate(format!(
            "no {label} PEM block found"
        )));
    }
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(&b64)
        .map_err(|e| ProtocolError::Certificate(format!("bad base64 in {label}: {e}")))
}

pub fn cert_der_from_pem(pem: &str) -> Result<Vec<u8>, ProtocolError> {
    pem_decode("CERTIFICATE", pem)
}

/// Wrap DER bytes in a PEM `CERTIFICATE` block.
pub fn cert_pem_from_der(der: &[u8]) -> Result<String, ProtocolError> {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(der);
    let mut out = String::from("-----BEGIN CERTIFICATE-----\n");
    for chunk in b64.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(chunk).unwrap_or(""));
        out.push('\n');
    }
    out.push_str("-----END CERTIFICATE-----\n");
    Ok(out)
}

/// Parsed certificate data needed by pairing and identity validation.
pub struct ParsedCertificate {
    pub spki_der: Vec<u8>,
    pub not_before: i64,
    pub not_after: i64,
}

pub fn parse_certificate_der(der: &[u8]) -> Result<ParsedCertificate, ProtocolError> {
    let (_, cert) = X509Certificate::from_der(der)
        .map_err(|e| ProtocolError::Certificate(format!("invalid X.509: {e}")))?;
    Ok(ParsedCertificate {
        spki_der: cert.public_key().raw.to_vec(),
        not_before: cert.validity().not_before.timestamp(),
        not_after: cert.validity().not_after.timestamp(),
    })
}

// ---------------------------------------------------------------------------
// Certificate verifiers
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct PeerCertCapture {
    der: Mutex<Option<Vec<u8>>>,
}

impl PeerCertCapture {
    fn capture(&self, cert: &CertificateDer<'_>) {
        *self.der.lock().unwrap() = Some(cert.as_ref().to_vec());
    }
}

/// A verifier that only accepts one exact certificate DER (TOFU pinning).
#[derive(Debug)]
struct PinnedCertVerifier {
    expected: Vec<u8>,
}

impl ServerCertVerifier for PinnedCertVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        if end_entity.as_ref() != self.expected.as_slice() {
            return Err(TlsError::InvalidCertificate(CertificateError::BadEncoding));
        }
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

impl ClientCertVerifier for PinnedCertVerifier {
    fn offer_client_auth(&self) -> bool {
        true
    }

    fn client_auth_mandatory(&self) -> bool {
        true
    }

    fn root_hint_subjects(&self) -> &[RustlsDn] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, TlsError> {
        if end_entity.as_ref() != self.expected.as_slice() {
            return Err(TlsError::InvalidCertificate(CertificateError::BadEncoding));
        }
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Trust-all verifier used before pairing. It captures the peer certificate so
/// the caller can pin it after a successful `kdeconnect.pair` exchange.
#[derive(Debug)]
struct TrustAllVerifier {
    capture: Arc<PeerCertCapture>,
    mandatory_client_auth: bool,
}

impl TrustAllVerifier {
    fn new(mandatory_client_auth: bool) -> Self {
        Self {
            capture: Arc::new(PeerCertCapture {
                der: Mutex::new(None),
            }),
            mandatory_client_auth,
        }
    }
}

impl ServerCertVerifier for TrustAllVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        self.capture.capture(end_entity);
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

impl ClientCertVerifier for TrustAllVerifier {
    fn offer_client_auth(&self) -> bool {
        true
    }

    fn client_auth_mandatory(&self) -> bool {
        self.mandatory_client_auth
    }

    fn root_hint_subjects(&self) -> &[RustlsDn] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, TlsError> {
        self.capture.capture(end_entity);
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Reusable TLS configuration factory.
#[derive(Clone, Debug)]
pub struct TlsContext {
    identity: Identity,
}

impl TlsContext {
    pub fn new(identity: Identity) -> Self {
        Self { identity }
    }

    pub fn identity(&self) -> &Identity {
        &self.identity
    }

    fn client_auth(
        &self,
    ) -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>), ProtocolError> {
        Ok((
            vec![CertificateDer::from(self.identity.cert_der.clone())],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
                self.identity.private_key_der.clone(),
            )),
        ))
    }

    /// Trust-all client config for pre-pairing connections.
    pub fn client_config_untrusted(&self) -> Result<Arc<ClientConfig>, ProtocolError> {
        let (certs, key) = self.client_auth()?;
        let verifier = Arc::new(TrustAllVerifier::new(false));
        let config = ClientConfig::builder_with_protocol_versions(&[&rustls::version::TLS12])
            .dangerous()
            .with_custom_certificate_verifier(verifier)
            .with_client_auth_cert(certs, key)
            .map_err(|e| ProtocolError::Tls(e.to_string()))?;
        Ok(Arc::new(config))
    }

    /// Certificate-pinned client config for trusted peers.
    pub fn client_config_trusted(
        &self,
        peer_cert_der: &[u8],
    ) -> Result<Arc<ClientConfig>, ProtocolError> {
        let (certs, key) = self.client_auth()?;
        let verifier = Arc::new(PinnedCertVerifier {
            expected: peer_cert_der.to_vec(),
        });
        let config = ClientConfig::builder_with_protocol_versions(&[&rustls::version::TLS12])
            .dangerous()
            .with_custom_certificate_verifier(verifier)
            .with_client_auth_cert(certs, key)
            .map_err(|e| ProtocolError::Tls(e.to_string()))?;
        Ok(Arc::new(config))
    }

    fn finish_server(
        &self,
        builder: rustls::ConfigBuilder<ServerConfig, rustls::server::WantsServerCert>,
    ) -> Result<Arc<ServerConfig>, ProtocolError> {
        let config = builder
            .with_single_cert(
                vec![CertificateDer::from(self.identity.cert_der.clone())],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
                    self.identity.private_key_der.clone(),
                )),
            )
            .map_err(|e| ProtocolError::Tls(e.to_string()))?;
        Ok(Arc::new(config))
    }

    /// Trust-all server config (client auth requested but optional), used
    /// before the peer is paired.
    pub fn server_config_untrusted(&self) -> Result<Arc<ServerConfig>, ProtocolError> {
        let verifier = Arc::new(TrustAllVerifier::new(false));
        let builder = ServerConfig::builder_with_protocol_versions(&[&rustls::version::TLS12])
            .with_client_cert_verifier(verifier);
        self.finish_server(builder)
    }

    /// Server config that requires the pinned client certificate.
    pub fn server_config_trusted(
        &self,
        peer_cert_der: &[u8],
    ) -> Result<Arc<ServerConfig>, ProtocolError> {
        let verifier = Arc::new(PinnedCertVerifier {
            expected: peer_cert_der.to_vec(),
        });
        let builder = ServerConfig::builder_with_protocol_versions(&[&rustls::version::TLS12])
            .with_client_cert_verifier(verifier);
        self.finish_server(builder)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_identity_is_usable_for_tls() {
        let id = generate_identity("0123456789abcdef0123456789abcdef").unwrap();
        assert!(id.cert_der.len() > 100);
        assert!(id.cert_pem.starts_with("-----BEGIN CERTIFICATE-----"));
        assert!(id.public_key_der.len() > 40);
        let ctx = TlsContext::new(id.clone());
        let client = ctx.client_config_untrusted().unwrap();
        let server = ctx.server_config_untrusted().unwrap();
        assert!(client.alpn_protocols.is_empty());
        assert!(!server.alpn_protocols.is_empty() || server.alpn_protocols.is_empty());
    }

    #[test]
    fn pem_roundtrip() {
        let id = generate_identity("0123456789abcdef0123456789abcdef").unwrap();
        let back = cert_der_from_pem(&id.cert_pem).unwrap();
        assert_eq!(back, id.cert_der);
        let pem = cert_pem_from_der(&back).unwrap();
        assert_eq!(cert_der_from_pem(&pem).unwrap(), back);
    }
}
