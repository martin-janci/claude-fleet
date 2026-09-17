//! Flag > `FLEET_HUB_*` env > `settings` row > default.

use clap::Args;
use fleet_core::service::hub::{
    HubBase, SETTING_ALLOWED_HOSTS, SETTING_BIND, SETTING_LOCAL_HOST, SETTING_PUBLIC_URL,
};
use std::collections::HashMap;
use std::net::IpAddr;
use std::path::PathBuf;

pub const DEFAULT_BIND: &str = "127.0.0.1";

/// Options shared by `init`, `serve` and `token`. Env names are spelled out so
/// `--help` shows them; the precedence itself is applied in [`resolve`].
/// Every arg is `global` so `token show --data-dir D` parses as well as
/// `token --data-dir D show`.
#[derive(Args, Debug, Clone, Default)]
pub struct HubOptions {
    /// Data directory holding state.db and logs/ [env: FLEET_HUB_DATA_DIR]
    #[arg(long, global = true)]
    pub data_dir: Option<PathBuf>,
    /// Listen address [env: FLEET_HUB_BIND] [default: 127.0.0.1]
    #[arg(long, global = true)]
    pub bind: Option<String>,
    /// Listen port [env: FLEET_HUB_PORT] [default: 4180]
    #[arg(long, global = true)]
    pub port: Option<u16>,
    /// Public base URL hosts and clients reach this hub at, e.g. https://fleet.example.com [env: FLEET_HUB_PUBLIC_URL]
    #[arg(long, global = true)]
    pub public_url: Option<String>,
    /// Extra Host/Origin values to accept (repeatable) [env: FLEET_HUB_ALLOWED_HOSTS, comma-separated]
    #[arg(long = "allowed-host", global = true)]
    pub allowed_host: Vec<String>,
    /// Treat this machine as a fleet host too [env: FLEET_HUB_LOCAL_HOST] [default: false]
    #[arg(long, action = clap::ArgAction::Set, global = true)]
    pub local_host: Option<bool>,
    /// Permit a non-loopback bind without an https:// public URL (plaintext http: container-internal or a private network only) [env: FLEET_HUB_ALLOW_PLAINTEXT]
    #[arg(long, global = true)]
    pub allow_plaintext: bool,
    /// Log directory [env: FLEET_HUB_LOG_DIR] [default: <data-dir>/logs]
    #[arg(long, global = true)]
    pub log_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub data_dir: PathBuf,
    pub bind: IpAddr,
    pub port: u16,
    pub public_url: Option<String>,
    /// The effective Host/Origin allowlist handed to the server: the explicit
    /// list plus the public URL's host, normalized and de-duplicated.
    pub allowed_hosts: Vec<String>,
    /// Only the explicitly configured hosts (flag > env > setting), which is
    /// what `hub.allowed_hosts` stores, so a saved list never pins an old
    /// public host.
    pub allowed_hosts_explicit: Vec<String>,
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

/// Platform default data dir under the hub's own app id (`fleet-hub`), never
/// the desktop's (`claude-fleet`): a bare `fleet-hub init` on a machine that
/// also runs the desktop must not open and rewrite its live `state.db`.
/// `~/.local/share/fleet-hub` on Linux,
/// `~/Library/Application Support/sk.rlt.fleet-hub` on macOS; migrating
/// copies `state.db` in explicitly.
pub fn default_data_dir() -> PathBuf {
    directories::ProjectDirs::from("sk", "rlt", "fleet-hub")
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

/// The data dir alone (flag > env > default): it locates the store the other
/// values are then resolved against, so it cannot depend on them.
pub fn resolve_data_dir(opts: &HubOptions, env: &HashMap<String, String>) -> PathBuf {
    opts.data_dir
        .clone()
        .or_else(|| env.get("FLEET_HUB_DATA_DIR").map(PathBuf::from))
        .unwrap_or_else(default_data_dir)
}

/// `settings` reads a `settings` row by key (None when there is no store yet).
pub fn resolve(
    opts: &HubOptions,
    env: &HashMap<String, String>,
    settings: &dyn Fn(&str) -> Option<String>,
) -> Result<Resolved, String> {
    let data_dir = resolve_data_dir(opts, env);

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

    // Explicit list: flag > env > setting; an empty env value or setting
    // counts as unset (persist writes "" for "none given").
    let from_setting = settings(SETTING_ALLOWED_HOSTS);
    let explicit_raw: Vec<String> = if !opts.allowed_host.is_empty() {
        opts.allowed_host.clone()
    } else if let Some(v) = env
        .get("FLEET_HUB_ALLOWED_HOSTS")
        .filter(|v| !v.trim().is_empty())
        .or(from_setting.as_ref().filter(|v| !v.trim().is_empty()))
    {
        v.split(',').map(str::to_string).collect()
    } else {
        vec![]
    };
    let allowed_hosts_explicit = dedup(fleet_core::mcp::normalize_allowed_hosts(&explicit_raw));
    // Effective list: the explicit hosts plus the public URL's own host.
    let mut effective_raw = allowed_hosts_explicit.clone();
    if base.public {
        effective_raw.push(base.host());
    }
    let allowed_hosts = dedup(fleet_core::mcp::normalize_allowed_hosts(&effective_raw));

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

    // Only an https:// public URL means TLS sits in front of a routable bind;
    // an http:// one, or none at all, is plaintext on the wire.
    let tls_in_front = base.public && base.url.starts_with("https://");
    let allow_plaintext = opts.allow_plaintext
        || env
            .get("FLEET_HUB_ALLOW_PLAINTEXT")
            .is_some_and(|v| v == "1" || v == "true");
    if !bind.is_loopback() && !tls_in_front && !allow_plaintext {
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
        allowed_hosts_explicit,
        local_host,
        log_dir,
    })
}

/// Drop repeats, keeping the first occurrence's position.
fn dedup(list: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    list.into_iter()
        .filter(|h| seen.insert(h.clone()))
        .collect()
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
        assert_eq!(r.allowed_hosts_explicit, Vec::<String>::new());
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
        // Routable binds with no https public URL: plaintext must be allowed.
        let mut o = opts();
        o.allow_plaintext = true;
        o.port = Some(7000);
        let r = resolve(&o, &e, &settings).unwrap();
        assert_eq!(r.port, 7000, "flag wins");
        assert_eq!(r.bind.to_string(), "10.0.0.2", "env beats setting");
        let mut o = opts();
        o.allow_plaintext = true;
        let r = resolve(&o, &env(&[]), &settings).unwrap();
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
                "b.example.com:8443".to_string(),
                "fleet.example.com".to_string(),
            ]
        );
    }

    #[test]
    fn explicit_env_hosts_are_joined_by_the_public_host() {
        let mut o = opts();
        o.public_url = Some("https://fleet.example.com".into());
        let e = env(&[(
            "FLEET_HUB_ALLOWED_HOSTS",
            "a.example.com, fleet.example.com",
        )]);
        let r = resolve(&o, &e, &|_| None).unwrap();
        assert_eq!(
            r.allowed_hosts,
            vec!["a.example.com".to_string(), "fleet.example.com".to_string()],
            "both, de-duplicated"
        );
        assert_eq!(
            r.allowed_hosts_explicit,
            vec!["a.example.com".to_string(), "fleet.example.com".to_string()]
        );
    }

    #[test]
    fn without_an_explicit_list_only_the_public_host_is_effective_and_nothing_is_persisted() {
        let mut o = opts();
        o.public_url = Some("https://fleet.example.com".into());
        let r = resolve(&o, &env(&[]), &|k| {
            (k == "hub.allowed_hosts").then(String::new)
        })
        .unwrap();
        assert_eq!(r.allowed_hosts_explicit, Vec::<String>::new());
        assert_eq!(r.allowed_hosts, vec!["fleet.example.com".to_string()]);
    }

    #[test]
    fn a_saved_allowlist_does_not_shadow_a_new_public_url() {
        let settings = |k: &str| (k == "hub.allowed_hosts").then(|| "old.example.com".to_string());
        let mut o = opts();
        o.public_url = Some("https://new.example.com".into());
        let r = resolve(&o, &env(&[]), &settings).unwrap();
        assert!(
            r.allowed_hosts.contains(&"old.example.com".to_string()),
            "{:?}",
            r.allowed_hosts
        );
        assert!(
            r.allowed_hosts.contains(&"new.example.com".to_string()),
            "{:?}",
            r.allowed_hosts
        );
    }

    #[test]
    fn default_data_dir_is_not_the_desktops() {
        let dir = default_data_dir();
        let last = dir.file_name().unwrap().to_string_lossy().to_string();
        assert!(
            last != "claude-fleet" && last != "sk.rlt.claude-fleet",
            "the hub must not open the desktop's state.db by default: {}",
            dir.display()
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
        // A routable bind with no public URL at all is plaintext too.
        let mut bare = opts();
        bare.bind = Some("0.0.0.0".into());
        let e = resolve(&bare, &env(&[]), &|_| None).unwrap_err();
        assert!(e.contains("--allow-plaintext"), "{e}");
        bare.allow_plaintext = true;
        assert!(resolve(&bare, &env(&[]), &|_| None).is_ok());
        bare.allow_plaintext = false;
        let allow_env = env(&[("FLEET_HUB_ALLOW_PLAINTEXT", "1")]);
        assert!(resolve(&bare, &allow_env, &|_| None).is_ok());
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
