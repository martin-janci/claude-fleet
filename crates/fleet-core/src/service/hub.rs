//! Where the hub is reachable, as every host must be told: the base URL its
//! hooks and MCP entry point at. Loopback + reverse tunnel on the desktop;
//! a public URL on a `fleet-hub` daemon (no tunnels then).

use crate::ipc_error::{codes, IpcError};
use crate::store::Store;

pub const SETTING_BIND: &str = "hub.bind";
pub const SETTING_PUBLIC_URL: &str = "hub.public_url";
pub const SETTING_ALLOWED_HOSTS: &str = "hub.allowed_hosts";
pub const SETTING_LOCAL_HOST: &str = "hub.local_host";

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
    pub fn public(url: &str, port: u16) -> Result<Self, IpcError> {
        let trimmed = url.trim().trim_end_matches('/').to_ascii_lowercase();
        let rest = trimmed
            .strip_prefix("https://")
            .or_else(|| trimmed.strip_prefix("http://"))
            .ok_or_else(|| {
                IpcError::new(
                    codes::E_VALIDATE,
                    "public URL must start with http:// or https://",
                )
            })?;
        if rest.is_empty() || rest.contains('/') || rest.contains('?') || rest.contains('#') {
            return Err(IpcError::new(
                codes::E_VALIDATE,
                "public URL must be scheme://host[:port] with no path or query",
            ));
        }
        Ok(Self {
            url: trimmed,
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
pub fn read_local_host(s: &Store) -> bool {
    !matches!(
        s.get_setting(SETTING_LOCAL_HOST).ok().flatten().as_deref(),
        Some("false")
    )
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
        for bad in [
            "ftp://x",
            "fleet.example.com",
            "https://x/mcp",
            "https://x?y=1",
            "",
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
}
