//! Flag > `FLEET_HUB_*` env > `settings` row > default.

use clap::Args;
use fleet_core::service::hub::{
    HubBase, SETTING_ALLOWED_HOSTS, SETTING_BIND, SETTING_LOCAL_HOST, SETTING_PUBLIC_URL,
};
use std::collections::HashMap;
use std::net::IpAddr;
use std::path::PathBuf;

pub const DEFAULT_BIND: &str = "127.0.0.1";

/// Options shared by `init` and `serve`. Env names are spelled out so
/// `--help` shows them; the precedence itself is applied in [`resolve`].
#[derive(Args, Debug, Clone, Default)]
pub struct HubOptions {
    /// Data directory holding state.db and logs/ [env: FLEET_HUB_DATA_DIR]
    #[arg(long)]
    pub data_dir: Option<PathBuf>,
    /// Listen address [env: FLEET_HUB_BIND] [default: 127.0.0.1]
    #[arg(long)]
    pub bind: Option<String>,
    /// Listen port [env: FLEET_HUB_PORT] [default: 4180]
    #[arg(long)]
    pub port: Option<u16>,
    /// Public base URL hosts and clients reach this hub at, e.g. https://fleet.example.com [env: FLEET_HUB_PUBLIC_URL]
    #[arg(long)]
    pub public_url: Option<String>,
    /// Extra Host/Origin values to accept (repeatable) [env: FLEET_HUB_ALLOWED_HOSTS, comma-separated]
    #[arg(long = "allowed-host")]
    pub allowed_host: Vec<String>,
    /// Treat this machine as a fleet host too [env: FLEET_HUB_LOCAL_HOST] [default: false]
    #[arg(long, action = clap::ArgAction::Set)]
    pub local_host: Option<bool>,
    /// Permit a non-loopback bind with an http:// public URL (container-internal use only) [env: FLEET_HUB_ALLOW_PLAINTEXT]
    #[arg(long)]
    pub allow_plaintext: bool,
    /// Log directory [env: FLEET_HUB_LOG_DIR] [default: <data-dir>/logs]
    #[arg(long)]
    pub log_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub data_dir: PathBuf,
    pub bind: IpAddr,
    pub port: u16,
    pub public_url: Option<String>,
    pub allowed_hosts: Vec<String>,
    pub local_host: bool,
    pub log_dir: PathBuf,
}

impl Resolved {
    pub fn base(&self) -> Result<HubBase, String> {
        match &self.public_url {
            Some(u) => HubBase::public(u, self.port).map_err(|e| e.message),
            None => Ok(HubBase::loopback(self.port)),
        }
    }
}

/// Platform default data dir (same app id as the desktop, so a copied
/// `state.db` lands where the desktop would look for it on that OS).
pub fn default_data_dir() -> PathBuf {
    directories::ProjectDirs::from("sk", "rlt", "claude-fleet")
        .map(|d| d.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("/var/lib/fleet-hub"))
}

