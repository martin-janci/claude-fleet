//! The control API's persisted settings, read in one place.
//!
//! Every reader used to re-implement the same three rules — `mcp.enabled`
//! is the literal `"true"`, `mcp.port` falls back to [`DEFAULT_PORT`] when
//! missing or unparseable, and an empty `mcp.token` means "no token" — in
//! seven copies across the commands, the bootstrap, the MCP tools and the
//! diagnostics. They all go through [`McpSettings::read`] now.
//!
//! Callers hold the `Store` lock already (`let s = lock(store)?;`), so these
//! take `&Store` rather than the mutex and never lock themselves.

use super::{generate_token, DEFAULT_PORT, SETTING_ENABLED, SETTING_PORT, SETTING_TOKEN};
use crate::ipc_error::{codes, IpcError};
use crate::store::Store;

/// A snapshot of the `mcp.*` settings rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpSettings {
    /// `mcp.enabled == "true"`.
    pub enabled: bool,
    /// `mcp.port`, or [`DEFAULT_PORT`] when missing / not a `u16`.
    pub port: u16,
    /// The master bearer token; `None` when missing OR empty (an empty
    /// string is how "no token" was persisted historically).
    pub token: Option<String>,
    /// `mcp.confirm_destructive == "true"` (see `guard::SETTING_CONFIRM_DESTRUCTIVE`).
    pub confirm_destructive: bool,
}

impl McpSettings {
    /// Read the four rows. Missing rows take their defaults (off, the
    /// default port, no token, no confirmation).
    pub fn read(s: &Store) -> Result<Self, IpcError> {
        let enabled = s.get_setting(SETTING_ENABLED)?.as_deref() == Some("true");
        let port = s
            .get_setting(SETTING_PORT)?
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(DEFAULT_PORT);
        let token = s.get_setting(SETTING_TOKEN)?.filter(|t| !t.is_empty());
        let confirm_destructive = s
            .get_setting(super::guard::SETTING_CONFIRM_DESTRUCTIVE)?
            .as_deref()
            == Some("true");
        Ok(Self {
            enabled,
            port,
            token,
            confirm_destructive,
        })
    }
}

/// The master token, minting and persisting a fresh one when none is stored
/// (or the stored one is empty). Every path that needs a usable token —
/// the status panel, a server start — goes through here so the API is never
/// tokenless.
pub fn ensure_master_token(s: &Store) -> Result<String, IpcError> {
    match s.get_setting(SETTING_TOKEN)? {
        Some(t) if !t.is_empty() => Ok(t),
        _ => {
            let fresh = generate_token();
            s.set_setting(SETTING_TOKEN, &fresh)?;
            Ok(fresh)
        }
    }
}

/// The configured port, refusing (`E_PROVISION`) when the control API has
/// never been enabled — no master token yet means nothing to provision a
/// host against.
pub fn configured_port(s: &Store) -> Result<u16, IpcError> {
    let cfg = McpSettings::read(s)?;
    if cfg.token.is_none() {
        return Err(IpcError::new(
            codes::E_PROVISION,
            "enable the control API first (no token yet)",
        ));
    }
    Ok(cfg.port)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_defaults_on_an_empty_store() {
        let s = Store::open_in_memory().unwrap();
        assert_eq!(
            McpSettings::read(&s).unwrap(),
            McpSettings {
                enabled: false,
                port: DEFAULT_PORT,
                token: None,
                confirm_destructive: false,
            }
        );
    }

    #[test]
    fn read_parses_rows_and_treats_an_empty_token_as_none() {
        let s = Store::open_in_memory().unwrap();
        s.set_setting(SETTING_ENABLED, "true").unwrap();
        s.set_setting(SETTING_PORT, "4181").unwrap();
        s.set_setting(SETTING_TOKEN, "").unwrap();
        s.set_setting(super::super::guard::SETTING_CONFIRM_DESTRUCTIVE, "true")
            .unwrap();
        let cfg = McpSettings::read(&s).unwrap();
        assert!(cfg.enabled);
        assert_eq!(cfg.port, 4181);
        assert_eq!(cfg.token, None, "empty string means no token");
        assert!(cfg.confirm_destructive);

        s.set_setting(SETTING_PORT, "not-a-port").unwrap();
        s.set_setting(SETTING_TOKEN, "tok").unwrap();
        let cfg = McpSettings::read(&s).unwrap();
        assert_eq!(cfg.port, DEFAULT_PORT, "unparseable port falls back");
        assert_eq!(cfg.token.as_deref(), Some("tok"));
    }

    #[test]
    fn ensure_master_token_mints_once_and_replaces_an_empty_one() {
        let s = Store::open_in_memory().unwrap();
        let first = ensure_master_token(&s).unwrap();
        assert_eq!(first.len(), 64);
        assert_eq!(ensure_master_token(&s).unwrap(), first, "stable once set");
        assert_eq!(
            McpSettings::read(&s).unwrap().token.as_deref(),
            Some(first.as_str())
        );

        s.set_setting(SETTING_TOKEN, "").unwrap();
        let minted = ensure_master_token(&s).unwrap();
        assert_ne!(minted, first);
        assert!(!minted.is_empty());
    }

    #[test]
    fn configured_port_refuses_without_a_token() {
        let s = Store::open_in_memory().unwrap();
        s.set_setting(SETTING_PORT, "4182").unwrap();
        let err = configured_port(&s).unwrap_err();
        assert_eq!(err.code, codes::E_PROVISION);
        s.set_setting(SETTING_TOKEN, "").unwrap();
        assert_eq!(configured_port(&s).unwrap_err().code, codes::E_PROVISION);
        s.set_setting(SETTING_TOKEN, "tok").unwrap();
        assert_eq!(configured_port(&s).unwrap(), 4182);
    }
}
