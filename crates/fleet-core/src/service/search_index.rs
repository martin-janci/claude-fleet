//! The transcript pass (search phase 3): copies what is said in each
//! session into the search index, so `search` finds it in any conversation.
//!
//! **Off by default** (`search.index_transcripts`): it is the one place the
//! hub stores conversation text, which may hold secrets. Turning it off
//! deletes every chunk and cursor on the next tick.
//!
//! Like the usage pass (`service::usage`), one batched script per host
//! over its live sessions, reading only the bytes past each session's
//! cursor (`search_transcript_cursors`): at most [`FILE_CAP_BYTES`] per
//! file and [`HOST_BUDGET_BYTES`] per host, so a backlog converges over
//! several passes. Each chunk comes back base64-encoded, so no byte of it
//! can break the framing. The hub keeps only the text of user prompts and
//! assistant replies — never tool calls, tool output or thinking — from
//! complete lines (a half-written last line waits for the next pass),
//! skips lines older than `search.transcript_days`, and prunes chunks past
//! it.

use crate::ipc_error::{codes, lock, IpcError};
use crate::service::settings;
use crate::shell::quote;
use crate::ssh::{SshClient, SshExec};
use crate::store::{Store, TranscriptChunk};
use base64::Engine as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Bytes of one transcript one pass reads.
pub const FILE_CAP_BYTES: i64 = 2 * 1024 * 1024;
/// Bytes of one host's transcripts one pass reads.
pub const HOST_BUDGET_BYTES: i64 = 8 * 1024 * 1024;
/// How often the pass runs while it is on.
pub const INTERVAL: Duration = Duration::from_secs(300);
/// Text per indexed chunk: a hit's snippet comes from one chunk.
const PIECE_CHARS: usize = 8 * 1024;
/// One message's text kept, at most.
const MESSAGE_CHARS: usize = 4 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const LOCAL_WALL_CLOCK: Duration = Duration::from_secs(30);

/// One session the pass reads.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub session_id: i64,
    pub transcript_path: Option<String>,
    pub claude_session_id: String,
    /// The file the cursor is in, and the bytes of it indexed.
    pub source: Option<String>,
    pub offset: i64,
}

/// The script for one host. Prints, per session:
/// `F\t<sid>\t<file name>\t<offset>\t<size>` then the chunk as one base64
/// line (`B\t<sid>\t<data>`), or `M\t<sid>` when the transcript is missing.
pub fn batch_script(targets: &[Target], cap: i64, budget: i64) -> String {
    let mut s = String::with_capacity(1024 + targets.len() * 256);
    s.push_str("set +e\nexport LC_ALL=C\n");
    s.push_str(&format!(
        "cap={}\nbudget={}\n",
        quote(&cap.max(1).to_string()),
        quote(&budget.max(1).to_string())
    ));
    s.push_str(
        r#"one() {
  sid=$1; f=$2; cid=$3; off=$4; src=$5
  if [ "$budget" -le 0 ]; then return 0; fi
  if [ -z "$f" ] || [ ! -f "$f" ]; then
    f=''
    for c in "$HOME"/.claude/projects/*/"$cid".jsonl; do
      if [ -f "$c" ]; then f=$c; break; fi
    done
  fi
  if [ -z "$f" ]; then printf 'M\t%s\n' "$sid"; return 0; fi
  size=$(wc -c < "$f" 2>/dev/null | tr -d ' ')
  case "$size" in ''|*[!0-9]*) printf 'M\t%s\n' "$sid"; return 0;; esac
  base=${f##*/}
  if [ "$base" != "$src" ] || [ "$size" -lt "$off" ]; then off=0; fi
  n=$((size - off))
  if [ "$n" -gt "$cap" ]; then n=$cap; fi
  if [ "$n" -gt "$budget" ]; then n=$budget; fi
  printf 'F\t%s\t%s\t%s\t%s\n' "$sid" "$base" "$off" "$size"
  if [ "$n" -le 0 ]; then return 0; fi
  budget=$((budget - n))
  printf 'B\t%s\t' "$sid"
  tail -c +$((off + 1)) "$f" 2>/dev/null | head -c "$n" | base64 | tr -d '\n'
  printf '\n'
}
"#,
    );
    for t in targets {
        s.push_str(&format!(
            "one {} {} {} {} {}\n",
            quote(&t.session_id.to_string()),
            quote(t.transcript_path.as_deref().unwrap_or("")),
            quote(&t.claude_session_id),
            quote(&t.offset.max(0).to_string()),
            quote(t.source.as_deref().unwrap_or("")),
        ));
    }
    s
}

