//! Per-session token usage and estimated cost (Wave 5 G1, PROD-7).
//!
//! Source: every assistant line of Claude Code's JSONL transcript carries
//! `message.model` and `message.usage` with `input_tokens`, `output_tokens`,
//! `cache_creation_input_tokens` (split by TTL in
//! `cache_creation.ephemeral_5m_input_tokens` / `ephemeral_1h_input_tokens`)
//! and `cache_read_input_tokens` (verified on live installs). Claude writes
//! ONE LINE PER CONTENT BLOCK, each repeating the message's `id`; newer
//! versions stream those lines with GROWING usage (on this box 1,216 of
//! 8,543 repeated lines grew `output_tokens`, none shrank), so a repeated id
//! adds only the growth over what was already counted for it.
//!
//! Collection is incremental: each session row keeps `usage_offset_bytes`,
//! the transcript file name the offset refers to, and the id + counted usage
//! of the last message. Once per `usage.interval_secs` the reconcile tick
//! spawns ONE pass (single-flight, off the tick) that runs one batched script
//! per reachable host over its live sessions; for each file it reads only
//! the bytes past the offset (`tail -c +N | head -c M`), extracts the
//! counters with a jq-free awk, and prints per-model sums plus the number of
//! bytes of COMPLETE lines consumed (a half-written last line is left for
//! the next pass). Each file reads at most [`MAX_CHUNK_BYTES`] and a host's
//! batch at most [`HOST_BUDGET_BYTES`], stalest cursor first, so a backlog
//! converges over several passes inside the ssh wall clock. A file that
//! shrank restarts from 0 and replaces the totals; a different file (e.g.
//! after `/clear`) restarts from 0 and keeps accumulating. Only numbers and
//! model ids leave the host — never transcript content.
//!
//! Cost is an ESTIMATE: tokens priced with [`BUILTIN_PRICES`] (overridable
//! per model through the `usage.prices_json` setting) when each delta lands.

use crate::ipc_error::IpcError;
use crate::service::settings;
use crate::shell::quote;
use crate::ssh::{SshClient, SshExec};
use crate::store::{SessionRow, Store, UsageCursor, UsageDelta, UsageTotals};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Upper bound on the bytes one pass reads from one transcript.
pub const MAX_CHUNK_BYTES: i64 = 8 * 1024 * 1024;

/// Upper bound on the bytes one host's batch reads in one pass, across all
/// of its sessions. Sessions past it are skipped this pass (their cursor is
/// untouched); the store hands out cursors stalest first, so every session
/// gets its turn and a large backlog converges.
pub const HOST_BUDGET_BYTES: i64 = 32 * 1024 * 1024;

/// ssh `ConnectTimeout`; the whole batch is bounded by the client's wall
/// clock (`SshClient::default_wall_clock`, 30 s for this value).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Wall clock for the local (`host == "local"`) batch.
const LOCAL_WALL_CLOCK: Duration = Duration::from_secs(30);

/// Days `usage_report` covers when `since_secs` is omitted.
pub const DEFAULT_REPORT_DAYS: i64 = 30;

/// Days of per-day totals `fleet_health` reports.
pub const HEALTH_DAYS: i64 = 7;

/// Cap on the session rows one `usage_report` returns (highest cost first).
const MAX_REPORT_SESSIONS: usize = 200;

pub const SECS_PER_DAY: i64 = 86_400;

/// Wording every usage surface carries: the numbers are estimates.
pub const ESTIMATE_NOTE: &str = "Estimated: token counts summed from the Claude Code \
transcript, priced with claude-fleet's built-in per-model table (override with the \
usage.prices_json setting). Not a bill.";

/// USD per million tokens.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    /// 1-hour-TTL cache write (2x input), the TTL Claude Code uses.
    pub cache_write: f64,
    pub cache_read: f64,
    /// 5-minute-TTL cache write; `None` = 1.25x `input`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_5m: Option<f64>,
}

impl Price {
    pub fn cache_write_5m_rate(&self) -> f64 {
        self.cache_write_5m.unwrap_or(self.input * 1.25)
    }
}

const fn price(input: f64, output: f64, cache_write: f64, cache_read: f64) -> Price {
    Price {
        input,
        output,
        cache_write,
        cache_read,
        cache_write_5m: None,
    }
}

/// Built-in price table: first-party Anthropic API list prices, USD per
/// million tokens, as of 2026-09. A key matches when it is a substring of the
/// lower-cased model id; the LONGEST matching key wins, so `fable-5-1` beats
/// `fable`, `sonnet-5` beats `sonnet` and `opus-4-1` beats `opus`.
///
/// - `cache_write` is the 1-hour-TTL write rate (2x input); the 5-minute
///   rate is 1.25x input. Transcripts report both
///   (`cache_creation.ephemeral_5m/1h_input_tokens`) and each is priced at
///   its own rate.
/// - `cache_read` is 0.1x input, except Claude Fable 5.1 ($0.25).
/// - `opus` is the current $5 / $25 tier (Opus 4.5 and later); Opus 4 /
///   4.1 and Claude 3 Opus were $15 / $75 and have their own rows. Bedrock /
///   Vertex pricing differs. Override any row with `usage.prices_json`, e.g.
///   `{"sonnet-4-5":{"input":3,"output":15,"cache_write":6,"cache_read":0.3}}`.
pub const BUILTIN_PRICES: &[(&str, Price)] = &[
    ("fable-5-1", price(10.0, 50.0, 20.0, 0.25)),
    ("fable", price(10.0, 50.0, 20.0, 1.0)),
    ("mythos", price(10.0, 50.0, 20.0, 1.0)),
    ("opus", price(5.0, 25.0, 10.0, 0.5)),
    ("opus-4-1", price(15.0, 75.0, 30.0, 1.5)),
    ("opus-4-2025", price(15.0, 75.0, 30.0, 1.5)),
    ("3-opus", price(15.0, 75.0, 30.0, 1.5)),
    ("sonnet-5", price(2.0, 10.0, 4.0, 0.2)),
    ("sonnet", price(3.0, 15.0, 6.0, 0.3)),
    ("haiku", price(1.0, 5.0, 2.0, 0.1)),
];

/// Most keys a `usage.prices_json` override may hold.
pub const MAX_PRICE_KEYS: usize = 64;
/// Sanity cap on one price (USD per million tokens).
pub const MAX_PRICE_PER_MTOK: f64 = 10_000.0;

/// Parse + validate a `usage.prices_json` value: a JSON object of
/// model-id fragment → `{input, output, cache_write, cache_read,
/// cache_write_5m?}` (USD per million tokens). Keys are lower-cased.
/// `E_INVALID` on any bad shape.
pub fn parse_price_overrides(raw: &str) -> Result<BTreeMap<String, Price>, IpcError> {
    let bad = |msg: String| {
        IpcError::new(
            "E_INVALID",
            format!("{}: {msg}", settings::USAGE_PRICES_JSON),
        )
    };
    let map: BTreeMap<String, Price> = serde_json::from_str(raw.trim()).map_err(|_| {
        bad("must be a JSON object of model-name fragment to \
             {\"input\",\"output\",\"cache_write\",\"cache_read\"[,\"cache_write_5m\"]} \
             USD per million tokens"
            .into())
    })?;
    if map.len() > MAX_PRICE_KEYS {
        return Err(bad(format!("at most {MAX_PRICE_KEYS} models")));
    }
    let mut out = BTreeMap::new();
    for (k, p) in map {
        let key = k.trim().to_ascii_lowercase();
        if key.is_empty()
            || key.len() > 64
            || !key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'))
        {
            return Err(bad(format!("invalid model key {k:?}")));
        }
        for v in [p.input, p.output, p.cache_write, p.cache_read]
            .into_iter()
            .chain(p.cache_write_5m)
        {
            if !v.is_finite() || !(0.0..=MAX_PRICE_PER_MTOK).contains(&v) {
                return Err(bad(format!(
                    "prices for {key} must be between 0 and {MAX_PRICE_PER_MTOK}"
                )));
            }
        }
        out.insert(key, p);
    }
    Ok(out)
}

/// The effective `usage.prices_json` overrides (empty when unset/malformed).
pub fn price_overrides(s: &Store) -> BTreeMap<String, Price> {
    parse_price_overrides(&settings::get_string(s, settings::USAGE_PRICES_JSON)).unwrap_or_default()
}

/// Price for a model id: the longest key (override or built-in) contained in
/// the lower-cased id; an override wins a tie. `None` for an unknown model
/// (its tokens are still counted, at zero cost — the UI says "unpriced").
pub fn price_for(model: &str, overrides: &BTreeMap<String, Price>) -> Option<Price> {
    let m = model.to_ascii_lowercase();
    let mut best: Option<(usize, Price)> = None;
    let candidates = overrides
        .iter()
        .map(|(k, p)| (k.as_str(), *p))
        .chain(BUILTIN_PRICES.iter().map(|(k, p)| (*k, *p)));
    for (key, p) in candidates {
        if m.contains(key) && best.is_none_or(|(len, _)| key.len() > len) {
            best = Some((key.len(), p));
        }
    }
    best.map(|(_, p)| p)
}

