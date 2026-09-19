//! Flag > `FLEET_HUB_*` env > `settings` row > default.

use clap::Args;
use fleet_core::service::hub::{
    HubBase, SETTING_ALLOWED_HOSTS, SETTING_ALLOW_PLAINTEXT, SETTING_BIND, SETTING_LOCAL_HOST,
    SETTING_PUBLIC_URL, SETTING_TLS, SETTING_TLS_CERT, SETTING_TLS_KEY,
};
use std::collections::HashMap;
use std::net::IpAddr;
use std::path::PathBuf;

pub const DEFAULT_BIND: &str = "127.0.0.1";

/// How `serve` terminates TLS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TlsMode {
    /// Plaintext http. Something in front — Caddy in `deploy/hub`, or nothing
    /// at all on loopback — is responsible for TLS. The documented default.
    #[default]
    Off,
    /// A certificate obtained and renewed automatically over ACME.
    /// **Not built into this binary** — see [`ACME_UNAVAILABLE`].
    Auto,
    /// A PEM certificate chain and private key the operator supplies.
    Cert,
}

/// Why `--tls auto` is refused rather than served.
///
/// `rustls-acme` reaches the ACME directory through `async-web-client`, which
/// depends unconditionally on `webpki-roots` (the Mozilla root store) under
/// `CDLA-Permissive-2.0`. That licence is not in `deny.toml`'s allowlist, and
/// widening the allowlist is not this binary's call — so the ACME stack is not
/// a dependency at all and `auto` has no implementation to fall back on.
/// It stays a *recognised* value so that the operator who asked for it reads
/// this instead of clap's "invalid value for --tls".
pub const ACME_UNAVAILABLE: &str = "--tls auto (automatic ACME certificates) is not available in \
     this build: rustls-acme pulls webpki-roots, whose CDLA-Permissive-2.0 licence is not in \
     deny.toml's allowlist. Use --tls cert with --tls-cert/--tls-key, or terminate TLS in a \
     proxy in front of the hub (see docs/hub.md).";

impl TlsMode {
    /// Also used by `healthcheck`, which resolves the mode without a store.
    pub(crate) fn parse(v: &str) -> Result<Self, String> {
        match v {
            "off" => Ok(Self::Off),
            "auto" => Ok(Self::Auto),
            "cert" => Ok(Self::Cert),
            other => Err(format!("--tls must be off, auto or cert, got '{other}'")),
        }
    }

    /// What `persist` writes back, so a later bare `serve` keeps the mode.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Auto => "auto",
            Self::Cert => "cert",
        }
    }

    /// True when the hub itself terminates TLS, which is the protection the
    /// plaintext refusal asks for.
    pub fn terminates_tls(self) -> bool {
        !matches!(self, Self::Off)
    }
}

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
    /// Terminate TLS in the hub itself: off (a proxy in front does it) or cert (supply --tls-cert and --tls-key) [env: FLEET_HUB_TLS] [default: off]
    #[arg(long, global = true)]
    pub tls: Option<String>,
    /// PEM certificate chain for --tls cert, leaf first [env: FLEET_HUB_TLS_CERT]
    #[arg(long, global = true)]
    pub tls_cert: Option<PathBuf>,
    /// PEM private key for --tls cert (PKCS#8, PKCS#1 or SEC1) [env: FLEET_HUB_TLS_KEY]
    #[arg(long, global = true)]
    pub tls_key: Option<PathBuf>,
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
    /// Whether a non-loopback bind without an https:// public URL is
    /// permitted (flag > env > `hub.allow_plaintext` > false). Persisted, so
    /// a later bare `serve` keeps the allowance it was started with.
    pub allow_plaintext: bool,
    pub log_dir: PathBuf,
    /// How the hub itself terminates TLS (flag > env > `hub.tls` > off).
    pub tls: TlsMode,
    /// The PEM pair for [`TlsMode::Cert`]; both `Some` whenever `tls` is
    /// `Cert`, both `None` otherwise (`resolve` refuses anything between).
    pub tls_cert: Option<PathBuf>,
    pub tls_key: Option<PathBuf>,
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

