// SPDX-License-Identifier: Apache-2.0
//! rustls-based TLS configuration for both the
//! [`crate::client`] and [`crate::server`] modules.
//!
//! The client default trusts the [`webpki-roots`][wpr] CA bundle.
//! Custom CAs may be added via
//! [`TlsConfigBuilder::add_ca_pem_file`] or
//! [`TlsConfigBuilder::add_ca_pem_bytes`].
//!
//! Server-side configuration is supplied via
//! [`ServerTlsConfig`], which loads a certificate chain and private
//! key from PEM material.
//!
//! ### Insecure mode (client only)
//!
//! For development against self-signed Logstash deployments the
//! client builder offers
//! [`TlsConfigBuilder::dangerous_disable_verification`]. It disables
//! server-certificate validation entirely — **including hostname
//! verification** — and is **not** suitable for any production
//! deployment. The method name reflects this; the audit trail is in
//! your `Cargo.toml`.
//!
//! [wpr]: https://crates.io/crates/webpki-roots

#![cfg(feature = "tls")]

use std::path::Path;
use std::sync::Arc;

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use rustls::{ClientConfig, ServerConfig};

use crate::ProtocolError;

/// Public-facing TLS configuration. Wraps a [`rustls::ClientConfig`].
///
/// Construct via [`TlsConfig::builder`].
#[derive(Clone, Debug)]
pub struct TlsConfig {
    inner: Arc<ClientConfig>,
}

impl TlsConfig {
    /// Begin building a config — call methods on the returned builder.
    #[must_use]
    pub fn builder() -> TlsConfigBuilder {
        TlsConfigBuilder::default()
    }

    /// The wrapped `rustls::ClientConfig`. Useful if you need to share
    /// the same config with another client.
    #[must_use]
    pub fn inner(&self) -> Arc<ClientConfig> {
        Arc::clone(&self.inner)
    }
}

/// Builder for [`TlsConfig`].
///
/// Default behaviour (no methods called):
///
/// - Trust roots: `webpki-roots` bundled set.
/// - No client authentication.
/// - Cert verification: enabled.
#[derive(Default)]
pub struct TlsConfigBuilder {
    custom_roots: Vec<CertificateDer<'static>>,
    insecure_skip_verification: bool,
}

impl TlsConfigBuilder {
    /// Trust the certificates in the supplied PEM file (CA roots).
    ///
    /// May be called multiple times to add multiple files. If no custom
    /// roots are added the bundled `webpki-roots` set is trusted instead.
    pub fn add_ca_pem_file(self, path: impl AsRef<Path>) -> Result<Self, ProtocolError> {
        let bytes = std::fs::read(path.as_ref())
            .map_err(|e| ProtocolError::Tls(format!("read {}: {e}", path.as_ref().display())))?;
        self.add_ca_pem_bytes(&bytes)
    }

    /// Trust the certificates contained in the supplied PEM bytes.
    pub fn add_ca_pem_bytes(mut self, pem: &[u8]) -> Result<Self, ProtocolError> {
        let parsed = CertificateDer::pem_slice_iter(pem)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| ProtocolError::Tls(format!("parse PEM: {e}")))?;
        if parsed.is_empty() {
            return Err(ProtocolError::Tls(
                "no CERTIFICATE blocks found in PEM input".into(),
            ));
        }
        self.custom_roots.extend(parsed);
        Ok(self)
    }

    /// **Disable** server certificate and hostname verification.
    ///
    /// Use only for development against self-signed deployments. The
    /// method name is intentionally long to discourage casual use; the
    /// resulting client is vulnerable to man-in-the-middle attacks.
    #[must_use]
    pub const fn dangerous_disable_verification(mut self) -> Self {
        self.insecure_skip_verification = true;
        self
    }

    /// Finalise the builder into a [`TlsConfig`].
    pub fn build(self) -> Result<TlsConfig, ProtocolError> {
        if self.insecure_skip_verification {
            tracing::warn!(
                "ferro-lumberjack: TLS server certificate verification is disabled — connection is vulnerable to MITM"
            );
            let provider = rustls::crypto::ring::default_provider();
            let supported = provider
                .signature_verification_algorithms
                .supported_schemes();
            let verifier: Arc<dyn rustls::client::danger::ServerCertVerifier> =
                Arc::new(insecure::AcceptAnyVerifier { supported });
            let cfg = ClientConfig::builder()
                .dangerous()
                .with_custom_certificate_verifier(verifier)
                .with_no_client_auth();
            return Ok(TlsConfig {
                inner: Arc::new(cfg),
            });
        }

        let mut roots = rustls::RootCertStore::empty();
        if self.custom_roots.is_empty() {
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        } else {
            for cert in self.custom_roots {
                roots
                    .add(cert)
                    .map_err(|e| ProtocolError::Tls(format!("add root cert: {e}")))?;
            }
        }
        let cfg = ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        Ok(TlsConfig {
            inner: Arc::new(cfg),
        })
    }
}

// ---------------------------------------------------------------------------
// Server-side TLS
// ---------------------------------------------------------------------------

