//! Which fleet this app is a window onto.
//!
//! Standalone (the default, and everything before this module existed) the app
//! *is* the fleet: its own SQLite database, its own SSH connections, its own
//! reconcile tick and its own embedded control API. Pointed at a `fleet-hub`
//! it becomes a client of someone else's fleet instead, and must not run any
//! of those — two hubs managing one fleet is the failure the remote mode
//! exists to prevent.
//!
//! The choice is made once, at startup, from one setting (`hub.remote_url`)
//! and a client token kept outside the database (see [`token_store`]). It is
//! deliberately not re-read later: a mode flip mid-run would leave half the
//! app talking to a hub and half to the local store.
//!
//! Nothing routes through [`Backend`] yet — this module only makes the choice
//! and makes it observable.

pub mod token_store;

use fleet_core::store::Store;
use std::sync::Mutex;
pub use token_store::{OsTokenStore, TokenStore};

/// The hub's base URL, e.g. `https://fleet.example.com`. Empty or absent means
/// standalone.
pub const REMOTE_URL_KEY: &str = "hub.remote_url";
/// The name this desktop was paired under, shown in Settings. Cosmetic.
pub const CLIENT_NAME_KEY: &str = "hub.client_name";
/// What Settings shows before pairing has told us otherwise.
const DEFAULT_CLIENT_NAME: &str = "desktop";

/// Everything the remote path needs: where the hub is, what to authenticate
/// with, and what this client is called there.
#[derive(Clone, PartialEq, Eq)]
pub struct RemoteConfig {
    /// Normalised: scheme, authority and any path prefix, no trailing slash.
    pub base_url: String,
    /// The client token. Never logged, never serialised, never in `state.db`.
    pub token: String,
    pub client_name: String,
}

/// Hand-written so that a `{:?}` anywhere — a `tracing` field, a panic
/// message, a test failure — cannot spill the token.
impl std::fmt::Debug for RemoteConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteConfig")
            .field("base_url", &self.base_url)
            .field("token", &"<redacted>")
            .field("client_name", &self.client_name)
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Backend {
    Local,
    Remote(RemoteConfig),
}

/// The outcome of resolution: the backend plus, when a hub *was* configured
/// but could not be used, the reason. `resolve` logs the reason; the tests
/// assert on it, so the warning is part of the contract rather than log noise.
#[derive(Debug)]
pub struct Resolution {
    pub backend: Backend,
    pub warning: Option<String>,
}

impl Backend {
    /// Decide once, at startup, from the stored setting and the stored token.
    ///
    /// Falling back to `Local` is the safe direction for every failure except
    /// one: a half-paired app (a URL with no token) would silently go back to
    /// managing hosts itself, which is exactly the double-brain problem. That
    /// case is `Local` too — the app cannot talk to the hub without a token —
    /// but it warns loudly so the operator can finish or undo the pairing.
    pub fn resolve(store: &Mutex<Store>, tokens: &dyn TokenStore) -> Backend {
        let resolved = Self::resolve_detail(store, tokens);
        if let Some(warning) = &resolved.warning {
            tracing::warn!("hub: {warning}");
        }
        resolved.backend
    }

