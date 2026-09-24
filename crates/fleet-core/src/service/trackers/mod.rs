//! Tracker providers (work graph M3, design §0.4): the trait every tracker
//! adapter implements, the normalised snapshot it produces, and the errors it
//! maps its HTTP answers onto. Jira Cloud is [`jira`]; the sync tick that
//! drives a provider is [`sync`]; the admin surface (`work_admin`) is
//! [`admin`].
//!
//! Trackers **enrich and never gate**: every function here is reached only
//! from the sync tick, `work_admin`'s `test`, or a `work { lookup }` that
//! falls through the cache. Nothing in M1/M2 waits on a tracker.

pub mod admin;
pub mod asana;
#[cfg(test)]
pub mod conformance;
pub mod github;
pub mod jira;
pub mod sync;
pub mod tickets;

use crate::net::https::{DirectTransport, HostPolicy, HttpTransport};
use crate::store::{TrackerConfig, TrackerCredential, TrackerRow};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// How a provider reads only what changed (M6.0).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Incremental {
    /// A time watermark per view (`updated >= since`), minus an overlap.
    #[default]
    Watermark,
    /// An opaque per-view sync token ([`TrackerProvider::changes`]); an
    /// expired token means one whole listing (Asana's events API).
    SyncToken,
    /// Every pass lists every view whole.
    None,
}

/// What a provider can do; the UI degrades on what is missing (M6.0).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Caps {
    /// `jql` (Jira), `gql` (Linear, GitHub), `search`, or none.
    #[serde(default)]
    pub query_lang: Option<String>,
    /// Items have parents (epics, sub-issues, subtasks).
    #[serde(default)]
    pub hierarchy: bool,
    /// Sprints / cycles.
    #[serde(default)]
    pub iterations: bool,
    /// Items have human keys (`ABC-123`) with prefixes the probe learns;
    /// without them, detection is by URL (and repo-relative `#n`) only.
    #[serde(default)]
    pub human_keys: bool,
    /// A bare `#123` names an item of the session's own `owner/repo`.
    #[serde(default)]
    pub repo_relative: bool,
    /// One item can sit in several containers (Asana projects).
    #[serde(default)]
    pub multi_container: bool,
    #[serde(default)]
    pub incremental: Incremental,
    /// Always false in M3–M6: read-only.
    #[serde(default)]
    pub write: bool,
}

/// What a probe learned: the site's instance id and the config to store.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrackerInfo {
    pub instance_id: Option<String>,
    pub config: TrackerConfig,
}

/// One query a sync runs (a built-in or a favourite filter).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewDef {
    pub id: String,
    pub label: String,
    pub query: String,
}

/// An item's status, normalised.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusSnapshot {
    /// The tracker's own name ("In Review").
    pub name: String,
    /// todo | in_progress | done.
    pub category: String,
    /// completed | not_planned | duplicate, once resolved.
    #[serde(default)]
    pub resolution: Option<String>,
}

/// One tracker item, normalised (review §1, C24–C28). Identity is
/// `external_id`; everything else is an attribute.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkItemSnapshot {
    pub external_id: String,
    pub key: Option<String>,
    /// Former keys, when the tracker says (or a by-key fetch reveals) the
    /// item moved.
    pub aliases: Vec<String>,
    pub title: String,
    pub url: Option<String>,
    pub kind: Option<String>,
    pub hierarchy_level: Option<i64>,
    pub status: StatusSnapshot,
    pub parent_external_id: Option<String>,
    pub parent_key: Option<String>,
    /// The project key(s) the item lives in.
    pub containers: Vec<String>,
    /// Display names.
    pub assignees: Vec<String>,
    /// The assignee's account id, for the local `mine` view.
    pub assignee_id: Option<String>,
    /// The current sprint's name.
    pub iteration: Option<String>,
    /// The sprint named in `iteration` is active.
    pub iteration_active: bool,
    /// The tracker's `updated`, unix seconds.
    pub updated: Option<i64>,
    /// The first [`DESCRIPTION_MAX_CHARS`] of the description as plain text.
    /// Third-party text: anything that reaches an agent goes through
    /// `mark_untrusted`.
    pub description: Option<String>,
}

/// Longest description excerpt kept (plan: the first 2k chars).
pub const DESCRIPTION_MAX_CHARS: usize = 2000;

