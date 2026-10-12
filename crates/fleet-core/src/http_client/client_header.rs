//! `X-Fleet-Client` for every request a [`super::TcpTransport`] makes. Its
//! own file because `mod.rs` holds no process-wide state of its own (the
//! trust-store test there forbids a `OnceLock`, which is about TLS caches,
//! not this).

/// `X-Fleet-Client` (update-channel design §6.3): what this process is,
/// sent on every request a [`super::TcpTransport`] makes once the embedding app has
/// said ([`set_client_header`]). The desktop sets it at start; the hub never
/// does, so a hub dialling a peer names nothing.
static CLIENT_HEADER: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Name this process to every hub it calls. Set once; a value with a line
/// break (which would end the header block) is refused.
pub fn set_client_header(value: String) -> Result<(), String> {
    if value.contains(['\r', '\n']) || value.is_empty() {
        return Err("an X-Fleet-Client value is one line".into());
    }
    CLIENT_HEADER
        .set(value)
        .map_err(|_| "X-Fleet-Client is already set".to_string())
}

/// The header line, with its CRLF, or nothing.
pub(super) fn client_header_line() -> String {
    CLIENT_HEADER
        .get()
        .map(|v| format!("{}: {v}\r\n", fleet_update::client_header::CLIENT_HEADER))
        .unwrap_or_default()
}