fn pick(
    flag: Option<String>,
    env: &HashMap<String, String>,
    env_key: &str,
    setting: Option<String>,
) -> Option<String> {
    flag.or_else(|| env.get(env_key).cloned())
        .or(setting)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// `settings` reads a `settings` row by key (None when there is no store yet).
pub fn resolve(
    opts: &HubOptions,
    env: &HashMap<String, String>,
    settings: &dyn Fn(&str) -> Option<String>,
) -> Result<Resolved, String> {
    let data_dir = opts
        .data_dir
        .clone()
        .or_else(|| env.get("FLEET_HUB_DATA_DIR").map(PathBuf::from))
        .unwrap_or_else(default_data_dir);

    let bind_s = pick(
        opts.bind.clone(),
        env,
        "FLEET_HUB_BIND",
        settings(SETTING_BIND),
    )
    .unwrap_or_else(|| DEFAULT_BIND.to_string());
    let bind: IpAddr = bind_s
        .parse()
        .map_err(|e| format!("bind '{bind_s}' is not an IP address: {e}"))?;

    let port = match pick(
        opts.port.map(|p| p.to_string()),
        env,
        "FLEET_HUB_PORT",
        settings(fleet_core::mcp::SETTING_PORT),
    ) {
        Some(p) => p.parse::<u16>().map_err(|e| format!("port '{p}': {e}"))?,
        None => fleet_core::mcp::DEFAULT_PORT,
    };

    let public_url = pick(
        opts.public_url.clone(),
        env,
        "FLEET_HUB_PUBLIC_URL",
        settings(SETTING_PUBLIC_URL),
    );
    let base = match &public_url {
        Some(u) => HubBase::public(u, port).map_err(|e| format!("public URL: {}", e.message))?,
        None => HubBase::loopback(port),
    };

    // An empty env value or setting counts as unset (persist writes "" for
    // "no extra hosts").
    let from_setting = settings(SETTING_ALLOWED_HOSTS);
    let allowed_raw: Vec<String> = if !opts.allowed_host.is_empty() {
        opts.allowed_host.clone()
    } else if let Some(v) = env
        .get("FLEET_HUB_ALLOWED_HOSTS")
        .filter(|v| !v.trim().is_empty())
        .or(from_setting.as_ref().filter(|v| !v.trim().is_empty()))
    {
        v.split(',').map(str::to_string).collect()
    } else if base.public {
        vec![base.host()]
    } else {
        vec![]
    };
    let allowed_hosts = fleet_core::mcp::normalize_allowed_hosts(&allowed_raw);

    let local_host = match pick(
        opts.local_host.map(|b| b.to_string()),
        env,
        "FLEET_HUB_LOCAL_HOST",
        settings(SETTING_LOCAL_HOST),
    ) {
        None => false,
        Some(v) if v == "true" => true,
        Some(v) if v == "false" => false,
        Some(v) => return Err(format!("local_host must be true or false, got '{v}'")),
    };

    let plaintext_public = base.public && base.url.starts_with("http://");
    let allow_plaintext = opts.allow_plaintext
        || env
            .get("FLEET_HUB_ALLOW_PLAINTEXT")
            .is_some_and(|v| v == "1" || v == "true");
    if !bind.is_loopback() && plaintext_public && !allow_plaintext {
        return Err(format!(
            "refusing to serve plaintext http on {bind}: use an https:// public URL, \
             bind to 127.0.0.1 behind a TLS proxy, or pass --allow-plaintext"
        ));
    }

    let log_dir = opts
        .log_dir
        .clone()
        .or_else(|| env.get("FLEET_HUB_LOG_DIR").map(PathBuf::from))
        .unwrap_or_else(|| data_dir.join("logs"));

    Ok(Resolved {
        data_dir,
        bind,
        port,
        public_url: public_url.map(|_| base.url.clone()),
        allowed_hosts,
        local_host,
        log_dir,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn opts() -> HubOptions {
        HubOptions {
            data_dir: None,
            bind: None,
            port: None,
            public_url: None,
            allowed_host: vec![],
            local_host: None,
            allow_plaintext: false,
            log_dir: None,
        }
    }

    #[test]
    fn defaults_are_loopback_no_public_url_no_local_host() {
        let r = resolve(&opts(), &env(&[]), &|_| None).unwrap();
        assert_eq!(r.bind, "127.0.0.1".parse::<std::net::IpAddr>().unwrap());
        assert_eq!(r.port, 4180);
        assert_eq!(r.public_url, None);
        assert!(!r.local_host);
        assert_eq!(r.allowed_hosts, Vec::<String>::new());
        assert_eq!(r.log_dir, r.data_dir.join("logs"));
    }

    #[test]
    fn flag_beats_env_beats_setting_beats_default() {
        let settings = |k: &str| match k {
            "mcp.port" => Some("5000".to_string()),
            "hub.bind" => Some("10.0.0.1".to_string()),
            _ => None,
        };
        let e = env(&[("FLEET_HUB_PORT", "6000"), ("FLEET_HUB_BIND", "10.0.0.2")]);
        let mut o = opts();
        o.port = Some(7000);
        let r = resolve(&o, &e, &settings).unwrap();
        assert_eq!(r.port, 7000, "flag wins");
        assert_eq!(r.bind.to_string(), "10.0.0.2", "env beats setting");
        let r = resolve(&opts(), &env(&[]), &settings).unwrap();
        assert_eq!(r.port, 5000, "setting beats default");
        assert_eq!(r.bind.to_string(), "10.0.0.1");
    }

    #[test]
    fn allowed_hosts_default_to_the_public_url_host() {
        let mut o = opts();
        o.public_url = Some("https://Fleet.Example.com".into());
        let r = resolve(&o, &env(&[]), &|_| None).unwrap();
        assert_eq!(r.allowed_hosts, vec!["fleet.example.com".to_string()]);
        let e = env(&[(
            "FLEET_HUB_ALLOWED_HOSTS",
            "a.example.com, b.example.com:8443",
        )]);
        let r = resolve(&o, &e, &|_| None).unwrap();
        assert_eq!(
            r.allowed_hosts,
            vec![
                "a.example.com".to_string(),
                "b.example.com:8443".to_string()
            ]
        );
    }

    #[test]
    fn plaintext_on_a_routable_bind_is_refused_unless_allowed() {
        let mut o = opts();
        o.bind = Some("0.0.0.0".into());
        o.public_url = Some("http://fleet.example.com".into());
        let e = resolve(&o, &env(&[]), &|_| None).unwrap_err();
        assert!(e.contains("--allow-plaintext"), "{e}");
        o.allow_plaintext = true;
        assert!(resolve(&o, &env(&[]), &|_| None).is_ok());
        // https is always fine; loopback is always fine.
        o.allow_plaintext = false;
        o.public_url = Some("https://fleet.example.com".into());
        assert!(resolve(&o, &env(&[]), &|_| None).is_ok());
        o.bind = Some("127.0.0.1".into());
        o.public_url = Some("http://fleet.example.com".into());
        assert!(resolve(&o, &env(&[]), &|_| None).is_ok());
    }

    #[test]
    fn bad_values_are_reported_by_name() {
        let mut o = opts();
        o.bind = Some("not-an-ip".into());
        assert!(resolve(&o, &env(&[]), &|_| None)
            .unwrap_err()
            .contains("bind"));
        let mut o = opts();
        o.public_url = Some("fleet.example.com".into());
        assert!(resolve(&o, &env(&[]), &|_| None)
            .unwrap_err()
            .contains("public URL"));
        let e = env(&[("FLEET_HUB_LOCAL_HOST", "yes")]);
        assert!(resolve(&opts(), &e, &|_| None)
            .unwrap_err()
            .contains("local_host"));
    }
}
