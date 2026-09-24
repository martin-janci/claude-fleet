//! Journal harvest (work graph M2.1): what hooks and transcripts leave behind
//! for a later resume. The `conversation` row is written by migration 047's
//! triggers (on every close and before every session delete); `progress`
//! rows come from the Stop hook's `turn_done` detail; this module adds the
//! compaction summary Claude Code itself wrote, read off the transcript tail
//! the same way `context::refresh_context` reads it — never under the store
//! lock.

use crate::ipc_error::lock;
use crate::service::transcript::{parse_conversation, ConvItem};
use crate::ssh::SshClient;
use crate::store::{Store, COMPACT_SUMMARY_MAX_CHARS};
use std::sync::{Arc, Mutex};

/// Tail read for a compaction summary: the summary entry sits right after
/// the boundary at the end of the file, and is at most ~20k chars.
const COMPACT_READ_BYTES: usize = 262_144;

/// PURE: the summary of the LAST compaction in `jsonl` (a transcript tail),
/// capped at [`COMPACT_SUMMARY_MAX_CHARS`]. `None` when the tail has no
/// compaction, or its summary entry is not written yet.
pub fn last_compact_summary(jsonl: &str) -> Option<String> {
    let turns = parse_conversation(jsonl);
    let summary = turns
        .iter()
        .flat_map(|t| t.items.iter())
        .filter_map(|i| match i {
            ConvItem::Compact { summary, .. } => Some(summary.as_deref()),
            _ => None,
        })
        .next_back()??;
    let summary = summary.trim();
    if summary.is_empty() {
        return None;
    }
    Some(summary.chars().take(COMPACT_SUMMARY_MAX_CHARS).collect())
}

/// Read `claude_session_id`'s transcript tail and journal its last
/// compaction summary for session `row_id`. Best-effort: every failure is
/// logged at debug. Retries once after 500 ms when the summary entry is not
/// flushed yet (PostCompact can land before it).
pub async fn harvest_compact_summary(
    store: Arc<Mutex<Store>>,
    ssh: Arc<SshClient>,
    row_id: i64,
    claude_session_id: String,
) {
    for attempt in 0..2 {
        if attempt == 1 {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        let row = match lock(&store).and_then(|s| Ok(s.get_session_by_id(row_id)?)) {
            Ok(Some(r)) => r,
            _ => return,
        };
        let Ok(args) =
            crate::service::transcript::resolve_args_for(&store, &row, &claude_session_id, 1, 1)
        else {
            return;
        };
        let text = match crate::service::transcript::read_tail_bytes(
            &args,
            COMPACT_READ_BYTES,
            &ssh,
        )
        .await
        {
            Ok(t) => t,
            Err(e) => {
                tracing::debug!(row_id, error = %e.message, "[journal] tail read failed");
                return;
            }
        };
        if let Some(summary) = last_compact_summary(&text) {
            if let Ok(s) = lock(&store) {
                if let Err(e) = s.journal_for_session(
                    row_id,
                    &claude_session_id,
                    "compact_summary",
                    "transcript",
                    &summary,
                ) {
                    tracing::debug!(row_id, error = %e.message, "[journal] summary not stored");
                }
            }
            return;
        }
    }
}

/// Spawn [`harvest_compact_summary`] off the hook's response path. Skipped
/// when no runtime is reachable (sync tests).
pub fn spawn_harvest_compact_summary(
    store: &Arc<Mutex<Store>>,
    ssh: &Arc<SshClient>,
    row_id: i64,
    claude_session_id: &str,
) {
    let store = Arc::clone(store);
    let ssh = Arc::clone(ssh);
    let id = claude_session_id.to_string();
    let _ = crate::rt::try_spawn(async move {
        harvest_compact_summary(store, ssh, row_id, id).await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(v: serde_json::Value) -> String {
        v.to_string()
    }

    fn compaction(summary: &str, at: &str) -> Vec<String> {
        vec![
            line(
                serde_json::json!({"type":"system","subtype":"compact_boundary",
                "timestamp": at, "compactMetadata":{"trigger":"manual","preTokens":1000}}),
            ),
            line(
                serde_json::json!({"type":"user","isCompactSummary":true,"timestamp": at,
                "message":{"role":"user","content": summary}}),
            ),
        ]
    }

    #[test]
    fn the_last_compaction_summary_wins() {
        let mut jsonl = vec![line(
            serde_json::json!({"type":"user","timestamp":"2026-09-18T10:00:00Z",
            "message":{"role":"user","content":"fix login"}}),
        )];
        jsonl.extend(compaction("first summary", "2026-09-18T10:01:00Z"));
        jsonl.push(line(
            serde_json::json!({"type":"user","timestamp":"2026-09-18T10:02:00Z",
            "message":{"role":"user","content":"go on"}}),
        ));
        jsonl.extend(compaction(
            "  Branch abc-1 has the fix; tests pending.  ",
            "2026-09-18T10:03:00Z",
        ));
        assert_eq!(
            last_compact_summary(&jsonl.join("\n")).as_deref(),
            Some("Branch abc-1 has the fix; tests pending.")
        );
    }

    #[test]
    fn no_compaction_or_an_unflushed_summary_yields_none() {
        let plain = line(
            serde_json::json!({"type":"user","timestamp":"2026-09-18T10:00:00Z",
            "message":{"role":"user","content":"hi"}}),
        );
        assert_eq!(last_compact_summary(&plain), None);
        let boundary = compaction("x", "2026-09-18T10:01:00Z").remove(0);
        assert_eq!(last_compact_summary(&boundary), None);
    }

    #[test]
    fn a_long_summary_is_capped() {
        let long = "y".repeat(COMPACT_SUMMARY_MAX_CHARS + 100);
        let jsonl = compaction(&long, "2026-09-18T10:01:00Z").join("\n");
        assert_eq!(
            last_compact_summary(&jsonl).unwrap().chars().count(),
            COMPACT_SUMMARY_MAX_CHARS
        );
    }
}
