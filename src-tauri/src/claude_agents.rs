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

#[cfg(test)]
mod tests {
    use super::*;

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
        let json = r#"[{"pid":999,"cwd":"/tmp","kind":"session","startedAt":"2026-05-22T10:00:00Z"}]"#;
        let rows = parse_claude_agents_json(json);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].session_id, None);
        assert_eq!(rows[0].name, None);
        assert_eq!(rows[0].status, None);
    }

    #[test]
    fn find_by_name_matches() {
        let rows = vec![
            ClaudeAgentRow { session_id: Some("s1".into()), name: Some("alpha".into()), status: Some("working".into()), cwd: None },
            ClaudeAgentRow { session_id: Some("s2".into()), name: Some("beta".into()), status: Some("blocked".into()), cwd: None },
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