/// A by-id fetch's answer for one item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fetched {
    Found(Box<WorkItemSnapshot>),
    /// Deleted, or no longer visible to the API user — a tracker cannot
    /// tell the two apart (C25), so neither may fleet.
    Unavailable {
        reference: String,
        reason: String,
    },
}

/// `Unavailable.reason` for a 404 or an id missing from a bulk answer.
pub const NOT_FOUND_OR_NO_PERMISSION: &str = "not_found_or_no_permission";

/// A reference to an item recognised in text (or stored on a link).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ItemRef {
    /// The tracker's own id (`external_id`).
    Id(String),
    /// A human key (`ABC-123`), or the canonical reference of a provider
    /// without human keys (`owner/repo#42`, `asana:<gid>`).
    Key(String),
    /// An issue number in a repository (`owner/repo`, lower case).
    RepoNumber { repo: String, n: u64 },
    /// An item's web URL, as pasted.
    Url(String),
}

impl ItemRef {
    /// The canonical text of the reference: what `Fetched::Unavailable`
    /// carries back and what a link's `ref_key` holds.
    pub fn reference(&self) -> String {
        match self {
            ItemRef::Id(s) | ItemRef::Key(s) | ItemRef::Url(s) => s.clone(),
            ItemRef::RepoNumber { repo, n } => format!("{repo}#{n}"),
        }
    }

    /// A stored reference (`ref_key`, a lookup's text) as an [`ItemRef`]:
    /// `owner/repo#n` is a repo number, a URL is a URL, anything else a key.
    pub fn parse(r: &str) -> ItemRef {
        let r = r.trim();
        if r.starts_with("https://") || r.starts_with("http://") {
            return ItemRef::Url(r.to_string());
        }
        if let Some((repo, n)) = r.rsplit_once('#') {
            if repo.contains('/') {
                if let Ok(n) = n.parse::<u64>() {
                    return ItemRef::RepoNumber {
                        repo: repo.to_ascii_lowercase(),
                        n,
                    };
                }
            }
        }
        ItemRef::Key(r.to_string())
    }
}

/// What recognition knows besides the text (M6.0).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RefCtx<'a> {
    /// The session's GitHub `owner/repo`, for a bare `#123`.
    pub repo: Option<&'a str>,
}

/// What [`TrackerProvider::changes`] found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Changes {
    /// Items that changed since the mark (already normalised).
    pub items: Vec<WorkItemSnapshot>,
    /// The mark to pass next time (opaque, per provider).
    pub mark: Option<String>,
    /// The mark was missing or had expired: the caller lists the view
    /// whole once, then continues from `mark`.
    pub expired: bool,
}

/// One page of a view.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Page {
    pub items: Vec<WorkItemSnapshot>,
    /// Opaque; never persisted (C23).
    pub next: Option<String>,
}

/// Why a provider call failed, and what it means for the tracker's state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrackerError {
    /// 401, or a 403 on the identity call: the credential is wrong or
    /// expired (Atlassian API tokens expire within a year).
    Auth(String),
    /// Atlassian wants a browser login first (`X-Seraph-LoginReason:
    /// AUTHENTICATION_DENIED`).
    Captcha,
    /// 429 (or 503 with `Retry-After`).
    RateLimited { retry_after_secs: Option<u64> },
    /// Network: DNS, TCP, TLS, timeout.
    Unreachable(String),
    /// A 403 on one query: that view is disabled, the tracker stays ok.
    Forbidden(String),
    /// 404.
    NotFound,
    /// Refused before sending (a host outside the fence, plaintext).
    Refused(String),
    /// An answer fleet could not use (bad JSON, a redirect, a 5xx).
    Invalid(String),
    /// No credential is set, or the reference cannot be read.
    Unconfigured,
}