/// Server-side TLS configuration used by [`crate::server::ServerBuilder::tls`].
///
/// Wraps a [`rustls::ServerConfig`] built from a PEM-encoded certificate
/// chain and private key.
#[derive(Clone, Debug)]
pub struct ServerTlsConfig {
    inner: std::sync::Arc<ServerConfig>,
}

impl ServerTlsConfig {
    /// Begin building a server config.
    #[must_use]
    pub fn builder() -> ServerTlsConfigBuilder {
        ServerTlsConfigBuilder::default()
    }

    /// The wrapped `rustls::ServerConfig`. Useful if you need to share
    /// the same config with another listener.
    #[must_use]
    pub fn inner(&self) -> std::sync::Arc<ServerConfig> {
        std::sync::Arc::clone(&self.inner)
    }
}

/// Builder for [`ServerTlsConfig`].
#[derive(Default)]
pub struct ServerTlsConfigBuilder {
    cert_chain: Vec<CertificateDer<'static>>,
    key: Option<PrivateKeyDer<'static>>,
}

impl std::fmt::Debug for ServerTlsConfigBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerTlsConfigBuilder")
            .field("cert_chain_len", &self.cert_chain.len())
            .field("key_loaded", &self.key.is_some())
            .finish()
    }
}

impl ServerTlsConfigBuilder {
    /// Load the certificate chain from a PEM file. Multiple certificates
    /// in the same file are kept in order.
    pub fn cert_pem_file(self, path: impl AsRef<Path>) -> Result<Self, ProtocolError> {
        let bytes = std::fs::read(path.as_ref()).map_err(|e| {
            ProtocolError::Tls(format!("read cert {}: {e}", path.as_ref().display()))
        })?;
        self.cert_pem_bytes(&bytes)
    }

    /// Load the certificate chain from PEM bytes.
    pub fn cert_pem_bytes(mut self, pem: &[u8]) -> Result<Self, ProtocolError> {
        let parsed = CertificateDer::pem_slice_iter(pem)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| ProtocolError::Tls(format!("parse cert PEM: {e}")))?;
        if parsed.is_empty() {
            return Err(ProtocolError::Tls(
                "no CERTIFICATE blocks found in cert PEM".into(),
            ));
        }
        self.cert_chain.extend(parsed);
        Ok(self)
    }

    /// Load the private key from a PEM file. PKCS#8, RSA, or SEC1
    /// formats are accepted; the first matching block is used.
    pub fn key_pem_file(self, path: impl AsRef<Path>) -> Result<Self, ProtocolError> {
        let bytes = std::fs::read(path.as_ref()).map_err(|e| {
            ProtocolError::Tls(format!("read key {}: {e}", path.as_ref().display()))
        })?;
        self.key_pem_bytes(&bytes)
    }

    /// Load the private key from PEM bytes. PKCS#8, RSA, or SEC1
    /// formats are accepted; the first matching block is used.
    pub fn key_pem_bytes(mut self, pem: &[u8]) -> Result<Self, ProtocolError> {
        let key = PrivateKeyDer::from_pem_slice(pem).map_err(|e| {
            // Surface the "no PRIVATE KEY block found" wording the
            // older `rustls-pemfile`-based API used so existing
            // call-site error matching keeps working.
            ProtocolError::Tls(format!("no PRIVATE KEY block found in key PEM: {e}"))
        })?;
        self.key = Some(key);
        Ok(self)
    }

    /// Finalise the builder.
    pub fn build(self) -> Result<ServerTlsConfig, ProtocolError> {
        if self.cert_chain.is_empty() {
            return Err(ProtocolError::Tls(
                "ServerTlsConfig: no certificate chain configured".into(),
            ));
        }
        let key = self.key.ok_or_else(|| {
            ProtocolError::Tls("ServerTlsConfig: no private key configured".into())
        })?;
        let cfg = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(self.cert_chain, key)
            .map_err(|e| ProtocolError::Tls(format!("build server config: {e}")))?;
        Ok(ServerTlsConfig {
            inner: std::sync::Arc::new(cfg),
        })
    }
}

/// Parse a `host:port` (or `[v6]:port`) literal into a [`ServerName`]
/// suitable for SNI. The port is stripped; brackets around an IPv6
/// literal are stripped before parsing.
pub(crate) fn parse_sni(host_port: &str) -> Result<ServerName<'static>, ProtocolError> {
    let host = host_port
        .rsplit_once(':')
        .map_or(host_port, |(h, _)| h)
        .trim_start_matches('[')
        .trim_end_matches(']');
    ServerName::try_from(host.to_string())
        .map_err(|e| ProtocolError::Tls(format!("invalid server name {host:?}: {e}")))
}

mod insecure {
    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
    use rustls::{DigitallySignedStruct, Error, SignatureScheme};

    #[derive(Debug)]
    pub(super) struct AcceptAnyVerifier {
        pub(super) supported: Vec<SignatureScheme>,
    }

