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
//! There is a third answer, and it is the one that matters most: a hub is
//! configured but this launch cannot use it ([`Backend::Unavailable`]). The
//! app then owns **nothing** — no tick, no usage poll, no control API, and
//! every fleet command refused with the reason — until the operator pairs
//! again or disconnects. Falling back to standalone there would make it a
//! second brain for the hub's fleet.

pub mod connection;
pub mod contract;
pub mod events;
mod http1;
pub mod pairing;
pub mod remote;
pub mod routing;
pub mod startup;
pub mod token_store;
// Generator for `src/lib/hub_verdicts.generated.json` and the refusal table
// in `docs/hub.md`. Test-only, like `fleet_core::mcp::doc_gen` (the
// identical pattern for `REGEN_DOCS`): nothing outside
// `tests_verdict_gen.rs` calls it, so it must not compile into the release
// binary.
#[cfg(test)]
mod verdict_gen;
pub mod verdicts;

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
/// Opt-in for sending **this app's client token** over plain `http://` to a
/// host that is not loopback. Mirrors the hub's own `--allow-plaintext`
/// (`docs/hub.md`), which refuses to serve a routable bind in the clear
/// without being told to.
///
/// The asymmetry this closes: the hub makes plaintext an explicit, named,
/// saved decision; before this key the desktop made it invisible. Someone who
/// typed `http://` instead of `https://` in Settings got a working app that
/// put a fleet-control bearer token on the wire in the clear on every call,
/// forever, with nothing ever saying so.
///
/// **Not the daemon's key**, though both are about plaintext and both live in
/// the same settings table: `fleet_core::service::hub::SETTING_ALLOW_PLAINTEXT`
/// (`hub.allow_plaintext`) says a `fleet-hub` may *serve* a routable bind in
/// the clear, and this one says this *client* may send its own credential
/// that way. They were once the same string, so a `state.db` copied from a
/// daemon to a desktop silently answered a question nobody had asked it.
/// There is no fallback read of the old name, because a fallback would be
/// that same cross-contamination.
///
/// The IPC field on the Tauri command args and in `src/lib/hub.ts` is still
/// `allow_plaintext`: that is a wire name between this app's halves, not a
/// row in anyone's database.
pub const ALLOW_PLAINTEXT_KEY: &str = "hub.client_plaintext_token";

/// `Some(reason)` when reaching this hub would put the bearer token on the
/// wire in the clear — plain `http://` to anything but a loopback address.
/// `None` for `https://`, and for `http://` to loopback, which is the tunnelled
/// or port-forwarded setup and needs no ceremony.
///
/// The host is read with the **same parser the socket is opened from**
/// ([`fleet_proto::net::Endpoint`], via [`remote::Endpoint`]), not with a
/// second one. `url` answers only "is this `http://` at all", because that is
/// the question `normalise_base_url` has already been asked. Two parsers over
/// one string can be talked into seeing two different hosts —
/// `http://evil.example@localhost` is the short version — and the one that
/// must win is the one that picks where the token goes.
///
/// **Fails closed.** An `http://` URL the shared parser refuses is a URL this
/// app cannot reach at all, and that must be reported as a risk rather than
/// as "no risk": a caller that reached here without `normalise_base_url`
/// would otherwise be cleared for a hop nobody inspected.
///
/// Public because pairing hits `POST /pair` with a URL the user just
/// typed, *before* any token is stored and therefore before [`Backend::resolve`]
/// has ever seen it. That path must ask the same question, and must ask it
/// with the same answer.
pub fn plaintext_risk(base_url: &str) -> Option<String> {
    let parsed = url::Url::parse(base_url).ok()?;
    if parsed.scheme() != "http" {
        return None;
    }
    if fleet_proto::net::Endpoint::parse(base_url).is_ok_and(|at| at.is_loopback()) {
        return None;
    }
    Some(format!(
        "{base_url} is plain http to a host that is not loopback, so this \
         app's client token — a credential for the whole fleet — would cross \
         the network in the clear on every call"
    ))
}

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
    Unavailable(UnavailableHub),
}

