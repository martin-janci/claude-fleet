//! Read a session's Claude Code JSONL transcript and render the last
//! assistant turn(s) as plain text (MCP-4).
//!
//! Claude Code writes one transcript per session at
//! `~/.claude/projects/<encoded cwd>/<claude_session_id>.jsonl`, where the
//! cwd is encoded by replacing every character outside `[A-Za-z0-9]` with
//! `-` (verified against a live install: `/mnt/sda4/projects/github.com/x/y/
//! .worktrees/z` → `-mnt-sda4-projects-github-com-x-y--worktrees-z`).
//!
//! The file is read over ssh (or a local `bash`) with every interpolated
//! value shell-quoted, bounded by the ssh wall clock and a byte cap; the
//! parse then keeps only `assistant` entries, splitting turns on human
//! `user` prompts. Tool calls become one summary line each so a caller gets
//! the reply text without the tool-result noise (and without the pane's
//! TUI chrome that `capture_session` returns).

use crate::ipc_error::IpcError;
use crate::shell::quote;
use crate::ssh::SshClient;
use std::sync::Arc;

/// Default / hard cap on the characters returned by `session_transcript`.
pub const DEFAULT_MAX_CHARS: usize = 8_000;
pub const MAX_MAX_CHARS: usize = 64_000;

/// Bytes of the JSONL tail read for a given character budget. Assistant
/// entries carry thinking signatures and usage blobs, so a turn's text is a
/// small fraction of its on-disk size.
const MIN_READ_BYTES: usize = 256 * 1024;
const MAX_READ_BYTES: usize = 4 * 1024 * 1024;

/// Wall-clock bound for the remote read. Reading a few MB over an
/// established ControlMaster is sub-second; this only bounds a wedged host.
const READ_WALL_CLOCK: std::time::Duration = std::time::Duration::from_secs(20);

/// Cap on a single tool_use summary line.
const TOOL_SUMMARY_CHARS: usize = 160;