    impl ServerCertVerifier for AcceptAnyVerifier {
        fn verify_server_cert(
            &self,
            _end_entity: &CertificateDer<'_>,
            _intermediates: &[CertificateDer<'_>],
            _server_name: &ServerName<'_>,
            _ocsp_response: &[u8],
            _now: UnixTime,
        ) -> Result<ServerCertVerified, Error> {
            Ok(ServerCertVerified::assertion())
        }

        fn verify_tls12_signature(
            &self,
            _message: &[u8],
            _cert: &CertificateDer<'_>,
            _dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, Error> {
            Ok(HandshakeSignatureValid::assertion())
        }

        fn verify_tls13_signature(
            &self,
            _message: &[u8],
            _cert: &CertificateDer<'_>,
            _dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, Error> {
            Ok(HandshakeSignatureValid::assertion())
        }

        fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
            self.supported.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_builder_uses_webpki_roots() {
        let cfg = TlsConfig::builder().build().expect("build default config");
        // Cannot inspect trust anchors directly through the public API,
        // but `build()` succeeding without panic exercises the path.
        let _ = cfg.inner();
    }

    #[test]
    fn empty_pem_input_is_rejected() {
        let err = TlsConfig::builder()
            .add_ca_pem_bytes(b"")
            .err()
            .expect("must reject empty PEM");
        let msg = err.to_string();
        assert!(msg.contains("no CERTIFICATE"), "{msg}");
    }

    #[test]
    fn malformed_pem_input_is_rejected() {
        let err = TlsConfig::builder()
            .add_ca_pem_bytes(
                b"-----BEGIN CERTIFICATE-----\nnotbase64\n-----END CERTIFICATE-----\n",
            )
            .err()
            .expect("must reject bad PEM");
        let msg = err.to_string();
        // Either the PEM parser rejected the body, or the empty "no certs"
        // path triggered. Either is acceptable.
        assert!(
            msg.contains("parse PEM") || msg.contains("no CERTIFICATE"),
            "{msg}"
        );
    }

    #[test]
    fn parse_sni_strips_port() {
        let sn = parse_sni("logstash.example.com:5044").unwrap();
        assert_eq!(
            format!("{sn:?}"),
            format!(
                "{:?}",
                ServerName::try_from("logstash.example.com").unwrap()
            )
        );
    }

    #[test]
    fn parse_sni_strips_v6_brackets() {
        let sn = parse_sni("[::1]:5044").unwrap();
        let _ = sn;
    }

    #[test]
    fn dangerous_disable_builds() {
        let cfg = TlsConfig::builder()
            .dangerous_disable_verification()
            .build()
            .expect("dangerous build");
        let _ = cfg.inner();
    }

    #[test]
    fn server_builder_requires_cert_chain() {
        let err = ServerTlsConfig::builder()
            .build()
            .expect_err("must require cert");
        assert!(err.to_string().contains("certificate chain"), "{err}");
    }

    #[test]
    fn server_builder_requires_key_after_cert() {
        let params = rcgen::CertificateParams::new(vec!["localhost".to_string()]).unwrap();
        let kp = rcgen::KeyPair::generate().unwrap();
        let cert = params.self_signed(&kp).unwrap();
        let err = ServerTlsConfig::builder()
            .cert_pem_bytes(cert.pem().as_bytes())
            .unwrap()
            .build()
            .expect_err("must require key");
        assert!(err.to_string().contains("private key"), "{err}");
    }

    #[test]
    fn server_builder_round_trip() {
        let params = rcgen::CertificateParams::new(vec!["localhost".to_string()]).unwrap();
        let kp = rcgen::KeyPair::generate().unwrap();
        let cert = params.self_signed(&kp).unwrap();
        let cfg = ServerTlsConfig::builder()
            .cert_pem_bytes(cert.pem().as_bytes())
            .unwrap()
            .key_pem_bytes(kp.serialize_pem().as_bytes())
            .unwrap()
            .build()
            .unwrap();
        let _ = cfg.inner();
    }

    #[test]
    fn server_builder_rejects_empty_cert_pem() {
        let err = ServerTlsConfig::builder()
            .cert_pem_bytes(b"")
            .expect_err("must reject empty cert PEM");
        assert!(err.to_string().contains("no CERTIFICATE"), "{err}");
    }

    #[test]
    fn server_builder_rejects_missing_key_pem() {
        let err = ServerTlsConfig::builder()
            .key_pem_bytes(b"-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n")
            .expect_err("must reject non-key PEM");
        assert!(err.to_string().contains("PRIVATE KEY"), "{err}");
    }

    #[test]
    fn add_self_signed_pem_ok() {
        let params = rcgen::CertificateParams::new(vec!["localhost".to_string()]).expect("params");
        let key = rcgen::KeyPair::generate().expect("keypair");
        let cert = params.self_signed(&key).expect("self-sign");
        let pem = cert.pem();
        let cfg = TlsConfig::builder()
            .add_ca_pem_bytes(pem.as_bytes())
            .expect("parse self-signed pem")
            .build()
            .expect("build");
        let _ = cfg.inner();
    }
}