/// One session's read, from the script's output.
#[derive(Debug, Clone, PartialEq)]
pub struct FileChunk {
    pub source: String,
    /// The offset the chunk starts at (0 when the file is new or shrank).
    pub offset: i64,
    pub size: i64,
    pub bytes: Vec<u8>,
}

/// session id → its read. A session the script found no transcript for,
/// or whose lines did not parse, is absent.
pub fn parse_output(stdout: &str) -> std::collections::HashMap<i64, FileChunk> {
    let mut out = std::collections::HashMap::new();
    for line in stdout.lines() {
        let mut f = line.splitn(5, '\t');
        match f.next() {
            Some("F") => {
                let (Some(sid), Some(src), Some(off), Some(size)) =
                    (f.next(), f.next(), f.next(), f.next())
                else {
                    continue;
                };
                let (Ok(sid), Ok(off), Ok(size)) =
                    (sid.parse::<i64>(), off.parse::<i64>(), size.parse::<i64>())
                else {
                    continue;
                };
                out.insert(
                    sid,
                    FileChunk {
                        source: src.to_string(),
                        offset: off,
                        size,
                        bytes: Vec::new(),
                    },
                );
            }
            Some("B") => {
                let (Some(sid), Some(data)) = (f.next(), f.next()) else {
                    continue;
                };
                let Ok(sid) = sid.parse::<i64>() else {
                    continue;
                };
                if let (Some(c), Ok(bytes)) = (
                    out.get_mut(&sid),
                    base64::engine::general_purpose::STANDARD.decode(data.trim()),
                ) {
                    c.bytes = bytes;
                }
            }
            _ => {}
        }
    }
    out
}

/// `2026-10-10T12:34:56.789Z` → unix seconds.
fn parse_timestamp(ts: &str) -> Option<i64> {
    let (date, time) = ts.split_once('T')?;
    let day = crate::service::usage::day_number(date)?;
    let mut hms = time.trim_end_matches('Z').split(':');
    let h: i64 = hms.next()?.parse().ok()?;
    let m: i64 = hms.next()?.parse().ok()?;
    let s: i64 = hms.next()?.split('.').next()?.parse().ok()?;
    Some(day * 86_400 + h * 3_600 + m * 60 + s)
}

/// The words of one transcript line a person would search for: a user's
/// prompt or an assistant's reply (text blocks only), with its time.
pub fn line_text(line: &str) -> Option<(String, Option<i64>)> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let kind = v.get("type")?.as_str()?;
    if kind != "user" && kind != "assistant" {
        return None;
    }
    if v.get("isMeta").and_then(serde_json::Value::as_bool) == Some(true) {
        return None;
    }
    let content = v.get("message")?.get("content")?;
    let text = match content {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(blocks) => blocks
            .iter()
            .filter(|b| b.get("type").and_then(serde_json::Value::as_str) == Some("text"))
            .filter_map(|b| b.get("text").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => return None,
    };
    let text = text.trim();
    // A command's echo or a caveat Claude Code writes as a user line.
    if text.is_empty() || text.starts_with("<command-") || text.starts_with("<local-command-") {
        return None;
    }
    let at = v
        .get("timestamp")
        .and_then(serde_json::Value::as_str)
        .and_then(parse_timestamp);
    Some((text.chars().take(MESSAGE_CHARS).collect(), at))
}

