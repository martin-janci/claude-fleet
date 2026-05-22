//! Parse the JSON emitted by `claude agents --json`.

use serde::Deserialize;

/// One row from `claude agents --json`.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ClaudeAgentRow {
    /// The Claude-internal session ID (used to call `claude logs <id>`).
    #[serde(rename = "sessionId", default)]
    pub session_id: Option<String>,
    /// Display name, matches tmux session name when created with `--name`.
    #[serde(default)]
    pub name: Option<String>,
    /// "working" | "blocked" | "completed" | "failed" | "stopped" | "idle"
    #[serde(default)]
    pub status: Option<String>,
    /// Working directory of the Claude session.
    #[serde(default)]
    pub cwd: Option<String>,
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
/// guess rather than resume the wrong conversation).
pub fn find_for_session<'a>(
    rows: &'a [ClaudeAgentRow],
    tmux_name: &str,
    cwd: &str,
) -> Option<&'a ClaudeAgentRow> {
    if let Some(by_name) = find_by_name(rows, tmux_name) {
        return Some(by_name);
    }
    let mut in_cwd = rows.iter().filter(|r| r.cwd.as_deref() == Some(cwd));
    match (in_cwd.next(), in_cwd.next()) {
        (Some(only), None) => Some(only),
        _ => None,
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
            find_for_session(&rows, "dev-x", "/zzz").unwrap().session_id.as_deref(),
            Some("byname")
        );
        // No name match → unique cwd match.
        assert_eq!(
            find_for_session(&rows, "no-name", "/b").unwrap().session_id.as_deref(),
            Some("bycwd")
        );
        // Ambiguous cwd (two agents) → None (refuse to guess).
        assert!(find_for_session(&rows, "no-name", "/c").is_none());
        // No match at all → None.
        assert!(find_for_session(&rows, "no-name", "/nope").is_none());
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
}
