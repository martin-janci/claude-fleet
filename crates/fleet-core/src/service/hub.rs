//! Where the hub is reachable, as every host must be told: the base URL its
//! hooks and MCP entry point at. Loopback + reverse tunnel on the desktop;
//! a public URL on a `fleet-hub` daemon (no tunnels then).

use crate::ipc_error::{codes, IpcError};
use crate::store::Store;
use std::sync::atomic::{AtomicBool, Ordering};

pub const SETTING_BIND: &str = "hub.bind";
pub const SETTING_PUBLIC_URL: &str = "hub.public_url";
pub const SETTING_ALLOWED_HOSTS: &str = "hub.allowed_hosts";
pub const SETTING_LOCAL_HOST: &str = "hub.local_host";
/// Written by `fleet-hub` only: whether a plaintext routable bind is allowed
/// — that is, whether this daemon may *serve* in the clear.
///
/// **Not the desktop's key.** The desktop has its own plaintext opt-in,
/// `hub.client_plaintext_token` (`src-tauri/src/backend/mod.rs`'s
/// `ALLOW_PLAINTEXT_KEY`), and it answers the opposite question: whether this
/// *client* may send its own token in the clear. They were once the same
/// string, so a `state.db` copied between a daemon and a desktop carried one
/// decision into the other; the desktop reads its own name and no fallback.
pub const SETTING_ALLOW_PLAINTEXT: &str = "hub.allow_plaintext";
/// Written by `fleet-hub` only: how the daemon terminates TLS itself —
/// `off` (a proxy in front) or `cert` (an operator-supplied PEM pair).
pub const SETTING_TLS: &str = "hub.tls";
/// Written by `fleet-hub` only: the PEM certificate chain for `hub.tls=cert`.
pub const SETTING_TLS_CERT: &str = "hub.tls_cert";
/// Written by `fleet-hub` only: the PEM private key for `hub.tls=cert`.
pub const SETTING_TLS_KEY: &str = "hub.tls_key";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HubBase {
    /// `http://127.0.0.1:<port>` or the public URL, no trailing slash.
    pub url: String,
    /// The listening port (the tunnel's remote port on the desktop).
    pub port: u16,
    /// True for a public URL: hooks post to it directly, no tunnel.
    pub public: bool,
}

impl HubBase {
    pub fn loopback(port: u16) -> Self {
        Self {
            url: format!("http://127.0.0.1:{port}"),
            port,
            public: false,
        }
    }

    /// A public base: `http(s)://host[:port]`, nothing after the authority
    /// (a single trailing `/` is tolerated and stripped). Lower-cased.
    ///
    /// The value is written into every host's hook block and MCP entry,
    /// printed by `fleet-hub init` and logged by `serve`, so anything beyond a
    /// bare authority is refused: userinfo (`user:pass@`), an empty host, a
    /// port that is not a u16, whitespace, a path, a query or a fragment.
    pub fn public(url: &str, port: u16) -> Result<Self, IpcError> {
        let lowered = url.trim().to_ascii_lowercase();
        let (scheme, rest) = if let Some(r) = lowered.strip_prefix("https://") {
            ("https", r)
        } else if let Some(r) = lowered.strip_prefix("http://") {
            ("http", r)
        } else {
            return Err(IpcError::new(
                codes::E_VALIDATE,
                "public URL must start with http:// or https://",
            ));
        };
        let authority = rest.strip_suffix('/').unwrap_or(rest);
        let invalid = |why: &str| {
            IpcError::new(
                codes::E_VALIDATE,
                format!("public URL must be scheme://host[:port] with nothing else: {why}"),
            )
        };
        if authority.chars().any(char::is_whitespace) {
            return Err(invalid("it contains whitespace"));
        }
        if authority.contains('@') {
            return Err(invalid("credentials (user@) are not allowed"));
        }
        if authority.contains(['/', '?', '#']) {
            return Err(invalid("no path, query or fragment"));
        }
        let parsed: axum::http::uri::Authority = authority
            .parse()
            .map_err(|e| invalid(&format!("'{authority}' is not a host[:port] ({e})")))?;
        if parsed.host().is_empty() {
            return Err(invalid("the host is empty"));
        }
        // The only acceptable text after the host is `:<digits fitting a u16>`;
        // do not rely on `Authority` for that (a bare `:` parses).
        let after_host = parsed
            .as_str()
            .strip_prefix(parsed.host())
            .unwrap_or(authority);
        let port_ok = match after_host.strip_prefix(':') {
            None => after_host.is_empty(),
            Some(p) => {
                !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) && p.parse::<u16>().is_ok()
            }
        };
        if !port_ok {
            return Err(invalid("the port must be a number from 0 to 65535"));
        }
        Ok(Self {
            url: format!("{scheme}://{}", parsed.as_str()),
            port,
            public: true,
        })
    }

    /// From settings: `hub.public_url` when set, else loopback on `mcp.port`.
    /// Refuses (`E_PROVISION`) when the control API has no master token yet.
    pub fn read(s: &Store) -> Result<Self, IpcError> {
        let port = crate::mcp::settings::configured_port(s)?;
        match s
            .get_setting(SETTING_PUBLIC_URL)?
            .filter(|u| !u.trim().is_empty())
        {
            Some(url) => Self::public(&url, port),
            None => Ok(Self::loopback(port)),
        }
    }

    pub fn mcp_url(&self) -> String {
        format!("{}/mcp", self.url)
    }

    pub fn hook_url(&self) -> String {
        format!("{}/hook", self.url)
    }

    /// The authority part (`host[:port]`), for the Host allowlist.
    pub fn host(&self) -> String {
        self.url
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .to_string()
    }
}

