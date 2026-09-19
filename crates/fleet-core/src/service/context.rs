//! Context size of a conversation from its transcript (spec §1.5): the last
//! main-thread assistant entry's `usage` is the prompt size of the latest
//! request. The pane footer is a fallback only.

use crate::ipc_error::lock;
use crate::ssh::SshClient;
use crate::store::Store;
use std::sync::{Arc, Mutex};

/// Tail read for a context refresh: enough for several assistant entries.
const CONTEXT_READ_BYTES: usize = 262_144;
pub const WINDOW_DEFAULT: i64 = 200_000;
pub const WINDOW_1M: i64 = 1_000_000;

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ContextUsage {
    pub tokens: i64,
    pub window: i64,
    pub model: Option<String>,
}

/// Window of `model`: `[1m]` models get 1M. A token count above the default
/// window proves a 1M window whatever the model id says.
pub fn context_window_for(model: Option<&str>, tokens: i64) -> i64 {
    if model.is_some_and(|m| m.contains("[1m]")) || tokens > WINDOW_DEFAULT {
        WINDOW_1M
    } else {
        WINDOW_DEFAULT
    }
}

/// The context usage after the transcript's last entry, or `None` when no
/// usage is known — none in the tail, or a compaction after the last one
/// (the size is unknown until the next reply).
pub fn context_from_jsonl(jsonl: &str) -> Option<ContextUsage> {
    let mut last: Option<ContextUsage> = None;
    for line in jsonl.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("isSidechain").and_then(|b| b.as_bool()) == Some(true) {
            continue;
        }
        match v.get("type").and_then(|t| t.as_str()) {
            Some("system")
                if v.get("subtype").and_then(|s| s.as_str()) == Some("compact_boundary") =>
            {
                last = None;
            }
            Some("assistant") => {
                let msg = v.get("message");
                let model = msg.and_then(|m| m.get("model")).and_then(|m| m.as_str());
                if model == Some("<synthetic>") {
                    continue;
                }
                let Some(u) = msg.and_then(|m| m.get("usage")) else {
                    continue;
                };
                let n = |k: &str| u.get(k).and_then(|x| x.as_i64()).unwrap_or(0);
                let tokens = n("input_tokens")
                    + n("cache_read_input_tokens")
                    + n("cache_creation_input_tokens");
                last = Some(ContextUsage {
                    tokens,
                    window: context_window_for(model, tokens),
                    model: model.map(String::from),
                });
            }
            _ => {}
        }
    }
    last
}

/// Re-read the current conversation's transcript tail and store its context
/// size (source `transcript`). Best-effort: every failure is logged at debug
/// and leaves the stored value. The store lock is never held across the read.
/// Retries once after 500 ms when the tail has no usage yet (the Stop hook
/// can land before the line is flushed).
pub async fn refresh_context(store: Arc<Mutex<Store>>, ssh: Arc<SshClient>, session_id: i64) {
    for attempt in 0..2 {
        if attempt == 1 {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        let row = match lock(&store).and_then(|s| Ok(s.get_session_by_id(session_id)?)) {
            Ok(Some(r)) => r,
            _ => return,
        };
        let Ok(args) = crate::service::transcript::resolve_args(&store, &row, 1, 1) else {
            return;
        };
        let claude_id = args.claude_session_id.clone();
        let text = match crate::service::transcript::read_tail_bytes(
            &args,
            CONTEXT_READ_BYTES,
            &ssh,
        )
        .await
        {
            Ok(t) => t,
            Err(e) => {
                tracing::debug!(session_id, error = %e.message, "[context] tail read failed");
                return;
            }
        };
        if let Some(u) = context_from_jsonl(&text) {
            if let Ok(s) = lock(&store) {
                let _ = s.set_context(
                    session_id,
                    &claude_id,
                    u.tokens,
                    u.window,
                    "transcript",
                    u.model.as_deref(),
                );
            }
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asst(model: &str, input: i64, read: i64, write: i64, output: i64) -> String {
        serde_json::json!({
            "type": "assistant",
            "message": { "model": model, "usage": {
                "input_tokens": input, "cache_read_input_tokens": read,
                "cache_creation_input_tokens": write, "output_tokens": output } }
        })
        .to_string()
    }

    #[test]
    fn last_assistant_usage_wins_and_output_is_excluded() {
        let jsonl = [
            asst("claude-opus-5", 10, 1_000, 0, 50),
            r#"{"type":"user","message":{"content":"hi"}}"#.to_string(),
            asst("claude-opus-5", 5, 40_000, 2_000, 900),
        ]
        .join("\n");
        let u = context_from_jsonl(&jsonl).unwrap();
        assert_eq!(u.tokens, 42_005);
        assert_eq!(u.window, 200_000);
        assert_eq!(u.model.as_deref(), Some("claude-opus-5"));
    }

    #[test]
    fn sidechain_and_synthetic_entries_are_ignored() {
        let side = serde_json::json!({"type":"assistant","isSidechain":true,
            "message":{"model":"m","usage":{"input_tokens":999_999}}})
        .to_string();
        let synth = serde_json::json!({"type":"assistant",
            "message":{"model":"<synthetic>","usage":{"input_tokens":0}}})
        .to_string();
        let jsonl = [asst("claude-sonnet-5", 100, 0, 0, 0), side, synth].join("\n");
        assert_eq!(context_from_jsonl(&jsonl).unwrap().tokens, 100);
    }

    #[test]
    fn compact_boundary_after_last_usage_yields_none() {
        let boundary = r#"{"type":"system","subtype":"compact_boundary"}"#;
        let jsonl = [asst("m", 150_000, 0, 0, 0), boundary.to_string()].join("\n");
        assert_eq!(context_from_jsonl(&jsonl), None);
    }

    #[test]
    fn window_detects_one_million_models() {
        assert_eq!(context_window_for(Some("claude-opus-5[1m]"), 10), 1_000_000);
        assert_eq!(context_window_for(Some("claude-sonnet-5"), 10), 200_000);
        assert_eq!(context_window_for(None, 250_000), 1_000_000);
        assert_eq!(context_window_for(None, 10), 200_000);
    }

    #[test]
    fn a_partial_leading_line_is_tolerated() {
        let jsonl = format!("ut_tokens\":1}}}}\n{}", asst("m", 7, 0, 0, 0));
        assert_eq!(context_from_jsonl(&jsonl).unwrap().tokens, 7);
    }
}