impl TrackerError {
    /// The `trackers.state` this error leaves the tracker in; `None` when
    /// the tracker's state is unaffected (one view, one item, a bad page).
    pub fn state(&self) -> Option<&'static str> {
        match self {
            TrackerError::Auth(_) => Some("auth_failed"),
            TrackerError::Captcha => Some("captcha"),
            TrackerError::RateLimited { .. } => Some("rate_limited"),
            TrackerError::Unreachable(_) => Some("unreachable"),
            TrackerError::Unconfigured => Some("unconfigured"),
            TrackerError::Refused(_) => Some("unreachable"),
            TrackerError::Forbidden(_) | TrackerError::NotFound | TrackerError::Invalid(_) => None,
        }
    }

    /// A sentence for `last_error` and the UI.
    pub fn explain(&self) -> String {
        match self {
            TrackerError::Auth(m) => format!(
                "the tracker refused the credential ({m}); Atlassian API tokens expire \
                 within a year — create a new one and set it again"
            ),
            TrackerError::Captcha => "Atlassian wants a browser login first (CAPTCHA); log in \
                 to the site in a browser, then test again"
                .into(),
            TrackerError::RateLimited { retry_after_secs } => match retry_after_secs {
                Some(s) => format!("rate-limited by the tracker; retrying after {s}s"),
                None => "rate-limited by the tracker; backing off".into(),
            },
            TrackerError::Unreachable(m) => format!("the tracker could not be reached: {m}"),
            TrackerError::Forbidden(m) => format!("not permitted: {m}"),
            TrackerError::NotFound => "not found, or not visible to this account".into(),
            TrackerError::Refused(m) => format!("refused: {m}"),
            TrackerError::Invalid(m) => format!("unexpected answer from the tracker: {m}"),
            TrackerError::Unconfigured => {
                "no credential is set (or its env:/file: reference cannot be read)".into()
            }
        }
    }

    /// As an [`IpcError`](crate::ipc_error::IpcError) for a live call.
    pub fn to_ipc(&self) -> crate::ipc_error::IpcError {
        use crate::ipc_error::{codes, IpcError};
        let code = match self {
            TrackerError::NotFound => codes::E_NOTFOUND,
            TrackerError::Forbidden(_) => codes::E_FORBIDDEN,
            _ => codes::E_TRACKER,
        };
        let mut e = IpcError::new(code, crate::logging::redact(&self.explain()).into_owned());
        if let Some(state) = self.state() {
            e = e.with_details(serde_json::json!({ "state": state }));
        }
        e
    }
}

/// The provider trait (design §0.4). Async methods are one HTTP exchange or
/// a short fixed sequence of them; the paging loop and every decision about
/// what to store live in [`sync`].
#[async_trait::async_trait]
pub trait TrackerProvider: Send + Sync {
    fn caps(&self) -> Caps;
    /// Who am I, which site is this, which keys and sprints does it have.
    async fn probe(&self) -> Result<TrackerInfo, TrackerError>;
    /// The views a sync runs, given what the probe learned.
    async fn views(&self, config: &TrackerConfig) -> Result<Vec<ViewDef>, TrackerError>;
    /// One page of `view`, restricted to items updated since `since` (unix
    /// seconds, already including any overlap) when given.
    async fn list(
        &self,
        view: &ViewDef,
        since: Option<i64>,
        cursor: Option<String>,
    ) -> Result<Page, TrackerError>;
    /// Items by id or key; every reference gets an answer.
    async fn fetch(&self, refs: &[ItemRef]) -> Result<Vec<Fetched>, TrackerError>;
    /// Item references in free text: this site's URLs, keys with a known
    /// prefix, and (where `caps.repo_relative`) a bare `#n` of `ctx.repo`.
    fn recognize(&self, text: &str, ctx: RefCtx<'_>) -> Vec<ItemRef>;
    /// What changed in `view` since `mark`, for a provider whose
    /// `caps.incremental` is [`Incremental::SyncToken`]. The default says
    /// the view has no token, so the caller lists it whole.
    async fn changes(&self, _view: &ViewDef, _mark: Option<&str>) -> Result<Changes, TrackerError> {
        Ok(Changes {
            expired: true,
            ..Default::default()
        })
    }
}

/// The provider for `row`, over the transport `net` picks for it. `cred` is
/// `None` when no credential is set; a provider that needs one then fails
/// every call [`TrackerError::Unconfigured`] (GitHub through `gh` needs
/// none: the host's own `gh` login is used).
pub fn provider_for(
    row: &TrackerRow,
    cred: Option<TrackerCredential>,
    net: &TrackerNet,
) -> Result<Box<dyn TrackerProvider>, TrackerError> {
    let transport = net.transport_for(row)?;
    Ok(match row.provider.as_str() {
        "jira" => Box::new(jira::JiraCloud::new(
            &row.site_url,
            row.config.clone(),
            cred,
            transport,
        )),
        "asana" => Box::new(asana::Asana::new(
            &row.site_url,
            row.config.clone(),
            row.settings.clone(),
            cred,
            transport,
        )),
        // Through `gh` the host's own login is used: never a token.
        "github" => Box::new(github::GitHub::new(
            &row.site_url,
            row.config.clone(),
            row.settings.clone(),
            transport,
        )),
        other => {
            return Err(TrackerError::Refused(format!(
                "this build has no {other:?} tracker provider"
            )))
        }
    })
}

/// Whether a provider needs a credential stored in fleet. GitHub through
/// `gh` uses the host's own login, so fleet never holds its token.
pub fn needs_credential(row: &TrackerRow) -> bool {
    !matches!(
        TransportKind::parse(&row.transport),
        Ok(TransportKind::ViaCli(_))
    )
}

/// Where a tracker's requests leave from (`trackers.transport`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportKind {
    /// HTTPS from this process (the hub or a standalone desktop).
    Direct,
    /// `curl` on that host, the token piped on stdin (M6.3).
    ViaHost(String),
    /// A trusted CLI on that host (`gh`), with its own login (M6.1).
    ViaCli(String),
}

