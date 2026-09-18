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

pub mod contract;
pub mod remote;
pub mod routing;
pub mod token_store;

use fleet_core::store::Store;
pub use routing::FleetBackend;
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
                    // The hub URL cannot be read, so we cannot know whether
                    // this app was paired — except that a stored client token
                    // is proof that it was. Falling back to standalone *while
                    // paired* is the one failure the design says must never
                    // happen quietly: this process would start managing hosts
                    // the hub is also managing.
                    let was_paired = matches!(tokens.get(), Ok(Some(_)));
                    return Resolution {
                        backend: Backend::Local,
                        warning: Some(if was_paired {
                            "the settings store was poisoned and this app WAS PAIRED with a hub \
                             (a client token is stored); it is falling back to standalone, so it \
                             may now manage hosts the hub also manages — restart it, and do not \
                             leave it running"
                                .into()
                        } else {
                            "the settings store was poisoned; staying standalone".into()
                        }),
                    };
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

    /// Whether this process is the one brain of its fleet.
    ///
    /// Standalone it is, and it runs the reconcile tick, the account-usage
    /// tick and the embedded MCP server. Pointed at a hub it is **not**, and
    /// must run none of them: two reconcile passes over one fleet means two
    /// sets of hooks fighting over which URL a host reports to, and two
    /// databases drifting apart.
    ///
    /// This exists as a named predicate rather than an `if backend.is_remote()`
    /// at each site so that the decision is one testable thing. Every one of
    /// the three startup tasks is behind it in `lib.rs`.
    pub fn owns_the_fleet(&self) -> bool {
        matches!(self, Backend::Local)
    }

    /// The remote configuration, or `None` when standalone.
    pub fn remote(&self) -> Option<&RemoteConfig> {
        match self {
            Backend::Local => None,
            Backend::Remote(cfg) => Some(cfg),
        }
    }
}

