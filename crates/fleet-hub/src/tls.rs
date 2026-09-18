//! Terminating TLS in the hub itself, so one binary is reachable from a phone
//! without a proxy in front of it.
//!
//! `fleet-core` owns no TLS stack (see `fleet_core::mcp::TlsAcceptor`); this
//! module is the whole of it. Two modes reach here — `off`, which returns
//! `None` and leaves the server serving plaintext exactly as before, and
//! `cert`, an operator-supplied PEM pair. The third, `auto` (ACME), is refused
//! in [`crate::config`] and has no implementation: its dependency's licence is
//! not in `deny.toml`'s allowlist, so the crate is not in the tree at all.
//!
//! Failures here are startup failures: `serve` turns them into exit 1 with the
//! reason, before the listener is bound. A hub asked for TLS never falls back
//! to plaintext on the port a client expects to be encrypted.

use crate::config::{Resolved, TlsMode};
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use std::path::Path;
use tokio::net::TcpStream;
use tokio_rustls::rustls::ServerConfig;

/// The hub's [`fleet_core::mcp::TlsAcceptor`]: a rustls server config, applied
/// to each connection `fleet-core` accepts.
#[derive(Clone)]
pub struct HubTls(tokio_rustls::TlsAcceptor);

// Only so `Result<Option<HubTls>, String>` can be unwrapped in tests and
// error paths; the acceptor holds key material and prints nothing of it.
impl std::fmt::Debug for HubTls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HubTls")
    }
}

impl fleet_core::mcp::TlsAcceptor for HubTls {
    type Conn = tokio_rustls::server::TlsStream<TcpStream>;

    async fn accept(&self, stream: TcpStream) -> std::io::Result<Self::Conn> {
        self.0.accept(stream).await
    }
}

/// The acceptor `serve` hands `fleet-core`, or `None` when the hub serves
/// plaintext and something in front of it (or nothing, on loopback) is
/// responsible for TLS.
pub fn acceptor(r: &Resolved) -> Result<Option<HubTls>, String> {
    match r.tls {
        TlsMode::Off => Ok(None),
        // `config::resolve` refuses `auto` before anything gets here.
        TlsMode::Auto => Err(crate::config::ACME_UNAVAILABLE.to_string()),
        TlsMode::Cert => {
            // Both are `Some` whenever the mode is `Cert` — `resolve` will not
            // return half a pair — but say so rather than unwrap.
            let (cert, key) = match (&r.tls_cert, &r.tls_key) {
                (Some(c), Some(k)) => (c, k),
                _ => return Err("--tls cert needs both --tls-cert and --tls-key".into()),
            };
            Ok(Some(HubTls(from_pem(cert, key)?)))
        }
    }
}

/// Build the acceptor from a PEM chain and its private key.
fn from_pem(cert: &Path, key: &Path) -> Result<tokio_rustls::TlsAcceptor, String> {
    // Only the `ring` provider is compiled in, so rustls would pick it on its
    // own; installing it explicitly means a future second provider cannot
    // silently change which one serves.
    let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();

    let chain: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(cert)
        .map_err(|e| format!("--tls-cert {}: {e}", cert.display()))?
        .collect::<Result<_, _>>()
        .map_err(|e| format!("--tls-cert {}: {e}", cert.display()))?;
    if chain.is_empty() {
        return Err(format!(
            "--tls-cert {}: no CERTIFICATE block in this file",
            cert.display()
        ));
    }
    let private = PrivateKeyDer::from_pem_file(key)
        .map_err(|e| format!("--tls-key {}: {e}", key.display()))?;
    warn_if_key_is_readable_by_others(key);

    let config = ServerConfig::builder()
        .with_no_client_auth()
        // Catches the commonest misconfiguration there is — a certificate and
        // a key from two different pairs — at startup rather than on the first
        // request from a phone.
        .with_single_cert(chain, private)
        .map_err(|e| {
            format!(
                "--tls-cert {} and --tls-key {} are not a usable pair: {e}",
                cert.display(),
                key.display()
            )
        })?;
    Ok(tokio_rustls::TlsAcceptor::from(std::sync::Arc::new(config)))
}

/// Say so — once, at startup — when the private key is readable by anyone but
/// its owner. Never fatal: the hub may legitimately run on a mounted secret or
/// a file it does not own, and refusing to start over a permission bit would
/// be worse than serving with a warning in the journal.
#[cfg(unix)]
fn warn_if_key_is_readable_by_others(key: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let Ok(meta) = std::fs::metadata(key) else {
        return;
    };
    let mode = meta.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        tracing::warn!(
            path = %key.display(),
            mode = format!("{mode:04o}"),
            "the TLS private key is readable by group or others; chmod 600 it"
        );
    }
}