/// Estimated cost of `t` in micro-USD (tokens × USD-per-million-tokens is
/// exactly micro-USD). `cache_write_5m_tokens` of the cache writes are
/// priced at the 5-minute rate, the rest at the 1-hour rate.
pub fn cost_micros(t: &UsageTotals, cache_write_5m_tokens: i64, p: Price) -> i64 {
    let w = t.cache_write_tokens.max(0);
    let w5 = cache_write_5m_tokens.clamp(0, w);
    let c = t.input_tokens as f64 * p.input
        + t.output_tokens as f64 * p.output
        + (w - w5) as f64 * p.cache_write
        + w5 as f64 * p.cache_write_5m_rate()
        + t.cache_read_tokens as f64 * p.cache_read;
    c.round() as i64
}

// ── the batched host script ──

/// Usage extraction over one transcript chunk (stdin). POSIX awk only (gawk,
/// mawk, BSD awk, busybox), run under `LC_ALL=C` so `length` counts bytes.
///
/// A line is processed one record late so the LAST line can be checked for
/// completeness: it counts only if its bytes plus a newline fit in `chunk`
/// (a line still being written has no newline yet). `used` is the byte count
/// of the complete lines — the caller's offset advance.
///
/// Per counted line: skip unless it holds `"usage":{`; take the first
/// `"id":"msg_…"` and `"model":"…"` (message fields precede the content);
/// skip a line without a message id (a `user` line whose `toolUseResult`
/// carries a subagent's own `usage` — verified on a live transcript) and
/// `<synthetic>` entries; cut the LAST `"usage":{` object, read the 5-minute
/// cache-write split from it, strip its nested objects, and read the four
/// counters. A new message id adds its counters; a repeat of the last id
/// (its next content block) adds only what grew. `last`/`lu` carry that id
/// and its counted usage (`in,out,cw,cr,cw5m`) across passes. Escaped JSON
/// inside a string never contains an unescaped `"usage":{`. Must not contain
/// a single quote: it is embedded in `'…'`.
const AWK: &str = r##"BEGIN {
  split(lu, a, ",")
  ci = a[1] + 0; co = a[2] + 0; cw = a[3] + 0; cr = a[4] + 0; c5 = a[5] + 0
}
function num(u, k,   i, r) {
  i = index(u, "\"" k "\":")
  if (i == 0) return 0
  r = substr(u, i + length(k) + 3)
  if (match(r, /^[0-9]+/)) return substr(r, 1, RLENGTH) + 0
  return 0
}
function pos(x) { return x > 0 ? x : 0 }
function take(line,   i, u, raw, id, m, g, vi, vo, vw, vr, v5, di, dq, dw, dr, d5) {
  if (index(line, "\"usage\":{") == 0) return
  id = ""
  if (match(line, /"id":"msg_[A-Za-z0-9_-]+"/)) id = substr(line, RSTART + 6, RLENGTH - 7)
  if (id == "") return
  m = ""
  if (match(line, /"model":"[^"]*"/)) m = substr(line, RSTART + 9, RLENGTH - 10)
  if (m == "<synthetic>") return
  u = line
  while ((i = index(u, "\"usage\":{")) > 0) u = substr(u, i + 9)
  u = substr(u, 1, 16384)
  raw = u
  g = 1
  while (g > 0) g = gsub(/[{][^{}]*[}]/, "", u)
  i = index(u, "}")
  if (i > 0) u = substr(u, 1, i - 1)
  vi = num(u, "input_tokens")
  vo = num(u, "output_tokens")
  vw = num(u, "cache_creation_input_tokens")
  vr = num(u, "cache_read_input_tokens")
  v5 = num(raw, "ephemeral_5m_input_tokens")
  if (v5 > vw) v5 = vw
  if (id == last) {
    di = pos(vi - ci); dq = pos(vo - co); dw = pos(vw - cw); dr = pos(vr - cr); d5 = pos(v5 - c5)
  } else {
    di = vi; dq = vo; dw = vw; dr = vr; d5 = v5
    ci = 0; co = 0; cw = 0; cr = 0; c5 = 0
    last = id
  }
  if (vi > ci) ci = vi
  if (vo > co) co = vo
  if (vw > cw) cw = vw
  if (vr > cr) cr = vr
  if (v5 > c5) c5 = v5
  lastm = m
  if (di + dq + dw + dr + d5 == 0) return
  if (!(m in seen)) { seen[m] = 1; order[++nm] = m }
  ti[m] += di
  to[m] += dq
  tw[m] += dw
  tr[m] += dr
  t5[m] += d5
}
{ if (have) { used += length(prev) + 1; take(prev) } prev = $0; have = 1 }
END {
  if (have && used + length(prev) + 1 <= chunk + 0) { used += length(prev) + 1; take(prev) }
  for (k = 1; k <= nm; k++) { m = order[k]; printf "U\t%s\t%s\t%.0f\t%.0f\t%.0f\t%.0f\t%.0f\n", sid, m, ti[m], to[m], tw[m], tr[m], t5[m] }
  printf "E\t%s\t%.0f\t%s\t%s\t%.0f,%.0f,%.0f,%.0f,%.0f\n", sid, used + 0, last, lastm, ci, co, cw, cr, c5
}"##;

/// Shell function run once per session:
/// `one <session id> <stored path> <claude id> <offset> <source> <last id> <last usage>`.
///
/// Skips the session (no output) once the host's `budget` is spent.
/// Otherwise locates the file (the hook-reported path, else
/// `~/.claude/projects/*/<claude id>.jsonl` — ids are unique UUIDs), picks
/// the read mode, charges the chunk to `budget`, and prints:
/// - `M\t<sid>` — no transcript;
/// - `F\t<sid>\t<cont|new|shrink>\t<start offset>\t<chunk bytes>\t<file name>`
///   then zero or more
///   `U\t<sid>\t<model>\t<in>\t<out>\t<cache write>\t<cache read>\t<cache write 5m>`
///   and `E\t<sid>\t<bytes consumed>\t<last msg id>\t<last model>\t<last usage>`.
const ONE_FN: &str = r#"one() {
  sid=$1; f=$2; cid=$3; off=$4; src=$5; last=$6; lu=$7
  if [ "$budget" -le 0 ]; then return 0; fi
  if [ -z "$f" ] || [ ! -f "$f" ]; then
    f=''
    if [ -n "$cid" ]; then
      for c in "$HOME"/.claude/projects/*/"$cid".jsonl; do
        if [ -f "$c" ]; then f=$c; break; fi
      done
    fi
  fi
  if [ -z "$f" ]; then printf 'M\t%s\n' "$sid"; return 0; fi
  size=$(wc -c < "$f" 2>/dev/null | tr -d ' ')
  case "$size" in ''|*[!0-9]*) printf 'M\t%s\n' "$sid"; return 0;; esac
  base=${f##*/}
  mode=cont
  if [ "$base" != "$src" ]; then mode=new; off=0; last=''; lu=''
  elif [ "$size" -lt "$off" ]; then mode=shrink; off=0; last=''; lu=''
  fi
  n=$((size - off))
  if [ "$n" -gt "$cap" ]; then n=$cap; fi
  if [ "$n" -gt "$budget" ]; then n=$budget; fi
  if [ "$n" -gt 0 ]; then budget=$((budget - n)); fi
  printf 'F\t%s\t%s\t%s\t%s\t%s\n' "$sid" "$mode" "$off" "$n" "$base"
  if [ "$n" -le 0 ]; then printf 'E\t%s\t0\t%s\t\t%s\n' "$sid" "$last" "$lu"; return 0; fi
  tail -c +$((off + 1)) "$f" 2>/dev/null | head -c "$n" | awk -v sid="$sid" -v chunk="$n" -v last="$last" -v lu="$lu" '"#;

/// The batch script for one host: every value is shell-quoted. `cap` bounds
/// one file's chunk, `budget` the whole batch.
pub fn batch_script(cursors: &[UsageCursor], cap: i64, budget: i64) -> String {
    let mut s = String::with_capacity(4096 + cursors.len() * 256);
    s.push_str("set +e\nexport LC_ALL=C\n");
    s.push_str(&format!("cap={}\n", quote(&cap.max(1).to_string())));
    s.push_str(&format!("budget={}\n", quote(&budget.max(1).to_string())));
    s.push_str(ONE_FN);
    s.push_str(AWK);
    s.push_str("'\n}\n");
    for c in cursors {
        s.push_str(&format!(
            "one {} {} {} {} {} {} {}\n",
            quote(&c.session_id.to_string()),
            quote(c.transcript_path.as_deref().unwrap_or("")),
            quote(c.claude_session_id.as_deref().unwrap_or("")),
            quote(&c.offset_bytes.max(0).to_string()),
            quote(c.source.as_deref().unwrap_or("")),
            quote(c.last_msg_id.as_deref().unwrap_or("")),
            quote(c.last_msg_usage.as_deref().unwrap_or("")),
        ));
    }
    s
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadMode {
    /// Same file, read from the stored offset.
    Cont,
    /// A file other than the stored source (or the first read): from 0,
    /// totals keep accumulating.
    New,
    /// Same file but smaller than the offset (rewritten): from 0, the
    /// totals are REPLACED by what this pass counts.
    Shrink,
}

/// One model's token sums in a [`FileRead`] (cost not yet applied).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelUsage {
    pub model: Option<String>,
    pub totals: UsageTotals,
    /// Of `totals.cache_write_tokens`, the 5-minute-TTL writes.
    pub cache_write_5m_tokens: i64,
}

