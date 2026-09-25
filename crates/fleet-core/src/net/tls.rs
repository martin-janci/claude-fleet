//! The TLS client configuration every outbound HTTPS connection in the
//! workspace uses: `tokio-rustls` with the `ring` provider only, verified
//! against the **platform trust store** (`rustls-native-certs`).
//!
//! Lifted from `src-tauri/src/backend/remote.rs` with work graph M3.0, so the
//! desktop's hub client and fleet-core's tracker client share one audited
//! stack instead of growing a second one (`reqwest` 0.13 defaults to
//! `aws-lc-rs`, whose licence `deny.toml` does not allow; `webpki-roots`'
//! CDLA-Permissive-2.0 is not on the list either).
//!
//! The platform store rather than a bundled root set is deliberate: a desktop
//! is exactly where a corporate CA or a root the operator installed has to
//! work, and a bundled set would silently reject both. The hub's Docker image
//! ships `ca-certificates` for the same reason (`crates/fleet-hub/Dockerfile`).

/// Build the TLS client config: read the platform trust store and turn it
/// into a connector. Blocking (file I/O, and on macOS a keychain read), so
/// every caller goes through [`tls_connector`], which runs it on the blocking
/// pool.
fn build_tls_connector() -> Result<tokio_rustls::TlsConnector, String> {
    let (roots, errors) = platform_roots();
    if roots.is_empty() {
        // Failing closed. An empty root store would reject every server
        // with an opaque certificate error; saying so once, here, is
        // the difference between a diagnosable problem and a mystery.
        return Err(format!(
            "no usable certificates in this machine's trust store \
             ({errors} error(s) while reading it); an https:// server cannot be verified"
        ));
    }
    Ok(connector_from(roots))
}

/// The platform trust store as a root store, and how many errors reading it
/// produced. Blocking: called only from the two blocking-pool builders.
fn platform_roots() -> (tokio_rustls::rustls::RootCertStore, usize) {
    // Only the `ring` provider is compiled in, so rustls would pick it
    // anyway; installing it explicitly means a future second provider
    // cannot silently change which one is used. Same reasoning, and
    // the same line, as `fleet_hub::tls`.
    let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
    let mut roots = tokio_rustls::rustls::RootCertStore::empty();
    let found = rustls_native_certs::load_native_certs();
    for cert in found.certs {
        // A single unparseable root is not fatal: the store is a bag
        // of certificates from the OS and one bad entry must not stop
        // the app trusting the rest.
        let _ = roots.add(cert);
    }
    (roots, found.errors.len())
}

fn connector_from(roots: tokio_rustls::rustls::RootCertStore) -> tokio_rustls::TlsConnector {
    let config = tokio_rustls::rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    tokio_rustls::TlsConnector::from(std::sync::Arc::new(config))
}

/// Parse PEM certificates (an admin's `extra_ca`). `Err` when there is none
/// or one does not parse.
pub fn parse_pem_certs(
    pem: &str,
) -> Result<Vec<tokio_rustls::rustls::pki_types::CertificateDer<'static>>, String> {
    use tokio_rustls::rustls::pki_types::{pem::PemObject, CertificateDer};
    let certs = CertificateDer::pem_slice_iter(pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("extra_ca is not PEM certificates: {e:?}"))?;
    if certs.is_empty() {
        return Err("extra_ca holds no certificate".into());
    }
    Ok(certs)
}

/// A connector that trusts the platform store PLUS `pem` (work graph M6.5:
/// Jira Data Center behind an internal CA). Built on the blocking pool
/// (the platform store is blocking I/O) and cached per PEM text; a failure
/// is not cached, for the same reason as [`tls_connector`].
pub async fn tls_connector_with_extra_roots(
    pem: &str,
) -> Result<tokio_rustls::TlsConnector, String> {
    use std::collections::HashMap;
    use std::sync::Mutex;
    static CACHE: std::sync::OnceLock<Mutex<HashMap<String, tokio_rustls::TlsConnector>>> =
        std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(c) = cache.lock().ok().and_then(|m| m.get(pem).cloned()) {
        return Ok(c);
    }
    let extra = parse_pem_certs(pem)?;
    let built = tokio::task::spawn_blocking(move || {
        let (mut roots, _) = platform_roots();
        for c in extra {
            roots
                .add(c)
                .map_err(|e| format!("extra_ca cannot be a trust anchor: {e}"))?;
        }
        Ok::<_, String>(connector_from(roots))
    })
    .await
    .map_err(|e| format!("building the TLS config failed: {e}"))??;
    if let Ok(mut m) = cache.lock() {
        if m.len() >= 16 {
            m.clear();
        }
        m.insert(pem.to_string(), built.clone());
    }
    Ok(built)
}

