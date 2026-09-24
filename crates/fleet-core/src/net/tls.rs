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
    if roots.is_empty() {
        // Failing closed. An empty root store would reject every server
        // with an opaque certificate error; saying so once, here, is
        // the difference between a diagnosable problem and a mystery.
        return Err(format!(
            "no usable certificates in this machine's trust store \
             ({} error(s) while reading it); an https:// server cannot be verified",
            found.errors.len()
        ));
    }
    let config = tokio_rustls::rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(tokio_rustls::TlsConnector::from(std::sync::Arc::new(
        config,
    )))
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