/// Encode a working directory the way Claude Code names its per-project
/// transcript directory: every char outside `[A-Za-z0-9]` becomes `-`.
pub fn encode_project_dir(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Bytes to read for `max_chars` of rendered text.
pub fn read_bytes_for(max_chars: usize) -> usize {
    (max_chars.saturating_mul(64)).clamp(MIN_READ_BYTES, MAX_READ_BYTES)
}

/// Sentinels the read script prints (on stderr) so the caller can map a
/// missing transcript / unknown cwd to a code instead of parsing prose.
const NO_CWD: &str = "__CF_NO_CWD__";
const NO_TRANSCRIPT: &str = "__CF_NO_TRANSCRIPT__";

/// The bash script that prints the last `max_bytes` of the transcript.
///
/// The cwd comes from the tmux pane when `tmux_name` is given (the session
/// may have `cd`-ed since it was created) and is encoded on the host with a
/// `sed` that mirrors [`encode_project_dir`]; otherwise the `cwd` fallback,
/// encoded here, is used. Every interpolated value is shell-quoted; the
/// session id is validated by the caller (`validate::claude_session_id`).
pub fn read_script(
    tmux_name: Option<&str>,
    cwd: Option<&str>,
    claude_session_id: &str,
    max_bytes: usize,
) -> String {
    let tmux_q = quote(tmux_name.unwrap_or(""));
    let fallback_enc_q = quote(&cwd.map(encode_project_dir).unwrap_or_default());
    let id_q = quote(claude_session_id);
    format!(
        r#"set +e
cwd=''
if [ -n {tmux_q} ]; then
  cwd=$(tmux display-message -p -t {tmux_q} '#{{pane_current_path}}' 2>/dev/null)
fi
if [ -n "$cwd" ]; then enc=$(printf '%s' "$cwd" | sed 's/[^A-Za-z0-9]/-/g'); else enc={fallback_enc_q}; fi
if [ -z "$enc" ]; then echo {NO_CWD} >&2; exit 3; fi
f="$HOME/.claude/projects/$enc/"{id_q}.jsonl
if [ ! -f "$f" ]; then printf '{NO_TRANSCRIPT} %s\n' "$f" >&2; exit 4; fi
tail -c {max_bytes} "$f"
"#
    )
}

/// One-line summary of a `tool_use` block: `[tool_use] Name(input…)`.
fn summarize_tool_use(block: &serde_json::Value) -> String {
    let name = block.get("name").and_then(|v| v.as_str()).unwrap_or("tool");
    let input = block
        .get("input")
        .map(|v| match v {
            serde_json::Value::Object(map) => {
                // Prefer the fields that identify what the tool touched.
                let mut parts: Vec<String> = Vec::new();
                for key in [
                    "command",
                    "file_path",
                    "path",
                    "pattern",
                    "query",
                    "description",
                    "skill",
                    "prompt",
                ] {
                    if let Some(s) = map.get(key).and_then(|x| x.as_str()) {
                        parts.push(format!("{key}={}", one_line(s)));
                        break;
                    }
                }
                if parts.is_empty() {
                    one_line(&v.to_string())
                } else {
                    parts.join(" ")
                }
            }
            other => one_line(&other.to_string()),
        })
        .unwrap_or_default();
    let mut s = format!("[tool_use] {name}({input})");
    if s.chars().count() > TOOL_SUMMARY_CHARS {
        s = s.chars().take(TOOL_SUMMARY_CHARS).collect::<String>() + "…)";
    }
    s
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Split a transcript (JSONL text; a leading partial line is tolerated)
/// into assistant turns. A turn starts at every human `user` prompt (a
/// string body, or a content array without `tool_result` blocks); every
/// `assistant` entry until the next prompt contributes its text blocks and
/// one summary line per `tool_use`. Turns with no assistant content are
/// dropped. Sidechain (subagent) entries are ignored.
pub fn parse_turns(jsonl: &str) -> Vec<String> {
    let mut turns: Vec<String> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let flush = |current: &mut Vec<String>, turns: &mut Vec<String>| {
        let text = current.join("\n").trim().to_string();
        if !text.is_empty() {
            turns.push(text);
        }
        current.clear();
    };
    for line in jsonl.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("isSidechain").and_then(|b| b.as_bool()) == Some(true) {
            continue;
        }
        let kind = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let content = v.get("message").and_then(|m| m.get("content"));
        match kind {
            "user" => {
                let is_prompt = match content {
                    Some(serde_json::Value::String(_)) => true,
                    Some(serde_json::Value::Array(blocks)) => !blocks
                        .iter()
                        .any(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_result")),
                    _ => false,
                };
                if is_prompt {
                    flush(&mut current, &mut turns);
                }
            }
            "assistant" => {
                if let Some(serde_json::Value::Array(blocks)) = content {
                    for b in blocks {
                        match b.get("type").and_then(|t| t.as_str()) {
                            Some("text") => {
                                if let Some(t) = b.get("text").and_then(|t| t.as_str()) {
                                    if !t.trim().is_empty() {
                                        current.push(t.trim_end().to_string());
                                    }
                                }
                            }
                            Some("tool_use") => current.push(summarize_tool_use(b)),
                            _ => {}
                        }
                    }
                }
            }
            _ => {}
        }
    }
    flush(&mut current, &mut turns);
    turns
}

/// The last `count` turns joined with a separator, trimmed from the FRONT
/// to `max_chars` (the end of the reply is what a caller waited for).
pub fn render_tail(turns: &[String], count: usize, max_chars: usize) -> String {
    let count = count.max(1);
    let start = turns.len().saturating_sub(count);
    let joined = turns[start..].join("\n\n---\n\n");
    let total = joined.chars().count();
    if total <= max_chars {
        return joined;
    }
    let dropped = total - max_chars;
    let tail: String = joined.chars().skip(dropped).collect();
    format!("[session_transcript: {dropped} chars dropped from the start — raise max_chars to see more]\n{tail}")
}

/// What to read and how much to render.
pub struct TranscriptArgs {
    pub host_alias: String,
    /// tmux session whose pane cwd locates the transcript (interactive
    /// sessions). `None` for background sessions — then `cwd` is required.
    pub tmux_name: Option<String>,
    /// Fallback cwd when the pane lookup fails / there is no pane.
    pub cwd: Option<String>,
    pub claude_session_id: String,
    /// Number of most-recent turns to return (≥ 1).
    pub turns: usize,
    pub max_chars: usize,
}

/// Fetch and render a transcript. Errors: `E_INVALID` (bad id),
/// `E_INVALID_STATE` (no cwd resolvable), `E_NO_TRANSCRIPT` (file absent —
/// the session has not written a turn yet, or runs on another cwd),
/// `E_SHELL` / `E_SSH*` for transport failures.
pub async fn fetch_transcript(
    args: TranscriptArgs,
    ssh: &Arc<SshClient>,
) -> Result<String, IpcError> {
    crate::validate::host_alias(&args.host_alias)?;
    crate::validate::claude_session_id(&args.claude_session_id)?;
    if let Some(name) = args.tmux_name.as_deref() {
        crate::validate::tmux_name_addressable(name)?;
    }
    if args.tmux_name.is_none() && args.cwd.as_deref().is_none_or(|c| c.trim().is_empty()) {
        return Err(IpcError::new(
            "E_INVALID_STATE",
            "no working directory known for this session — cannot locate its transcript",
        ));
    }
    let max_chars = args.max_chars.clamp(1, MAX_MAX_CHARS);
    let script = read_script(
        args.tmux_name.as_deref(),
        args.cwd.as_deref(),
        &args.claude_session_id,
        read_bytes_for(max_chars),
    );
    let out = run_shell(ssh, &args.host_alias, &script).await?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        if stderr.contains(NO_CWD) {
            return Err(IpcError::new(
                "E_INVALID_STATE",
                "could not resolve the session's working directory (pane gone?)",
            ));
        }
        if stderr.contains(NO_TRANSCRIPT) {
            return Err(IpcError::new(
                "E_NO_TRANSCRIPT",
                format!(
                    "no transcript for claude session {} on {}: {}",
                    args.claude_session_id,
                    args.host_alias,
                    stderr.trim()
                ),
            ));
        }
        return Err(IpcError::new(
            "E_SHELL",
            format!("transcript read failed: {}", stderr.trim()),
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let turns = parse_turns(&text);
    Ok(render_tail(&turns, args.turns, max_chars))
}

/// Run a bash script on `host_alias` (local or via ssh), bounded by
/// [`READ_WALL_CLOCK`].
async fn run_shell(
    ssh: &Arc<SshClient>,
    host_alias: &str,
    script: &str,
) -> Result<std::process::Output, IpcError> {
    if host_alias == "local" {
        let child = tokio::process::Command::new("bash")
            .args(["-lc", script])
            .output();
        tokio::time::timeout(READ_WALL_CLOCK, child)
            .await
            .map_err(|_| IpcError::new("E_SHELL", "transcript read timed out"))?
            .map_err(|e| IpcError::new("E_SHELL", format!("spawn bash: {e}")))
    } else {
        ssh.run_bounded(
            host_alias,
            &["bash", "-lc", &quote(script)],
            std::time::Duration::from_secs(10),
            READ_WALL_CLOCK,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_dir_encoding_matches_claude_code() {
        // Observed on a live install (see module docs).
        assert_eq!(
            encode_project_dir(
                "/mnt/sda4/projects/github.com/martin-janci/claude-fleet/.worktrees/jhkljh"
            ),
            "-mnt-sda4-projects-github-com-martin-janci-claude-fleet--worktrees-jhkljh"
        );
        assert_eq!(encode_project_dir("/home/u"), "-home-u");
        assert_eq!(encode_project_dir("/home/u/a_b c"), "-home-u-a-b-c");
    }

    #[test]
    fn read_bytes_scales_with_the_char_budget_within_bounds() {
        assert_eq!(read_bytes_for(1), MIN_READ_BYTES);
        assert_eq!(read_bytes_for(8_000), 512 * 1024);
        assert_eq!(read_bytes_for(64_000), MAX_READ_BYTES);
        assert_eq!(read_bytes_for(usize::MAX), MAX_READ_BYTES);
    }

    #[test]
    fn read_script_quotes_every_interpolated_value() {
        let s = read_script(Some("dev-x'; rm -rf /"), Some("/home/u/it's"), "abc", 1024);
        assert!(s.contains("tmux display-message -p -t 'dev-x'\\''; rm -rf /'"));
        assert!(s.contains("enc='-home-u-it-s'"), "{s}");
        assert!(s.contains("/\"'abc'.jsonl"));
        assert!(s.contains("tail -c 1024"));
        assert!(
            s.contains("sed 's/[^A-Za-z0-9]/-/g'"),
            "remote encoding mirrors encode_project_dir"
        );
        // No tmux lookup when the pane name is absent: the `-n ''` test fails.
        let bg = read_script(None, Some("/x"), "abc", 1);
        assert!(bg.contains("if [ -n '' ]; then"));
    }

    fn line(v: serde_json::Value) -> String {
        v.to_string()
    }

    #[test]
    fn parse_turns_splits_on_human_prompts_and_summarises_tool_use() {
        let jsonl = [
            line(serde_json::json!({"type":"user","message":{"role":"user","content":"first"}})),
            line(serde_json::json!({"type":"assistant","message":{"content":[
                {"type":"thinking","thinking":"secret"},
                {"type":"text","text":"Let me look."},
                {"type":"tool_use","name":"Bash","input":{"command":"ls -la","description":"list"}}
            ]}})),
            line(serde_json::json!({"type":"user","message":{"content":[
                {"type":"tool_result","tool_use_id":"x","content":"a\nb"}]}})),
            line(serde_json::json!({"type":"assistant","message":{"content":[
                {"type":"text","text":"Two files."}]}})),
            line(serde_json::json!({"type":"user","message":{"role":"user","content":"second"}})),
            line(
                serde_json::json!({"type":"assistant","isSidechain":true,"message":{"content":[
                {"type":"text","text":"subagent noise"}]}}),
            ),
            line(serde_json::json!({"type":"assistant","message":{"content":[
                {"type":"text","text":"Done: all good."}]}})),
        ]
        .join("\n");
        let turns = parse_turns(&jsonl);
        assert_eq!(turns.len(), 2);
        assert_eq!(
            turns[0],
            "Let me look.\n[tool_use] Bash(command=ls -la)\nTwo files."
        );
        assert_eq!(turns[1], "Done: all good.");
        assert!(!jsonl.is_empty());
        // Thinking blocks and sidechain entries never leak.
        assert!(!turns
            .iter()
            .any(|t| t.contains("secret") || t.contains("subagent")));
    }

    #[test]
    fn parse_turns_tolerates_a_truncated_leading_line_and_garbage() {
        let jsonl = format!(
            "\"content\":[{{\"type\":\"text\",\"text\":\"cut\"}}]}}}}\nnot json\n{}\n{}",
            line(serde_json::json!({"type":"user","message":{"content":"q"}})),
            line(
                serde_json::json!({"type":"assistant","message":{"content":[{"type":"text","text":"a"}]}})
            ),
        );
        assert_eq!(parse_turns(&jsonl), vec!["a".to_string()]);
        assert!(parse_turns("").is_empty());
    }

    #[test]
    fn render_tail_keeps_the_last_turns_and_trims_from_the_front() {
        let turns = vec!["one".to_string(), "two".to_string(), "three".to_string()];
        assert_eq!(render_tail(&turns, 1, 100), "three");
        assert_eq!(render_tail(&turns, 2, 100), "two\n\n---\n\nthree");
        assert_eq!(
            render_tail(&turns, 0, 100),
            "three",
            "count is floored at 1"
        );
        assert_eq!(
            render_tail(&turns, 10, 100),
            "one\n\n---\n\ntwo\n\n---\n\nthree"
        );
        let cut = render_tail(&turns, 1, 3);
        assert!(cut.starts_with("[session_transcript: 2 chars dropped"));
        assert!(cut.ends_with("\nree"));
        assert_eq!(render_tail(&[], 1, 10), "");
    }

    #[test]
    fn tool_use_summary_is_one_line_and_capped() {
        let b = serde_json::json!({"type":"tool_use","name":"Edit","input":{"file_path":"/a/b.rs","old_string":"x\ny"}});
        assert_eq!(summarize_tool_use(&b), "[tool_use] Edit(file_path=/a/b.rs)");
        let long = serde_json::json!({"type":"tool_use","name":"Bash","input":{"command":"x".repeat(500)}});
        let s = summarize_tool_use(&long);
        assert!(s.chars().count() <= TOOL_SUMMARY_CHARS + 2);
        assert!(!s.contains('\n'));
        let none = serde_json::json!({"type":"tool_use","name":"Skill","input":{"other":1}});
        assert_eq!(summarize_tool_use(&none), "[tool_use] Skill({\"other\":1})");
    }

    #[tokio::test]
    async fn fetch_requires_a_cwd_source_and_a_valid_id() {
        let ssh = Arc::new(SshClient::new());
        let err = fetch_transcript(
            TranscriptArgs {
                host_alias: "local".into(),
                tmux_name: None,
                cwd: None,
                claude_session_id: "550e8400-e29b-41d4-a716-446655440000".into(),
                turns: 1,
                max_chars: 100,
            },
            &ssh,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "E_INVALID_STATE");
        let err = fetch_transcript(
            TranscriptArgs {
                host_alias: "local".into(),
                tmux_name: None,
                cwd: Some("/tmp".into()),
                claude_session_id: "../../etc".into(),
                turns: 1,
                max_chars: 100,
            },
            &ssh,
        )
        .await
        .unwrap_err();
        assert!(err.code.starts_with("E_"), "{}", err.code);
        assert_ne!(err.code, "E_NO_TRANSCRIPT");
    }

    #[tokio::test]
    async fn fetch_maps_a_missing_local_transcript_to_e_no_transcript() {
        let ssh = Arc::new(SshClient::new());
        let dir = tempfile::tempdir().unwrap();
        let err = fetch_transcript(
            TranscriptArgs {
                host_alias: "local".into(),
                tmux_name: None,
                cwd: Some(dir.path().to_string_lossy().into_owned()),
                claude_session_id: "550e8400-e29b-41d4-a716-446655440000".into(),
                turns: 1,
                max_chars: 100,
            },
            &ssh,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "E_NO_TRANSCRIPT");
    }
}