/// The TLS client config, built once and cached.
///
/// Two things this deliberately does NOT do:
///
/// - It does not load the trust store on a runtime worker.
///   `load_native_certs` reads (and on macOS unlocks and queries) the
///   platform store; on a locked-down or network-mounted box that is slow
///   blocking I/O, and a runtime worker parked in it is a worker not running
///   anyone else's future. It goes to [`tokio::task::spawn_blocking`].
/// - It does not cache a FAILURE. A trust store that was momentarily
///   unreadable — a keychain still locked at login, a profile not yet
///   mounted — used to poison every https call for the lifetime of the
///   process, so the app had to be restarted to recover from a condition that
///   had already cleared. Only a success is remembered; a failure is retried
///   on the next call.
///
/// Note that "the platform trust store" is really "the platform trust store,
/// unless the environment says otherwise": `load_native_certs` honours
/// `SSL_CERT_FILE` and `SSL_CERT_DIR` when they are set
/// (`rustls-native-certs`'s `CertPaths::from_env`).
pub async fn tls_connector() -> Result<&'static tokio_rustls::TlsConnector, String> {
    static CONNECTOR: std::sync::OnceLock<tokio_rustls::TlsConnector> = std::sync::OnceLock::new();
    if let Some(ready) = CONNECTOR.get() {
        return Ok(ready);
    }
    let built = tokio::task::spawn_blocking(build_tls_connector)
        .await
        .map_err(|e| format!("reading this machine's trust store failed: {e}"))??;
    // Two callers racing both build one; `get_or_init` keeps whichever
    // arrived first and drops the other. Both are equivalent.
    Ok(CONNECTOR.get_or_init(|| built))
}

#[cfg(test)]
mod tests {
    /// `load_native_certs` is blocking file I/O — and on macOS a keychain
    /// query, which can be slow or prompt. Calling it straight from an
    /// `async fn` parks a runtime worker, and there are only as many workers
    /// as cores.
    ///
    /// A source assertion rather than a behavioural one because the defect is
    /// a *thread*, and nothing in-process can observe which thread a blocking
    /// read happened on. It catches the regression that matters: someone
    /// calling the builder directly again. (Moved from the desktop's
    /// `tests_remote.rs` with the code, M3.0.)
    #[test]
    fn the_trust_store_is_never_loaded_on_a_runtime_worker() {
        let src = include_str!("tls.rs");
        let src = &src[..src.find("#[cfg(test)]").unwrap()];
        let calls: Vec<&str> = src
            .lines()
            .filter(|l| l.contains("build_tls_connector") && !l.trim_start().starts_with("//"))
            .collect();
        assert_eq!(
            calls.len(),
            2,
            "expected exactly the definition and the spawn_blocking call, got: {calls:#?}"
        );
        assert!(
            calls
                .iter()
                .any(|l| l.contains("spawn_blocking(build_tls_connector)")),
            "the only call must go through spawn_blocking: {calls:#?}"
        );
        assert_eq!(
            src.matches("load_native_certs()").count(),
            1,
            "the platform trust store is read in one place only"
        );
        // Its one reader runs only on the blocking pool: from the cached
        // builder, and from the extra-roots builder's spawn_blocking.
        let readers: Vec<&str> = src
            .lines()
            .filter(|l| l.contains("platform_roots()") && !l.trim_start().starts_with("//"))
            .collect();
        assert_eq!(
            readers.len(),
            3,
            "the definition, build_tls_connector and the extra-roots spawn_blocking: {readers:#?}"
        );
    }

    /// The `OnceLock` holds a `TlsConnector`, not a `Result<TlsConnector, _>`:
    /// a trust store that was momentarily unreadable must not poison every
    /// https call for the life of the process. A cell that cannot hold an
    /// error cannot cache one; this test stops the type quietly widening.
    #[test]
    fn a_failed_trust_store_read_is_not_remembered() {
        let src = include_str!("tls.rs");
        assert!(
            src.contains("static CONNECTOR: std::sync::OnceLock<tokio_rustls::TlsConnector>"),
            "CONNECTOR must not be a OnceLock<Result<..>>: a cached failure \
             survives the condition that caused it, and only a restart clears it"
        );
    }
}