/// One file's result from the batch script.
#[derive(Debug, Clone, PartialEq)]
pub struct FileRead {
    pub mode: ReadMode,
    pub start: i64,
    pub chunk: i64,
    pub source: String,
    pub by_model: Vec<ModelUsage>,
    pub consumed: i64,
    pub last_msg_id: Option<String>,
    pub last_model: Option<String>,
    /// What was counted for `last_msg_id` (`in,out,cw,cr,cw5m`).
    pub last_msg_usage: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FileOutcome {
    Missing,
    Read(FileRead),
}

/// A model id as printed by awk: kept when short and printable.
fn clean_model(raw: &str) -> Option<String> {
    let m = raw.trim();
    (!m.is_empty() && m.len() <= 128 && !m.chars().any(char::is_control)).then(|| m.to_string())
}

/// A message id / file name token: `[A-Za-z0-9._-]`, at most 128 bytes.
fn clean_token(raw: &str) -> Option<String> {
    let t = raw.trim();
    (!t.is_empty()
        && t.len() <= 128
        && t.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')))
    .then(|| t.to_string())
}

/// A last-message usage state: exactly five non-negative integers joined by
/// commas.
fn clean_usage_state(raw: &str) -> Option<String> {
    let t = raw.trim();
    let parts: Vec<&str> = t.split(',').collect();
    (t.len() <= 128
        && parts.len() == 5
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())))
    .then(|| t.to_string())
}

fn count(raw: &str) -> Option<i64> {
    raw.trim().parse::<i64>().ok().map(|n| n.max(0))
}

/// Parse the batch script's stdout. A file whose `E` line never arrived (the
/// script was cut off) is dropped, so its offset is not advanced; a session
/// skipped for budget prints nothing and is absent.
pub fn parse_batch_output(stdout: &str) -> BTreeMap<i64, FileOutcome> {
    let mut out = BTreeMap::new();
    let mut pending: BTreeMap<i64, FileRead> = BTreeMap::new();
    for line in stdout.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        let Some(sid) = f.get(1).and_then(|s| s.trim().parse::<i64>().ok()) else {
            continue;
        };
        match f.as_slice() {
            ["M", _] => {
                out.insert(sid, FileOutcome::Missing);
            }
            ["F", _, mode, start, chunk, source] => {
                let mode = match *mode {
                    "cont" => ReadMode::Cont,
                    "new" => ReadMode::New,
                    "shrink" => ReadMode::Shrink,
                    _ => continue,
                };
                let (Some(start), Some(chunk), Some(source)) =
                    (count(start), count(chunk), clean_token(source))
                else {
                    continue;
                };
                pending.insert(
                    sid,
                    FileRead {
                        mode,
                        start,
                        chunk,
                        source,
                        by_model: Vec::new(),
                        consumed: 0,
                        last_msg_id: None,
                        last_model: None,
                        last_msg_usage: None,
                    },
                );
            }
            ["U", _, model, i, o, w, r, w5] => {
                if let Some(read) = pending.get_mut(&sid) {
                    read.by_model.push(ModelUsage {
                        model: clean_model(model),
                        totals: UsageTotals {
                            input_tokens: count(i).unwrap_or(0),
                            output_tokens: count(o).unwrap_or(0),
                            cache_write_tokens: count(w).unwrap_or(0),
                            cache_read_tokens: count(r).unwrap_or(0),
                            cost_micros: 0,
                        },
                        cache_write_5m_tokens: count(w5).unwrap_or(0),
                    });
                }
            }
            ["E", _, used, last, last_model, last_usage] => {
                if let Some(mut read) = pending.remove(&sid) {
                    read.consumed = count(used).unwrap_or(0);
                    read.last_msg_id = clean_token(last);
                    read.last_model = clean_model(last_model);
                    read.last_msg_usage = read
                        .last_msg_id
                        .as_ref()
                        .and_then(|_| clean_usage_state(last_usage));
                    out.insert(sid, FileOutcome::Read(read));
                }
            }
            _ => {}
        }
    }
    out
}

/// Turn one file's read into the store update: price each model's tokens,
/// and advance the offset by the complete lines consumed. A single line
/// longer than the whole `cap` would pin the offset forever, so a chunk that
/// reached the cap with nothing consumed skips the chunk instead.
pub fn plan_delta(
    read: &FileRead,
    overrides: &BTreeMap<String, Price>,
    cap: i64,
    now: i64,
) -> UsageDelta {
    let mut totals = UsageTotals::default();
    for mu in &read.by_model {
        let mut t = mu.totals;
        t.cost_micros = mu
            .model
            .as_deref()
            .and_then(|m| price_for(m, overrides))
            .map(|p| cost_micros(&t, mu.cache_write_5m_tokens, p))
            .unwrap_or(0);
        totals.add(&t);
    }
    let consumed = read.consumed.clamp(0, read.chunk.max(0));
    let advance = if consumed == 0 && cap > 0 && read.chunk >= cap {
        read.chunk
    } else {
        consumed
    };
    UsageDelta {
        reset: read.mode == ReadMode::Shrink,
        totals,
        model: read.last_model.clone(),
        offset: read.start + advance,
        source: read.source.clone(),
        last_msg_id: read.last_msg_id.clone(),
        last_msg_usage: read.last_msg_usage.clone(),
        now,
    }
}

/// Drop values that must never reach the script: an invalid claude id (no
/// cursor), a stored path that does not name `<claude id>.jsonl` under
/// `.claude/projects/` (lookup falls back to the id), malformed tokens.
fn sanitize_cursor(mut c: UsageCursor) -> Option<UsageCursor> {
    let id = c
        .claude_session_id
        .take()
        .filter(|id| crate::validate::claude_session_id(id).is_ok())?;
    c.transcript_path = c
        .transcript_path
        .take()
        .filter(|p| crate::service::hooks::valid_transcript_path(p, &id));
    c.source = c.source.as_deref().and_then(clean_token);
    c.last_msg_id = c.last_msg_id.as_deref().and_then(clean_token);
    c.last_msg_usage = c
        .last_msg_id
        .as_ref()
        .and(c.last_msg_usage.as_deref())
        .and_then(clean_usage_state);
    c.claude_session_id = Some(id);
    Some(c)
}

async fn run_script(
    exec: &dyn SshExec,
    host: &str,
    script: &str,
) -> Result<std::process::Output, IpcError> {
    if host == "local" {
        let child = tokio::process::Command::new("bash")
            .arg("-c")
            .arg(script)
            .kill_on_drop(true)
            .output();
        tokio::time::timeout(LOCAL_WALL_CLOCK, child)
            .await
            .map_err(|_| IpcError::new("E_SSH_TIMEOUT", "usage collection timed out"))?
            .map_err(|e| IpcError::new("E_SHELL", format!("spawn bash: {e}")))
    } else {
        exec.run(host, &["bash", "-lc", &quote(script)], CONNECT_TIMEOUT)
            .await
    }
}