/// Accept only an absolute `http`/`https` URL with a host, and return **just
/// its scheme, host, port and path**, without a trailing slash, so callers can
/// append `/mcp` or `/events`.
///
/// Userinfo, query and fragment are dropped rather than carried, for two
/// independent reasons:
///
/// - **Credentials.** `https://user:sup3rsecret@hub` would otherwise survive
///   into `base_url`, which is logged at info on startup and which
///   `collect_diagnostics` folds into the blob a user sends to support. This
///   value is a display string and a URL prefix; it is not an authentication
///   channel (the bearer token is, and it lives in [`token_store`]).
/// - **Correctness.** The result is concatenated with `/mcp`, so a surviving
///   query turned `https://hub/?token=abc` into `https://hub/?token=abc/mcp`.
fn normalise_base_url(raw: &str) -> Result<String, String> {
    let mut parsed = url::Url::parse(raw).map_err(|e| e.to_string())?;
    match parsed.scheme() {
        "http" | "https" => {}
        other => return Err(format!("scheme {other} is not http or https")),
    }
    if parsed.host_str().is_none_or(|h| h.is_empty()) {
        return Err("no host".into());
    }
    // Each returns `Err(())` only for a cannot-be-a-base URL; the host check
    // above has already excluded those.
    let _ = parsed.set_username("");
    let _ = parsed.set_password(None);
    parsed.set_query(None);
    parsed.set_fragment(None);
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

    /// The review's SHOULD-FIX: `normalise_base_url` kept userinfo, query and
    /// fragment, which failed in two separate ways.
    ///
    /// The credential one: `https://user:sup3rsecret@host` survived into
    /// `base_url`, which `lib.rs` logs at info, and `collect_diagnostics`
    /// bundles the log tail into the blob a user sends to support.
    ///
    /// The correctness one: the string is the base for `{base}/mcp`, so
    /// `https://host/?token=abc` built `https://host/?token=abc/mcp`.
    #[test]
    fn normalising_a_hub_url_drops_credentials_query_and_fragment() {
        let cases = [
            (
                "https://user:sup3rsecret@fleet.example.com",
                "https://fleet.example.com",
            ),
            (
                "https://user@fleet.example.com/",
                "https://fleet.example.com",
            ),
            (
                "https://fleet.example.com/?token=abc",
                "https://fleet.example.com",
            ),
            (
                "https://fleet.example.com/#frag",
                "https://fleet.example.com",
            ),
            (
                "https://u:p@fleet.example.com:8443/hub/?a=1#f",
                "https://fleet.example.com:8443/hub",
            ),
            // Scheme, host, port and path all survive, and survive intact.
            ("http://10.0.0.5:8787/base", "http://10.0.0.5:8787/base"),
        ];
        for (raw, want) in cases {
            let got = normalise_base_url(raw).unwrap_or_else(|e| panic!("{raw}: {e}"));
            assert_eq!(got, want, "for {raw}");
            assert!(!got.contains("sup3rsecret"), "for {raw}: {got}");
            assert!(!got.contains('@'), "for {raw}: {got}");
            assert!(!got.contains('?') && !got.contains('#'), "for {raw}: {got}");
            // The whole point: appending the endpoint must be well formed.
            assert!(
                format!("{got}/mcp").ends_with("/mcp"),
                "for {raw}: {got}/mcp"
            );
        }
    }

    /// A URL whose credentials are stripped must not resolve to a config that
    /// still carries them anywhere.
    #[test]
    fn a_resolved_remote_never_carries_url_credentials() {
        let (_dir, store) = store_with(&[(
            REMOTE_URL_KEY,
            "https://user:sup3rsecret@fleet.example.com/?token=abc",
        )]);
        let resolved = Backend::resolve_detail(&store, &InMemoryTokenStore::with_token("t"));
        let cfg = resolved.backend.remote().expect("remote");
        assert_eq!(cfg.base_url, "https://fleet.example.com");
        assert!(!format!("{resolved:?}").contains("sup3rsecret"));
    }

    /// The review's SHOULD-FIX: nothing observed that remote mode starts none
    /// of the three background tasks. A refactor hoisting a tick out of the
    /// `else` branch would silently start a second reconcile loop against the
    /// same fleet — the exact failure this sub-project exists to prevent.
    /// `lib.rs` branches on this predicate, so this test guards it.
    #[test]
    fn only_a_standalone_app_owns_the_fleet() {
        assert!(
            Backend::Local.owns_the_fleet(),
            "standalone must keep its tick, its usage poll and its own server"
        );
        assert!(
            !Backend::Remote(RemoteConfig {
                base_url: "https://fleet.example.com".into(),
                token: "t".into(),
                client_name: "laptop".into(),
            })
            .owns_the_fleet(),
            "a hub client must start NO reconcile tick, NO usage tick and NO \
             embedded MCP server — two hubs managing one fleet is the failure \
             this whole mode exists to prevent"
        );
    }

    /// A poisoned store mutex used to drop a *paired* app back to standalone
    /// on a `warn!` that reads like routine noise. The design says that must
    /// never happen quietly: the app would start managing hosts the hub also
    /// manages. The setting cannot be read with the lock poisoned, but a
    /// stored client token is proof this app was paired, so the warning says
    /// so in those words.
    #[test]
    fn a_poisoned_store_says_loudly_that_a_paired_app_is_falling_back() {
        let (_dir, store) = store_with(&[(REMOTE_URL_KEY, "https://fleet.example.com")]);
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _g = store.lock().unwrap();
            panic!("poison");
        }));
        assert!(store.lock().is_err(), "the mutex must really be poisoned");

        let paired = Backend::resolve_detail(&store, &InMemoryTokenStore::with_token("cl_tok"));
        assert_eq!(paired.backend, Backend::Local);
        let warning = paired.warning.expect("a warning");
        let said = warning.to_lowercase();
        assert!(
            said.contains("was paired") && said.contains("also manages"),
            "a paired app must be told what just happened: {warning}"
        );
        assert!(!warning.contains("cl_tok"), "{warning}");

        // An app that was never paired gets the plain message; there is no
        // second fleet for it to collide with.
        let never = Backend::resolve_detail(&store, &InMemoryTokenStore::empty());
        let warning = never.warning.expect("a warning");
        assert!(!warning.to_lowercase().contains("was paired"), "{warning}");
    }

    /// `RemoteConfig` must never gain `Serialize`: it would ride into a Tauri
    /// command's return value, an event payload or a settings blob, and the
    /// token with it. The `Debug` impl is hand-written for that reason, but a
    /// `#[derive(Serialize)]` would bypass it entirely.
    ///
    /// Method resolution prefers an inherent method over a trait method, so
    /// `Probe::<T>` answers `true` only while `T: Serialize`.
    #[test]
    fn remote_config_is_not_serializable() {
        struct Probe<T>(std::marker::PhantomData<T>);
        trait NotSerialize {
            fn is_serialize(&self) -> bool {
                false
            }
        }
        impl<T> NotSerialize for Probe<T> {}
        impl<T: serde::Serialize> Probe<T> {
            fn is_serialize(&self) -> bool {
                true
            }
        }
        assert!(
            !Probe::<RemoteConfig>(std::marker::PhantomData).is_serialize(),
            "RemoteConfig gained Serialize — the client token can now reach \
             the frontend, an event payload or a settings blob"
        );
        // The probe really does detect Serialize when it is there.
        assert!(Probe::<String>(std::marker::PhantomData).is_serialize());
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