/// `hub.local_host`: whether this hub's own machine is a fleet host.
/// Unset -> true (the desktop); the daemon sets it false by default.
///
/// `false`, `0`, `no` and `off` all read as off, case- and
/// whitespace-insensitively; anything else — including a value nobody meant as
/// a boolean — keeps the default of on, so a typo never silently detaches a
/// desktop from its own machine. `fleet-hub` itself persists only the
/// canonical `"true"` / `"false"`; this is for a hand-edited or older state.db.
pub fn read_local_host(s: &Store) -> bool {
    match s.get_setting(SETTING_LOCAL_HOST).ok().flatten() {
        Some(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "false" | "0" | "no" | "off"
        ),
        None => true,
    }
}

/// Set once `hub.local_host` is off: every explicit `local` target is refused.
static LOCAL_HOST_DISABLED: AtomicBool = AtomicBool::new(false);

/// The refusal for a `local` target on a hub without a local host.
const LOCAL_DISABLED_MESSAGE: &str = "host local is disabled on this hub (hub.local_host=false)";

/// Turn the `local` host off for this process. Called once by `fleet-hub
/// serve` when `hub.local_host` is false; the desktop never calls it.
/// Idempotent, and there is no way back: a process that disabled `local`
/// never runs anything on its own machine as a fleet host.
pub fn disable_local_host() {
    LOCAL_HOST_DISABLED.store(true, Ordering::SeqCst);
}

/// False only after [`disable_local_host`]; always true on the desktop.
pub fn local_host_enabled() -> bool {
    !LOCAL_HOST_DISABLED.load(Ordering::SeqCst)
}

/// Pure guard: `E_NOTFOUND` iff `alias` is `local` and the local host is off.
pub fn check_local_allowed(alias: &str, local_enabled: bool) -> Result<(), IpcError> {
    if alias == crate::service::projects::LOCAL_HOST && !local_enabled {
        return Err(IpcError::new(codes::E_NOTFOUND, LOCAL_DISABLED_MESSAGE));
    }
    Ok(())
}

