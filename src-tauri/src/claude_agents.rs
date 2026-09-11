//! Parse the JSON emitted by `claude agents --json`.
//!
//! The CLI has emitted two shapes over time, and a fleet can see both at
//! once (each host runs its own CLI version):
//!
//! * **old:** `status` already in fleet's `claude_status` vocabulary
//!   (`working | blocked | completed | failed | stopped | idle`);
//! * **new** (live-checked on Claude Code 2.1.267): interactive rows carry
//!   `kind: "interactive"` and `status: busy | idle` with no `state`;
//!   background rows carry `kind: "background"` and `state` (`blocked`
//!   observed; documented `working | blocked | done | failed | stopped`),
//!   sometimes with `status: "idle"` as well. The docs also describe
//!   `status: waiting` plus a `waitingFor` reason while a session waits on
//!   the user.
//!
//! [`normalize_status`] folds both shapes into the fleet vocabulary at parse
//! time, so `ClaudeAgentRow::status` only ever holds a known value (or
//! `None`) and callers such as `known_agent_status` never see CLI spellings.

use crate::service::pane_intel::ClaudeStatus;
use serde::Deserialize;
use serde_json::Value;

/// One row from `claude agents --json`, with `status` already normalized.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(from = "RawAgentRow")]
pub struct ClaudeAgentRow {
    /// The Claude-internal session ID (used to call `claude logs <id>`).
    pub session_id: Option<String>,
    /// Display name, matches tmux session name when created with `--name`.
    pub name: Option<String>,
    /// Fleet `claude_status` value derived from the row's `state`,
    /// `waitingFor` and `status` (see [`normalize_status`]); `None` when the
    /// row carries nothing recognisable.
    pub status: Option<String>,
    /// Working directory of the Claude session.
    pub cwd: Option<String>,
}

/// A row exactly as the CLI prints it. The status-ish fields are loose
/// `Value`s: one row with an unexpected type must not fail the whole array,
/// because a parse failure drops every agent on the host.
#[derive(Deserialize)]
struct RawAgentRow {
    #[serde(rename = "sessionId", default)]
    session_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    /// `"interactive"` | `"background"` (new CLI). Logged only: `state`
    /// already wins over `status`, whichever kind of row carries it.
    #[serde(default)]
    kind: Option<Value>,
    /// Lifecycle state (new CLI, background rows).
    #[serde(default)]
    state: Option<Value>,
    /// Process status: the fleet vocabulary (old CLI) or `busy|waiting|idle`.
    #[serde(default)]
    status: Option<Value>,
    /// What the session is waiting on, e.g. a permission prompt.
    #[serde(rename = "waitingFor", alias = "waiting_for", default)]
    waiting_for: Option<Value>,
}

impl From<RawAgentRow> for ClaudeAgentRow {
    fn from(raw: RawAgentRow) -> Self {
        let state = value_str(raw.state.as_ref());
        let status = value_str(raw.status.as_ref());
        let waiting_for = waiting_reason(raw.waiting_for.as_ref());
        let normalized =
            normalize_status(state.as_deref(), status.as_deref(), waiting_for.as_deref());
        let agent = raw.name.as_deref().unwrap_or("");
        if let Some(reason) = &waiting_for {
            // No column holds the waiting reason yet, so the log is the only
            // place it is recorded.
            tracing::debug!(agent, waiting_for = %reason, "claude agents: session waits on the user");
        }
        if normalized.is_none() && (state.is_some() || status.is_some()) {
            tracing::debug!(
                agent,
                kind = ?raw.kind,
                state = state.as_deref().unwrap_or(""),
                status = status.as_deref().unwrap_or(""),
                "claude agents: no known status in row; the pane fallback decides"
            );
        }
        ClaudeAgentRow {
            session_id: raw.session_id,
            name: raw.name,
            status: normalized.map(|s| s.as_str().to_string()),
            cwd: raw.cwd,
        }
    }
}

/// A trimmed, lowercased, non-empty string out of a loose JSON value.
fn value_str(v: Option<&Value>) -> Option<String> {
    let s = v?.as_str()?.trim().to_ascii_lowercase();
    (!s.is_empty()).then_some(s)
}

/// A readable waiting reason, or `None` when the session is not waiting.
/// Accepts a string (`"permission prompt"`) or an object carrying one under a
/// common key; `null`, `false`, `""` and `{}` all mean "not waiting".
fn waiting_reason(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::String(s) => {
            let s = s.trim();
            (!s.is_empty()).then(|| s.to_string())
        }
        Value::Object(map) if !map.is_empty() => Some(
            ["type", "kind", "reason", "tool", "message"]
                .iter()
                .find_map(|k| map.get(*k).and_then(Value::as_str))
                .unwrap_or("input")
                .to_string(),
        ),
        Value::Bool(true) => Some("input".to_string()),
        _ => None,
    }
}