/// A chunk's complete lines as pieces to index: `(offset of the piece's
/// first line, text, newest time)`, each at most [`PIECE_CHARS`]; lines
/// before `since` are skipped. The second value is how many bytes of the
/// chunk were complete lines (what the cursor moves by).
pub fn pieces(bytes: &[u8], start: i64, since: i64, now: i64) -> (Vec<(i64, String, i64)>, i64) {
    let complete = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    let mut out: Vec<(i64, String, i64)> = Vec::new();
    let mut at = 0usize;
    for raw in bytes[..complete].split(|b| *b == b'\n') {
        let line_offset = start + at as i64;
        at += raw.len() + 1;
        let Ok(line) = std::str::from_utf8(raw) else {
            continue;
        };
        let Some((text, ts)) = line_text(line) else {
            continue;
        };
        let ts = ts.unwrap_or(now);
        if ts < since {
            continue;
        }
        match out.last_mut() {
            Some((_, piece, newest))
                if piece.chars().count() + text.chars().count() < PIECE_CHARS =>
            {
                piece.push('\n');
                piece.push_str(&text);
                *newest = (*newest).max(ts);
            }
            _ => out.push((line_offset, text, ts)),
        }
    }
    (out, complete as i64)
}

async fn run_script(
    exec: &dyn SshExec,
    host: &str,
    script: &str,
) -> Result<std::process::Output, IpcError> {
    if host == "local" {
        crate::service::hub::ensure_local_allowed(host)?;
        let child = crate::proc::command("bash")
            .arg("-c")
            .arg(script)
            .kill_on_drop(true)
            .output();
        tokio::time::timeout(LOCAL_WALL_CLOCK, child)
            .await
            .map_err(|_| IpcError::new(codes::E_SSH_TIMEOUT, "transcript indexing timed out"))?
            .map_err(|e| IpcError::new(codes::E_SHELL, format!("spawn bash: {e}")))
    } else {
        exec.run(host, &["bash", "-lc", &quote(script)], CONNECT_TIMEOUT)
            .await
    }
}

/// Index one host's new transcript text; how many chunks were written.
pub async fn index_host(
    store: &Mutex<Store>,
    exec: &dyn SshExec,
    host: &str,
    now: i64,
) -> Result<usize, IpcError> {
    crate::validate::host_alias(host)?;
    let (targets, since) = {
        let s = lock(store)?;
        let cursors = s.transcript_cursors()?;
        let days = transcript_days(&s);
        let targets: Vec<Target> = s
            .list_usage_cursors(host)?
            .into_iter()
            .filter_map(|c| {
                let cid = c.claude_session_id?;
                // The id names a file: anything else is not read.
                if cid.is_empty()
                    || !cid
                        .chars()
                        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
                {
                    return None;
                }
                let (source, offset) = cursors
                    .get(&c.session_id)
                    .cloned()
                    .map_or((None, 0), |(s, o)| (Some(s), o));
                Some(Target {
                    session_id: c.session_id,
                    transcript_path: c.transcript_path,
                    claude_session_id: cid,
                    source,
                    offset,
                })
            })
            .collect();
        (targets, now - days * 86_400)
    };
    if targets.is_empty() {
        return Ok(0);
    }
    let out = run_script(
        exec,
        host,
        &batch_script(&targets, FILE_CAP_BYTES, HOST_BUDGET_BYTES),
    )
    .await?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let reads = parse_output(&stdout);
    let s = lock(store)?;
    s.atomically(|s| {
        let mut written = 0;
        for t in &targets {
            let Some(read) = reads.get(&t.session_id) else {
                continue;
            };
            let claude_id = read.source.trim_end_matches(".jsonl");
            // The same file, rewritten shorter: what was indexed of it is stale.
            if t.source.as_deref() == Some(read.source.as_str()) && read.offset == 0 && t.offset > 0
            {
                s.clear_transcript_chunks_of(claude_id)?;
            }
            let (found, consumed) = pieces(&read.bytes, read.offset, since, now);
            for (offset, text, at) in &found {
                s.upsert_transcript_chunk(&TranscriptChunk {
                    session_id: t.session_id,
                    claude_session_id: claude_id,
                    offset: *offset as u64,
                    text,
                    at: *at,
                })?;
                written += 1;
            }
            s.set_transcript_cursor(t.session_id, &read.source, read.offset + consumed, now)?;
        }
        Ok(written)
    })
}