/// Collect one host: one batched script over its live sessions that have a
/// Claude session id, bounded by [`MAX_CHUNK_BYTES`] per file and
/// [`HOST_BUDGET_BYTES`] per batch. Returns the number of sessions whose
/// totals changed.
pub async fn collect_host(
    store: &Mutex<Store>,
    exec: &dyn SshExec,
    host: &str,
    overrides: &BTreeMap<String, Price>,
    now: i64,
) -> Result<usize, IpcError> {
    crate::validate::host_alias(host)?;
    let cursors: Vec<UsageCursor> = {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        s.list_usage_cursors(host)?
    }
    .into_iter()
    .filter_map(sanitize_cursor)
    .collect();
    if cursors.is_empty() {
        return Ok(0);
    }
    let script = batch_script(&cursors, MAX_CHUNK_BYTES, HOST_BUDGET_BYTES);
    let out = run_script(exec, host, &script).await?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() && stdout.trim().is_empty() {
        return Err(IpcError::new(
            "E_SHELL",
            format!(
                "usage collection on {host} failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        ));
    }
    let results = parse_batch_output(&stdout);
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    let mut changed = 0;
    for c in &cursors {
        if let Some(FileOutcome::Read(read)) = results.get(&c.session_id) {
            let delta = plan_delta(read, overrides, MAX_CHUNK_BYTES, now);
            if s.apply_usage(c.session_id, host, &delta)? {
                changed += 1;
            }
        }
    }
    Ok(changed)
}

/// One pass over every reachable host (and `local`), sequentially: each
/// host is one bounded ssh call. Failures are logged and skipped.
pub async fn collect_all(store: &Mutex<Store>, exec: &dyn SshExec, now: i64) -> usize {
    let (hosts, overrides) = match store.lock() {
        Ok(s) => (
            s.list_hosts()
                .unwrap_or_default()
                .into_iter()
                .filter(|h| h.reachable || h.alias == "local")
                .map(|h| h.alias)
                .collect::<Vec<_>>(),
            price_overrides(&s),
        ),
        Err(_) => return 0,
    };
    let mut changed = 0;
    for host in hosts {
        match collect_host(store, exec, &host, &overrides, now).await {
            Ok(n) => changed += n,
            Err(e) => tracing::debug!("usage: {host}: {} {}", e.code, e.message),
        }
    }
    changed
}

/// Whether a collection pass is due. Pure (the caller supplies the time
/// since the last pass).
pub fn due(enabled: bool, interval_secs: u64, since_last: Option<Duration>) -> bool {
    enabled && interval_secs > 0 && since_last.is_none_or(|e| e.as_secs() >= interval_secs)
}

/// Single-flight guard: at most one collection pass runs at a time.
static RUNNING: AtomicBool = AtomicBool::new(false);

/// Held while a pass runs; clears [`RUNNING`] on drop (also on panic).
struct Flight;

impl Drop for Flight {
    fn drop(&mut self) {
        RUNNING.store(false, Ordering::SeqCst);
    }
}

fn begin_flight() -> Option<Flight> {
    RUNNING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .ok()
        .map(|_| Flight)
}

/// Reconcile-tick hook: spawn [`maybe_collect`] as its own task so a slow
/// or wedged host never stalls the tick (playbooks, gc, task sweep). Must be
/// called from inside the tokio runtime.
pub fn spawn_collect(store: &Arc<Mutex<Store>>, ssh: &Arc<SshClient>) {
    let (store, ssh) = (Arc::clone(store), Arc::clone(ssh));
    tokio::spawn(async move {
        if let Some(n) = maybe_collect(&store, &ssh).await {
            if n > 0 {
                tracing::debug!("usage: updated {n} session(s)");
            }
        }
    });
}

/// Run [`collect_all`] when no pass is in flight, `usage.enabled` is on and
/// `usage.interval_secs` have elapsed since the last pass. `None` when
/// skipped.
pub async fn maybe_collect(store: &Arc<Mutex<Store>>, ssh: &Arc<SshClient>) -> Option<usize> {
    static LAST: once_cell::sync::Lazy<Mutex<Option<std::time::Instant>>> =
        once_cell::sync::Lazy::new(|| Mutex::new(None));
    let _flight = begin_flight()?;
    let (enabled, interval) = {
        let s = store.lock().ok()?;
        (
            settings::get_bool(&s, settings::USAGE_ENABLED),
            settings::get_secs(&s, settings::USAGE_INTERVAL_SECS),
        )
    };
    {
        let mut last = LAST.lock().ok()?;
        if !due(enabled, interval, last.map(|t| t.elapsed())) {
            return None;
        }
        *last = Some(std::time::Instant::now());
    }
    let exec: &dyn SshExec = ssh.as_ref();
    Some(collect_all(store, exec, now_unix()).await)
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ── roll-ups ──

/// `YYYY-MM-DD` of a UTC day number (unix secs / 86400).
pub fn day_string(day: i64) -> String {
    // Howard Hinnant's civil_from_days.
    let z = day + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Totals for one UTC day.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DayUsage {
    pub day: String,
    #[serde(flatten)]
    pub totals: UsageTotals,
}

/// Usage per host over the given session rows; hosts with nothing counted
/// are omitted.
pub fn per_host_totals(sessions: &[SessionRow]) -> BTreeMap<String, UsageTotals> {
    let mut by: BTreeMap<String, UsageTotals> = BTreeMap::new();
    for s in sessions {
        let t = s.usage.totals();
        if !t.is_zero() {
            by.entry(s.host_alias.clone()).or_default().add(&t);
        }
    }
    by
}

/// Per-day totals from `usage_daily` since `since_day` (inclusive), summed
/// over hosts (or one host), oldest first.
pub fn daily_totals(
    s: &Store,
    since_day: i64,
    host: Option<&str>,
) -> Result<Vec<DayUsage>, IpcError> {
    let mut by: BTreeMap<i64, UsageTotals> = BTreeMap::new();
    for (day, _host, t) in s.usage_daily_since(since_day, host)? {
        by.entry(day).or_default().add(&t);
    }
    Ok(by
        .into_iter()
        .map(|(day, totals)| DayUsage {
            day: day_string(day),
            totals,
        })
        .collect())
}

/// The last `days` UTC days (today included) for `fleet_health`, all hosts
/// or one; empty on a read error.
pub fn recent_days(s: &Store, now: i64, days: i64, host: Option<&str>) -> Vec<DayUsage> {
    let today = now.div_euclid(SECS_PER_DAY);
    daily_totals(s, today - (days - 1).max(0), host).unwrap_or_default()
}

/// One session's usage in a [`UsageReport`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SessionUsage {
    pub session_id: i64,
    pub host_alias: String,
    pub tmux_name: String,
    pub friendly_name: Option<String>,
    pub model: Option<String>,
    #[serde(flatten)]
    pub totals: UsageTotals,
    pub updated_at: Option<i64>,
}

/// `usage_report` result.
#[derive(Debug, serde::Serialize)]
pub struct UsageReport {
    pub note: &'static str,
    pub host_alias: Option<String>,
    /// Unix secs the window starts at (`None` = every session, and the last
    /// [`DEFAULT_REPORT_DAYS`] days of per-day totals).
    pub since: Option<i64>,
    /// Sum over the live session rows in `sessions` (before the row cap),
    /// each over its whole lifetime.
    pub total: UsageTotals,
    /// Same as `total`, per host.
    pub by_host: BTreeMap<String, UsageTotals>,
    /// From the durable `usage_daily` roll-up (killed sessions included),
    /// scoped to the window.
    pub by_day: Vec<DayUsage>,
    /// Highest estimated cost first, at most 200 rows.
    pub sessions: Vec<SessionUsage>,
    pub sessions_truncated: bool,
}

/// Build the `usage_report`: sessions with counted usage (on `host`, and
/// whose usage changed at or after `now - since_secs`), plus per-day totals
/// over the same window. Costs are estimates in micro-USD.
pub fn report(
    s: &Store,
    host: Option<&str>,
    since_secs: Option<u64>,
    now: i64,
) -> Result<UsageReport, IpcError> {
    let since = since_secs.map(|n| now - n.min(settings::MAX_SECS) as i64);
    let rows: Vec<SessionRow> = s
        .list_all_sessions()?
        .into_iter()
        .filter(|r| host.is_none_or(|h| r.host_alias == h))
        .filter(|r| !r.usage.totals().is_zero())
        .filter(|r| since.is_none_or(|t| r.usage.usage_updated_at.is_some_and(|u| u >= t)))
        .collect();
    let by_host = per_host_totals(&rows);
    let mut total = UsageTotals::default();
    for t in by_host.values() {
        total.add(t);
    }
    let mut sessions: Vec<SessionUsage> = rows
        .into_iter()
        .map(|r| SessionUsage {
            session_id: r.id,
            totals: r.usage.totals(),
            model: r.usage.usage_model,
            updated_at: r.usage.usage_updated_at,
            host_alias: r.host_alias,
            tmux_name: r.tmux_name,
            friendly_name: r.friendly_name,
        })
        .collect();
    sessions.sort_by(|a, b| {
        b.totals
            .cost_micros
            .cmp(&a.totals.cost_micros)
            .then(a.session_id.cmp(&b.session_id))
    });
    let sessions_truncated = sessions.len() > MAX_REPORT_SESSIONS;
    sessions.truncate(MAX_REPORT_SESSIONS);
    let since_day = since
        .unwrap_or(now - (DEFAULT_REPORT_DAYS - 1) * SECS_PER_DAY)
        .div_euclid(SECS_PER_DAY);
    Ok(UsageReport {
        note: ESTIMATE_NOTE,
        host_alias: host.map(str::to_string),
        since,
        total,
        by_host,
        by_day: daily_totals(s, since_day, host)?,
        sessions,
        sessions_truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ssh_fake::{FakeSsh, Match, Reply};
    use std::path::{Path, PathBuf};

    const SID: &str = "550e8400-e29b-41d4-a716-446655440000";

    fn tokens(i: i64, o: i64, w: i64, r: i64) -> UsageTotals {
        UsageTotals {
            input_tokens: i,
            output_tokens: o,
            cache_write_tokens: w,
            cache_read_tokens: r,
            cost_micros: 0,
        }
    }

    // ── price table ──

    #[test]
    fn price_table_matches_families_and_prefers_the_longest_key() {
        let none = BTreeMap::new();
        assert_eq!(price_for("claude-opus-5", &none).unwrap().input, 5.0);
        assert_eq!(price_for("claude-opus-4-8", &none).unwrap().output, 25.0);
        assert_eq!(
            price_for("claude-opus-4-5-20251101", &none).unwrap().input,
            5.0
        );
        assert_eq!(price_for("claude-sonnet-5", &none).unwrap().input, 2.0);
        assert_eq!(price_for("claude-sonnet-4-6", &none).unwrap().input, 3.0);
        assert_eq!(price_for("claude-haiku-4-5", &none).unwrap().output, 5.0);
        assert_eq!(price_for("claude-fable-5", &none).unwrap().cache_read, 1.0);
        assert_eq!(
            price_for("claude-fable-5-1", &none).unwrap().cache_read,
            0.25
        );
        assert_eq!(price_for("CLAUDE-OPUS-5", &none).unwrap().input, 5.0);
        assert_eq!(price_for("gpt-5", &none), None);
        assert_eq!(price_for("", &none), None);
    }

    #[test]
    fn legacy_opus_models_are_priced_at_15_75() {
        let none = BTreeMap::new();
        for model in [
            "claude-opus-4-1",
            "claude-opus-4-1-20250805",
            "claude-opus-4-20250514",
            "claude-3-opus-20240229",
        ] {
            let p = price_for(model, &none).unwrap();
            assert_eq!((p.input, p.output), (15.0, 75.0), "{model}");
            assert_eq!(p.cache_write, 30.0, "{model} 1h write");
            assert_eq!(p.cache_write_5m_rate(), 18.75, "{model} 5m write");
            assert_eq!(p.cache_read, 1.5, "{model}");
        }
    }

    #[test]
    fn overrides_win_ties_and_add_models() {
        let o = parse_price_overrides(
            r#"{"Opus":{"input":1,"output":2,"cache_write":3,"cache_read":4},
                "opus-4-1":{"input":15,"output":75,"cache_write":30,"cache_read":1.5,"cache_write_5m":20},
                "local-llm":{"input":0,"output":0,"cache_write":0,"cache_read":0}}"#,
        )
        .unwrap();
        assert_eq!(price_for("claude-opus-5", &o).unwrap().input, 1.0);
        assert_eq!(
            price_for("claude-opus-4-1", &o)
                .unwrap()
                .cache_write_5m_rate(),
            20.0
        );
        assert_eq!(price_for("local-llm-7b", &o).unwrap().output, 0.0);
        // A built-in with a longer key still beats a shorter override.
        let o = parse_price_overrides(
            r#"{"fable":{"input":1,"output":1,"cache_write":1,"cache_read":1}}"#,
        )
        .unwrap();
        assert_eq!(price_for("claude-fable-5-1", &o).unwrap().input, 10.0);
    }

    #[test]
    fn cost_is_tokens_times_price_per_million_in_micros() {
        let p = price_for("claude-opus-5", &BTreeMap::new()).unwrap();
        assert_eq!(cost_micros(&tokens(1_000_000, 0, 0, 0), 0, p), 5_000_000);
        assert_eq!(cost_micros(&tokens(0, 1_000, 0, 0), 0, p), 25_000);
        assert_eq!(
            cost_micros(&tokens(0, 0, 1_000, 10_000), 0, p),
            10_000 + 5_000
        );
        assert_eq!(cost_micros(&tokens(1, 0, 0, 0), 0, p), 5);
        // 600 1-hour writes at $10 and 400 5-minute writes at $6.25.
        assert_eq!(cost_micros(&tokens(0, 0, 1_000, 0), 400, p), 6_000 + 2_500);
        // A 5-minute count above the total is clamped to it.
        assert_eq!(cost_micros(&tokens(0, 0, 100, 0), 500, p), 625);
    }

    #[test]
    fn price_overrides_validate_shape_keys_and_ranges() {
        assert!(parse_price_overrides("{}").unwrap().is_empty());
        for bad in [
            "",
            "[]",
            r#"{"opus":1}"#,
            r#"{"opus":{"input":1,"output":1,"cache_write":1}}"#,
            r#"{"opus":{"input":1,"output":1,"cache_write":1,"cache_read":1,"x":1}}"#,
            r#"{"opus":{"input":-1,"output":1,"cache_write":1,"cache_read":1}}"#,
            r#"{"opus":{"input":1e9,"output":1,"cache_write":1,"cache_read":1}}"#,
            r#"{"opus":{"input":1,"output":1,"cache_write":1,"cache_read":1,"cache_write_5m":-2}}"#,
            r#"{"op us":{"input":1,"output":1,"cache_write":1,"cache_read":1}}"#,
            r#"{"":{"input":1,"output":1,"cache_write":1,"cache_read":1}}"#,
        ] {
            assert_eq!(
                parse_price_overrides(bad).unwrap_err().code,
                "E_INVALID",
                "{bad}"
            );
        }
    }

    // ── script + awk extraction (run with a private $HOME) ──

    /// One assistant block line shaped like a live transcript, with `w5` of
    /// the `w` cache writes on the 5-minute TTL (and a decoy
    /// `ephemeral_5m_input_tokens` inside `iterations`).
    #[allow(clippy::too_many_arguments)]
    fn line_with(
        id: &str,
        model: &str,
        i: i64,
        o: i64,
        w: i64,
        w5: i64,
        r: i64,
        block: &str,
    ) -> String {
        let w1h = w - w5;
        format!(
            r#"{{"parentUuid":"p","isSidechain":false,"type":"assistant","message":{{"model":"{model}","id":"{id}","type":"message","role":"assistant","content":[{{"type":"text","text":"{block}"}}],"stop_reason":null,"usage":{{"input_tokens":{i},"cache_creation_input_tokens":{w},"cache_read_input_tokens":{r},"output_tokens":{o},"output_tokens_details":{{"thinking_tokens":3}},"server_tool_use":{{"web_search_requests":0,"web_fetch_requests":0}},"service_tier":"standard","cache_creation":{{"ephemeral_1h_input_tokens":{w1h},"ephemeral_5m_input_tokens":{w5}}},"inference_geo":"not_available","iterations":[{{"input_tokens":999,"output_tokens":999,"cache_read_input_tokens":999,"cache_creation":{{"ephemeral_5m_input_tokens":999}}}}]}}}},"requestId":"req_1","type":"assistant","uuid":"u"}}"#
        )
    }

    fn assistant(id: &str, model: &str, i: i64, o: i64, w: i64, r: i64, block: &str) -> String {
        line_with(id, model, i, o, w, 0, r, block)
    }

    fn user_line() -> String {
        // A tool result quoting a transcript: the escaped `\"usage\":{` inside
        // a JSON string must not be counted.
        r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t","content":"{\"message\":{\"usage\":{\"input_tokens\":777}}}"}]},"uuid":"v"}"#
            .to_string()
    }

    struct Fixture {
        _tmp: tempfile::TempDir,
        home: PathBuf,
        file: PathBuf,
    }

    fn fixture() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let dir = home.join(".claude/projects/-srv-proj");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(format!("{SID}.jsonl"));
        Fixture {
            _tmp: tmp,
            home,
            file,
        }
    }

    fn append(path: &Path, text: &str) {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        f.write_all(text.as_bytes()).unwrap();
    }

    fn cursor(
        path: Option<&Path>,
        offset: i64,
        source: Option<&str>,
        last: Option<&str>,
    ) -> UsageCursor {
        UsageCursor {
            session_id: 7,
            transcript_path: path.map(|p| p.to_string_lossy().into_owned()),
            claude_session_id: Some(SID.into()),
            offset_bytes: offset,
            source: source.map(str::to_string),
            last_msg_id: last.map(str::to_string),
            last_msg_usage: None,
        }
    }

    fn run_batch(
        home: &Path,
        cursors: &[UsageCursor],
        cap: i64,
        budget: i64,
    ) -> BTreeMap<i64, FileOutcome> {
        let out = std::process::Command::new("bash")
            .arg("-c")
            .arg(batch_script(cursors, cap, budget))
            .env("HOME", home)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        parse_batch_output(&String::from_utf8_lossy(&out.stdout))
    }

    fn run(home: &Path, c: &UsageCursor, cap: i64) -> FileOutcome {
        run_batch(home, std::slice::from_ref(c), cap, HOST_BUDGET_BYTES)
            .remove(&c.session_id)
            .expect("a result for the session")
    }

    fn read(o: FileOutcome) -> FileRead {
        match o {
            FileOutcome::Read(r) => r,
            FileOutcome::Missing => panic!("transcript not found"),
        }
    }

    fn model_totals(r: &FileRead, model: &str) -> UsageTotals {
        r.by_model
            .iter()
            .find(|mu| mu.model.as_deref() == Some(model))
            .map(|mu| mu.totals)
            .unwrap_or_default()
    }

    /// The cursor the store would hold after applying `r`.
    fn advance(c: &UsageCursor, r: &FileRead, cap: i64) -> UsageCursor {
        let d = plan_delta(r, &BTreeMap::new(), cap, 0);
        UsageCursor {
            offset_bytes: d.offset,
            source: Some(d.source),
            last_msg_id: d.last_msg_id,
            last_msg_usage: d.last_msg_usage,
            ..c.clone()
        }
    }

    #[test]
    fn awk_sums_usage_dedupes_blocks_and_leaves_a_truncated_last_line() {
        let fx = fixture();
        let complete = [
            r#"{"type":"user","message":{"role":"user","content":"hi"},"uuid":"a"}"#.to_string(),
            assistant("msg_01A", "claude-opus-5", 10, 20, 1000, 500, "thinking"),
            // Same message, next content block: identical usage, not recounted.
            assistant("msg_01A", "claude-opus-5", 10, 20, 1000, 500, "text"),
            user_line(),
            assistant("msg_01B", "claude-sonnet-5", 1, 5, 0, 2000, "x"),
            r#"{"type":"assistant","message":{"model":"<synthetic>","id":"msg_syn","usage":{"input_tokens":50,"output_tokens":50}}}"#.to_string(),
            r#"{"type":"summary","summary":"no usage here"}"#.to_string(),
            // A subagent result: real JSON usage, but no message id.
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t","content":"done"}]},"toolUseResult":{"status":"completed","usage":{"input_tokens":5000,"output_tokens":6000}}}"#.to_string(),
        ];
        let head: String = complete.iter().map(|l| format!("{l}\n")).collect();
        let partial = r#"{"type":"assistant","message":{"model":"claude-opus-5","id":"msg_01C","usage":{"input_tokens":4000"#;
        append(&fx.file, &head);
        append(&fx.file, partial);

        let c0 = cursor(Some(&fx.file), 0, None, None);
        let r = read(run(&fx.home, &c0, MAX_CHUNK_BYTES));
        assert_eq!(r.mode, ReadMode::New);
        assert_eq!(r.start, 0);
        assert_eq!(r.source, format!("{SID}.jsonl"));
        assert_eq!(r.chunk as usize, head.len() + partial.len());
        assert_eq!(
            r.consumed as usize,
            head.len(),
            "the half-written line waits"
        );
        assert_eq!(model_totals(&r, "claude-opus-5"), tokens(10, 20, 1000, 500));
        assert_eq!(model_totals(&r, "claude-sonnet-5"), tokens(1, 5, 0, 2000));
        assert_eq!(r.by_model.len(), 2, "synthetic entries are skipped");
        assert_eq!(r.last_msg_id.as_deref(), Some("msg_01B"));
        assert_eq!(r.last_model.as_deref(), Some("claude-sonnet-5"));
        assert_eq!(r.last_msg_usage.as_deref(), Some("1,5,0,2000,0"));

        // The line completes, a duplicate block and a new message follow:
        // only the new bytes are read and msg_01C counts once.
        append(&fx.file, ",\"output_tokens\":1}}}\n");
        let c1 = advance(&c0, &r, MAX_CHUNK_BYTES);
        append(
            &fx.file,
            &format!(
                "{}\n",
                assistant("msg_01C", "claude-opus-5", 4000, 1, 0, 0, "dup")
            ),
        );
        append(
            &fx.file,
            &format!(
                "{}\n",
                assistant("msg_01D", "claude-opus-5", 2, 3, 0, 0, "d")
            ),
        );
        let r = read(run(&fx.home, &c1, MAX_CHUNK_BYTES));
        assert_eq!(r.mode, ReadMode::Cont);
        assert_eq!(r.start as usize, head.len());
        assert_eq!(model_totals(&r, "claude-opus-5"), tokens(4002, 4, 0, 0));
        assert_eq!(r.last_msg_id.as_deref(), Some("msg_01D"));
        let size = std::fs::metadata(&fx.file).unwrap().len() as i64;
        assert_eq!(r.start + r.consumed, size);

        // A block of msg_01D written after the pass with the same usage adds
        // nothing.
        let c2 = advance(&c1, &r, MAX_CHUNK_BYTES);
        append(
            &fx.file,
            &format!(
                "{}\n",
                assistant("msg_01D", "claude-opus-5", 2, 3, 0, 0, "d2")
            ),
        );
        let r = read(run(&fx.home, &c2, MAX_CHUNK_BYTES));
        assert!(r.by_model.is_empty(), "{:?}", r.by_model);
        assert_eq!(r.last_msg_id.as_deref(), Some("msg_01D"));
        let c3 = advance(&c2, &r, MAX_CHUNK_BYTES);
        assert_eq!(
            c3.offset_bytes,
            std::fs::metadata(&fx.file).unwrap().len() as i64
        );

        // Nothing new: an empty chunk keeps the cursor.
        let r = read(run(&fx.home, &c3, MAX_CHUNK_BYTES));
        assert_eq!((r.chunk, r.consumed), (0, 0));
        assert_eq!(
            advance(&c3, &r, MAX_CHUNK_BYTES).offset_bytes,
            c3.offset_bytes
        );
    }

    #[test]
    fn growing_usage_across_block_lines_adds_only_the_growth() {
        let fx = fixture();
        // Within one pass: the message's blocks stream 5 → 20 output tokens.
        append(
            &fx.file,
            &format!(
                "{}\n",
                assistant("msg_G", "claude-opus-5", 3, 5, 100, 50, "a")
            ),
        );
        append(
            &fx.file,
            &format!(
                "{}\n",
                assistant("msg_G", "claude-opus-5", 3, 20, 100, 50, "b")
            ),
        );
        let c0 = cursor(Some(&fx.file), 0, None, None);
        let r = read(run(&fx.home, &c0, MAX_CHUNK_BYTES));
        assert_eq!(model_totals(&r, "claude-opus-5"), tokens(3, 20, 100, 50));
        assert_eq!(r.last_msg_usage.as_deref(), Some("3,20,100,50,0"));

        // Across passes: the next block, written later, grew to 32.
        let c1 = advance(&c0, &r, MAX_CHUNK_BYTES);
        append(
            &fx.file,
            &format!(
                "{}\n",
                assistant("msg_G", "claude-opus-5", 3, 32, 100, 50, "c")
            ),
        );
        let r = read(run(&fx.home, &c1, MAX_CHUNK_BYTES));
        assert_eq!(model_totals(&r, "claude-opus-5"), tokens(0, 12, 0, 0));
        assert_eq!(r.last_msg_usage.as_deref(), Some("3,32,100,50,0"));
    }

    #[test]
    fn five_minute_cache_writes_are_split_out_and_priced_at_their_rate() {
        let fx = fixture();
        append(
            &fx.file,
            &format!(
                "{}\n",
                line_with("msg_S", "claude-opus-5", 0, 0, 1_000, 400, 0, "s")
            ),
        );
        let r = read(run(
            &fx.home,
            &cursor(Some(&fx.file), 0, None, None),
            MAX_CHUNK_BYTES,
        ));
        assert_eq!(
            r.by_model[0].cache_write_5m_tokens, 400,
            "the top-level split, not the one inside iterations"
        );
        let d = plan_delta(&r, &BTreeMap::new(), MAX_CHUNK_BYTES, 0);
        assert_eq!(d.totals.cache_write_tokens, 1_000);
        assert_eq!(d.totals.cost_micros, 600 * 10 + 400 * 25 / 4);
    }

    #[test]
    fn a_shrunk_file_restarts_from_zero_and_a_new_file_from_zero_too() {
        let fx = fixture();
        append(
            &fx.file,
            &format!("{}\n", assistant("msg_1", "claude-opus-5", 7, 7, 0, 0, "a")),
        );
        let source = format!("{SID}.jsonl");
        // Stored offset beyond the (rewritten) file: shrink → from 0.
        let c = cursor(Some(&fx.file), 1_000_000, Some(&source), Some("msg_1"));
        let r = read(run(&fx.home, &c, MAX_CHUNK_BYTES));
        assert_eq!(r.mode, ReadMode::Shrink);
        assert_eq!(r.start, 0);
        assert_eq!(
            model_totals(&r, "claude-opus-5"),
            tokens(7, 7, 0, 0),
            "last id reset too"
        );
        assert!(plan_delta(&r, &BTreeMap::new(), MAX_CHUNK_BYTES, 0).reset);
        // The offset refers to another file (e.g. after /clear): new → from 0.
        let c = cursor(Some(&fx.file), 3, Some("other.jsonl"), Some("msg_1"));
        let r = read(run(&fx.home, &c, MAX_CHUNK_BYTES));
        assert_eq!(r.mode, ReadMode::New);
        assert_eq!(model_totals(&r, "claude-opus-5"), tokens(7, 7, 0, 0));
        assert!(!plan_delta(&r, &BTreeMap::new(), MAX_CHUNK_BYTES, 0).reset);
    }

    #[test]
    fn the_file_is_found_by_session_id_when_no_path_is_stored() {
        let fx = fixture();
        append(
            &fx.file,
            &format!(
                "{}\n",
                assistant("msg_1", "claude-haiku-4-5", 1, 2, 3, 4, "a")
            ),
        );
        let r = read(run(&fx.home, &cursor(None, 0, None, None), MAX_CHUNK_BYTES));
        assert_eq!(model_totals(&r, "claude-haiku-4-5"), tokens(1, 2, 3, 4));
        // A stale stored path falls back to the id lookup as well.
        let gone = fx
            .home
            .join(".claude/projects/gone")
            .join(format!("{SID}.jsonl"));
        let r = read(run(
            &fx.home,
            &cursor(Some(&gone), 0, None, None),
            MAX_CHUNK_BYTES,
        ));
        assert_eq!(model_totals(&r, "claude-haiku-4-5"), tokens(1, 2, 3, 4));
        let empty = fx.home.join("nobody");
        assert_eq!(
            run(&empty, &cursor(None, 0, None, None), MAX_CHUNK_BYTES),
            FileOutcome::Missing
        );
    }

    #[test]
    fn a_capped_chunk_reads_in_steps_and_a_giant_line_is_skipped() {
        let fx = fixture();
        let a = format!("{}\n", assistant("msg_1", "claude-opus-5", 1, 0, 0, 0, "a"));
        let b = format!("{}\n", assistant("msg_2", "claude-opus-5", 2, 0, 0, 0, "b"));
        append(&fx.file, &a);
        append(&fx.file, &b);
        let cap = (a.len() + 10) as i64;
        let c0 = cursor(Some(&fx.file), 0, None, None);
        let r = read(run(&fx.home, &c0, cap));
        assert_eq!(r.chunk, cap);
        assert_eq!(r.consumed as usize, a.len());
        assert_eq!(model_totals(&r, "claude-opus-5").input_tokens, 1);
        let c1 = advance(&c0, &r, cap);
        let r = read(run(&fx.home, &c1, cap));
        assert_eq!(model_totals(&r, "claude-opus-5").input_tokens, 2);
        // One line longer than the cap: nothing consumable, the chunk is skipped.
        let giant = read(run(&fx.home, &c0, 16));
        assert_eq!(giant.consumed, 0);
        assert_eq!(advance(&c0, &giant, 16).offset_bytes, 16);
    }

    #[test]
    fn a_backlog_larger_than_the_host_budget_converges_over_passes() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let dir = home.join(".claude/projects/-p");
        std::fs::create_dir_all(&dir).unwrap();
        let lines_per_file = 20;
        let mut cursors = Vec::new();
        let mut sizes = Vec::new();
        let mut line_len = 0;
        for f in 0..3i64 {
            let path = dir.join(format!("f{f}.jsonl"));
            for n in 0..lines_per_file {
                let l = format!(
                    "{}\n",
                    assistant(&format!("msg_{f}_{n:03}"), "claude-opus-5", 1, 0, 0, 0, "x")
                );
                line_len = l.len() as i64;
                append(&path, &l);
            }
            sizes.push(std::fs::metadata(&path).unwrap().len() as i64);
            cursors.push(UsageCursor {
                session_id: f,
                ..cursor(Some(&path), 0, None, None)
            });
        }
        // Room for a little over three lines per pass, across all files.
        let budget = line_len * 3 + 7;
        let mut counted = [0i64; 3];
        let mut passes = 0;
        while cursors.iter().zip(&sizes).any(|(c, s)| c.offset_bytes < *s) {
            passes += 1;
            assert!(passes <= 100, "no convergence: {cursors:?}");
            let results = run_batch(&home, &cursors, MAX_CHUNK_BYTES, budget);
            let mut read_this_pass = 0;
            let mut progressed = false;
            for c in cursors.iter_mut() {
                if let Some(FileOutcome::Read(r)) = results.get(&c.session_id) {
                    read_this_pass += r.chunk;
                    counted[c.session_id as usize] += model_totals(r, "claude-opus-5").input_tokens;
                    let next = advance(c, r, MAX_CHUNK_BYTES);
                    progressed |= next.offset_bytes > c.offset_bytes;
                    *c = next;
                }
            }
            assert!(
                read_this_pass <= budget,
                "pass {passes} read {read_this_pass}"
            );
            assert!(progressed, "pass {passes} made no progress");
        }
        assert!(
            passes >= 20,
            "the budget bounded each pass ({passes} passes)"
        );
        assert_eq!(counted, [lines_per_file as i64; 3]);
    }

    #[test]
    fn batch_script_quotes_every_value_and_embeds_quote_free_awk() {
        assert!(!AWK.contains('\''));
        let c = UsageCursor {
            session_id: 3,
            transcript_path: Some("/h/.claude/projects/it's/x.jsonl".into()),
            claude_session_id: Some(SID.into()),
            offset_bytes: 42,
            source: Some("x.jsonl".into()),
            last_msg_id: Some("msg_1".into()),
            last_msg_usage: Some("1,2,3,4,0".into()),
        };
        let s = batch_script(&[c], 1024, 4096);
        assert!(s.contains("cap='1024'"));
        assert!(s.contains("budget='4096'"));
        assert!(
            s.contains(&format!(
                "one '3' '/h/.claude/projects/it'\\''s/x.jsonl' '{SID}' '42' 'x.jsonl' 'msg_1' '1,2,3,4,0'"
            )),
            "{s}"
        );
        assert!(s.contains("tail -c +$((off + 1))"));
        assert!(s.contains("LC_ALL=C"));
    }

    #[test]
    fn parse_drops_cut_off_files_and_garbage() {
        let out = "junk\nM\t1\nF\t2\tcont\t10\t5\ta.jsonl\nU\t2\tclaude-opus-5\t1\t2\t3\t4\t0\n\
                   F\t3\tnew\t0\t9\tb.jsonl\nU\t3\t\t5\t0\t0\t0\t0\nE\t3\t9\tmsg_x\t\t5,0,0,0,0\n\
                   F\t4\tweird\t0\t0\tc\nF\t5\tcont\t0\t1\td.jsonl\nE\t5\t1\t\t\tnot,a,state\n";
        let all = parse_batch_output(out);
        assert_eq!(all.get(&1), Some(&FileOutcome::Missing));
        assert!(!all.contains_key(&2), "no E line: the pass was cut off");
        assert!(!all.contains_key(&4));
        let FileOutcome::Read(r) = &all[&3] else {
            panic!()
        };
        assert_eq!(r.by_model.len(), 1);
        assert_eq!(r.by_model[0].model, None);
        assert_eq!(r.by_model[0].totals, tokens(5, 0, 0, 0));
        assert_eq!(r.last_model, None);
        assert_eq!(r.last_msg_usage.as_deref(), Some("5,0,0,0,0"));
        // Unknown model: tokens counted, zero cost.
        let d = plan_delta(r, &BTreeMap::new(), MAX_CHUNK_BYTES, 9);
        assert_eq!(d.totals, tokens(5, 0, 0, 0));
        assert_eq!(d.offset, 9);
        let FileOutcome::Read(r) = &all[&5] else {
            panic!()
        };
        assert_eq!(
            (r.last_msg_id.as_deref(), r.last_msg_usage.as_deref()),
            (None, None)
        );
    }

    #[test]
    fn plan_delta_prices_each_model_and_advances_by_consumed_bytes() {
        let r = FileRead {
            mode: ReadMode::Cont,
            start: 100,
            chunk: 50,
            source: "a.jsonl".into(),
            by_model: vec![
                ModelUsage {
                    model: Some("claude-opus-5".into()),
                    totals: tokens(1_000_000, 0, 0, 0),
                    cache_write_5m_tokens: 0,
                },
                ModelUsage {
                    model: Some("claude-haiku-4-5".into()),
                    totals: tokens(0, 1_000_000, 0, 0),
                    cache_write_5m_tokens: 0,
                },
            ],
            consumed: 40,
            last_msg_id: Some("msg_9".into()),
            last_model: Some("claude-haiku-4-5".into()),
            last_msg_usage: Some("0,1,0,0,0".into()),
        };
        let d = plan_delta(&r, &BTreeMap::new(), MAX_CHUNK_BYTES, 77);
        assert_eq!(d.totals.input_tokens, 1_000_000);
        assert_eq!(d.totals.output_tokens, 1_000_000);
        assert_eq!(d.totals.cost_micros, 5_000_000 + 5_000_000);
        assert_eq!(d.offset, 140);
        assert!(!d.reset);
        assert_eq!(d.model.as_deref(), Some("claude-haiku-4-5"));
        assert_eq!(d.last_msg_usage.as_deref(), Some("0,1,0,0,0"));
        assert_eq!(d.now, 77);
        // consumed is clamped to the chunk.
        let d = plan_delta(
            &FileRead { consumed: 500, ..r },
            &BTreeMap::new(),
            MAX_CHUNK_BYTES,
            0,
        );
        assert_eq!(d.offset, 150);
    }

    #[test]
    fn sanitize_drops_bad_ids_and_foreign_paths() {
        let mut c = cursor(None, 0, Some("a b"), Some("msg;rm"));
        c.transcript_path = Some("/etc/passwd".into());
        c.last_msg_usage = Some("1,2,3,4,0".into());
        let c = sanitize_cursor(c).unwrap();
        assert_eq!(c.transcript_path, None);
        assert_eq!(c.source, None);
        assert_eq!(c.last_msg_id, None);
        assert_eq!(c.last_msg_usage, None, "no id, no carried usage");
        let good = format!("/h/.claude/projects/x/{SID}.jsonl");
        let mut c = cursor(Some(Path::new(&good)), 0, None, Some("msg_1"));
        c.last_msg_usage = Some("1,2,3,4,0; rm".into());
        let c = sanitize_cursor(c).unwrap();
        assert_eq!(c.transcript_path.as_deref(), Some(good.as_str()));
        assert_eq!(c.last_msg_usage, None);
        let mut bad = cursor(None, 0, None, None);
        bad.claude_session_id = Some("../x".into());
        assert!(sanitize_cursor(bad).is_none());
    }

    #[test]
    fn due_respects_enabled_interval_and_elapsed() {
        assert!(due(true, 300, None));
        assert!(!due(false, 300, None));
        assert!(!due(true, 0, None));
        assert!(!due(true, 300, Some(Duration::from_secs(299))));
        assert!(due(true, 300, Some(Duration::from_secs(300))));
    }

    #[test]
    fn only_one_collection_pass_is_in_flight() {
        let first = begin_flight().expect("no pass running");
        assert!(begin_flight().is_none(), "a second pass is refused");
        drop(first);
        assert!(begin_flight().is_some(), "released on drop");
    }

    #[test]
    fn day_string_is_the_utc_calendar_date() {
        assert_eq!(day_string(0), "1970-01-01");
        assert_eq!(day_string(20_342), "2025-09-11");
        assert_eq!(day_string(20_707), "2026-09-11");
        assert_eq!(day_string(11_016), "2000-02-29");
        assert_eq!(day_string(-1), "1969-12-31");
    }

    // ── collection against a FakeSsh host + the store ──

    fn store_with_session(host: &str) -> (Mutex<Store>, i64, String) {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host(host).unwrap();
        let id = s
            .upsert_session("dev-a", host, None, None, 1, 1, "running", None)
            .unwrap();
        s.set_claude_session_id(id, SID).unwrap();
        let path = format!("/home/u/.claude/projects/-srv-it's/{SID}.jsonl");
        s.set_transcript_path_by_claude_id(SID, &path).unwrap();
        (Mutex::new(s), id, path)
    }

    #[tokio::test]
    async fn collect_host_runs_one_quoted_batch_and_accumulates() {
        let (store, id, path) = store_with_session("vps");
        let fake = FakeSsh::new();
        let src = format!("{SID}.jsonl");
        fake.on(
            Match::script_contains("tail -c +"),
            Reply::ok(&format!(
                "F\t{id}\tnew\t0\t500\t{src}\nU\t{id}\tclaude-opus-5\t1000000\t10\t0\t0\t0\nE\t{id}\t480\tmsg_a\tclaude-opus-5\t1000000,10,0,0,0\n"
            )),
        );
        let n = collect_host(&store, &fake, "vps", &BTreeMap::new(), 1_000)
            .await
            .unwrap();
        assert_eq!(n, 1);
        let calls = fake.calls_for("vps");
        assert_eq!(calls.len(), 1, "one ssh call per host");
        let script = calls[0].script().expect("bash -lc script");
        assert!(script.contains(&crate::shell::quote(&path)), "{script}");
        assert!(
            script.contains("'0' '' '' ''"),
            "first pass: offset 0, no source"
        );
        assert!(script.contains(&format!("budget='{HOST_BUDGET_BYTES}'")));
        {
            let s = store.lock().unwrap();
            let row = s.get_session_by_id(id).unwrap().unwrap();
            assert_eq!(row.usage.usage_input_tokens, 1_000_000);
            assert_eq!(row.usage.usage_output_tokens, 10);
            assert_eq!(row.usage.usage_cost_micros, 5_000_000 + 250);
            assert_eq!(row.usage.usage_model.as_deref(), Some("claude-opus-5"));
            assert_eq!(row.usage.usage_updated_at, Some(1_000));
            let c = &s.list_usage_cursors("vps").unwrap()[0];
            assert_eq!(c.offset_bytes, 480);
            assert_eq!(c.source.as_deref(), Some(src.as_str()));
            assert_eq!(c.last_msg_id.as_deref(), Some("msg_a"));
            assert_eq!(c.last_msg_usage.as_deref(), Some("1000000,10,0,0,0"));
        }

        // Next pass continues from the stored cursor and adds.
        fake.clear_calls();
        fake.on(
            Match::script_contains("tail -c +"),
            Reply::ok(&format!(
                "F\t{id}\tcont\t480\t20\t{src}\nU\t{id}\tclaude-opus-5\t0\t4\t0\t0\t0\nE\t{id}\t20\tmsg_b\tclaude-opus-5\t0,4,0,0,0\n"
            )),
        );
        collect_host(&store, &fake, "vps", &BTreeMap::new(), 2_000)
            .await
            .unwrap();
        let script = fake.calls_for("vps")[0].script().unwrap();
        assert!(
            script.contains(&format!("'480' '{src}' 'msg_a' '1000000,10,0,0,0'")),
            "{script}"
        );
        {
            let s = store.lock().unwrap();
            let row = s.get_session_by_id(id).unwrap().unwrap();
            assert_eq!(row.usage.usage_output_tokens, 14);
            assert_eq!(s.list_usage_cursors("vps").unwrap()[0].offset_bytes, 500);
        }

        // A shrunk file replaces the totals.
        fake.on(
            Match::script_contains("tail -c +"),
            Reply::ok(&format!(
                "F\t{id}\tshrink\t0\t30\t{src}\nU\t{id}\tclaude-opus-5\t3\t0\t0\t0\t0\nE\t{id}\t30\tmsg_c\tclaude-opus-5\t3,0,0,0,0\n"
            )),
        );
        collect_host(&store, &fake, "vps", &BTreeMap::new(), 3_000)
            .await
            .unwrap();
        let s = store.lock().unwrap();
        let row = s.get_session_by_id(id).unwrap().unwrap();
        assert_eq!(row.usage.usage_input_tokens, 3);
        assert_eq!(row.usage.usage_output_tokens, 0);
        assert_eq!(row.usage.usage_cost_micros, 15);
    }

    #[tokio::test]
    async fn collect_host_skips_hosts_without_claude_sessions_and_reports_failures() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        store.lock().unwrap().upsert_host("vps").unwrap();
        let fake = FakeSsh::new();
        assert_eq!(
            collect_host(&store, &fake, "vps", &BTreeMap::new(), 0)
                .await
                .unwrap(),
            0
        );
        assert!(fake.calls().is_empty(), "no session, no ssh");

        let (store, _, _) = store_with_session("vps");
        fake.unreachable("vps");
        let err = collect_host(&store, &fake, "vps", &BTreeMap::new(), 0)
            .await
            .unwrap_err();
        assert!(err.code.starts_with("E_"), "{}", err.code);
    }

    /// One real pass over `local`: the store hands out the cursors and the
    /// real script reads the real files.
    async fn collect_local(store: &Mutex<Store>, now: i64) -> usize {
        let fake = FakeSsh::new();
        collect_host(store, &fake, "local", &BTreeMap::new(), now)
            .await
            .unwrap()
    }

    fn usage_of(store: &Mutex<Store>, id: i64) -> crate::store::SessionUsage {
        store
            .lock()
            .unwrap()
            .get_session_by_id(id)
            .unwrap()
            .unwrap()
            .usage
    }

    #[tokio::test]
    async fn an_inherited_cursor_counts_only_lines_appended_after_a_move() {
        // move_session keeps the Claude id and copies a whole-line prefix of
        // the source transcript to the target; the target's cursor starts
        // where the source's stood, so the copied history is not recounted.
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let src_file = root
            .join("a/.claude/projects/-p")
            .join(format!("{SID}.jsonl"));
        let dst_file = root
            .join("b/.claude/projects/-p")
            .join(format!("{SID}.jsonl"));
        std::fs::create_dir_all(src_file.parent().unwrap()).unwrap();
        std::fs::create_dir_all(dst_file.parent().unwrap()).unwrap();
        append(
            &src_file,
            &format!(
                "{}\n{}\n",
                assistant("msg_1", "claude-opus-5", 100, 10, 0, 0, "a"),
                assistant("msg_2", "claude-opus-5", 200, 20, 0, 0, "b")
            ),
        );

        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let source = s
            .upsert_session("src", "local", None, None, 1, 1, "running", None)
            .unwrap();
        s.set_claude_session_id(source, SID).unwrap();
        s.set_transcript_path_by_claude_id(SID, &src_file.to_string_lossy())
            .unwrap();
        let store = Mutex::new(s);
        collect_local(&store, 10).await;
        assert_eq!(usage_of(&store, source).usage_input_tokens, 300);

        // The move: copy the prefix, create the target with the same Claude
        // id, inherit the cursor, carry the lifetime totals, kill the source.
        std::fs::copy(&src_file, &dst_file).unwrap();
        let copied = std::fs::metadata(&dst_file).unwrap().len() as i64;
        let target = {
            let s = store.lock().unwrap();
            let target = s
                .upsert_session("dst", "local", None, None, 1, 1, "running", None)
                .unwrap();
            s.set_claude_session_id(target, SID).unwrap();
            assert!(s.inherit_usage_cursor(target, source, copied).unwrap());
            s.carry_usage_totals(target, source).unwrap();
            s.delete_session(source).unwrap();
            s.set_transcript_path_by_claude_id(SID, &dst_file.to_string_lossy())
                .unwrap();
            target
        };
        collect_local(&store, 20).await;
        assert_eq!(
            usage_of(&store, target).usage_input_tokens,
            300,
            "carried totals only; the copied history is not recounted"
        );

        // A line appended after the move counts.
        append(
            &dst_file,
            &format!("{}\n", assistant("msg_3", "claude-opus-5", 7, 1, 0, 0, "c")),
        );
        collect_local(&store, 30).await;
        let u = usage_of(&store, target);
        assert_eq!(u.usage_input_tokens, 307);
        assert_eq!(u.usage_output_tokens, 31);
        // The daily roll-up saw each token once.
        let days = store
            .lock()
            .unwrap()
            .usage_daily_since(0, Some("local"))
            .unwrap();
        let daily_in: i64 = days.iter().map(|(_, _, t)| t.input_tokens).sum();
        assert_eq!(daily_in, 307);
    }

    #[test]
    fn report_filters_by_host_and_window_and_sorts_by_cost() {
        let s = Store::open_in_memory().unwrap();
        let mut ids = Vec::new();
        for (name, host) in [("a", "alpha"), ("b", "beta"), ("c", "alpha")] {
            s.upsert_host(host).unwrap();
            ids.push(
                s.upsert_session(name, host, None, None, 1, 1, "running", None)
                    .unwrap(),
            );
        }
        let now = 20_707 * SECS_PER_DAY + 3_600;
        let apply = |id: i64, host: &str, cost: i64, at: i64| {
            s.apply_usage(
                id,
                host,
                &UsageDelta {
                    reset: false,
                    totals: UsageTotals {
                        input_tokens: 10,
                        cost_micros: cost,
                        ..Default::default()
                    },
                    model: Some("claude-opus-5".into()),
                    offset: 1,
                    source: "x.jsonl".into(),
                    last_msg_id: None,
                    last_msg_usage: None,
                    now: at,
                },
            )
            .unwrap();
        };
        apply(ids[0], "alpha", 100, now - 10);
        apply(ids[1], "beta", 300, now - 2 * SECS_PER_DAY);
        apply(ids[2], "alpha", 200, now - 20);

        let all = report(&s, None, None, now).unwrap();
        assert_eq!(all.note, ESTIMATE_NOTE);
        assert_eq!(
            all.sessions
                .iter()
                .map(|r| r.session_id)
                .collect::<Vec<_>>(),
            vec![ids[1], ids[2], ids[0]]
        );
        assert_eq!(all.total.cost_micros, 600);
        assert_eq!(all.by_host["alpha"].cost_micros, 300);
        assert_eq!(all.by_day.len(), 2);
        assert_eq!(all.by_day[1].day, "2026-09-11");
        assert_eq!(all.by_day[1].totals.cost_micros, 300);

        let alpha = report(&s, Some("alpha"), None, now).unwrap();
        assert_eq!(alpha.sessions.len(), 2);
        assert!(!alpha.by_host.contains_key("beta"));
        assert_eq!(alpha.by_day.len(), 1);

        let recent = report(&s, None, Some(3_600), now).unwrap();
        assert_eq!(recent.sessions.len(), 2, "beta's usage is two days old");
        assert_eq!(recent.by_day.len(), 1);
        assert_eq!(recent.since, Some(now - 3_600));

        assert_eq!(recent_days(&s, now, HEALTH_DAYS, None).len(), 2);
        assert_eq!(recent_days(&s, now, HEALTH_DAYS, Some("beta")).len(), 1);
    }
}