/// Map one CLI spelling, old or new, onto the fleet vocabulary.
fn map_value(v: &str) -> Option<ClaudeStatus> {
    Some(match v {
        "working" | "busy" => ClaudeStatus::Working,
        "blocked" | "waiting" => ClaudeStatus::Blocked,
        "completed" | "done" => ClaudeStatus::Completed,
        "failed" => ClaudeStatus::Failed,
        "stopped" => ClaudeStatus::Stopped,
        "idle" => ClaudeStatus::Idle,
        _ => return None,
    })
}

/// Fold a row's `state`, `waitingFor` and `status` into one fleet status.
///
/// Precedence, most specific first:
/// 1. `state`, the lifecycle: a background row with `state: blocked` and
///    `status: idle` is blocked, and a `done` one is completed;
/// 2. a non-empty `waitingFor` means blocked, whatever `status` says;
/// 3. `status`, in the old vocabulary or as `busy | waiting | idle`.
///
/// An unrecognised value falls through to the next source. `None` comes back
/// when nothing is recognised, so the pane-derived fallback decides.
fn normalize_status(
    state: Option<&str>,
    status: Option<&str>,
    waiting_for: Option<&str>,
) -> Option<ClaudeStatus> {
    if let Some(st) = state.and_then(map_value) {
        return Some(st);
    }
    if waiting_for.is_some() {
        return Some(ClaudeStatus::Blocked);
    }
    status.and_then(map_value)
}

/// Parse the stdout of `claude agents --json`. Returns empty vec on any parse
/// failure (the fleet treats missing data as degraded-gracefully, not an error).
pub fn parse_claude_agents_json(json: &str) -> Vec<ClaudeAgentRow> {
    serde_json::from_str(json).unwrap_or_default()
}

/// Find the first `ClaudeAgentRow` whose `name` matches `tmux_name`.
pub fn find_by_name<'a>(rows: &'a [ClaudeAgentRow], tmux_name: &str) -> Option<&'a ClaudeAgentRow> {
    rows.iter().find(|r| r.name.as_deref() == Some(tmux_name))
}