    /// `resolve` without the logging, so tests can see why.
    pub fn resolve_detail(store: &Mutex<Store>, tokens: &dyn TokenStore) -> Resolution {
        // Read both settings under one short lock and drop the guard: nothing
        // below awaits, and nothing below needs the store.
        let settings = {
            match store.lock() {
                Ok(s) => (
                    s.get_setting(REMOTE_URL_KEY),
                    s.get_setting(CLIENT_NAME_KEY),
                ),
                Err(_) => {
                    return Resolution {
                        backend: Backend::Local,
                        warning: Some("the settings store was poisoned; staying standalone".into()),
                    }
                }
            }
        };
        let raw_url = match settings.0 {
            Ok(v) => v.unwrap_or_default(),
            Err(e) => {
                return Resolution {
                    backend: Backend::Local,
                    warning: Some(format!(
                        "cannot read {REMOTE_URL_KEY}: {e}; staying standalone"
                    )),
                }
            }
        };
        let raw_url = raw_url.trim();
        if raw_url.is_empty() {
            // The ordinary standalone install. Not a warning.
            return Resolution {
                backend: Backend::Local,
                warning: None,
            };
        }
        let base_url = match normalise_base_url(raw_url) {
            Ok(u) => u,
            Err(why) => {
                return Resolution {
                    backend: Backend::Local,
                    warning: Some(format!(
                        "{REMOTE_URL_KEY} is not a usable hub address ({why}); \
                         staying standalone — fix it in Settings"
                    )),
                }
            }
        };
        let token = match tokens.get() {
            Ok(Some(t)) => t,
            Ok(None) => {
                return Resolution {
                    backend: Backend::Local,
                    warning: Some(format!(
                        "{base_url} is configured but no client token is stored; \
                         staying standalone — pair again in Settings, or clear the \
                         hub URL to keep managing this fleet locally"
                    )),
                }
            }
            Err(e) => {
                return Resolution {
                    backend: Backend::Local,
                    warning: Some(format!(
                        "cannot read the client token for {base_url} ({e}); staying standalone"
                    )),
                }
            }
        };
        let client_name = settings
            .1
            .ok()
            .flatten()
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| DEFAULT_CLIENT_NAME.to_string());
        Resolution {
            backend: Backend::Remote(RemoteConfig {
                base_url,
                token,
                client_name,
            }),
            warning: None,
        }
    }

    pub fn is_remote(&self) -> bool {
        matches!(self, Backend::Remote(_))
    }

    /// The remote configuration, or `None` when standalone.
    pub fn remote(&self) -> Option<&RemoteConfig> {
        match self {
            Backend::Local => None,
            Backend::Remote(cfg) => Some(cfg),
        }
    }
}

/// Accept only an absolute `http`/`https` URL with a host, and return it
/// without a trailing slash so callers can append `/mcp` or `/events`.
fn normalise_base_url(raw: &str) -> Result<String, String> {
    let parsed = url::Url::parse(raw).map_err(|e| e.to_string())?;
    match parsed.scheme() {
        "http" | "https" => {}
        other => return Err(format!("scheme {other} is not http or https")),
    }
    if parsed.host_str().is_none_or(|h| h.is_empty()) {
        return Err("no host".into());
    }
    Ok(parsed.as_str().trim_end_matches('/').to_string())
}

#[cfg(test)]
mod tests {
    use super::token_store::InMemoryTokenStore;
    use super::*;
    use fleet_core::events::NoopEventBus;
    use std::sync::Arc;

    /// A real on-disk store (the in-memory constructor is `fleet-core`-test
    /// only), seeded with the given settings.
    fn store_with(settings: &[(&str, &str)]) -> (tempfile::TempDir, Mutex<Store>) {
        let dir = tempfile::tempdir().unwrap();
        let store =
            Store::open_with_bus(&dir.path().join("state.db"), Arc::new(NoopEventBus)).unwrap();
        for (k, v) in settings {
            store.set_setting(k, v).unwrap();
        }
        (dir, Mutex::new(store))
    }

    #[test]
    fn no_hub_setting_resolves_local_without_a_warning() {
        let (_dir, store) = store_with(&[]);
        let resolved = Backend::resolve_detail(&store, &InMemoryTokenStore::with_token("tok"));
        assert_eq!(resolved.backend, Backend::Local);
        assert!(!resolved.backend.is_remote());
        assert_eq!(resolved.warning, None);
    }

    #[test]
    fn an_empty_or_blank_hub_url_resolves_local_without_a_warning() {
        for value in ["", "   ", "\n"] {
            let (_dir, store) = store_with(&[(REMOTE_URL_KEY, value)]);
            let resolved = Backend::resolve_detail(&store, &InMemoryTokenStore::with_token("tok"));
            assert_eq!(resolved.backend, Backend::Local, "for {value:?}");
            assert_eq!(resolved.warning, None, "for {value:?}");
        }
    }