/// A hub is configured but this launch cannot use it.
///
/// **Configured means `hub.remote_url` is set.** That setting, and nothing
/// else, is what points this app at a hub. The single exception is a settings
/// store that cannot be read at all: the URL is then unknown, and a stored
/// client token is the only remaining evidence that this app was ever paired,
/// so [`Backend::unreadable_settings`] treats it as proof and refuses to guess
/// standalone.
///
/// A stored token beside a **readable, blank** URL is deliberately *not* this
/// state. There is no hub address, so there is nothing to be a client of and
/// no second brain to collide with; that app is standalone and keeps its tick,
/// its usage poll and its control API (`tests_startup`'s
/// `with_no_hub_configured_the_resolved_app_still_starts_all_three`). The
/// token is then a **leftover**, not a configuration — the only thing it needs
/// is a way to be cleared, which is
/// [`crate::commands::hub::hub_stranded_token`] plus Settings' Disconnect,
/// rather than an app that owns nothing and names no hub to pair with again.
///
/// Resolution therefore never reads the token store on a blank URL, and
/// `standalone_resolution_never_reads_the_token_store` below holds it to that:
/// a keychain read on every launch is exactly what prompts, or fails, on a
/// locked macOS keychain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnavailableHub {
    /// The configured hub, normalised. `None` when the stored value is not a
    /// usable URL, or could not be read at all.
    pub url: Option<String>,
    /// Why this launch cannot use it, in words a person can act on.
    pub reason: String,
}

/// The outcome of resolution: the backend plus, when a hub *was* configured
/// but could not be used, the reason. `resolve` logs the reason; the tests
/// assert on it, so the warning is part of the contract rather than log noise.
#[derive(Debug)]
pub struct Resolution {
    pub backend: Backend,
    pub warning: Option<String>,
}

impl Resolution {
    /// A configured hub this launch cannot use. The warning is the reason plus
    /// what it costs, because the reason alone ("no client token is stored")
    /// does not say that this app is now managing nothing.
    fn unavailable(url: Option<String>, reason: String) -> Self {
        let hub = UnavailableHub { url, reason };
        Resolution {
            warning: Some(hub.explain()),
            backend: Backend::Unavailable(hub),
        }
    }
}

impl UnavailableHub {
    /// The reason, and what it means for this process. Used for the startup
    /// log line, the Settings warning and every refused command alike, so the
    /// three can never disagree.
    pub fn explain(&self) -> String {
        format!(
            "{} — this app is set to use a hub, so it is managing no fleet of its \
             own until that is fixed: pair again, or Disconnect, in Settings → Hub",
            self.reason
        )
    }
}