/// `search.transcript_days`, at least one.
fn transcript_days(s: &Store) -> i64 {
    settings::get_string(s, settings::SEARCH_TRANSCRIPT_DAYS)
        .parse::<i64>()
        .unwrap_or(30)
        .max(1)
}

/// Single-flight guard.
static RUNNING: AtomicBool = AtomicBool::new(false);
struct Flight;
impl Drop for Flight {
    fn drop(&mut self) {
        RUNNING.store(false, Ordering::SeqCst);
    }
}

/// Reconcile-tick hook: when on and due, index every reachable host (off
/// the tick); when off, drop what was indexed. Must be called from inside
/// the tokio runtime.
pub fn spawn_index(store: &Arc<Mutex<Store>>, ssh: &Arc<SshClient>) {
    static LAST: std::sync::LazyLock<Mutex<Option<std::time::Instant>>> =
        std::sync::LazyLock::new(|| Mutex::new(None));
    static CLEARED: AtomicBool = AtomicBool::new(false);
    let enabled = match store.lock() {
        Ok(s) => settings::get_bool(&s, settings::SEARCH_INDEX_TRANSCRIPTS),
        Err(_) => return,
    };
    if !enabled {
        // Once per process while off: the chunks of a previous "on" go.
        if !CLEARED.swap(true, Ordering::SeqCst) {
            if let Ok(s) = store.lock() {
                if let Err(e) = s.clear_transcript_chunks() {
                    tracing::warn!(error = %e.message, "clearing indexed transcript text failed");
                }
            }
        }
        return;
    }
    CLEARED.store(false, Ordering::SeqCst);
    {
        let Ok(mut last) = LAST.lock() else { return };
        if last.is_some_and(|t| t.elapsed() < INTERVAL) {
            return;
        }
        if RUNNING
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }
        *last = Some(std::time::Instant::now());
    }
    let flight = Flight;
    let (store, ssh) = (Arc::clone(store), Arc::clone(ssh));
    tokio::spawn(async move {
        let _flight = flight;
        let now = crate::store::now_unix();
        let hosts: Vec<String> = match store.lock() {
            Ok(s) => crate::service::hosts::active_hosts(
                s.list_hosts().unwrap_or_default(),
                crate::service::hub::local_host_enabled(),
            )
            .into_iter()
            .filter(|h| h.alias == "local" || h.reachable)
            .map(|h| h.alias)
            .collect(),
            Err(_) => return,
        };
        let exec: &dyn SshExec = ssh.as_ref();
        let mut written = 0;
        for host in hosts {
            match index_host(&store, exec, &host, now).await {
                Ok(n) => written += n,
                Err(e) => {
                    tracing::warn!(host = %host, code = %e.code, error = %e.message, "transcript indexing failed (retried next pass)")
                }
            }
        }
        if let Ok(s) = store.lock() {
            let days = transcript_days(&s);
            if let Err(e) = s.prune_transcript_chunks(now - days * 86_400) {
                tracing::warn!(error = %e.message, "pruning indexed transcript text failed");
            }
        }
        crate::service::loops::report("search_index", Ok::<_, String>(()), Some(INTERVAL));
        if written > 0 {
            tracing::debug!("search: indexed {written} transcript chunk(s)");
        }
    });
}

#[cfg(test)]
#[path = "search_index_tests.rs"]
mod tests;