    #[test]
    fn a_hub_url_with_a_stored_token_resolves_remote() {
        let (_dir, store) = store_with(&[
            (REMOTE_URL_KEY, "https://fleet.example.com/"),
            (CLIENT_NAME_KEY, "laptop"),
        ]);
        let resolved = Backend::resolve_detail(&store, &InMemoryTokenStore::with_token("cl_tok"));
        assert_eq!(resolved.warning, None);
        assert!(resolved.backend.is_remote());
        let cfg = resolved.backend.remote().expect("remote");
        // Normalised: no trailing slash, so `{base}/mcp` is well formed.
        assert_eq!(cfg.base_url, "https://fleet.example.com");
        assert_eq!(cfg.token, "cl_tok");
        assert_eq!(cfg.client_name, "laptop");
    }

    #[test]
    fn the_client_name_falls_back_when_pairing_never_recorded_one() {
        let (_dir, store) = store_with(&[(REMOTE_URL_KEY, "http://10.0.0.5:8787")]);
        let resolved = Backend::resolve_detail(&store, &InMemoryTokenStore::with_token("t"));
        let cfg = resolved.backend.remote().expect("remote");
        assert_eq!(cfg.base_url, "http://10.0.0.5:8787");
        assert_eq!(cfg.client_name, "desktop");
    }

    #[test]
    fn a_hub_url_with_no_token_resolves_local_and_says_why() {
        let (_dir, store) = store_with(&[(REMOTE_URL_KEY, "https://fleet.example.com")]);
        let resolved = Backend::resolve_detail(&store, &InMemoryTokenStore::empty());
        assert_eq!(resolved.backend, Backend::Local);
        let warning = resolved.warning.expect("a half-paired app must warn");
        assert!(warning.contains("no client token"), "{warning}");
        assert!(warning.contains("fleet.example.com"), "{warning}");
    }

    #[test]
    fn an_unusable_hub_url_resolves_local_and_says_why() {
        for value in [
            "not a url",
            "ftp://fleet.example.com",
            "http://",
            "/just/a/path",
        ] {
            let (_dir, store) = store_with(&[(REMOTE_URL_KEY, value)]);
            let resolved = Backend::resolve_detail(&store, &InMemoryTokenStore::with_token("t"));
            assert_eq!(resolved.backend, Backend::Local, "for {value:?}");
            let warning = resolved.warning.expect("an unusable URL must warn");
            assert!(warning.contains(REMOTE_URL_KEY), "for {value:?}: {warning}");
        }
    }

    #[test]
    fn a_token_store_failure_resolves_local_and_says_why() {
        let (_dir, store) = store_with(&[(REMOTE_URL_KEY, "https://fleet.example.com")]);
        let resolved =
            Backend::resolve_detail(&store, &InMemoryTokenStore::failing("keychain locked"));
        assert_eq!(resolved.backend, Backend::Local);
        let warning = resolved.warning.expect("a keychain failure must warn");
        assert!(warning.contains("keychain locked"), "{warning}");
    }

    #[test]
    fn no_warning_ever_carries_the_token() {
        let (_dir, store) = store_with(&[(REMOTE_URL_KEY, "https://fleet.example.com")]);
        for tokens in [
            InMemoryTokenStore::with_token("s3cret-token"),
            InMemoryTokenStore::empty(),
        ] {
            let resolved = Backend::resolve_detail(&store, &tokens);
            let text = format!("{:?}", resolved);
            assert!(!text.contains("s3cret-token"), "{text}");
        }
    }

    #[test]
    fn debugging_a_remote_config_redacts_the_token() {
        let cfg = RemoteConfig {
            base_url: "https://fleet.example.com".into(),
            token: "s3cret-token".into(),
            client_name: "laptop".into(),
        };
        let shown = format!("{:?}", Backend::Remote(cfg));
        assert!(!shown.contains("s3cret-token"), "{shown}");
        assert!(shown.contains("<redacted>"), "{shown}");
    }
}