/// Flag > env > setting, with empties dropped — but an EXPLICITLY empty flag
/// (`--bind ""`) is an error naming `option`, not a silent fall-through to the
/// env or the stored value. An empty env value or setting still counts as
/// unset: `persist` writes `""` for "none given".
pub(crate) fn pick(
    option: &str,
    flag: Option<String>,
    env: &HashMap<String, String>,
    env_key: &str,
    setting: Option<String>,
) -> Result<Option<String>, String> {
    if let Some(f) = flag {
        if f.trim().is_empty() {
            return Err(format!(
                "{option} was given an empty value; omit it to fall back to {env_key} \
                 or the stored setting"
            ));
        }
        return Ok(Some(f.trim().to_string()));
    }
    Ok(env
        .get(env_key)
        .cloned()
        .or(setting)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty()))
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
        "--bind",
        opts.bind.clone(),
        env,
        "FLEET_HUB_BIND",
        settings(SETTING_BIND),
    )?
    .unwrap_or_else(|| DEFAULT_BIND.to_string());
    let bind: IpAddr = bind_s
        .parse()
        .map_err(|e| format!("bind '{bind_s}' is not an IP address: {e}"))?;

    let port = match pick(
        "--port",
        opts.port.map(|p| p.to_string()),
        env,
        "FLEET_HUB_PORT",
        settings(fleet_core::mcp::SETTING_PORT),
    )? {
        Some(p) => p.parse::<u16>().map_err(|e| format!("port '{p}': {e}"))?,
        None => fleet_core::mcp::DEFAULT_PORT,
    };

    let public_url = pick(
        "--public-url",
        opts.public_url.clone(),
        env,
        "FLEET_HUB_PUBLIC_URL",
        settings(SETTING_PUBLIC_URL),
    )?;
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
        "--local-host",
        opts.local_host.map(|b| b.to_string()),
        env,
        "FLEET_HUB_LOCAL_HOST",
        settings(SETTING_LOCAL_HOST),
    )? {
        None => false,
        Some(v) if v == "true" => true,
        Some(v) if v == "false" => false,
        Some(v) => return Err(format!("local_host must be true or false, got '{v}'")),
    };

    let tls = match pick(
        "--tls",
        opts.tls.clone(),
        env,
        "FLEET_HUB_TLS",
        settings(SETTING_TLS),
    )? {
        Some(v) => TlsMode::parse(&v)?,
        None => TlsMode::default(),
    };
    if tls == TlsMode::Auto {
        return Err(ACME_UNAVAILABLE.to_string());
    }
    let path_opt =
        |option: &str, flag: &Option<PathBuf>, env_key: &str, setting: Option<String>| {
            pick(
                option,
                flag.as_ref().map(|p| p.display().to_string()),
                env,
                env_key,
                setting,
            )
            .map(|v| v.map(PathBuf::from))
        };
    let tls_cert = path_opt(
        "--tls-cert",
        &opts.tls_cert,
        "FLEET_HUB_TLS_CERT",
        settings(SETTING_TLS_CERT),
    )?;
    let tls_key = path_opt(
        "--tls-key",
        &opts.tls_key,
        "FLEET_HUB_TLS_KEY",
        settings(SETTING_TLS_KEY),
    )?;
    let (tls_cert, tls_key) = match tls {
        TlsMode::Cert => {
            let missing: Vec<&str> = [("--tls-cert", &tls_cert), ("--tls-key", &tls_key)]
                .iter()
                .filter(|(_, v)| v.is_none())
                .map(|(name, _)| *name)
                .collect();
            if !missing.is_empty() {
                return Err(format!(
                    "--tls cert needs {}: point it at the PEM certificate chain and its private key",
                    missing.join(" and ")
                ));
            }
            (tls_cert, tls_key)
        }
        // Carrying half a configuration forward would make `--tls off` look
        // like it was serving the pair it is not.
        _ => (None, None),
    };
    // The public URL is what hosts POST hooks to and what `mcp/mod.rs` hands a
    // freshly paired client as its `hub` base. A hub that terminates TLS must
    // therefore advertise an `https://` URL, and it must advertise one at all:
    // with no public URL the base falls back to `HubBase::loopback`, i.e.
    // `http://127.0.0.1:<port>` — a plaintext URL for a port that now speaks
    // only TLS, so every hook POST and every paired client is pointed at
    // something that will not answer. Rewriting that fallback to
    // `https://127.0.0.1` would not help: a certificate is issued for a
    // domain, so verification would fail for every client anyway.
    if tls.terminates_tls() && !(base.public && base.url.starts_with("https://")) {
        let found = if base.public {
            format!("the public URL is {}", base.url)
        } else {
            "no public URL is set, so hooks and paired clients would be sent to \
             the loopback base http://127.0.0.1"
                .to_string()
        };
        return Err(format!(
            "--tls {} serves https, but {found}: set --public-url to the \
             https:// address this certificate is issued for",
            tls.as_str(),
        ));
    }

    // Either an https:// public URL (a proxy terminates in front of a routable
    // bind) or the hub's own TLS means nothing crosses the wire in the clear;
    // an http:// public URL, or none at all, is plaintext.
    let tls_in_front = (base.public && base.url.starts_with("https://")) || tls.terminates_tls();
    // The flag is a presence flag (it can only turn the allowance on); the
    // env can say either, so `FLEET_HUB_ALLOW_PLAINTEXT=0` turns off a stored
    // `hub.allow_plaintext=true`.
    let allow_plaintext = if opts.allow_plaintext {
        true
    } else if let Some(v) = env.get("FLEET_HUB_ALLOW_PLAINTEXT") {
        match v.trim() {
            "1" | "true" => true,
            "0" | "false" => false,
            other => {
                return Err(format!(
                    "FLEET_HUB_ALLOW_PLAINTEXT must be 1, true, 0 or false, got '{other}'"
                ))
            }
        }
    } else {
        settings(SETTING_ALLOW_PLAINTEXT).is_some_and(|v| v.trim() == "true")
    };
    // The same rule the agent applies before it dials and the desktop before
    // it sends its token, from the one place it is written down.
    if !fleet_proto::net::is_loopback_ip(&bind) && !tls_in_front && !allow_plaintext {
        return Err(format!(
            "refusing to serve plaintext http on {bind}: use an https:// public URL, \
             terminate TLS in the hub itself with --tls cert, bind to 127.0.0.1 behind a \
             TLS proxy, or pass --allow-plaintext"
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
        allow_plaintext,
        log_dir,
        tls,
        tls_cert,
        tls_key,
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
        HubOptions::default()
    }

    /// A complete `--tls cert` configuration: both PEM halves (the paths are
    /// not opened by `resolve`; `tls::acceptor` is what reads them) and the
    /// `https://` public URL that TLS now requires.
    fn cert_opts() -> HubOptions {
        let mut o = opts();
        o.tls = Some("cert".into());
        o.tls_cert = Some("/etc/fleet/tls.crt".into());
        o.tls_key = Some("/etc/fleet/tls.key".into());
        o.public_url = Some("https://fleet.example.com".into());
        o
    }

    #[test]
    fn tls_defaults_to_off_and_changes_nothing() {
        let r = resolve(&opts(), &env(&[]), &|_| None).unwrap();
        assert_eq!(r.tls, TlsMode::Off);
        assert_eq!(r.tls_cert, None);
        assert_eq!(r.tls_key, None);
        // Today's behaviour, unchanged: loopback is fine, a routable bind
        // without https still demands the flag.
        let mut o = opts();
        o.bind = Some("0.0.0.0".into());
        o.tls = Some("off".into());
        let e = resolve(&o, &env(&[]), &|_| None).unwrap_err();
        assert!(e.contains("--allow-plaintext"), "{e}");
    }

    #[test]
    fn tls_auto_is_refused_because_acme_is_not_in_this_build() {
        // The ACME stack (`rustls-acme` -> `async-web-client` -> `webpki-roots`)
        // carries CDLA-Permissive-2.0, which `deny.toml` does not allow, so
        // `auto` is not built. It stays a recognised value so the operator gets
        // this explanation instead of a clap "invalid value".
        let mut o = opts();
        o.tls = Some("auto".into());
        let e = resolve(&o, &env(&[]), &|_| None).unwrap_err();
        assert!(e.contains("--tls auto"), "{e}");
        assert!(e.contains("not available in this build"), "{e}");
        assert!(e.contains("--tls cert"), "names the way forward: {e}");

        // The brief's two `auto` refusals are subsumed by this one: no
        // `--acme-email` can make it work, and neither can a domain.
        let mut with_domain = o.clone();
        with_domain.public_url = Some("https://fleet.example.com".into());
        assert!(resolve(&with_domain, &env(&[]), &|_| None).is_err());
        // ... including the IP-literal public URL ACME could never issue for.
        let mut ip = o.clone();
        ip.public_url = Some("https://203.0.113.9".into());
        assert!(resolve(&ip, &env(&[]), &|_| None).is_err());

        // From the env and from a stored setting too, not just the flag.
        let e = resolve(&opts(), &env(&[("FLEET_HUB_TLS", "auto")]), &|_| None).unwrap_err();
        assert!(e.contains("--tls auto"), "{e}");
        let e = resolve(&opts(), &env(&[]), &|k| {
            (k == "hub.tls").then(|| "auto".to_string())
        })
        .unwrap_err();
        assert!(e.contains("--tls auto"), "{e}");
    }

    #[test]
    fn tls_cert_needs_both_halves() {
        let mut o = opts();
        o.tls = Some("cert".into());
        let e = resolve(&o, &env(&[]), &|_| None).unwrap_err();
        assert!(e.contains("--tls-cert"), "{e}");
        assert!(e.contains("--tls-key"), "{e}");

        o.tls_cert = Some("/etc/fleet/tls.crt".into());
        let e = resolve(&o, &env(&[]), &|_| None).unwrap_err();
        assert!(e.contains("--tls-key"), "{e}");
        assert!(!e.contains("--tls-cert"), "only the missing half: {e}");

        o.tls_cert = None;
        o.tls_key = Some("/etc/fleet/tls.key".into());
        let e = resolve(&o, &env(&[]), &|_| None).unwrap_err();
        assert!(e.contains("--tls-cert"), "{e}");
        assert!(!e.contains("--tls-key"), "only the missing half: {e}");

        let r = resolve(&cert_opts(), &env(&[]), &|_| None).unwrap();
        assert_eq!(r.tls, TlsMode::Cert);
        assert_eq!(r.tls_cert, Some(PathBuf::from("/etc/fleet/tls.crt")));
        assert_eq!(r.tls_key, Some(PathBuf::from("/etc/fleet/tls.key")));
    }

    #[test]
    fn tls_replaces_allow_plaintext_on_a_routable_bind() {
        // The refusal exists to stop tokens crossing a network in the clear.
        // Terminating TLS in the hub is exactly that protection, so it must
        // not also demand the flag that waives it.
        let mut o = cert_opts();
        o.bind = Some("0.0.0.0".into());
        let r = resolve(&o, &env(&[]), &|_| None).unwrap();
        assert_eq!(r.tls, TlsMode::Cert);
        assert!(!r.allow_plaintext, "nothing was waived");
    }

    #[test]
    fn tls_needs_an_https_public_url() {
        // Hooks post to the public URL and a paired phone is sent back to it,
        // so a hub that terminates TLS and advertises anything else points
        // every one of them at a port that will not answer plaintext.

        // http:// — the obvious case.
        let mut http = cert_opts();
        http.public_url = Some("http://fleet.example.com".into());
        let e = resolve(&http, &env(&[]), &|_| None).unwrap_err();
        assert!(e.contains("--tls cert"), "{e}");
        assert!(e.contains("--public-url"), "{e}");
        assert!(e.contains("http://fleet.example.com"), "{e}");

        // No public URL at all — the base then falls back to
        // `http://127.0.0.1:<port>`, which is the same failure one branch over.
        let mut none = cert_opts();
        none.public_url = None;
        let e = resolve(&none, &env(&[]), &|_| None).unwrap_err();
        assert!(e.contains("--tls cert"), "{e}");
        assert!(e.contains("--public-url"), "{e}");
        assert!(
            e.contains("127.0.0.1"),
            "names what would be advertised: {e}"
        );
        // ... on a loopback bind too, where the plaintext rule never fired.
        let mut loopback = none.clone();
        loopback.bind = Some("127.0.0.1".into());
        assert!(resolve(&loopback, &env(&[]), &|_| None).is_err());

        // The sibling case: with the https:// public URL it resolves.
        let r = resolve(&cert_opts(), &env(&[]), &|_| None).unwrap();
        assert_eq!(r.tls, TlsMode::Cert);
        assert_eq!(r.public_url.as_deref(), Some("https://fleet.example.com"));

        // With `--tls off` an http:// public URL, and none at all, are both
        // still fine (a private network, a container-internal hop, loopback +
        // reverse tunnels) — that rule is unchanged.
        let mut off = opts();
        off.public_url = Some("http://fleet.example.com".into());
        assert!(resolve(&off, &env(&[]), &|_| None).is_ok());
        assert!(resolve(&opts(), &env(&[]), &|_| None).is_ok());
    }

    #[test]
    fn tls_flag_beats_env_beats_setting_and_bad_values_name_the_option() {
        let settings = |k: &str| match k {
            "hub.tls" => Some("cert".to_string()),
            "hub.tls_cert" => Some("/s/tls.crt".to_string()),
            "hub.tls_key" => Some("/s/tls.key".to_string()),
            // TLS requires one; a stored `cert` comes with a stored URL too.
            "hub.public_url" => Some("https://fleet.example.com".to_string()),
            _ => None,
        };
        let r = resolve(&opts(), &env(&[]), &settings).unwrap();
        assert_eq!(r.tls, TlsMode::Cert);
        assert_eq!(r.tls_cert, Some(PathBuf::from("/s/tls.crt")));

        let e = env(&[
            ("FLEET_HUB_TLS", "cert"),
            ("FLEET_HUB_TLS_CERT", "/e/tls.crt"),
            ("FLEET_HUB_TLS_KEY", "/e/tls.key"),
        ]);
        let r = resolve(&opts(), &e, &settings).unwrap();
        assert_eq!(r.tls_cert, Some(PathBuf::from("/e/tls.crt")));
        assert_eq!(r.tls_key, Some(PathBuf::from("/e/tls.key")));

        let r = resolve(&cert_opts(), &e, &settings).unwrap();
        assert_eq!(r.tls_cert, Some(PathBuf::from("/etc/fleet/tls.crt")));

        // The env turns a stored `cert` back off.
        let r = resolve(&opts(), &env(&[("FLEET_HUB_TLS", "off")]), &settings).unwrap();
        assert_eq!(r.tls, TlsMode::Off);

        let mut bad = opts();
        bad.tls = Some("yes".into());
        let err = resolve(&bad, &env(&[]), &|_| None).unwrap_err();
        assert!(err.contains("--tls"), "{err}");
        assert!(err.contains("yes"), "{err}");
    }

    #[test]
    fn an_explicitly_empty_flag_value_is_an_error_naming_the_option() {
        // `--bind ""` used to be indistinguishable from "not given", so it
        // silently fell through to the env or the stored setting — the
        // opposite of what someone clearing a value expects.
        let mut o = opts();
        o.bind = Some(String::new());
        let err = resolve(&o, &env(&[("FLEET_HUB_BIND", "10.0.0.9")]), &|_| None).unwrap_err();
        assert!(err.contains("--bind"), "{err}");

        let mut o = opts();
        o.public_url = Some("   ".into());
        let err = resolve(&o, &env(&[]), &|_| None).unwrap_err();
        assert!(err.contains("--public-url"), "{err}");

        // An empty ENV value or an empty stored setting still means "unset":
        // `persist` writes "" for "none given".
        let r = resolve(&opts(), &env(&[("FLEET_HUB_BIND", "")]), &|_| {
            Some(String::new())
        })
        .unwrap();
        assert_eq!(r.bind.to_string(), DEFAULT_BIND);
        assert_eq!(r.public_url, None);
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

    fn routable() -> HubOptions {
        let mut o = opts();
        o.bind = Some("0.0.0.0".into());
        o
    }

    #[test]
    fn a_stored_allow_plaintext_permits_a_bare_resolve() {
        // `serve --bind 0.0.0.0 --allow-plaintext` persisted the bind and
        // the allowance; a later bare `serve` must not demand the flag again.
        let settings = |k: &str| match k {
            "hub.bind" => Some("0.0.0.0".to_string()),
            "hub.allow_plaintext" => Some("true".to_string()),
            _ => None,
        };
        let r = resolve(&opts(), &env(&[]), &settings).unwrap();
        assert!(r.allow_plaintext);
        assert_eq!(r.bind.to_string(), "0.0.0.0");
        // Stored false (what persist writes when it was off) still refuses.
        let off = |k: &str| match k {
            "hub.bind" => Some("0.0.0.0".to_string()),
            "hub.allow_plaintext" => Some("false".to_string()),
            _ => None,
        };
        assert!(resolve(&opts(), &env(&[]), &off).is_err());
    }

    #[test]
    fn allow_plaintext_env_zero_overrides_a_stored_true() {
        let settings = |k: &str| (k == "hub.allow_plaintext").then(|| "true".to_string());
        for off in ["0", "false"] {
            let e = env(&[("FLEET_HUB_ALLOW_PLAINTEXT", off)]);
            let err = resolve(&routable(), &e, &settings).unwrap_err();
            assert!(err.contains("refusing to serve plaintext"), "{off}: {err}");
        }
        // The flag beats the env's `0`.
        let mut o = routable();
        o.allow_plaintext = true;
        let e = env(&[("FLEET_HUB_ALLOW_PLAINTEXT", "0")]);
        assert!(resolve(&o, &e, &settings).unwrap().allow_plaintext);
        // On loopback nothing is refused, and the resolved value is still
        // the env's, so persisting it turns a stored allowance off.
        let r = resolve(&opts(), &e, &settings).unwrap();
        assert!(!r.allow_plaintext);
    }

    #[test]
    fn allow_plaintext_env_true_and_default() {
        for on in ["1", "true"] {
            let e = env(&[("FLEET_HUB_ALLOW_PLAINTEXT", on)]);
            assert!(resolve(&routable(), &e, &|_| None).unwrap().allow_plaintext);
        }
        assert!(
            !resolve(&opts(), &env(&[]), &|_| None)
                .unwrap()
                .allow_plaintext
        );
    }

    #[test]
    fn allow_plaintext_env_garbage_is_an_error_naming_the_variable() {
        let e = env(&[("FLEET_HUB_ALLOW_PLAINTEXT", "yes")]);
        let err = resolve(&opts(), &e, &|_| None).unwrap_err();
        assert!(err.contains("FLEET_HUB_ALLOW_PLAINTEXT"), "{err}");
    }

    #[test]
    fn public_url_flag_beats_env_beats_setting_beats_default() {
        let settings =
            |k: &str| (k == "hub.public_url").then(|| "https://s.example.com".to_string());
        let e = env(&[("FLEET_HUB_PUBLIC_URL", "https://e.example.com")]);
        let mut o = opts();
        o.public_url = Some("https://f.example.com".into());
        let r = resolve(&o, &e, &settings).unwrap();
        assert_eq!(r.public_url.as_deref(), Some("https://f.example.com"));
        let r = resolve(&opts(), &e, &settings).unwrap();
        assert_eq!(r.public_url.as_deref(), Some("https://e.example.com"));
        let r = resolve(&opts(), &env(&[]), &settings).unwrap();
        assert_eq!(r.public_url.as_deref(), Some("https://s.example.com"));
        let r = resolve(&opts(), &env(&[]), &|_| None).unwrap();
        assert_eq!(r.public_url, None);
    }

    #[test]
    fn local_host_flag_beats_env_beats_setting_beats_default() {
        let stored_true = |k: &str| (k == "hub.local_host").then(|| "true".to_string());
        let stored_false = |k: &str| (k == "hub.local_host").then(|| "false".to_string());
        let env_false = env(&[("FLEET_HUB_LOCAL_HOST", "false")]);
        let env_true = env(&[("FLEET_HUB_LOCAL_HOST", "true")]);
        let mut flag_true = opts();
        flag_true.local_host = Some(true);
        let mut flag_false = opts();
        flag_false.local_host = Some(false);
        assert!(
            resolve(&flag_true, &env_false, &stored_false)
                .unwrap()
                .local_host
        );
        assert!(
            !resolve(&flag_false, &env_true, &stored_true)
                .unwrap()
                .local_host
        );
        assert!(
            !resolve(&opts(), &env_false, &stored_true)
                .unwrap()
                .local_host
        );
        assert!(
            resolve(&opts(), &env_true, &stored_false)
                .unwrap()
                .local_host
        );
        assert!(
            resolve(&opts(), &env(&[]), &stored_true)
                .unwrap()
                .local_host
        );
        assert!(!resolve(&opts(), &env(&[]), &|_| None).unwrap().local_host);
    }

    #[test]
    fn allowed_host_flag_beats_env_and_setting() {
        let settings = |k: &str| (k == "hub.allowed_hosts").then(|| "s.example.com".to_string());
        let e = env(&[("FLEET_HUB_ALLOWED_HOSTS", "e.example.com")]);
        let mut o = opts();
        o.allowed_host = vec!["f.example.com".into(), "g.example.com".into()];
        let r = resolve(&o, &e, &settings).unwrap();
        assert_eq!(
            r.allowed_hosts_explicit,
            vec!["f.example.com".to_string(), "g.example.com".to_string()]
        );
        let r = resolve(&opts(), &e, &settings).unwrap();
        assert_eq!(r.allowed_hosts_explicit, vec!["e.example.com".to_string()]);
        let r = resolve(&opts(), &env(&[]), &settings).unwrap();
        assert_eq!(r.allowed_hosts_explicit, vec!["s.example.com".to_string()]);
    }

    #[test]
    fn data_dir_and_log_dir_come_from_env_when_no_flag() {
        let e = env(&[
            ("FLEET_HUB_DATA_DIR", "/env/data"),
            ("FLEET_HUB_LOG_DIR", "/env/logs"),
        ]);
        let r = resolve(&opts(), &e, &|_| None).unwrap();
        assert_eq!(r.data_dir, PathBuf::from("/env/data"));
        assert_eq!(r.log_dir, PathBuf::from("/env/logs"));
        // Env data dir alone: logs default beneath it.
        let e = env(&[("FLEET_HUB_DATA_DIR", "/env/data")]);
        let r = resolve(&opts(), &e, &|_| None).unwrap();
        assert_eq!(r.log_dir, PathBuf::from("/env/data/logs"));
        // Flags still win.
        let mut o = opts();
        o.data_dir = Some("/flag/data".into());
        o.log_dir = Some("/flag/logs".into());
        let e = env(&[
            ("FLEET_HUB_DATA_DIR", "/env/data"),
            ("FLEET_HUB_LOG_DIR", "/env/logs"),
        ]);
        let r = resolve(&o, &e, &|_| None).unwrap();
        assert_eq!(r.data_dir, PathBuf::from("/flag/data"));
        assert_eq!(r.log_dir, PathBuf::from("/flag/logs"));
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