#[cfg(not(unix))]
fn warn_if_key_is_readable_by_others(_key: &Path) {}

/// A TLS client that does **not** verify the server's certificate, for
/// `fleet-hub healthcheck`.
///
/// Deliberate, and safe only because of what the probe is: an unauthenticated
/// `GET /healthz` to `127.0.0.1` on this machine, carrying no credential and
/// reading one fixed string back. It is a liveness check, not an
/// authentication one — and the certificate a TLS hub presents is issued for
/// its public domain, which `127.0.0.1` will never match, so verification
/// could only ever fail. Nothing else in the hub uses this.
pub fn insecure_probe_client() -> tokio_rustls::TlsConnector {
    use tokio_rustls::rustls::ClientConfig;
    let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
    let config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(std::sync::Arc::new(AcceptAnyServerCert))
        .with_no_client_auth();
    tokio_rustls::TlsConnector::from(std::sync::Arc::new(config))
}

/// The verifier behind [`insecure_probe_client`]. See its doc comment for why
/// asserting validity is the right answer here and nowhere else.
#[derive(Debug)]
struct AcceptAnyServerCert;

impl tokio_rustls::rustls::client::danger::ServerCertVerifier for AcceptAnyServerCert {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &rustls_pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls_pki_types::UnixTime,
    ) -> Result<tokio_rustls::rustls::client::danger::ServerCertVerified, tokio_rustls::rustls::Error>
    {
        Ok(tokio_rustls::rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &tokio_rustls::rustls::DigitallySignedStruct,
    ) -> Result<
        tokio_rustls::rustls::client::danger::HandshakeSignatureValid,
        tokio_rustls::rustls::Error,
    > {
        Ok(tokio_rustls::rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &tokio_rustls::rustls::DigitallySignedStruct,
    ) -> Result<
        tokio_rustls::rustls::client::danger::HandshakeSignatureValid,
        tokio_rustls::rustls::Error,
    > {
        Ok(tokio_rustls::rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<tokio_rustls::rustls::SignatureScheme> {
        tokio_rustls::rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

// `pub(crate)` so `serve`'s healthcheck tests can reuse `self_signed` and
// `cert_resolved` rather than mint a second certificate helper.
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A complete `--tls cert` [`Resolved`], for tests that want the acceptor.
    pub(crate) fn cert_resolved(cert: PathBuf, key: PathBuf) -> Resolved {
        resolved(TlsMode::Cert, Some(cert), Some(key))
    }

    fn resolved(tls: TlsMode, cert: Option<PathBuf>, key: Option<PathBuf>) -> Resolved {
        Resolved {
            data_dir: "/unused".into(),
            bind: "127.0.0.1".parse().unwrap(),
            port: 0,
            public_url: None,
            allowed_hosts: vec![],
            allowed_hosts_explicit: vec![],
            local_host: false,
            allow_plaintext: false,
            log_dir: "/unused/logs".into(),
            tls,
            tls_cert: cert,
            tls_key: key,
        }
    }

    /// A CA and a `localhost` leaf signed by it, written as PEM into `dir`.
    /// Returns the chain path, the key path, and the CA in DER for the test
    /// client's trust store.
    pub(crate) fn self_signed(dir: &Path) -> (PathBuf, PathBuf, CertificateDer<'static>) {
        use rcgen::{
            BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair,
        };
        let ca_key = KeyPair::generate().unwrap();
        let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params
            .distinguished_name
            .push(DnType::CommonName, "fleet-hub test ca");
        let ca = ca_params.self_signed(&ca_key).unwrap();

        let leaf_key = KeyPair::generate().unwrap();
        let mut leaf_params = CertificateParams::new(vec!["localhost".to_string()]).unwrap();
        leaf_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let leaf = leaf_params.signed_by(&leaf_key, &ca, &ca_key).unwrap();

        let cert_path = dir.join("tls.crt");
        let key_path = dir.join("tls.key");
        // Leaf first, then the issuer: the order a server chain is sent in.
        std::fs::write(&cert_path, format!("{}{}", leaf.pem(), ca.pem())).unwrap();
        std::fs::write(&key_path, leaf_key.serialize_pem()).unwrap();
        (cert_path, key_path, ca.der().clone())
    }

    #[test]
    fn off_terminates_nothing() {
        assert!(acceptor(&resolved(TlsMode::Off, None, None))
            .unwrap()
            .is_none());
    }

    /// A world-readable private key is warned about, never fatal: the hub may
    /// legitimately run on a mounted secret it does not own.
    #[cfg(unix)]
    #[test]
    fn a_loosely_permissioned_key_still_starts() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let (cert, key, _) = self_signed(dir.path());
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(acceptor(&cert_resolved(cert.clone(), key.clone()))
            .unwrap()
            .is_some());
        // And 0600 is the quiet path.
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(acceptor(&cert_resolved(cert, key)).unwrap().is_some());
    }

    #[test]
    fn a_pem_pair_builds_an_acceptor() {
        let dir = tempfile::tempdir().unwrap();
        let (cert, key, _) = self_signed(dir.path());
        assert!(acceptor(&resolved(TlsMode::Cert, Some(cert), Some(key)))
            .unwrap()
            .is_some());
    }

    #[test]
    fn a_bad_pem_pair_fails_at_startup_naming_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let (cert, key, _) = self_signed(dir.path());

        let missing = dir.path().join("nope.crt");
        let e = acceptor(&resolved(
            TlsMode::Cert,
            Some(missing.clone()),
            Some(key.clone()),
        ))
        .unwrap_err();
        assert!(e.contains("--tls-cert"), "{e}");
        assert!(e.contains("nope.crt"), "{e}");

        let garbage = dir.path().join("garbage.crt");
        std::fs::write(&garbage, "not a certificate\n").unwrap();
        let e = acceptor(&resolved(TlsMode::Cert, Some(garbage), Some(key))).unwrap_err();
        assert!(e.contains("--tls-cert"), "{e}");

        // A key from a different pair: caught here, not on the first request.
        let other = tempfile::tempdir().unwrap();
        let (_, other_key, _) = self_signed(other.path());
        let e = acceptor(&resolved(TlsMode::Cert, Some(cert), Some(other_key))).unwrap_err();
        assert!(e.contains("not a usable pair"), "{e}");
    }

    /// The whole point of the task: a real HTTPS request to a real hub server,
    /// terminated by the acceptor this module builds.
    #[tokio::test]
    async fn the_hub_serves_https_with_a_supplied_certificate() {
        use fleet_core::events::NoopEventBus;
        use std::sync::{Arc, Mutex};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio_rustls::rustls::pki_types::ServerName;
        use tokio_rustls::rustls::{ClientConfig, RootCertStore};

        let dir = tempfile::tempdir().unwrap();
        let (cert, key, ca) = self_signed(dir.path());
        let tls = acceptor(&resolved(TlsMode::Cert, Some(cert), Some(key)))
            .unwrap()
            .expect("cert mode terminates TLS");

        let store = fleet_core::store::Store::open_with_bus(
            &dir.path().join("state.db"),
            Arc::new(NoopEventBus),
        )
        .unwrap();
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let (shutdown, serve_task) = fleet_core::mcp::start_with_listener(
            Arc::new(Mutex::new(store)),
            Arc::new(fleet_core::ssh::SshClient::new()),
            fleet_core::cancel::CancellationRegistry::new(),
            Arc::new(fleet_core::service::tunnel::TunnelSupervisor::new()),
            fleet_core::mcp::McpGuards::new(Arc::new(
                |_: &fleet_core::mcp::guard::ConfirmRequest| {},
            )),
            listener,
            "test-token".to_string(),
            vec![],
            None,
            Some(tls),
        )
        .await
        .unwrap();

        // A client that trusts only the CA the test just minted, so a
        // successful request proves the server really presented that chain.
        let mut roots = RootCertStore::empty();
        roots.add(ca).unwrap();
        let client = tokio_rustls::TlsConnector::from(Arc::new(
            ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth(),
        ));
        let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
        let mut conn = client
            .connect(ServerName::try_from("localhost").unwrap(), tcp)
            .await
            .expect("TLS handshake");
        conn.write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut raw = Vec::new();
        conn.read_to_end(&mut raw).await.unwrap();
        let text = String::from_utf8_lossy(&raw);
        assert!(text.starts_with("HTTP/1.1 200"), "{text}");
        assert!(text.contains("fleet-hub ok"), "{text}");

        // And nothing plaintext answers on that port.
        let mut plain = tokio::net::TcpStream::connect(addr).await.unwrap();
        plain
            .write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut raw = Vec::new();
        let _ = plain.read_to_end(&mut raw).await;
        assert!(
            !String::from_utf8_lossy(&raw).contains("fleet-hub ok"),
            "a plaintext request must not be answered on the TLS port"
        );

        shutdown.cancel();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), serve_task).await;
    }

    /// `--tls auto` is refused in `config::resolve`, so this module never sees
    /// it; the guard exists so a future `auto` cannot reach a `todo!()`.
    #[test]
    fn auto_never_builds_an_acceptor() {
        let e = acceptor(&resolved(TlsMode::Auto, None, None)).unwrap_err();
        assert!(e.contains("not available in this build"), "{e}");
    }
}
