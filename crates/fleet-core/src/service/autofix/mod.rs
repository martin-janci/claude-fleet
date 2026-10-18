//! Autofix: detect known broken states, explain them in plain language, and
//! offer (or, for safe ones, apply) a fix. Spec:
//! `docs/superpowers/specs/2026-10-17-autofix-design.md`.
//!
//! Detectors are PURE over a [`ScanCx`] the runner gathers under one brief
//! lock. A fix is a [`FixPlan`] the hub wrote itself; `apply::FixExec` runs it.

pub mod api;
pub mod apply;
pub mod host_stale;
pub mod mcp_entry;
pub mod operator_unbound;
pub mod stuck;
pub mod sync;
pub mod tick;

use crate::service::health::HostHealthRow;
use crate::service::playbooks::PlaybookConfig;
use crate::store::{ClientTokenRow, HostRow, SessionRow};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Blocking,
    Degraded,
    Info,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Blocking => "blocking",
            Severity::Degraded => "degraded",
            Severity::Info => "info",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Safety {
    Safe,
    NeedsApproval,
}

/// What a fix does, as data the hub wrote. Never built from a caller's input.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum FixPlan {
    BindOperator,
    ProvisionContent {
        host: String,
    },
    UpdateAgent {
        host: String,
    },
    /// Rewrite the host's `claude-fleet` entry in `~/.claude.json` with the
    /// hub's URL and the host's own stored token (`merge_mcp_entry`).
    RewriteMcpEntry {
        host: String,
    },
    /// Press Enter in a stuck session's pane, through the playbook
    /// executor (which skips a pane a person is attached to). Refused for
    /// the controller, and when the session no longer shows `stuck_kind`.
    PressEnter {
        session_id: i64,
        host: String,
        tmux_name: String,
        stuck_kind: String,
    },
    /// Recreate a session that ran out of memory (the `oom` playbook's
    /// action). Refused for the controller and while a turn is running.
    Recreate {
        session_id: i64,
    },
}

/// A step the app takes for a finding that has no fix fleet can run: a
/// login on a host (the `error` card's `login` next step).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ClientAction {
    Login { host: String },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Fix {
    pub summary: String,
    pub changes: Vec<String>,
    pub safety: Safety,
    pub reversible: bool,
    /// Whether the runner may apply it by itself when `safety` is Safe.
    pub auto: bool,
    pub plan: FixPlan,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FindingBody {
    pub title: String,
    pub explanation: String,
    pub evidence: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<Fix>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<ClientAction>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// Detector id, or `<id>.<variant>`.
    pub kind: String,
    /// `token:<name>`, `host:<alias>`, `session:<id>` or `hub`.
    pub subject: String,
    pub host_alias: Option<String>,
    pub severity: Severity,
    pub body: FindingBody,
}

/// Everything a detector may read. Gathered by [`apply::gather`].
#[derive(Debug, Clone, Default)]
pub struct ScanCx {
    pub now: i64,
    pub owner: Option<i64>,
    pub client_tokens: Vec<ClientTokenRow>,
    pub hosts: Vec<HostRow>,
    pub health: Vec<HostHealthRow>,
    /// Every session row (detector 3 reads `stuck_kind`).
    pub sessions: Vec<SessionRow>,
    /// The registered controller `(host, tmux_name)`: never sent keys.
    pub controller: Option<(String, String)>,
    pub playbooks: PlaybookConfig,
    /// Detector 2: the hub's MCP URL, `None` when it cannot be read.
    pub mcp_url: Option<String>,
    /// Detector 2: the last probe of each host's global MCP entry.
    pub mcp_probes: Vec<mcp_entry::HostProbe>,
    /// Detector 2: `(host, sha256 of its stored token)`.
    pub host_token_shas: Vec<(String, String)>,
    /// Detector 2: revoked client tokens, to name a stale bearer.
    pub revoked_client_tokens: Vec<ClientTokenRow>,
    /// Detector 2: the operator's token hash (`operator.token_sha`).
    pub operator_sha: Option<String>,
}

pub struct Detector {
    pub id: &'static str,
    pub scan: fn(&ScanCx) -> Vec<Finding>,
}

pub const DETECTORS: &[Detector] = &[
    Detector {
        id: "operator_unbound",
        scan: operator_unbound::scan,
    },
    Detector {
        id: "host",
        scan: host_stale::scan,
    },
    Detector {
        id: "host_mcp_entry",
        scan: mcp_entry::scan,
    },
    Detector {
        id: "stuck",
        scan: stuck::scan,
    },
];

pub fn scan_all(cx: &ScanCx) -> Vec<Finding> {
    DETECTORS.iter().flat_map(|d| (d.scan)(cx)).collect()
}

/// The detector that owns `kind` (`id` or `id.variant`).
pub fn detector_of(kind: &str) -> Option<&'static Detector> {
    DETECTORS
        .iter()
        .find(|d| kind == d.id || kind.strip_prefix(d.id).is_some_and(|r| r.starts_with('.')))
}

/// A token as evidence may name it: its name and a 12-char hash prefix.
pub(crate) fn token_label(name: &str, sha: &str) -> String {
    format!("{name} (sha256 {}…)", &sha[..sha.len().min(12)])
}