impl TransportKind {
    pub fn parse(s: &str) -> Result<TransportKind, TrackerError> {
        let s = s.trim();
        if s.is_empty() || s == "direct" {
            return Ok(TransportKind::Direct);
        }
        let alias = |a: &str| -> Result<String, TrackerError> {
            crate::validate::host_alias(a)
                .map(|_| a.to_string())
                .map_err(|e| TrackerError::Refused(e.message))
        };
        if let Some(a) = s.strip_prefix("via_host:") {
            return Ok(TransportKind::ViaHost(alias(a)?));
        }
        if let Some(a) = s.strip_prefix("via_cli:") {
            return Ok(TransportKind::ViaCli(alias(a)?));
        }
        Err(TrackerError::Refused(format!(
            "unknown tracker transport {s:?}"
        )))
    }
}

/// What a tracker's transport is built from: the process's own HTTPS, and
/// SSH for `via_host` / `via_cli`. Tests put a fake in front of all of it.
#[derive(Clone, Default)]
pub struct TrackerNet {
    /// Every request goes here instead (tests).
    fake: Option<Arc<dyn HttpTransport>>,
    ssh: Option<Arc<dyn crate::ssh::SshExec>>,
}

impl TrackerNet {
    /// The real thing: direct HTTPS fenced per provider, and `ssh` (when
    /// this process has hosts) for `via_host` / `via_cli`.
    pub fn real(ssh: Option<Arc<dyn crate::ssh::SshExec>>) -> Self {
        TrackerNet { fake: None, ssh }
    }

    /// Every tracker, whatever its transport, talks to `t` (tests).
    pub fn fake(t: Arc<dyn HttpTransport>) -> Self {
        TrackerNet {
            fake: Some(t),
            ssh: None,
        }
    }

    /// Real transport selection over a scripted SSH (tests of `via_host` /
    /// `via_cli`).
    pub fn with_ssh(ssh: Arc<dyn crate::ssh::SshExec>) -> Self {
        TrackerNet {
            fake: None,
            ssh: Some(ssh),
        }
    }

    /// The transport `row` says, with the provider's host fence.
    pub fn transport_for(&self, row: &TrackerRow) -> Result<Arc<dyn HttpTransport>, TrackerError> {
        if let Some(f) = &self.fake {
            return Ok(Arc::clone(f));
        }
        let policy = host_policy(row);
        let ssh = |h: &str| {
            self.ssh.clone().ok_or_else(|| {
                TrackerError::Unreachable(format!("this process has no SSH to reach {h} with"))
            })
        };
        match TransportKind::parse(&row.transport)? {
            TransportKind::Direct => Ok(Arc::new(DirectTransport::new(policy))),
            TransportKind::ViaCli(h) => match row.provider.as_str() {
                "github" => Ok(Arc::new(crate::net::via_host::GhCliTransport::new(
                    ssh(&h)?,
                    h,
                ))),
                p => Err(TrackerError::Refused(format!(
                    "no trusted CLI is known for {p} trackers"
                ))),
            },
            TransportKind::ViaHost(h) => Err(TrackerError::Refused(format!(
                "this build cannot reach a tracker through {h} yet"
            ))),
        }
    }
}