/// [`check_local_allowed`] against this process's flag. Every entry point
/// that takes a host alias reaches it through `validate::host_alias`; every
/// branch that spawns on, or touches files of, the `local` host checks it
/// again before doing so.
pub fn ensure_local_allowed(alias: &str) -> Result<(), IpcError> {
    check_local_allowed(alias, local_host_enabled())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    #[test]
    fn loopback_base_uses_the_port() {
        let b = HubBase::loopback(4180);
        assert_eq!(b.url, "http://127.0.0.1:4180");
        assert_eq!(b.mcp_url(), "http://127.0.0.1:4180/mcp");
        assert_eq!(b.hook_url(), "http://127.0.0.1:4180/hook");
        assert_eq!(b.host(), "127.0.0.1:4180");
        assert!(!b.public);
    }

    #[test]
    fn public_base_strips_trailing_slash_and_validates() {
        let b = HubBase::public("https://Fleet.Example.com/", 4180).unwrap();
        assert_eq!(b.url, "https://fleet.example.com");
        assert_eq!(b.hook_url(), "https://fleet.example.com/hook");
        assert_eq!(b.host(), "fleet.example.com");
        assert!(b.public);
        let with_port = HubBase::public("http://10.0.0.5:8443", 4180).unwrap();
        assert_eq!(with_port.host(), "10.0.0.5:8443");
        let named_port = HubBase::public("https://fleet.example.com:8443", 4180).unwrap();
        assert_eq!(named_port.url, "https://fleet.example.com:8443");
        assert_eq!(named_port.host(), "fleet.example.com:8443");
        let v6 = HubBase::public("http://[::1]:4180", 4180).unwrap();
        assert_eq!(v6.url, "http://[::1]:4180");
        assert_eq!(v6.host(), "[::1]:4180");
        for bad in [
            "ftp://x",
            "fleet.example.com",
            "https://x/mcp",
            "https://x?y=1",
            "https://x#frag",
            "https://x/?y=1",
            "https://x//",
            "",
            "https://",
            // userinfo would be written into every host's config and logged
            "https://user:pass@fleet.example.com",
            "https://@fleet.example.com",
            // port must be a u16
            "https://fleet.example.com:https",
            "https://fleet.example.com:99999",
            "https://fleet.example.com:",
            "https://fleet.example.com:+443",
            // no whitespace anywhere inside
            "https://fleet .example.com",
            "https://fleet.example.com\t:443",
            // empty host
            "https://:443",
        ] {
            let e = HubBase::public(bad, 4180).unwrap_err();
            assert_eq!(e.code, crate::ipc_error::codes::E_VALIDATE, "{bad}");
        }
    }

    #[test]
    fn read_prefers_the_public_url_setting() {
        let s = Store::open_in_memory().unwrap();
        s.set_setting(crate::mcp::SETTING_TOKEN, "tok").unwrap();
        s.set_setting(crate::mcp::SETTING_PORT, "4321").unwrap();
        assert_eq!(HubBase::read(&s).unwrap(), HubBase::loopback(4321));
        s.set_setting(SETTING_PUBLIC_URL, "https://fleet.example.com")
            .unwrap();
        let b = HubBase::read(&s).unwrap();
        assert_eq!(b.url, "https://fleet.example.com");
        assert_eq!(b.port, 4321);
        assert!(b.public);
    }

    #[test]
    fn read_refuses_without_a_master_token() {
        let s = Store::open_in_memory().unwrap();
        let e = HubBase::read(&s).unwrap_err();
        assert_eq!(e.code, crate::ipc_error::codes::E_PROVISION);
    }

    #[test]
    fn local_host_defaults_to_true_and_reads_false() {
        let s = Store::open_in_memory().unwrap();
        assert!(read_local_host(&s));
        s.set_setting(SETTING_LOCAL_HOST, "false").unwrap();
        assert!(!read_local_host(&s));
        s.set_setting(SETTING_LOCAL_HOST, "true").unwrap();
        assert!(read_local_host(&s));
    }

    /// `fleet-hub` persists canonical `"true"` / `"false"`, but a hand-edited
    /// state.db or an older writer can hold any of the usual spellings. Only
    /// an unambiguous "off" turns the local host off; everything else keeps
    /// the safe default of on.
    #[test]
    fn local_host_reads_every_falsey_spelling() {
        let s = Store::open_in_memory().unwrap();
        for off in [
            "false", "FALSE", "False", "0", "no", "NO", "off", "Off", " false ",
        ] {
            s.set_setting(SETTING_LOCAL_HOST, off).unwrap();
            assert!(!read_local_host(&s), "{off:?} must read as off");
        }
        for on in ["true", "TRUE", "1", "yes", "on", "", "banana"] {
            s.set_setting(SETTING_LOCAL_HOST, on).unwrap();
            assert!(read_local_host(&s), "{on:?} must read as on");
        }
    }

    #[test]
    fn check_local_allowed_refuses_only_local_when_disabled() {
        let e = check_local_allowed("local", false).unwrap_err();
        assert_eq!(e.code, crate::ipc_error::codes::E_NOTFOUND);
        assert_eq!(
            e.message,
            "host local is disabled on this hub (hub.local_host=false)"
        );
        assert!(check_local_allowed("local", true).is_ok());
        assert!(check_local_allowed("mefistos", false).is_ok());
        assert!(check_local_allowed("mefistos", true).is_ok());
    }

    #[test]
    fn local_host_is_enabled_unless_disabled() {
        // No unit test calls `disable_local_host` (it is process-wide; see
        // tests/local_host_guard.rs), so the default holds here.
        assert!(local_host_enabled());
        assert!(ensure_local_allowed("local").is_ok());
    }
}
