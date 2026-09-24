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

/// Providers a tracker may be. GitHub, Asana and Linear arrive with M6.
pub const TRACKER_PROVIDERS: &[&str] = &["jira"];

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
     s.auth_kind, s.username, s.value, s.credential_ref";

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
        let site = normalize_site_url(site_url)?;
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
        self.require_tracker(id)?;
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

    /// Stamp a finished sync pass.
    pub fn set_tracker_synced(&self, id: i64, at: i64) -> Result<(), IpcError> {
        self.conn.execute(
            "UPDATE trackers SET last_sync_at = ?1 WHERE id = ?2",
            rusqlite::params![at, id],
        )?;
        Ok(())
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
            "SELECT tracker_id, view_id, label, query, watermark, enabled FROM tracker_views \
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
            3,
            "tracker_secrets is read by resolve_tracker_credential, \
             tracker_secret_literals and deleted by remove_tracker only"
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