static DEFAULT_NET: std::sync::OnceLock<TrackerNet> = std::sync::OnceLock::new();

/// Install the process's tracker network once at startup (the hub and a
/// standalone desktop, with their SSH client, so `via_host` / `via_cli`
/// trackers work). A second call is ignored.
pub fn install_default_net(net: TrackerNet) {
    let _ = DEFAULT_NET.set(net);
}

/// The process's tracker network: what [`install_default_net`] set, else
/// direct HTTPS only.
pub fn default_net() -> TrackerNet {
    DEFAULT_NET.get().cloned().unwrap_or_default()
}

/// The hosts a tracker's requests may go to: its provider's API only (the
/// SSRF fence, enforced by every transport before anything is sent).
pub fn host_policy(row: &TrackerRow) -> HostPolicy {
    match row.provider.as_str() {
        "github" => Arc::new(|h: &str| h == crate::net::via_host::GITHUB_API_HOST),
        "asana" => Arc::new(|h: &str| h == asana::API_HOST),
        _ => Arc::new(crate::store::is_allowed_tracker_host),
    }
}

/// Which call a response answers; decides what a 403 means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CallKind {
    /// Who am I: a 403 here is an auth failure.
    Identity,
    /// A view's query: a 403 disables that view only.
    View,
    Other,
}

/// A transport failure → the tracker error it means.
pub(crate) fn map_transport(e: crate::net::https::TransportError) -> TrackerError {
    use crate::net::https::TransportError as T;
    match e {
        T::Refused(m) => TrackerError::Refused(m),
        T::Connect(m) => TrackerError::Unreachable(m),
        T::Timeout => TrackerError::Unreachable("timed out".into()),
        T::TooLarge(m) | T::Protocol(m) => TrackerError::Invalid(m),
    }
}

/// `Retry-After` in seconds, else the seconds until `X-RateLimit-Reset`
/// (a unix time: GitHub, Linear) when the quota is spent.
pub(crate) fn retry_after(resp: &crate::net::https::Response) -> Option<u64> {
    if let Some(s) = resp
        .header("Retry-After")
        .and_then(|v| v.trim().parse::<u64>().ok())
    {
        return Some(s);
    }
    let spent = resp
        .header("X-RateLimit-Remaining")
        .or_else(|| resp.header("X-RateLimit-Requests-Remaining"))
        .is_some_and(|v| v.trim() == "0");
    if !spent {
        return None;
    }
    let reset = resp
        .header("X-RateLimit-Reset")
        .or_else(|| resp.header("X-RateLimit-Requests-Reset"))
        .and_then(|v| v.trim().parse::<i64>().ok())?;
    // Linear sends milliseconds.
    let reset = if reset > 10_000_000_000 {
        reset / 1000
    } else {
        reset
    };
    Some((reset - crate::service::catalog::now_secs()).clamp(1, 3600) as u64)
}

/// Map a non-2xx answer for the providers after Jira (GitHub, Asana,
/// Linear, Jira DC): the same table as Jira's, plus a 403 that is really a
/// spent rate limit.
pub(crate) fn check_http(
    resp: &crate::net::https::Response,
    call: CallKind,
) -> Result<(), TrackerError> {
    if resp.is_success() {
        return Ok(());
    }
    let wait = retry_after(resp);
    Err(match resp.status {
        401 => TrackerError::Auth("401 Unauthorized".into()),
        403 | 429 if wait.is_some() => TrackerError::RateLimited {
            retry_after_secs: wait,
        },
        429 => TrackerError::RateLimited {
            retry_after_secs: None,
        },
        403 if call == CallKind::Identity => TrackerError::Auth("403 on the identity call".into()),
        403 if call == CallKind::View => TrackerError::Forbidden("403 on this view's query".into()),
        403 => TrackerError::Forbidden("403".into()),
        404 => TrackerError::NotFound,
        503 if wait.is_some() => TrackerError::RateLimited {
            retry_after_secs: wait,
        },
        300..=399 => TrackerError::Invalid(format!("{} redirect (not followed)", resp.status)),
        s => TrackerError::Invalid(format!("HTTP {s}")),
    })
}