impl Backend {
    /// Decide once, at startup, from the stored setting and the stored token.
    ///
    /// Three outcomes, and the third is the one that matters:
    ///
    /// - no `hub.remote_url` → [`Backend::Local`], the ordinary standalone app;
    /// - a usable hub → [`Backend::Remote`];
    /// - a hub is configured but this launch cannot use it (no stored token, a
    ///   keychain that would not open, plain http without the opt-in, a URL
    ///   that does not parse) → [`Backend::Unavailable`], which owns
    ///   **nothing**.
    ///
    /// That last case used to fall back to `Local`, with the reason in the log
    /// and nowhere a person would see it. `Local` starts the reconcile tick,
    /// the usage poll and the embedded control API, so an app someone had
    /// pointed at a hub quietly became a second brain for the hub's fleet —
    /// the one failure the remote mode exists to prevent, and the likely
    /// trigger is as ordinary as a locked macOS keychain at launch. Falling
    /// back is never safe here; owning nothing and saying why is.
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
                    s.get_setting(ALLOW_PLAINTEXT_KEY),
                ),
                Err(_) => {
                    return Self::unreadable_settings("the settings store was poisoned", tokens)
                }
            }
        };
        let raw_url = match settings.0 {
            Ok(v) => v.unwrap_or_default(),
            Err(e) => {
                return Self::unreadable_settings(
                    &format!("cannot read {REMOTE_URL_KEY}: {e}"),
                    tokens,
                )
            }
        };
        let raw_url = raw_url.trim();
        if raw_url.is_empty() {
            // The ordinary standalone install. Not a warning — and, just as
            // deliberately, NOT a token-store read: there is no hub address
            // here, so a stored token cannot make this app a client of
            // anything, and asking the keychain on every launch is the one
            // question that can prompt or hang on a locked one. A token found
            // in this state is a leftover from a pairing that never finished
            // writing its URL; `hub_stranded_token` is where it is looked for,
            // once, when someone opens Settings.
            return Resolution {
                backend: Backend::Local,
                warning: None,
            };
        }
        // From here a hub IS configured, so every early return is
        // `Unavailable` and never `Local`.
        let base_url = match normalise_base_url(raw_url) {
            Ok(u) => u,
            Err(why) => {
                // The raw value is not repeated: a URL that failed to parse can
                // still carry `user:pass@`, and this text is logged and shown.
                return Resolution::unavailable(
                    None,
                    format!("{REMOTE_URL_KEY} is not a usable hub address ({why})"),
                );
            }
        };
        // Plain http to anything but loopback puts the client token on the
        // wire in the clear. The hub itself refuses that without
        // `--allow-plaintext`; this is the client half of the same decision,
        // and it is made BEFORE the token is read so that a mistyped scheme
        // never reaches a request.
        let plaintext = plaintext_risk(&base_url);
        let allowed = matches!(
            settings.2.as_ref().ok().and_then(|v| v.as_deref()),
            Some("true" | "1" | "yes")
        );
        if let Some(risk) = &plaintext {
            if !allowed {
                return Resolution::unavailable(
                    Some(base_url),
                    format!(
                        "{risk}. Use https://, or set {ALLOW_PLAINTEXT_KEY}=true if this hop \
                         really is already private (a tunnel, a VPN, a container network)"
                    ),
                );
            }
        }

        let token = match tokens.get() {
            Ok(Some(t)) => t,
            Ok(None) => {
                let reason = format!("{base_url} is configured but no client token is stored");
                return Resolution::unavailable(Some(base_url), reason);
            }
            Err(e) => {
                let reason = format!("cannot read the client token for {base_url} ({e})");
                return Resolution::unavailable(Some(base_url), reason);
            }
        };
        let client_name = settings
            .1
            .ok()
            .flatten()
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| DEFAULT_CLIENT_NAME.to_string());
        // An accepted plaintext hub still says so on every launch. The
        // opt-in makes it a decision; the warning keeps it from becoming
        // something nobody remembers deciding.
        let warning = plaintext.map(|risk| {
            format!("{risk} — allowed by {ALLOW_PLAINTEXT_KEY}, so this is deliberate")
        });
        Resolution {
            backend: Backend::Remote(RemoteConfig {
                base_url,
                token,
                client_name,
            }),
            warning,
        }
    }

    /// The hub URL cannot be read, so whether this app was paired is not
    /// known from the settings — except that a stored client token is proof
    /// that it was. Only a token store that answers "none" proves it was not;
    /// one that cannot be read proves nothing, and guessing standalone is the
    /// guess that makes two brains.
    fn unreadable_settings(what: &str, tokens: &dyn TokenStore) -> Resolution {
        match tokens.get() {
            Ok(None) => Resolution {
                backend: Backend::Local,
                warning: Some(format!("{what}; staying standalone")),
            },
            Ok(Some(_)) => Resolution::unavailable(
                None,
                format!("{what}, and this app WAS PAIRED with a hub (a client token is stored)"),
            ),
            Err(e) => Resolution::unavailable(
                None,
                format!(
                    "{what}, and the client token store cannot be read either ({e}), so \
                     this app may be paired with a hub"
                ),
            ),
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
    /// databases drifting apart. Pointed at a hub it cannot use it is not
    /// either — the operator said the hub owns this fleet, and a failure to
    /// reach it does not change whose fleet it is.
    pub fn owns_the_fleet(&self) -> bool {
        matches!(self, Backend::Local)
    }

    /// The remote configuration, or `None` when standalone.
    pub fn remote(&self) -> Option<&RemoteConfig> {
        match self {
            Backend::Local | Backend::Unavailable(_) => None,
            Backend::Remote(cfg) => Some(cfg),
        }
    }

    /// The configured hub this launch cannot use, if that is the state.
    pub fn unavailable(&self) -> Option<&UnavailableHub> {
        match self {
            Backend::Unavailable(hub) => Some(hub),
            _ => None,
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
pub(crate) fn normalise_base_url(raw: &str) -> Result<String, String> {
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

    /// Counts reads, so a test can assert one never happened.
    struct CountingTokenStore {
        inner: InMemoryTokenStore,
        reads: std::sync::atomic::AtomicUsize,
    }

    impl CountingTokenStore {
        fn new(inner: InMemoryTokenStore) -> Self {
            Self {
                inner,
                reads: std::sync::atomic::AtomicUsize::new(0),
            }
        }

        fn reads(&self) -> usize {
            self.reads.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl TokenStore for CountingTokenStore {
        fn get(&self) -> Result<Option<String>, String> {
            self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.inner.get()
        }
        fn set(&self, token: &str) -> Result<(), String> {
            self.inner.set(token)
        }
        fn clear(&self) -> Result<(), String> {
            self.inner.clear()
        }
    }

    /// The decision behind [`UnavailableHub`]'s doc comment, pinned.
    ///
    /// A blank `hub.remote_url` is the ordinary standalone install — the
    /// overwhelming majority of launches — and resolution answers it without
    /// asking the token store at all. Two things break if that changes:
    ///
    /// - every standalone launch starts querying the OS keychain, which is the
    ///   one call that prompts, or blocks, on a locked macOS keychain;
    /// - whatever the answer is, it would have to *mean* something, and the
    ///   only safe meaning is the one already in force — a token with no hub
    ///   address is a leftover, not a configuration. Treating it as a
    ///   configuration gives an app that owns nothing and names no hub to pair
    ///   with again, which is strictly worse than the leftover.
    ///
    /// The leftover is cleared from Settings instead
    /// (`commands::hub::hub_stranded_token`), where the keychain is asked once
    /// and only because a person opened the screen.
    #[test]
    fn standalone_resolution_never_reads_the_token_store() {
        for settings in [
            &[][..],
            &[(REMOTE_URL_KEY, "")][..],
            &[(REMOTE_URL_KEY, " ")][..],
        ] {
            let (_dir, store) = store_with(settings);
            let tokens = CountingTokenStore::new(InMemoryTokenStore::with_token("cl_left_over"));
            let resolved = Backend::resolve_detail(&store, &tokens);
            assert_eq!(
                resolved.backend,
                Backend::Local,
                "a blank hub URL is standalone, whatever the keychain holds: {settings:?}"
            );
            assert!(resolved.backend.owns_the_fleet(), "for {settings:?}");
            assert_eq!(
                tokens.reads(),
                0,
                "standalone resolution asked the token store for {settings:?}; that \
                 puts a keychain query on every launch of every unpaired app, and a \
                 locked keychain then prompts or hangs at startup. A leftover token \
                 is Settings' business (hub_stranded_token), not resolution's."
            );
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

    /// This used to read `http://10.0.0.5:8787`. It is `https` now because
    /// plain http to a routable host became an explicit opt-in
    /// ([`ALLOW_PLAINTEXT_KEY`]) — a change this test caught when it went red,
    /// which is what it should do. Its subject is the client-name fallback,
    /// not the scheme; the plaintext gate has its own tests below.
    #[test]
    fn the_client_name_falls_back_when_pairing_never_recorded_one() {
        let (_dir, store) = store_with(&[(REMOTE_URL_KEY, "https://10.0.0.5:8787")]);
        let resolved = Backend::resolve_detail(&store, &InMemoryTokenStore::with_token("t"));
        let cfg = resolved.backend.remote().expect("remote");
        assert_eq!(cfg.base_url, "https://10.0.0.5:8787");
        assert_eq!(cfg.client_name, "desktop");
    }

    /// The final review's F1: every "configured but cannot be used" case
    /// below used to resolve `Local`, which starts the reconcile tick — a
    /// second brain for the hub's fleet. They resolve `Unavailable` now, which
    /// owns nothing (see `startup`'s tests for the tasks themselves).
    fn assert_unavailable(resolved: &Resolution, url: Option<&str>, context: &str) {
        match &resolved.backend {
            Backend::Unavailable(hub) => {
                assert_eq!(hub.url.as_deref(), url, "{context}");
                assert!(
                    resolved
                        .warning
                        .as_deref()
                        .is_some_and(|w| w.contains(&hub.reason)),
                    "{context}: the reason must reach the warning a person sees: {:?}",
                    resolved.warning
                );
            }
            other => panic!(
                "{context}: a configured hub this launch cannot use resolved to \
                 {other:?}; anything but Unavailable either talks to a hub it \
                 should not or makes this app a second brain for the hub's fleet"
            ),
        }
        assert!(!resolved.backend.owns_the_fleet(), "{context}");
        assert!(!resolved.backend.is_remote(), "{context}");
    }

    #[test]
    fn a_hub_url_with_no_token_is_unavailable_and_says_why() {
        let (_dir, store) = store_with(&[(REMOTE_URL_KEY, "https://fleet.example.com")]);
        let resolved = Backend::resolve_detail(&store, &InMemoryTokenStore::empty());
        assert_unavailable(&resolved, Some("https://fleet.example.com"), "no token");
        let warning = resolved.warning.expect("a half-paired app must warn");
        assert!(warning.contains("no client token"), "{warning}");
        assert!(warning.contains("fleet.example.com"), "{warning}");
    }

    #[test]
    fn an_unusable_hub_url_is_unavailable_and_says_why() {
        for value in [
            "not a url",
            "ftp://fleet.example.com",
            "http://",
            "/just/a/path",
        ] {
            let (_dir, store) = store_with(&[(REMOTE_URL_KEY, value)]);
            let resolved = Backend::resolve_detail(&store, &InMemoryTokenStore::with_token("t"));
            assert_unavailable(&resolved, None, &format!("for {value:?}"));
            let warning = resolved.warning.expect("an unusable URL must warn");
            assert!(warning.contains(REMOTE_URL_KEY), "for {value:?}: {warning}");
        }
    }

    /// The likely real trigger for F1: a locked macOS keychain at launch, or
    /// a keychain prompt the user denied.
    #[test]
    fn a_token_store_failure_is_unavailable_and_says_why() {
        let (_dir, store) = store_with(&[(REMOTE_URL_KEY, "https://fleet.example.com")]);
        let resolved =
            Backend::resolve_detail(&store, &InMemoryTokenStore::failing("keychain locked"));
        assert_unavailable(&resolved, Some("https://fleet.example.com"), "keychain");
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
        assert!(
            !Backend::Unavailable(UnavailableHub {
                url: Some("https://fleet.example.com".into()),
                reason: "no client token is stored".into(),
            })
            .owns_the_fleet(),
            "a configured hub this launch cannot use leaves the app owning nothing"
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
        assert_unavailable(&paired, None, "poisoned, paired");
        let warning = paired.warning.expect("a warning");
        assert!(
            warning.to_lowercase().contains("was paired"),
            "a paired app must be told what just happened: {warning}"
        );
        assert!(!warning.contains("cl_tok"), "{warning}");

        // A token that cannot be read does not prove the app was never
        // paired, so it is the same refusal rather than a guess.
        let unknown =
            Backend::resolve_detail(&store, &InMemoryTokenStore::failing("keychain locked"));
        assert_unavailable(&unknown, None, "poisoned, token unreadable");

        // An app that was never paired gets the plain message; there is no
        // second fleet for it to collide with, so it stays standalone.
        let never = Backend::resolve_detail(&store, &InMemoryTokenStore::empty());
        assert_eq!(never.backend, Backend::Local);
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

    /// Loopback is the tunnelled or port-forwarded hub and needs no
    /// ceremony; anything else on plain http puts a fleet-wide credential on
    /// the wire in the clear.
    #[test]
    fn plaintext_is_a_risk_everywhere_except_loopback() {
        for safe in [
            "https://fleet.example.com",
            "https://10.0.0.5:8443",
            "http://127.0.0.1:8787",
            "http://127.0.0.5:8787",
            "http://localhost:8787",
            "http://LOCALHOST:8787",
            "http://[::1]:8787",
        ] {
            assert_eq!(plaintext_risk(safe), None, "for {safe}");
        }
        for risky in [
            "http://fleet.example.com",
            "http://10.0.0.5:8787",
            "http://[2001:db8::1]:8787",
            // Not loopback: a name that merely *contains* localhost.
            "http://localhost.evil.example.com",
            // Nor a name UNDER `localhost`. RFC 6761 says a resolver should
            // keep the subtree on the machine, but "should" is not "must" —
            // musl's does not, so a DNS server that answers for
            // `<anything>.localhost` would choose where this app's
            // fleet-wide token goes. Such a URL now needs the plaintext
            // opt-in like any other, which is the fail-safe direction.
            "http://hub.localhost:8787",
            "http://localhost.localhost:8787",
            // An IPv4-mapped v6 address is routable, not `::1`.
            "http://[::ffff:127.0.0.1]:8787",
        ] {
            let risk = plaintext_risk(risky).unwrap_or_else(|| panic!("{risky} must be a risk"));
            assert!(risk.contains("in the clear"), "for {risky}: {risk}");
        }
    }

    #[test]
    fn a_plaintext_hub_is_refused_until_it_is_opted_into() {
        let (_dir, store) = store_with(&[(REMOTE_URL_KEY, "http://fleet.example.com")]);
        let resolved = Backend::resolve_detail(&store, &InMemoryTokenStore::with_token("cl_tok"));
        // Plain http to a routable host must not carry the token silently —
        // and must not turn the app into a second brain either.
        assert_unavailable(&resolved, Some("http://fleet.example.com"), "plaintext");
        let warning = resolved.warning.expect("a warning");
        assert!(warning.contains("in the clear"), "{warning}");
        assert!(
            warning.contains(ALLOW_PLAINTEXT_KEY),
            "the warning must name the opt-in, or the user cannot act on it: {warning}"
        );
    }

    /// Opting in works, and still says so every launch — the point is that it
    /// is a decision someone made, not that it goes quiet afterwards.
    #[test]
    fn an_opted_in_plaintext_hub_connects_and_keeps_saying_so() {
        for value in ["true", "1", "yes"] {
            let (_dir, store) = store_with(&[
                (REMOTE_URL_KEY, "http://fleet.example.com"),
                (ALLOW_PLAINTEXT_KEY, value),
            ]);
            let resolved =
                Backend::resolve_detail(&store, &InMemoryTokenStore::with_token("cl_tok"));
            assert!(resolved.backend.is_remote(), "for {value}");
            let warning = resolved.warning.expect("an opted-in hub still warns");
            assert!(warning.contains("deliberate"), "for {value}: {warning}");
        }
    }

    /// **The credential gate and the connect path must read one string one
    /// way.** `plaintext_risk` answers "may this app's fleet-wide token go
    /// out in the clear"; `remote::Endpoint` picks the host the socket is
    /// actually opened to. Two parsers over one string that agree by luck is
    /// exactly the shape #159 exists to remove, so this pins the invariant
    /// rather than leaving it to `normalise_base_url` happening to run first:
    ///
    /// > whenever `plaintext_risk` clears an `http://` URL, the connect path
    /// > parses the same string AND lands on a loopback host.
    ///
    /// The probes are the spellings where a URL parser can be talked into
    /// seeing a different host from the next one — userinfo that hides the
    /// real authority, and the short/hex/decimal IPv4 forms that `inet_aton`
    /// resolves to 127.0.0.1 but `Ipv4Addr::from_str` does not.
    #[test]
    fn whatever_plaintext_risk_clears_the_connect_path_reads_the_same_way() {
        let probes = [
            // Ordinary, and cleared.
            "http://localhost:8787",
            "http://127.0.0.1:8787",
            "http://[::1]:4180",
            "https://anything.example",
            "https://fleet.example.com/mcp",
            // Userinfo hiding the authority one parser reads.
            "http://evil.example@localhost",
            "http://localhost@evil.example",
            // Short, hex and decimal IPv4: `inet_aton` reaches 127.0.0.1,
            // Rust's parser does not.
            "http://127.1",
            "http://0x7f.0.0.1",
            "http://2130706433",
            // Names that merely look local.
            "http://localhost.evil.example",
            "http://127.0.0.1.evil.example",
            "http://x.localhost",
            "http://localhost.",
            "http://evil.example",
            // Local in fact, but not by this rule.
            "http://[::ffff:127.0.0.1]",
        ];
        for probe in probes {
            // Both the raw string and the form this app actually stores —
            // the invariant must not depend on `normalise_base_url` having
            // run.
            let forms = std::iter::once(probe.to_string()).chain(normalise_base_url(probe));
            for url in forms {
                if plaintext_risk(&url).is_some() {
                    // Naming a risk is always a safe answer.
                    continue;
                }
                if url.starts_with("https://") {
                    // TLS: the host does not have to be this machine.
                    continue;
                }
                let at = remote::Endpoint::parse(&url).unwrap_or_else(|e| {
                    panic!("{url}: cleared as no risk, but the connect path refuses it: {e}")
                });
                assert!(
                    fleet_proto::net::is_loopback(at.host()),
                    "{url}: cleared as no risk, but the socket would go to {}",
                    at.host()
                );
            }
        }
    }

    /// The daemon's `hub.allow_plaintext` and this app's opt-in were the same
    /// string in the same table, and they mean different things: the daemon's
    /// is "serve a routable bind in the clear", this app's is "put MY client
    /// token on the wire in the clear". A `state.db` copied from a hub to a
    /// desktop therefore carried one decision into the other. There is
    /// deliberately no fallback read of the old key — a fallback would be
    /// exactly the cross-contamination.
    #[test]
    fn the_daemons_plaintext_key_does_not_opt_this_app_in() {
        assert_ne!(
            ALLOW_PLAINTEXT_KEY,
            fleet_core::service::hub::SETTING_ALLOW_PLAINTEXT,
            "the two settings must not share a row"
        );
        let (_dir, store) = store_with(&[
            (REMOTE_URL_KEY, "http://fleet.example.com"),
            (fleet_core::service::hub::SETTING_ALLOW_PLAINTEXT, "true"),
        ]);
        let resolved = Backend::resolve_detail(&store, &InMemoryTokenStore::with_token("cl_tok"));
        assert_unavailable(
            &resolved,
            Some("http://fleet.example.com"),
            "the daemon's key must not opt this app in",
        );
    }

    /// A loopback hub is the tunnelled setup `docs/hub.md` describes. It must
    /// keep working with no setting at all, or the opt-in becomes a tax on
    /// the normal development case.
    #[test]
    fn a_loopback_plaintext_hub_needs_no_opt_in_and_no_warning() {
        let (_dir, store) = store_with(&[(REMOTE_URL_KEY, "http://127.0.0.1:8787")]);
        let resolved = Backend::resolve_detail(&store, &InMemoryTokenStore::with_token("cl_tok"));
        assert!(resolved.backend.is_remote());
        assert_eq!(resolved.warning, None);
    }

    /// The refusal happens before the token is read, so a mistyped scheme
    /// cannot put it into a request even once.
    #[test]
    fn a_plaintext_refusal_never_carries_the_token() {
        let (_dir, store) = store_with(&[(REMOTE_URL_KEY, "http://fleet.example.com")]);
        let resolved =
            Backend::resolve_detail(&store, &InMemoryTokenStore::with_token("s3cret-token"));
        assert!(!format!("{resolved:?}").contains("s3cret-token"));
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