/// Correlate a fleet session to its running Claude agent so we can capture the
/// real `sessionId`. Prefer an exact `name` match (set when the session was
/// launched with `--name <tmux_name>`). Otherwise — for sessions launched
/// before `--name` was passed — fall back to a UNIQUE `cwd` match: return the
/// single agent whose `cwd == cwd`, or `None` when zero or more than one match
/// (ambiguous, e.g. several Claude sessions share that directory — we refuse to
/// guess rather than resume the wrong conversation). For a LOCAL session
/// (`is_local`), when no cwd matches exactly the canonical forms are
/// compared, so a logical spelling (a pane's `$PWD` under a symlinked root)
/// still finds the agent `claude agents` reports under the physical path.
/// Exact matches cost no syscalls. A remote cwd is never canonicalized: it is
/// a path on another machine, and resolving it here would stat the LOCAL
/// filesystem under the store lock (on macOS `/home` is an autofs mount that
/// can stall).
pub fn find_for_session<'a>(
    rows: &'a [ClaudeAgentRow],
    tmux_name: &str,
    cwd: &str,
    is_local: bool,
) -> Option<&'a ClaudeAgentRow> {
    if let Some(by_name) = find_by_name(rows, tmux_name) {
        return Some(by_name);
    }
    // An empty cwd identifies nothing; never pair two unknowns.
    if cwd.is_empty() {
        return None;
    }
    let mut in_cwd = rows.iter().filter(|r| r.cwd.as_deref() == Some(cwd));
    match (in_cwd.next(), in_cwd.next()) {
        (Some(only), None) => Some(only),
        (Some(_), Some(_)) => None,
        (None, _) if !is_local => None,
        (None, _) => {
            use crate::projects::path_identity::canonical;
            use std::path::Path;
            let key = canonical(Path::new(cwd));
            let mut same = rows.iter().filter(|r| {
                r.cwd
                    .as_deref()
                    .is_some_and(|c| !c.is_empty() && canonical(Path::new(c)) == key)
            });
            match (same.next(), same.next()) {
                (Some(only), None) => Some(only),
                _ => None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(session_id: &str, name: Option<&str>, cwd: Option<&str>) -> ClaudeAgentRow {
        ClaudeAgentRow {
            session_id: Some(session_id.into()),
            name: name.map(Into::into),
            status: None,
            cwd: cwd.map(Into::into),
        }
    }

    #[test]
    fn find_for_session_prefers_name_then_unique_cwd() {
        let rows = vec![
            row("byname", Some("dev-x"), Some("/a")),
            row("bycwd", None, Some("/b")),
            row("amb1", None, Some("/c")),
            row("amb2", None, Some("/c")),
        ];
        // Exact name match wins, even if cwd differs.
        assert_eq!(
            find_for_session(&rows, "dev-x", "/zzz", true)
                .unwrap()
                .session_id
                .as_deref(),
            Some("byname")
        );
        // No name match → unique cwd match.
        assert_eq!(
            find_for_session(&rows, "no-name", "/b", true)
                .unwrap()
                .session_id
                .as_deref(),
            Some("bycwd")
        );
        // Ambiguous cwd (two agents) → None (refuse to guess).
        assert!(find_for_session(&rows, "no-name", "/c", true).is_none());
        // No match at all → None.
        assert!(find_for_session(&rows, "no-name", "/nope", true).is_none());
    }

    /// `(physical, logical)` spellings of one directory reached through a
    /// symlinked root, like `~/projects -> /mnt/sda4/projects`.
    #[cfg(unix)]
    fn symlinked_dir(tmp: &tempfile::TempDir) -> (String, String) {
        use crate::projects::path_identity::canonical;
        let real = tmp.path().join("mnt").join("r");
        std::fs::create_dir_all(&real).unwrap();
        let link = tmp.path().join("projects");
        std::os::unix::fs::symlink(tmp.path().join("mnt"), &link).unwrap();
        (
            canonical(&real).to_string_lossy().into_owned(),
            link.join("r").to_string_lossy().into_owned(),
        )
    }

    #[cfg(unix)]
    #[test]
    fn find_for_session_matches_a_logical_cwd_to_the_physical_agent_cwd() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (physical, logical) = symlinked_dir(&tmp);
        let rows = vec![
            row("a", None, Some(&physical)),
            row("b", None, Some("/elsewhere")),
        ];
        assert_eq!(
            find_for_session(&rows, "no-name", &logical, true)
                .unwrap()
                .session_id
                .as_deref(),
            Some("a")
        );
        // Two agents that are the same directory stay ambiguous.
        let rows = vec![
            row("a", None, Some(&physical)),
            row("c", None, Some(&physical)),
        ];
        assert!(find_for_session(&rows, "no-name", &logical, true).is_none());
        // An empty cwd never matches.
        assert!(find_for_session(&[row("e", None, Some(""))], "no-name", "", true).is_none());
    }

    /// A remote cwd is a path on another machine, so the canonical fallback
    /// must never run for it (it would stat the LOCAL filesystem). The same
    /// logical/physical pair that matches for a local session does not match
    /// for a remote one; exact matches still do.
    #[cfg(unix)]
    #[test]
    fn find_for_session_never_canonicalizes_a_remote_cwd() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (physical, logical) = symlinked_dir(&tmp);
        let rows = vec![row("a", None, Some(&physical))];
        assert!(find_for_session(&rows, "no-name", &logical, false).is_none());
        assert!(find_for_session(&rows, "no-name", &logical, true).is_some());
        assert_eq!(
            find_for_session(&rows, "no-name", &physical, false)
                .unwrap()
                .session_id
                .as_deref(),
            Some("a")
        );
    }

    #[test]
    fn parse_empty_array() {
        assert_eq!(parse_claude_agents_json("[]"), vec![]);
    }

    #[test]
    fn parse_invalid_json_returns_empty() {
        assert_eq!(parse_claude_agents_json("not json"), vec![]);
    }

    #[test]
    fn parse_full_row() {
        let json = r#"[{"pid":1234,"cwd":"/Users/u/proj","kind":"session","startedAt":"2026-05-22T10:00:00Z","sessionId":"abc123","name":"my-sess","status":"working"}]"#;
        let rows = parse_claude_agents_json(json);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].session_id.as_deref(), Some("abc123"));
        assert_eq!(rows[0].name.as_deref(), Some("my-sess"));
        assert_eq!(rows[0].status.as_deref(), Some("working"));
        assert_eq!(rows[0].cwd.as_deref(), Some("/Users/u/proj"));
    }

    #[test]
    fn parse_row_missing_optional_fields() {
        let json =
            r#"[{"pid":999,"cwd":"/tmp","kind":"session","startedAt":"2026-05-22T10:00:00Z"}]"#;
        let rows = parse_claude_agents_json(json);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].session_id, None);
        assert_eq!(rows[0].name, None);
        assert_eq!(rows[0].status, None);
    }

    #[test]
    fn find_by_name_matches() {
        let rows = vec![
            ClaudeAgentRow {
                session_id: Some("s1".into()),
                name: Some("alpha".into()),
                status: Some("working".into()),
                cwd: None,
            },
            ClaudeAgentRow {
                session_id: Some("s2".into()),
                name: Some("beta".into()),
                status: Some("blocked".into()),
                cwd: None,
            },
        ];
        let hit = find_by_name(&rows, "beta").unwrap();
        assert_eq!(hit.session_id.as_deref(), Some("s2"));
    }

    #[test]
    fn find_by_name_no_match_returns_none() {
        let rows: Vec<ClaudeAgentRow> = vec![];
        assert!(find_by_name(&rows, "missing").is_none());
    }

    fn statuses(json: &str) -> Vec<Option<String>> {
        parse_claude_agents_json(json)
            .into_iter()
            .map(|r| r.status)
            .collect()
    }

    /// Real shape, Claude Code 2.1.267: interactive rows report
    /// `status: busy|idle` and no `state`. `busy` used to be dropped by
    /// `known_agent_status`, so every working session fell back to the pane.
    #[test]
    fn new_cli_interactive_rows_map_busy_to_working() {
        let json = r#"[
          {"pid":41002,"cwd":"/home/u/claude-fleet","kind":"interactive","startedAt":"2026-09-11T07:58:12.431Z","sessionId":"5f0c7a2e-1b9d-4c33-9a57-0d6f2b1e8c41","name":"claude-fleet-f2","status":"busy"},
          {"pid":41877,"cwd":"/home/u/other","kind":"interactive","startedAt":"2026-09-11T08:03:40.002Z","sessionId":"a1d4e9b0-7c62-4f1e-8b35-2e9c0f7d6a18","name":"other-k7","status":"idle"}
        ]"#;
        let rows = parse_claude_agents_json(json);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].status.as_deref(), Some("working"));
        assert_eq!(
            rows[0].session_id.as_deref(),
            Some("5f0c7a2e-1b9d-4c33-9a57-0d6f2b1e8c41")
        );
        assert_eq!(rows[0].cwd.as_deref(), Some("/home/u/claude-fleet"));
        assert_eq!(rows[1].status.as_deref(), Some("idle"));
    }

    /// Real shape, Claude Code 2.1.267: background rows carry `id` and
    /// `state`, some with `status: "idle"` too. `state` wins, so a blocked
    /// background agent is no longer invisible.
    #[test]
    fn new_cli_background_rows_prefer_state_over_status() {
        let json = r#"[
          {"id":"bg_01J7ZK3Q9W","cwd":"/home/u/claude-fleet","kind":"background","startedAt":"2026-09-11T06:12:00.000Z","sessionId":"0b8e2f41-9d3c-4a7e-b1f0-6c5d4e3a2b19","name":"review-pr-42","state":"blocked","status":"idle"},
          {"id":"bg_01J7ZK4R2X","cwd":"/home/u/claude-fleet","kind":"background","startedAt":"2026-09-11T06:40:00.000Z","sessionId":"7c1a9e3d-2f6b-4d80-a5e4-9b0c8d7f6e25","name":"review-pr-43","state":"blocked"}
        ]"#;
        assert_eq!(
            statuses(json),
            vec![Some("blocked".into()), Some("blocked".into())]
        );
    }

    /// The full `claude agents --json` capture from the 2026-09-11 pre-check
    /// (Claude Code 2.1.267), with every field and value shape kept (`id`
    /// vs `pid`, millisecond `startedAt`, `kind`, `state` and `status`
    /// mixes); only paths, names and ids are anonymised.
    #[test]
    fn live_capture_2_1_267_maps_every_row() {
        let json = r#"[
  { "id": "d89375a1", "cwd": "/home/u", "kind": "background", "startedAt": 1782115539756,
    "sessionId": "d89375a1-0000-4000-8000-000000000001", "name": "task-retrain", "state": "blocked" },
  { "pid": 96281, "cwd": "/home/u/projects/app/.worktrees/eeeee", "kind": "interactive",
    "startedAt": 1785595436663, "sessionId": "56571c99-0000-4000-8000-000000000002", "name": "eeeee-22", "status": "idle" },
  { "pid": 2495934, "id": "841ae322", "cwd": "/home/u", "kind": "background", "startedAt": 1789072055522,
    "sessionId": "4505909e-0000-4000-8000-000000000003", "name": "841ae322", "status": "idle", "state": "blocked" },
  { "pid": 3173883, "cwd": "/home/u/projects/claude-fleet/.worktrees/jhkljh", "kind": "interactive",
    "startedAt": 1789072590555, "sessionId": "ca94f8a4-0000-4000-8000-000000000004", "name": "jhkljh-f2", "status": "busy" }
]"#;
        let rows = parse_claude_agents_json(json);
        let got: Vec<(&str, Option<&str>)> = rows
            .iter()
            .map(|r| (r.name.as_deref().unwrap(), r.status.as_deref()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("task-retrain", Some("blocked")),
                ("eeeee-22", Some("idle")),
                ("841ae322", Some("blocked")),
                ("jhkljh-f2", Some("working")),
            ]
        );
        // Every value that leaves the parser is one `known_agent_status`
        // keeps, so nothing is dropped any more.
        for r in &rows {
            assert!(r.status.as_deref().unwrap().parse::<ClaudeStatus>().is_ok());
        }
    }

    /// Documented `state` values (agent-view docs), mapped into the fleet
    /// vocabulary: done becomes completed.
    #[test]
    fn documented_states_map_into_the_vocabulary() {
        for (state, want) in [
            ("working", "working"),
            ("blocked", "blocked"),
            ("done", "completed"),
            ("failed", "failed"),
            ("stopped", "stopped"),
        ] {
            let json = format!(r#"[{{"kind":"background","state":"{state}","status":"idle"}}]"#);
            assert_eq!(statuses(&json), vec![Some(want.to_string())], "{state}");
        }
    }

    /// `status: waiting` and any non-empty `waitingFor` mean blocked.
    #[test]
    fn waiting_status_and_waiting_for_mean_blocked() {
        let json = r#"[
          {"kind":"interactive","status":"waiting","waitingFor":"permission prompt"},
          {"kind":"interactive","status":"waiting"},
          {"kind":"interactive","status":"idle","waitingFor":"input needed"},
          {"kind":"interactive","status":"busy","waitingFor":{"type":"permission","tool":"Bash"}},
          {"kind":"interactive","status":"idle","waitingFor":null},
          {"kind":"interactive","status":"idle","waitingFor":""},
          {"kind":"interactive","status":"busy","waitingFor":{}}
        ]"#;
        assert_eq!(
            statuses(json),
            vec![
                Some("blocked".into()),
                Some("blocked".into()),
                Some("blocked".into()),
                Some("blocked".into()),
                Some("idle".into()),
                Some("idle".into()),
                Some("working".into()),
            ]
        );
    }

    /// Old CLI: `status` already in the vocabulary is kept verbatim.
    #[test]
    fn old_cli_status_values_pass_through() {
        for v in ClaudeStatus::ALL {
            let json = format!(r#"[{{"pid":1,"cwd":"/p","status":"{}"}}]"#, v.as_str());
            assert_eq!(statuses(&json), vec![Some(v.as_str().to_string())]);
        }
    }

    /// Unknown values never leak out: an unknown `state` falls back to
    /// `status`, and a row with nothing known yields `None`. Case and
    /// whitespace are tolerated.
    #[test]
    fn unknown_values_fall_through_to_none() {
        let json = r#"[
          {"state":"hibernating","status":"busy"},
          {"state":"hibernating"},
          {"status":"awaiting_input"},
          {"status":"  Busy "},
          {}
        ]"#;
        assert_eq!(
            statuses(json),
            vec![
                Some("working".into()),
                None,
                None,
                Some("working".into()),
                None
            ]
        );
    }

    /// A mistyped status field must not fail the whole array, or every agent
    /// on the host would vanish.
    #[test]
    fn a_mistyped_status_field_does_not_drop_the_other_rows() {
        let json = r#"[
          {"sessionId":"a","status":3,"state":["x"],"waitingFor":7},
          {"sessionId":"b","status":"busy"}
        ]"#;
        let rows = parse_claude_agents_json(json);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].status, None);
        assert_eq!(rows[1].status.as_deref(), Some("working"));
    }

    #[test]
    fn normalize_status_precedence() {
        use ClaudeStatus::*;
        assert_eq!(
            normalize_status(Some("done"), Some("busy"), None),
            Some(Completed)
        );
        assert_eq!(
            normalize_status(Some("working"), Some("idle"), Some("permission prompt")),
            Some(Working),
            "state is the lifecycle and wins over a waiting reason"
        );
        assert_eq!(
            normalize_status(None, Some("idle"), Some("x")),
            Some(Blocked)
        );
        assert_eq!(normalize_status(None, Some("busy"), None), Some(Working));
        assert_eq!(normalize_status(None, None, None), None);
    }
}