/// Unix seconds → `YYYY-MM-DDTHH:MM:SSZ` (the inverse of
/// [`parse_timestamp`] for UTC).
pub fn format_timestamp(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Most pages one view listing reads in one pass.
pub const MAX_PAGES: usize = 20;

/// Every page of `view`, following cursors until the last one. A cursor
/// that repeats ends the listing (C23: `search/jql`'s token has been seen to
/// loop) with what was read; so does [`MAX_PAGES`].
pub async fn list_all(
    p: &dyn TrackerProvider,
    view: &ViewDef,
    since: Option<i64>,
) -> Result<Vec<WorkItemSnapshot>, TrackerError> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut cursor: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let page = p.list(view, since, cursor.clone()).await?;
        out.extend(page.items);
        match page.next {
            Some(next) if seen.insert(next.clone()) => cursor = Some(next),
            Some(_) => {
                tracing::warn!(view = %view.id, "tracker page token repeated; stopping the listing");
                break;
            }
            None => break,
        }
    }
    Ok(out)
}

/// Parse a tracker timestamp (`2026-09-20T10:15:30.123+0200`, `…Z`,
/// `…+02:00`) to unix seconds. `None` when it is not one.
pub fn parse_timestamp(s: &str) -> Option<i64> {
    let s = s.trim();
    let b = s.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || (b[10] != b'T' && b[10] != b' ') {
        return None;
    }
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, se) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || se > 60 {
        return None;
    }
    let mut rest = &s[19..];
    if let Some(r) = rest.strip_prefix('.') {
        let digits = r.bytes().take_while(u8::is_ascii_digit).count();
        rest = &r[digits..];
    }
    let offset = match rest {
        "" | "Z" | "z" => 0,
        _ => {
            let sign = match rest.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let hhmm: String = rest[1..].chars().filter(|c| *c != ':').collect();
            if hhmm.len() != 4 || !hhmm.bytes().all(|c| c.is_ascii_digit()) {
                return None;
            }
            let oh: i64 = hhmm[..2].parse().ok()?;
            let om: i64 = hhmm[2..].parse().ok()?;
            sign * (oh * 3600 + om * 60)
        }
    };
    // Days from the civil date (Howard Hinnant's algorithm).
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + h * 3600 + mi * 60 + se - offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_parse_with_every_offset_shape() {
        assert_eq!(parse_timestamp("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_timestamp("1970-01-01T01:00:00.000+0100"), Some(0));
        assert_eq!(parse_timestamp("1970-01-01T00:00:00-01:30"), Some(5400));
        assert_eq!(
            parse_timestamp("2026-09-20T10:15:30.123+0200"),
            Some(1_789_892_130)
        );
        assert_eq!(parse_timestamp("2024-02-29T12:00:00Z"), Some(1_709_208_000));
        for bad in [
            "",
            "yesterday",
            "2026-13-01T00:00:00Z",
            "2026-09-20T10:15:30+2",
        ] {
            assert_eq!(parse_timestamp(bad), None, "{bad}");
        }
    }

    #[test]
    fn timestamps_format_back() {
        for t in [0, 1_789_892_130, 1_709_208_000, 951_782_400] {
            assert_eq!(parse_timestamp(&format_timestamp(t)), Some(t));
        }
        assert_eq!(format_timestamp(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn errors_map_to_states_and_never_carry_a_secret() {
        assert_eq!(
            TrackerError::Auth("401".into()).state(),
            Some("auth_failed")
        );
        assert_eq!(TrackerError::Captcha.state(), Some("captcha"));
        assert_eq!(TrackerError::Forbidden("view".into()).state(), None);
        let e =
            TrackerError::Unreachable("Authorization: Basic bWVAeDpBVEFUVHh4eHh4eHh4eA==".into())
                .to_ipc();
        assert_eq!(e.code, crate::ipc_error::codes::E_TRACKER);
        assert!(!e.message.contains("bWVAeD"), "{}", e.message);
        assert!(TrackerError::Auth("x".into()).explain().contains("expire"));
    }
}
