//! Move a work session to another host (PROD-8, Wave 5 G2).
//!
//! Replaces the Handoff of the original spec (§8.3), which was never built;
//! see `docs/adr/0001-descope-freeze-ship-move.md`. A move carries the
//! conversation, not the process:
//!
//! 1. **Preflight** — no other move of the session is in flight; the source
//!    is a `work` row with a `claude_session_id`, a worktree branch and an
//!    idle Claude (freshly reconciled; a turn in progress or an unknown
//!    status is refused); both hosts are reachable, the target is
//!    provisioned; the source worktree is on the session's branch with a
//!    readable HEAD and no operation in progress (`E_MOVE_MIDOP`).
//!    Uncommitted and unpushed work is carried, not refused (see step 3);
//!    with `strict: true` it is refused as before (`E_MOVE_DIRTY`,
//!    `E_MOVE_UNPUSHED`). Nothing is ever pushed, committed or stashed on
//!    the user's behalf, and the source worktree is never modified.
//! 2. **Transcript** — the source JSONL is located (stored hook path, else
//!    `~/.claude/projects/*/<id>.jsonl`), size-checked against
//!    `move.max_transcript_mb` (`E_MOVE_TOO_LARGE`), read over ssh and
//!    written to the target's `~/.claude/projects/<encoded target cwd>/<id>.jsonl`
//!    through `SshExec::upload_file` stdin.
//! 3. **Target** — the target's main clone is seeded (present, cloned, or
//!    `git init` when origin is unreachable), the source worktree is
//!    snapshotted into private `refs/fleet/transfer/<id>/*` and bundled
//!    against what the target already has (`move.max_bundle_mb`), the bundle
//!    is relayed through this process in chunks and fetched there. The
//!    worktree is then created (or repaired, create-only) with
//!    `repair::ensure_for_new_session` from the same branch, fast-forwarded to
//!    the source HEAD when it lags; the snapshot is replayed into it and
//!    verified (its porcelain must equal the source's — `E_MOVE_CARRY`; a
//!    target worktree that already has uncommitted changes — its own, or an
//!    unfinished earlier attempt's — is `E_MOVE_TARGET_DIRTY`), and
//!    the small git-ignored files are carried over (never a failure, only a
//!    warning). The Claude-side state follows them, on the same terms: the
//!    session's own directory (subagent transcripts, tool results, the
//!    title) is merged beside the target's transcript, and the project's
//!    Claude memory gains the files the target lacks plus their `MEMORY.md`
//!    lines — both halves only ever ADD on the target, each can only warn,
//!    and a failure in one does not stop the other. Then tmux starts
//!    `cl --resume <id>` there (the recreate pane command). The move waits,
//!    bounded, for the row to be `running` and the transcript to be in
//!    place. The carry itself lives in [`carry`] and [`claude_state`]; see
//!    `docs/adr/0002-move-carries-work-as-is.md`.
//! 4. **Source** — only once the target is confirmed (the new row's
//!    `parent_session_id` is the source): unless `keep_source`, and only if
//!    the source transcript still has the size and mtime the copy was taken
//!    at, the normal kill path, followed by one more look (a write that
//!    slipped in before the kill becomes a report warning). Then — and only
//!    then — a `session_moved` event on both rows. A partial move records
//!    `session_move_partial` (with the failing step) instead.
//!
//! Failure handling: every step before the target tmux session starts —
//! every carry step included — leaves the source untouched and returns the
//! step's own error (`E_MOVE_MIDOP`, `E_MOVE_CARRY` with `details.step`,
//! `E_MOVE_TARGET_DIRTY`, `E_MOVE_TOO_LARGE` with `details.payload`). Any
//! failure after the target session exists returns `E_MOVE_PARTIAL` (with
//! the new session id in `details`) and leaves BOTH sessions alive. Once the
//! carry has begun, the private refs and transfer directories on both hosts
//! are cleaned up on every exit path; a failed cleanup is only logged.
//!
//! The lifecycle steps that need the real `SshClient` (workspace repair,
//! tmux start, reconcile, kill) sit behind [`MoveHooks`]; every data-plane
//! step (git inspection, transcript copy, target verification) goes through
//! `&dyn SshExec`, so the whole flow runs end-to-end over `FakeSsh` in tests.

pub mod carry;
pub mod claude_state;

use crate::ipc_error::lock;
use crate::ipc_error::{codes, IpcError};
use crate::service::safe_kill::{parse_porcelain, DirtyFile};
use crate::shell::quote;
use crate::ssh::{SshClient, SshExec};
use crate::store::{SessionRow, Store};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// `settings` key: largest transcript (MiB) a move copies. Absent /
/// unparseable / 0 → [`DEFAULT_MAX_TRANSCRIPT_MB`].
pub const SETTING_MAX_TRANSCRIPT_MB: &str = "move.max_transcript_mb";
pub const DEFAULT_MAX_TRANSCRIPT_MB: u64 = 200;

/// Timeline event recorded on both rows of a completed move.
pub const EVENT_MOVED: &str = "session_moved";

const LOCAL: &str = "local";

/// ssh connect timeout for the git / probe scripts (`run` bounds the whole
/// command by 3× this). `git ls-remote` / `fetch` reach origin.
const GIT_TIMEOUT: Duration = Duration::from_secs(40);
/// Connect timeout for the transcript read (wall clock 3× — 6 min — enough
/// for the 200 MiB default cap over a slow link).
const COPY_TIMEOUT: Duration = Duration::from_secs(120);
/// Wall clock for the seed (a clone) and the snapshot + bundle scripts.
const CARRY_TIMEOUT: Duration = Duration::from_secs(300);

/// Longest encoded project directory Claude Code uses verbatim; longer names
/// are truncated with a hash suffix we cannot reproduce, so `--resume` would
/// not find a copy written under the full name.
const MAX_ENCODED_DIR: usize = 200;

const NO_TRANSCRIPT: &str = "__CF_NO_TRANSCRIPT__";
const NO_WORKTREE: &str = "__CF_NO_WORKTREE__";
const NO_CWD: &str = "__CF_NO_CWD__";
const DIVERGED: &str = "__CF_DIVERGED__";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveSessionArgs {
    pub session_id: i64,
    pub target_host_alias: String,
    /// Leave the source session running after the target is confirmed.
    #[serde(default)]
    pub keep_source: bool,
    /// Refuse a dirty worktree (`E_MOVE_DIRTY`) or an unpushed branch
    /// (`E_MOVE_UNPUSHED`) instead of carrying them. Default false.
    #[serde(default)]
    pub strict: bool,
}

/// What a completed move did.
/// Every field is required on the wire, so a hub-read report fails loudly on
/// a rename rather than defaulting — see `service::repo_read` for the rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveReport {
    pub source_session_id: i64,
    pub target_session_id: i64,
    pub from_host: String,
    pub to_host: String,
    /// tmux name on the target (the source name unless taken there).
    pub tmux_name: String,
    pub claude_session_id: String,
    pub branch: String,
    pub target_cwd: String,
    /// Transcript bytes copied.
    pub transcript_bytes: u64,
    pub source_killed: bool,
    pub warnings: Vec<String>,
    /// What travelled besides the transcript.
    pub carried: carry::CarryReport,
    pub target: SessionRow,
}

/// Bounds for the "target is running" confirmation.
#[derive(Debug, Clone, Copy)]
pub struct MoveOptions {
    pub confirm_timeout: Duration,
    pub poll: Duration,
}

impl Default for MoveOptions {
    fn default() -> Self {
        Self {
            confirm_timeout: Duration::from_secs(60),
            poll: Duration::from_secs(2),
        }
    }
}

/// What the target workspace step needs (mirrors
/// `repair::NewSessionWorkspace`).
pub struct TargetWorkspace<'a> {
    pub host_alias: &'a str,
    pub project_id: i64,
    pub worktree_id: i64,
    pub tmux_name: &'a str,
    pub pane_cmd: &'a str,
    /// Layout-derived cwd (remote) or the worktree row's path (local).
    pub cwd_hint: &'a str,
}

/// Lifecycle steps that run through the existing services.
#[async_trait::async_trait]
pub trait MoveHooks: Send + Sync {
    /// Directory the source worktree is expected at (a fallback for the
    /// pane's own cwd). `None` when it cannot be derived.
    async fn source_cwd_hint(&self, store: &Mutex<Store>, row: &SessionRow) -> Option<String>;
    /// Create or repair (create-only) the worktree on the target; returns
    /// the verified cwd.
    async fn ensure_target_workspace(
        &self,
        store: &Mutex<Store>,
        w: TargetWorkspace<'_>,
    ) -> Result<String, IpcError>;
    /// `tmux new-session` on the target.
    async fn start_target(
        &self,
        host: &str,
        tmux_name: &str,
        cwd: &str,
        pane_cmd: &str,
    ) -> Result<(), IpcError>;
    /// Reconcile one host so its rows reflect tmux.
    async fn refresh_host(&self, store: &Mutex<Store>, host: &str) -> Result<(), IpcError>;
    /// The normal kill path for the source.
    async fn kill_source(
        &self,
        store: &Mutex<Store>,
        host: &str,
        tmux_name: &str,
    ) -> Result<(), IpcError>;
}

/// Production hooks over the real ssh client.
pub struct RealHooks<'a> {
    pub ssh: &'a Arc<SshClient>,
}

#[async_trait::async_trait]
impl MoveHooks for RealHooks<'_> {
    async fn source_cwd_hint(&self, store: &Mutex<Store>, row: &SessionRow) -> Option<String> {
        if row.host_alias == LOCAL {
            let s = store.lock().ok()?;
            return s.worktree_path(row.worktree_id?).ok().flatten();
        }
        crate::service::repair::spec_for_session(store, self.ssh, row.id)
            .await
            .ok()
            .and_then(|(spec, _)| spec.worktree.map(|w| w.path))
    }

    async fn ensure_target_workspace(
        &self,
        store: &Mutex<Store>,
        w: TargetWorkspace<'_>,
    ) -> Result<String, IpcError> {
        let rep = crate::service::repair::ensure_for_new_session(
            store,
            self.ssh,
            crate::service::repair::NewSessionWorkspace {
                host_alias: w.host_alias,
                project_id: w.project_id,
                worktree_id: Some(w.worktree_id),
                tmux_name: w.tmux_name,
                pane_cmd: w.pane_cmd,
                cwd: w.cwd_hint,
                base_branch: None,
            },
        )
        .await?;
        Ok(rep.cwd)
    }

    async fn start_target(
        &self,
        host: &str,
        tmux_name: &str,
        cwd: &str,
        pane_cmd: &str,
    ) -> Result<(), IpcError> {
        crate::service::sessions::exec_for(host, self.ssh)
            .new_session(tmux_name, std::path::Path::new(cwd), pane_cmd)
            .await
    }

    async fn refresh_host(&self, store: &Mutex<Store>, host: &str) -> Result<(), IpcError> {
        crate::service::sessions::reconcile_one_host(store, self.ssh, host).await
    }

    async fn kill_source(
        &self,
        store: &Mutex<Store>,
        host: &str,
        tmux_name: &str,
    ) -> Result<(), IpcError> {
        crate::service::sessions::kill_session(
            crate::service::sessions::KillSessionArgs {
                host_alias: host.to_string(),
                name: tmux_name.to_string(),
                force: false,
            },
            store,
            self.ssh,
        )
        .await
        .map(|_| ())
    }
}

/// Move a session (see the module docs).
pub async fn move_session(
    args: MoveSessionArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<MoveReport, IpcError> {
    let hooks = RealHooks { ssh };
    move_session_with(args, store, &**ssh, &hooks, MoveOptions::default()).await
}

// ── pure helpers ────────────────────────────────────────────────────────────

/// Claude Code's per-project directory name: every char outside
/// `[A-Za-z0-9]` becomes `-`. The target prep script applies the same rule
/// on the host with `sed` (the physical cwd is only known there); this is
/// the tested reference for that rule.
#[cfg(test)]
pub fn encode_project_dir(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// The size cap in bytes from the raw setting value.
pub fn max_transcript_bytes(raw: Option<&str>) -> u64 {
    let mb = raw
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&v| v > 0)
        .unwrap_or(DEFAULT_MAX_TRANSCRIPT_MB);
    mb.saturating_mul(1024 * 1024)
}

/// Keep whole JSONL lines only: a transcript read while Claude is writing
/// may end mid-line. Returns the input unchanged when it has no newline.
pub fn trim_to_last_newline(bytes: &mut Vec<u8>) {
    if let Some(pos) = bytes.iter().rposition(|&b| b == b'\n') {
        bytes.truncate(pos + 1);
    }
}

/// tmux name for the target: the source name, else `<name>-moved`,
/// `<name>-moved-2` … `-9`.
pub fn pick_target_name(source: &str, taken: &[String]) -> Result<String, IpcError> {
    let free = |n: &str| !taken.iter().any(|t| t == n);
    if free(source) {
        return Ok(source.to_string());
    }
    let base = format!("{source}-moved");
    if free(&base) {
        return Ok(base);
    }
    for i in 2..=9 {
        let n = format!("{base}-{i}");
        if free(&n) {
            return Ok(n);
        }
    }
    Err(IpcError::new(
        codes::E_INVALID_STATE,
        format!("no free tmux name for {source} on the target host"),
    ))
}

/// Git state of the source worktree.
#[derive(Debug, Clone)]
pub struct SourceState {
    pub worktree: String,
    pub dirty: Vec<DirtyFile>,
    pub head: String,
    pub current_branch: String,
    /// `refs/heads/<branch>` on origin (from `git ls-remote`).
    pub remote_sha: Option<String>,
    /// Commits in HEAD not on the origin branch; -1 when unknown.
    pub ahead: i64,
    /// An operation in progress (`merge`, `rebase`, `cherry-pick`, `revert`,
    /// `bisect`); its state is not carried, so the move is refused.
    pub midop: Option<String>,
}

/// Parse the inspect script's `\x1e`-separated output.
pub fn parse_inspection(stdout: &str) -> Result<SourceState, IpcError> {
    let parts: Vec<&str> = stdout.split('\x1e').collect();
    if parts.len() != 7 {
        return Err(IpcError::new(
            codes::E_PARSE,
            format!(
                "unexpected source inspection output ({} fields)",
                parts.len()
            ),
        ));
    }
    let remote = parts[4].trim();
    Ok(SourceState {
        worktree: parts[0].trim().to_string(),
        dirty: parse_porcelain(parts[1]),
        head: parts[2].trim().to_string(),
        current_branch: parts[3].trim().to_string(),
        remote_sha: (!remote.is_empty()).then(|| remote.to_string()),
        ahead: parts[5].trim().parse().unwrap_or(-1),
        midop: Some(parts[6].trim())
            .filter(|m| !m.is_empty())
            .map(str::to_string),
    })
}

/// The refusals that hold in every mode: an operation in progress, an
/// unreadable HEAD, a worktree on another branch. Dirty and unpushed work is
/// not refused here — the carry engine takes it along.
pub fn carry_verdict(state: &SourceState, branch: &str) -> Result<(), IpcError> {
    if let Some(op) = state.midop.as_deref() {
        return Err(IpcError::new(
            codes::E_MOVE_MIDOP,
            format!(
                "the source worktree {} is in the middle of a {op}; finish or abort it first — move_session does not carry an operation in progress",
                state.worktree
            ),
        )
        .with_details(serde_json::json!({ "operation": op })));
    }
    if state.head.is_empty() {
        return Err(IpcError::new(
            codes::E_GIT,
            format!("could not read HEAD in {}", state.worktree),
        ));
    }
    if state.current_branch != branch {
        return Err(IpcError::new(
            codes::E_INVALID_STATE,
            format!(
                "the source worktree is on {:?} but the session's branch is {branch:?}; check out {branch} first",
                state.current_branch
            ),
        ));
    }
    Ok(())
}

/// The strict-mode verdict: [`carry_verdict`] plus today's refusals of
/// uncommitted and unpushed work.
pub fn preflight_verdict(state: &SourceState, branch: &str) -> Result<(), IpcError> {
    carry_verdict(state, branch)?;
    if !state.dirty.is_empty() {
        let shown: Vec<String> = state
            .dirty
            .iter()
            .take(10)
            .map(|d| format!("{} {}", d.status.trim(), d.path))
            .collect();
        return Err(IpcError::new(
            codes::E_MOVE_DIRTY,
            format!(
                "the source worktree {} has {} uncommitted entr{} ({}); commit or discard them first — move_session never carries uncommitted work",
                state.worktree,
                state.dirty.len(),
                if state.dirty.len() == 1 { "y" } else { "ies" },
                shown.join(", ")
            ),
        )
        .with_details(serde_json::json!({ "dirty_files": state.dirty })));
    }
    if state.remote_sha.is_none() {
        return Err(IpcError::new(
            codes::E_MOVE_UNPUSHED,
            format!(
                "branch {branch} is not on origin; push it first (git push -u origin {branch}) — move_session never pushes"
            ),
        ));
    }
    if state.ahead != 0 {
        let what = if state.ahead < 0 {
            "could not be compared with origin".to_string()
        } else {
            format!("has {} commit(s) not on origin/{branch}", state.ahead)
        };
        return Err(IpcError::new(
            codes::E_MOVE_UNPUSHED,
            format!("branch {branch} {what}; push it first (git push origin {branch}) — move_session never pushes"),
        ));
    }
    Ok(())
}

/// Where the transcript will live on the target and what is there already.
#[derive(Debug, Clone, PartialEq)]
pub struct TargetPrep {
    pub head: String,
    pub encoded_dir: String,
    pub path: String,
    /// Size of an existing file at `path`; -1 when absent.
    pub existing: i64,
}

/// Parse and sanity-check the target prep script's output.
pub fn parse_target_prep(stdout: &str, claude_id: &str) -> Result<TargetPrep, IpcError> {
    let line = stdout.lines().last().unwrap_or("").trim_end();
    let parts: Vec<&str> = line.split('\t').collect();
    if parts.len() != 4 {
        return Err(IpcError::new(
            codes::E_PARSE,
            format!("unexpected target prep output: {line:?}"),
        ));
    }
    let prep = TargetPrep {
        head: parts[0].to_string(),
        encoded_dir: parts[1].to_string(),
        path: parts[2].to_string(),
        existing: parts[3].trim().parse().unwrap_or(-1),
    };
    let suffix = format!("/.claude/projects/{}/{claude_id}.jsonl", prep.encoded_dir);
    if !prep.path.starts_with('/')
        || !prep.path.ends_with(&suffix)
        || prep.encoded_dir.is_empty()
        || !prep
            .encoded_dir
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err(IpcError::new(
            codes::E_INVALID_STATE,
            format!("refusing unexpected target transcript path {:?}", prep.path),
        ));
    }
    if prep.encoded_dir.len() > MAX_ENCODED_DIR {
        return Err(IpcError::new(
            codes::E_INVALID_STATE,
            format!(
                "the target cwd encodes to a {}-char project directory; Claude Code shortens names over {MAX_ENCODED_DIR} chars with a hash, so --resume would not find the copied transcript",
                prep.encoded_dir.len()
            ),
        ));
    }
    Ok(prep)
}

// ── scripts (every interpolated value goes through `shell::quote`) ─────────

/// Source git state, `\x1e`-separated: worktree, porcelain, HEAD, current
/// branch, origin sha of `branch`, ahead count, mid-op name (or empty). Tries
/// the pane's cwd first, then `hint`. The porcelain goes through
/// [`carry::STATUS_PORCELAIN`]: it is compared with the target's after the
/// replay, so a difference between the two hosts' git configs must not be
/// able to fail a good move.
pub fn inspect_script(tmux_name: &str, hint: Option<&str>, branch: &str) -> String {
    format!(
        r#"# cf-move:inspect
set +e
name={name}
hint={hint}
br={br}
wt=''
if [ -n "$name" ]; then
  c=$(tmux display-message -p -t "=$name:" '#{{pane_current_path}}' 2>/dev/null)
  if [ -n "$c" ] && git -C "$c" rev-parse --git-dir >/dev/null 2>&1; then wt="$c"; fi
fi
if [ -z "$wt" ] && [ -n "$hint" ] && git -C "$hint" rev-parse --git-dir >/dev/null 2>&1; then wt="$hint"; fi
if [ -z "$wt" ]; then printf '{NO_WORKTREE}\n' >&2; exit 3; fi
top=$(git -C "$wt" rev-parse --show-toplevel 2>/dev/null)
if [ -n "$top" ]; then wt="$top"; fi
porcelain=$(git -C "$wt" {status} 2>/dev/null)
head=$(git -C "$wt" rev-parse HEAD 2>/dev/null)
cur=$(git -C "$wt" rev-parse --abbrev-ref HEAD 2>/dev/null)
rsha=$(git -C "$wt" ls-remote --heads origin "refs/heads/$br" 2>/dev/null | cut -f1 | head -n1)
ahead=-1
if [ -n "$rsha" ]; then
  git -C "$wt" cat-file -e "$rsha^{{commit}}" 2>/dev/null || git -C "$wt" fetch -q origin "refs/heads/$br" >/dev/null 2>&1
  ahead=$(git -C "$wt" rev-list --count "$rsha..HEAD" 2>/dev/null)
  if [ -z "$ahead" ]; then ahead=-1; fi
fi
gd=$(git -C "$wt" rev-parse --absolute-git-dir 2>/dev/null)
midop=''
if [ -n "$gd" ]; then
  if [ -d "$gd/rebase-merge" ] || [ -d "$gd/rebase-apply" ]; then midop=rebase
  elif [ -f "$gd/MERGE_HEAD" ]; then midop=merge
  elif [ -f "$gd/CHERRY_PICK_HEAD" ]; then midop=cherry-pick
  elif [ -f "$gd/REVERT_HEAD" ]; then midop=revert
  elif [ -f "$gd/BISECT_LOG" ]; then midop=bisect
  fi
fi
printf '%s\036%s\036%s\036%s\036%s\036%s\036%s' "$wt" "$porcelain" "$head" "$cur" "$rsha" "$ahead" "$midop"
"#,
        name = quote(tmux_name),
        hint = quote(hint.unwrap_or("")),
        br = quote(branch),
        status = carry::STATUS_PORCELAIN,
    )
}

/// Locate the source transcript: prints `<bytes>\t<mtime secs>\t<path>`
/// (mtime via GNU `stat -c %Y`, else BSD `stat -f %m`; empty if neither).
pub fn locate_script(stored_path: Option<&str>, claude_id: &str) -> String {
    format!(
        r#"# cf-move:locate
set +e
id={id}
sp={sp}
f=''
if [ -n "$sp" ] && [ -f "$sp" ]; then f="$sp"; fi
if [ -z "$f" ]; then
  for c in "$HOME"/.claude/projects/*/"$id".jsonl; do
    if [ -f "$c" ]; then f="$c"; break; fi
  done
fi
if [ -z "$f" ]; then printf '{NO_TRANSCRIPT} %s\n' "$id" >&2; exit 4; fi
n=$(wc -c < "$f" | tr -d ' ')
m=$(stat -c %Y -- "$f" 2>/dev/null || stat -f %m -- "$f" 2>/dev/null)
printf '%s\t%s\t%s\n' "$n" "$m" "$f"
"#,
        id = quote(claude_id),
        sp = quote(stored_path.unwrap_or("")),
    )
}

/// Where the source transcript is and the state the copy was taken at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Located {
    pub size: u64,
    /// Unix mtime; -1 when the host's `stat` gave nothing.
    pub mtime: i64,
    pub path: String,
}

/// Parse [`locate_script`] output.
pub fn parse_locate(stdout: &str) -> Result<Located, IpcError> {
    let line = stdout.trim();
    let mut parts = line.splitn(3, '\t');
    let size = parts.next().and_then(|n| n.trim().parse::<u64>().ok());
    let mtime = parts.next().map(|m| m.trim().parse::<i64>().unwrap_or(-1));
    let path = parts.next().filter(|p| p.starts_with('/'));
    match (size, mtime, path) {
        (Some(size), Some(mtime), Some(path)) => Ok(Located {
            size,
            mtime,
            path: path.to_string(),
        }),
        _ => Err(IpcError::new(
            codes::E_PARSE,
            format!("unexpected transcript locate output: {line:?}"),
        )),
    }
}

/// Refuse a source whose Claude may be mid-turn: the copy would miss the
/// rest of the turn and the two sessions would fork. Only a known idle
/// status (the `wait_for_session` idle set, plus `failed`) is accepted.
pub fn require_source_idle(status: Option<&str>) -> Result<(), IpcError> {
    match status {
        Some(s) if crate::store::IDLE_STATUSES.contains(&s) || s == "failed" => Ok(()),
        other => Err(IpcError::new(
            codes::E_INVALID_STATE,
            format!(
                "the source Claude is not idle (claude_status {}); moving now would lose the turn in progress — wait until it finishes, then retry",
                other
                    .map(|s| format!("{s:?}"))
                    .unwrap_or_else(|| "unknown".into())
            ),
        )),
    }
}

// ── in-flight guard ─────────────────────────────────────────────────────────

type InFlight = Mutex<std::collections::HashSet<(usize, i64)>>;

/// Sessions with a move in progress, keyed by (store address, session id):
/// one store per app, and the address keeps parallel tests on separate
/// in-memory stores from colliding.
fn moves_in_flight() -> &'static InFlight {
    static IN_FLIGHT: std::sync::OnceLock<InFlight> = std::sync::OnceLock::new();
    IN_FLIGHT.get_or_init(Default::default)
}

/// RAII claim on one session's move; released on drop (every exit path).
pub struct MoveClaim((usize, i64));

impl MoveClaim {
    /// `E_INVALID_STATE` when a move of `session_id` is already running
    /// (the UI and an MCP caller racing would otherwise both start one).
    pub fn acquire(store: &Mutex<Store>, session_id: i64) -> Result<Self, IpcError> {
        let key = (store as *const Mutex<Store> as usize, session_id);
        let mut set = moves_in_flight().lock().map_err(|_| IpcError::lock())?;
        if !set.insert(key) {
            return Err(IpcError::new(
                codes::E_INVALID_STATE,
                format!("a move of session {session_id} is already in progress"),
            ));
        }
        Ok(MoveClaim(key))
    }
}

impl Drop for MoveClaim {
    fn drop(&mut self) {
        if let Ok(mut set) = moves_in_flight().lock() {
            set.remove(&self.0);
        }
    }
}

/// Read at most `limit` bytes of the transcript.
pub fn read_script(path: &str, limit: u64) -> String {
    format!("# cf-move:read\nhead -c {limit} -- {}\n", quote(path))
}

/// Best-effort: refresh `origin/<branch>` in the target's main checkout so a
/// branch pushed after the host's last fetch is found by the create-only
/// workspace step.
pub fn prefetch_script(project_root: &str, branch: &str) -> String {
    format!(
        r#"# cf-move:prefetch
r={r}
br={br}
if [ -d "$r" ]; then git -C "$r" fetch -q origin "+refs/heads/$br:refs/remotes/origin/$br" >/dev/null 2>&1; fi
exit 0
"#,
        r = quote(project_root),
        br = quote(branch),
    )
}

/// In the target worktree: fast-forward to the source HEAD when behind
/// (refuse a divergence), then resolve the physical cwd, create the Claude
/// project dir and print `<head>\t<encoded dir>\t<transcript path>\t<existing
/// size or -1>`. A target that is merely BEHIND but has uncommitted changes
/// is not diverged — most often it is the copy this session left behind when
/// it moved away from that host — so it gets the apply's own
/// [`carry::TARGET_DIRTY`] sentinel and its truthful message, not
/// "reconcile the branch".
pub fn target_prep_script(cwd: &str, want_head: &str, branch: &str, claude_id: &str) -> String {
    format!(
        r#"# cf-move:prep
set +e
cwd={cwd}
want={want}
br={br}
id={id}
cd -- "$cwd" 2>/dev/null || {{ printf '{NO_CWD}\n' >&2; exit 3; }}
git fetch -q origin "+refs/heads/$br:refs/remotes/origin/$br" >/dev/null 2>&1
h=$(git rev-parse HEAD 2>/dev/null)
if [ "$h" != "$want" ]; then
  if git merge-base --is-ancestor "$want" HEAD 2>/dev/null; then :
  elif git merge-base --is-ancestor HEAD "$want" 2>/dev/null; then
    if [ -n "$(git {status} 2>/dev/null)" ]; then printf '{TARGET_DIRTY}\n' >&2; exit 9; fi
    git merge -q --ff-only "$want" >/dev/null 2>&1 || {{ printf '{DIVERGED} %s\n' "$h" >&2; exit 6; }}
  else
    printf '{DIVERGED} %s\n' "$h" >&2; exit 6
  fi
fi
h=$(git rev-parse HEAD 2>/dev/null)
p=$(pwd -P)
enc=$(printf '%s' "$p" | sed 's/[^A-Za-z0-9]/-/g')
d="$HOME/.claude/projects/$enc"
mkdir -p -- "$d" || exit 7
f="$d/$id.jsonl"
if [ -f "$f" ]; then n=$(wc -c < "$f" | tr -d ' '); else n=-1; fi
printf '%s\t%s\t%s\t%s\n' "$h" "$enc" "$f" "$n"
"#,
        cwd = quote(cwd),
        want = quote(want_head),
        br = quote(branch),
        id = quote(claude_id),
        status = carry::STATUS_PORCELAIN,
        TARGET_DIRTY = carry::TARGET_DIRTY,
    )
}

/// Size of the target transcript, or -1.
pub fn size_script(path: &str) -> String {
    format!(
        "# cf-move:size\nf={}\nif [ -f \"$f\" ]; then wc -c < \"$f\" | tr -d ' '; else echo -1; fi\n",
        quote(path)
    )
}

// ── transport ───────────────────────────────────────────────────────────────

/// Run `script` via `bash -lc` on `host` (a local spawn for `local`).
async fn sh(
    ssh: &dyn SshExec,
    host: &str,
    script: &str,
    timeout: Duration,
) -> Result<std::process::Output, IpcError> {
    crate::ssh::run_shell(ssh, host, script, timeout).await
}

/// Run a carry script with the long wall clock.
async fn sh_long(
    ssh: &dyn SshExec,
    host: &str,
    script: &str,
) -> Result<std::process::Output, IpcError> {
    crate::ssh::run_shell_bounded(ssh, host, script, GIT_TIMEOUT, CARRY_TIMEOUT).await
}

fn stderr_of(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).trim().to_string()
}

/// A private temp file removed on drop (the transcript is conversation
/// content: mode 0600, never left behind). Its name is unique per process
/// and per call: the pid, a process-wide counter and the clock. The counter
/// alone keeps concurrent calls apart. The clock is only microsecond-granular
/// on macOS, so two calls in the same tick used to collide on `create_new`
/// with `File exists (os error 17)`.
struct TempFile(std::path::PathBuf);

impl TempFile {
    /// An empty private file plus its open handle, for a payload written in
    /// pieces (see [`download`]).
    fn create(ext: &str) -> Result<(Self, std::fs::File), IpcError> {
        use std::os::unix::fs::OpenOptionsExt;
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "claude-fleet-move-{}-{seq}-{nanos}.{ext}",
            std::process::id()
        ));
        let f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        Ok((TempFile(path), f))
    }

    fn write(bytes: &[u8]) -> Result<Self, IpcError> {
        use std::io::Write;
        let (guard, mut f) = Self::create("jsonl")?;
        f.write_all(bytes)?;
        f.sync_all()?;
        Ok(guard)
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod temp_file_tests {
    use super::TempFile;
    use std::os::unix::fs::PermissionsExt;

    /// Writes started at once never collide on the name (the macOS clock is
    /// only microsecond-granular): each gets its own path, holds its own
    /// bytes, is private (0600), and is removed on drop.
    #[test]
    fn concurrent_writes_get_unique_private_files_removed_on_drop() {
        const N: usize = 32;
        let files: Vec<TempFile> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..N)
                .map(|i| {
                    scope.spawn(move || {
                        TempFile::write(format!("transcript {i}").as_bytes())
                            .expect("a concurrent write must not collide")
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        let paths: std::collections::HashSet<_> = files.iter().map(|f| f.0.clone()).collect();
        assert_eq!(paths.len(), N, "every write got its own path");
        // Joined in spawn order, so `files[i]` is the file thread `i` wrote:
        // asserting the exact body catches content crossing between them.
        for (i, f) in files.iter().enumerate() {
            let mode = std::fs::metadata(&f.0).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "{}", f.0.display());
            let body = std::fs::read_to_string(&f.0).unwrap();
            assert_eq!(body, format!("transcript {i}"), "{}", f.0.display());
        }
        drop(files);
        assert!(
            paths.iter().all(|p| !p.exists()),
            "every temp file is removed on drop"
        );
    }
}

/// Write the transcript to `path` on `host`.
async fn put(ssh: &dyn SshExec, host: &str, path: &str, bytes: &[u8]) -> Result<(), IpcError> {
    if host == LOCAL {
        crate::service::hub::ensure_local_allowed(host)?;
        return tokio::fs::write(path, bytes)
            .await
            .map_err(|e| IpcError::new(codes::E_UPLOAD, format!("write {path}: {e}")));
    }
    let tmp = TempFile::write(bytes)?;
    ssh.upload_file(host, &tmp.0, path, COPY_TIMEOUT).await
}

/// Pull `bytes` of `remote_path` on `host` into a private temp file, one
/// [`carry::CHUNK_BYTES`] at a time, so a payload is never held in memory.
/// Each chunk is read through [`carry::payload`]: a login-shell banner on
/// stdout is not part of the file.
async fn download(
    ssh: &dyn SshExec,
    host: &str,
    remote_path: &str,
    bytes: u64,
    ext: &str,
) -> Result<TempFile, IpcError> {
    download_chunked(ssh, host, remote_path, bytes, ext, carry::CHUNK_BYTES).await
}

/// [`download`] with the chunk size as a parameter, so the loop itself can be
/// tested against a payload that does not have to be megabytes long.
async fn download_chunked(
    ssh: &dyn SshExec,
    host: &str,
    remote_path: &str,
    bytes: u64,
    ext: &str,
    chunk_bytes: u64,
) -> Result<TempFile, IpcError> {
    use std::io::Write;
    let (guard, mut f) = TempFile::create(ext)?;
    let mut got = 0u64;
    while got < bytes {
        let want = chunk_bytes.min(bytes - got);
        let out = sh(
            ssh,
            host,
            &carry::chunk_script(remote_path, got, want),
            COPY_TIMEOUT,
        )
        .await?;
        let chunk = carry::payload(&out.stdout).unwrap_or_default();
        if !out.status.success() || chunk.is_empty() {
            return Err(carry_err(
                "download",
                &format!("reading {remote_path} on {host} stopped at {got} of {bytes} bytes"),
                &stderr_of(&out),
            ));
        }
        f.write_all(chunk)?;
        got += chunk.len() as u64;
    }
    f.sync_all()?;
    if got != bytes {
        return Err(carry_err(
            "download",
            &format!("{remote_path} on {host} gave {got} bytes, expected {bytes}"),
            "",
        ));
    }
    Ok(guard)
}

/// Put a local file at `path` on `host` (a plain copy when `host` is local).
async fn put_file(
    ssh: &dyn SshExec,
    host: &str,
    local: &std::path::Path,
    path: &str,
) -> Result<(), IpcError> {
    if host == LOCAL {
        crate::service::hub::ensure_local_allowed(host)?;
        return tokio::fs::copy(local, path)
            .await
            .map(|_| ())
            .map_err(|e| IpcError::new(codes::E_UPLOAD, format!("write {path}: {e}")));
    }
    ssh.upload_file(host, local, path, COPY_TIMEOUT).await
}

/// What the end-of-move cleanup must undo; filled in as the carry progresses.
#[derive(Default)]
struct CarryCleanup {
    id: String,
    /// (host, source worktree)
    source: Option<(String, String)>,
    /// (host, target project root)
    target: Option<(String, String)>,
}

impl CarryCleanup {
    /// Best effort on both hosts: a failure here never changes the move's
    /// result.
    async fn run(&self, ssh: &dyn SshExec) {
        for (host, dir) in self.source.iter().chain(self.target.iter()) {
            if let Err(e) = sh(
                ssh,
                host,
                &carry::cleanup_script(dir, &self.id),
                GIT_TIMEOUT,
            )
            .await
            {
                tracing::warn!(host = %host, error = %e.message, "[move_session] carry cleanup failed");
            }
        }
    }
}

type Ignored = (Vec<carry::IgnoredEntry>, Vec<carry::LeftBehind>);

/// List, select, pack, relay and extract the small git-ignored files. `Err`
/// carries what was left behind plus the reason, for a report warning.
#[allow(clippy::too_many_arguments)]
async fn carry_ignored(
    ssh: &dyn SshExec,
    src: &str,
    target: &str,
    worktree: &str,
    cwd: &str,
    target_dir: &str,
    id: &str,
    snap: &Snapshot,
) -> Result<Ignored, (Vec<carry::LeftBehind>, String)> {
    let fail = |left: &[carry::LeftBehind], why: String| (left.to_vec(), why);
    let out = sh(
        ssh,
        src,
        &carry::ignored_list_script(worktree),
        COPY_TIMEOUT,
    )
    .await
    .map_err(|e| fail(&[], e.message))?;
    if !out.status.success() {
        return Err(fail(
            &[],
            format!("listing on {src} failed: {}", stderr_of(&out)),
        ));
    }
    let sel = carry::select_ignored(
        carry::parse_ignored_list(&out.stdout),
        snap.ignored_entry_kb,
        snap.ignored_total_kb,
    );
    if sel.carry.is_empty() {
        return Ok((Vec::new(), sel.left));
    }
    let paths: Vec<String> = sel.carry.iter().map(|e| e.path.clone()).collect();
    let out = sh(
        ssh,
        src,
        &carry::ignored_pack_script(worktree, id, &paths),
        COPY_TIMEOUT,
    )
    .await
    .map_err(|e| fail(&sel.left, e.message))?;
    if !out.status.success() {
        return Err(fail(
            &sel.left,
            format!("packing on {src} failed: {}", stderr_of(&out)),
        ));
    }
    let (bytes, archive) = carry::parse_pack(&String::from_utf8_lossy(&out.stdout))
        .map_err(|e| fail(&sel.left, e.message))?;
    let local = download(ssh, src, &archive, bytes, "tgz")
        .await
        .map_err(|e| fail(&sel.left, e.message))?;
    let target_archive = format!("{target_dir}/ignored.tgz");
    put_file(ssh, target, &local.0, &target_archive)
        .await
        .map_err(|e| fail(&sel.left, e.message))?;
    let out = sh(
        ssh,
        target,
        &carry::ignored_extract_script(cwd, &target_archive),
        COPY_TIMEOUT,
    )
    .await
    .map_err(|e| fail(&sel.left, e.message))?;
    if !out.status.success() {
        return Err(fail(
            &sel.left,
            format!("extracting on {target} failed: {}", stderr_of(&out)),
        ));
    }
    Ok((sel.carry, sel.left))
}

/// Pull `archive` (`bytes` long) off `src` and put it at `to` on `target`.
async fn relay(
    ssh: &dyn SshExec,
    src: &str,
    archive: &str,
    bytes: u64,
    target: &str,
    to: &str,
) -> Result<(), String> {
    let local = download(ssh, src, archive, bytes, "tgz")
        .await
        .map_err(|e| e.message)?;
    put_file(ssh, target, &local.0, to)
        .await
        .map_err(|e| e.message)
}

/// Run a script whose failure is only ever a warning: `Err(why)`.
async fn sh_soft(
    ssh: &dyn SshExec,
    host: &str,
    script: &str,
    what: &str,
) -> Result<std::process::Output, String> {
    let out = sh(ssh, host, script, COPY_TIMEOUT)
        .await
        .map_err(|e| format!("{what} on {host}: {}", e.message))?;
    if out.status.success() {
        Ok(out)
    } else {
        Err(format!("{what} on {host} failed: {}", stderr_of(&out)))
    }
}

/// List, select, pack, relay and merge the session's own Claude directory
/// (`<project dir>/<id>/`: subagent transcripts, tool results, the title).
/// Every `Err` carries the best report built so far — `left_behind` above
/// all — plus the reason, for a report warning: this half never fails a move.
#[allow(clippy::too_many_arguments)]
async fn carry_session_state(
    ssh: &dyn SshExec,
    src: &str,
    target: &str,
    src_project_dir: &str,
    tgt_project_dir: &str,
    target_dir: &str,
    id: &str,
    cap: u64,
) -> Result<carry::SessionStateReport, (carry::SessionStateReport, String)> {
    let so_far = |left: &[carry::LeftBehind]| carry::SessionStateReport {
        left_behind: left.to_vec(),
        ..Default::default()
    };
    // The listing streams, so its own `find` can fail once records are
    // already flowing (see `session_list_script`): a non-zero exit means the
    // listing is not to be trusted, whatever reached stdout before it.
    let out = sh_soft(
        ssh,
        src,
        &claude_state::session_list_script(src_project_dir, id),
        "listing the session directory",
    )
    .await
    .map_err(|why| (so_far(&[]), why))?;
    // An exit-0 reply without the marker is NOT "no session directory": the
    // script prints the marker on every successful path, including the one
    // where `<id>/` does not exist, so a missing marker means something
    // swallowed the output and the listing cannot be trusted. Warn instead
    // of silently carrying nothing.
    if carry::payload(&out.stdout).is_none() {
        return Err((so_far(&[]), format!("the listing on {src} said nothing")));
    }
    let listed = claude_state::parse_file_list(&out.stdout);
    if listed.is_empty() {
        // No such directory, or an empty one: nothing to do, nothing to say.
        return Ok(carry::SessionStateReport::default());
    }
    let sel = claude_state::select_session_files(listed, cap);
    if let Some(why) = sel.skip {
        return Err((so_far(&sel.left), why));
    }
    if sel.carry.is_empty() {
        return Ok(so_far(&sel.left));
    }
    let out = sh_soft(
        ssh,
        src,
        &claude_state::session_pack_script(src_project_dir, id, &sel.exclude),
        "packing the session directory",
    )
    .await
    .map_err(|why| (so_far(&sel.left), why))?;
    let (bytes, archive) = carry::parse_pack(&String::from_utf8_lossy(&out.stdout))
        .map_err(|e| (so_far(&sel.left), e.message))?;
    // The orchestrator does not take the source's word for the size of what
    // it is about to relay — the same rule the bundle follows. The cap
    // bounds the CONTENT, so the archive may exceed it only by tar's own
    // overhead; anything more and nothing is downloaded.
    let allowed = cap.saturating_add(claude_state::PACK_OVERHEAD_ALLOWANCE_BYTES);
    if bytes > allowed {
        return Err((
            so_far(&sel.left),
            format!(
                "{src} announced a {bytes}-byte archive, over the {allowed}-byte bound; nothing was relayed"
            ),
        ));
    }
    let staged = format!("{target_dir}/state.tgz");
    relay(ssh, src, &archive, bytes, target, &staged)
        .await
        .map_err(|why| (so_far(&sel.left), why))?;
    let out = sh_soft(
        ssh,
        target,
        &claude_state::session_merge_script(tgt_project_dir, id, &staged),
        "merging the session directory",
    )
    .await
    .map_err(|why| (so_far(&sel.left), why))?;
    let merged =
        claude_state::parse_merge(&String::from_utf8_lossy(&out.stdout)).ok_or_else(|| {
            (
                so_far(&sel.left),
                format!("the merge on {target} said nothing"),
            )
        })?;
    // What the policy SELECTED, what the pack actually archived and what the
    // merge reports are three different sets: the pack takes the whole
    // `./<id>` minus the excludes, the merge refuses names outside the safe
    // charset, a file can vanish between the two, an `[!/]` exclude built
    // for one odd name can collaterally match a safe sibling, and an archive
    // without `./<id>` makes the merge's `cd` fail and report nothing at
    // all. Reconcile at the point of effect: every selected path must come
    // back accounted for, or the half warns — with whatever DID land still
    // in the report.
    let missing: Vec<String> = {
        let accounted: std::collections::HashSet<&str> = merged
            .carried
            .iter()
            .map(|e| e.path.as_str())
            .chain(merged.kept.iter().map(String::as_str))
            .chain(merged.failed.iter().map(String::as_str))
            .collect();
        sel.carry
            .iter()
            .map(|e| e.path.as_str())
            .filter(|p| !accounted.contains(p))
            .map(str::to_string)
            .collect()
    };
    let failed = merged.failed;
    let report = carry::SessionStateReport {
        carried: merged.carried,
        kept_target: merged.kept,
        left_behind: sel.left,
    };
    if !missing.is_empty() {
        let first: Vec<&str> = missing.iter().take(3).map(String::as_str).collect();
        let why = format!(
            "{} selected file(s) were not accounted for by the merge on {target}: {}",
            missing.len(),
            first.join(", ")
        );
        return Err((report, why));
    }
    if !failed.is_empty() {
        let first: Vec<&str> = failed.iter().take(3).map(String::as_str).collect();
        let why = format!(
            "{} file(s) could not be placed on {target}: {}",
            failed.len(),
            first.join(", ")
        );
        return Err((report, why));
    }
    Ok(report)
}

/// List both sides' memory, decide what the target lacks, carry it and merge
/// the index lines that describe it. Like [`carry_session_state`], every
/// `Err` carries the report built so far: this half never fails a move
/// either, and files already extracted on the target stay reported as
/// carried even when the index step goes on to fail.
async fn carry_memory(
    ssh: &dyn SshExec,
    src: &str,
    target: &str,
    src_worktree: &str,
    tgt_project_root: &str,
    target_dir: &str,
    id: &str,
) -> Result<carry::MemoryReport, (carry::MemoryReport, String)> {
    let nothing = carry::MemoryReport::default;
    // The source is keyed by the worktree's repo root, with the worktree
    // itself as the fallback; the target by its project root.
    let out = sh_soft(
        ssh,
        src,
        &claude_state::memory_list_script(src_worktree, Some(src_worktree)),
        "listing the memory",
    )
    .await
    .map_err(|why| (nothing(), why))?;
    let source = claude_state::parse_memory_list(&out.stdout).ok_or_else(|| {
        (
            nothing(),
            format!("the memory listing on {src} was unreadable"),
        )
    })?;
    if !source.exists || source.files.is_empty() {
        return Ok(nothing());
    }
    let out = sh_soft(
        ssh,
        target,
        &claude_state::memory_list_script(tgt_project_root, None),
        "listing the memory",
    )
    .await
    .map_err(|why| (nothing(), why))?;
    let tgt = claude_state::parse_memory_list(&out.stdout).ok_or_else(|| {
        (
            nothing(),
            format!("the memory listing on {target} was unreadable"),
        )
    })?;
    let decided = claude_state::decide_memory(&source.files, &tgt.files);
    let mut report = carry::MemoryReport {
        kept_target: decided.kept_target,
        identical: decided.identical,
        left_behind: decided.left,
        ..Default::default()
    };
    if decided.carry.is_empty() {
        return Ok(report);
    }
    let names: Vec<String> = decided.carry.iter().map(|e| e.path.clone()).collect();
    let out = sh_soft(
        ssh,
        src,
        &carry::pack_script(&source.dir, id, claude_state::MEMORY_ARCHIVE, &names),
        "packing the memory",
    )
    .await
    .map_err(|why| (report.clone(), why))?;
    let (bytes, archive) = carry::parse_pack(&String::from_utf8_lossy(&out.stdout))
        .map_err(|e| (report.clone(), e.message))?;
    // As in the session half: the announced size is re-checked here, before
    // a byte is downloaded. The content is bounded by
    // `MEMORY_TOTAL_MAX_BYTES`, so only tar's overhead may exceed it.
    let allowed = claude_state::MEMORY_TOTAL_MAX_BYTES
        .saturating_add(claude_state::PACK_OVERHEAD_ALLOWANCE_BYTES);
    if bytes > allowed {
        return Err((
            report.clone(),
            format!(
                "{src} announced a {bytes}-byte archive, over the {allowed}-byte bound; nothing was relayed"
            ),
        ));
    }
    let staged = format!("{target_dir}/{}", claude_state::MEMORY_ARCHIVE);
    relay(ssh, src, &archive, bytes, target, &staged)
        .await
        .map_err(|why| (report.clone(), why))?;
    sh_soft(
        ssh,
        target,
        &claude_state::memory_extract_script(&tgt.dir, &staged),
        "extracting the memory",
    )
    .await
    .map_err(|why| (report.clone(), why))?;
    // The files are on the target from here on: whatever the index does, they
    // travelled.
    report.carried = decided.carry;

    let index = |out: &std::process::Output, host: &str| -> Result<Option<String>, String> {
        match claude_state::parse_index(&String::from_utf8_lossy(&out.stdout)) {
            None => Err(format!(
                "the files travelled, but the index on {host} was unreadable and was left alone"
            )),
            Some(Some(text)) if text.len() as u64 > claude_state::INDEX_READ_MAX_BYTES => {
                Err(format!(
                    "the files travelled, but the index on {host} is over {} KiB and was left alone",
                    claude_state::INDEX_READ_MAX_BYTES / 1024
                ))
            }
            Some(text) => Ok(text),
        }
    };
    let out = sh_soft(
        ssh,
        src,
        &claude_state::memory_read_index_script(&source.dir),
        "reading the index",
    )
    .await
    .map_err(|why| (report.clone(), why))?;
    let Some(source_index) = index(&out, src).map_err(|why| (report.clone(), why))? else {
        // No index on the source: the files travel without one.
        return Ok(report);
    };
    let out = sh_soft(
        ssh,
        target,
        &claude_state::memory_read_index_script(&tgt.dir),
        "reading the index",
    )
    .await
    .map_err(|why| (report.clone(), why))?;
    let target_index = index(&out, target).map_err(|why| (report.clone(), why))?;
    let merged = claude_state::merge_index(&source_index, target_index.as_deref(), &names);
    if merged.lines == 0 {
        return Ok(report);
    }
    sh_soft(
        ssh,
        target,
        &claude_state::memory_append_index_script(&tgt.dir, &merged.append),
        "appending to the index",
    )
    .await
    .map_err(|why| (report.clone(), why))?;
    report.index_lines_added = merged.lines;
    Ok(report)
}

// ── the move ────────────────────────────────────────────────────────────────

/// Everything the move reads from the store, taken under one lock.
struct Snapshot {
    row: SessionRow,
    claude_id: String,
    branch: String,
    project_id: i64,
    worktree_id: i64,
    worktree_name: String,
    worktree_path: Option<String>,
    owner: String,
    repo: String,
    project_base: Option<String>,
    stored_transcript: Option<String>,
    cap: u64,
    bundle_cap: u64,
    ignored_entry_kb: u64,
    ignored_total_kb: u64,
    session_state_cap: u64,
    target_projects_root: String,
    layout: crate::projects::Layout,
    target_taken: Vec<String>,
    /// `(project root, worktree dir)` by the local projects root and layout:
    /// the fallback when a `local` target has no project / worktree row path.
    local_layout_paths: (String, String),
}

fn snapshot(s: &Store, args: &MoveSessionArgs) -> Result<Snapshot, IpcError> {
    let row = s
        .get_session_by_id(args.session_id)?
        .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, "session not found"))?;
    let target = args.target_host_alias.as_str();
    if row.host_alias == target {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("session {} already runs on {target}", row.id),
        ));
    }
    if row.kind != "work" {
        return Err(IpcError::new(
            codes::E_INVALID_STATE,
            format!(
                "only work sessions can move (this one is {:?}); a shell or background session has no conversation to carry",
                row.kind
            ),
        ));
    }
    let claude_id = row
        .claude_session_id
        .clone()
        .filter(|id| crate::validate::claude_session_id(id).is_ok())
        .ok_or_else(|| {
            IpcError::new(
                codes::E_INVALID_STATE,
                "the session has no Claude session id to resume on the target",
            )
        })?;
    let project_id = row.project_id.ok_or_else(|| {
        IpcError::new(
            codes::E_NOREPO,
            "the session has no project; nothing to move",
        )
    })?;
    let worktree_id = row.worktree_id.ok_or_else(|| {
        IpcError::new(
            codes::E_NO_WORKTREE,
            "the session has no worktree; only worktree sessions can move",
        )
    })?;
    let wt = s
        .get_worktree_row(worktree_id)?
        .ok_or_else(|| IpcError::new(codes::E_NO_WORKTREE, "the session's worktree row is gone"))?;
    if wt.name == "main" {
        return Err(IpcError::new(
            codes::E_NO_WORKTREE,
            "the session runs in the main checkout; only worktree sessions can move",
        ));
    }
    let branch = wt.branch.clone().unwrap_or_else(|| wt.name.clone());
    crate::validate::git_ref(&branch)?;
    crate::validate::path_component("worktree name", &wt.name)?;

    for (alias, role) in [(row.host_alias.as_str(), "source"), (target, "target")] {
        let host = s.get_host_row(alias)?.ok_or_else(|| {
            IpcError::new(codes::E_NOTFOUND, format!("{role} host {alias} not found"))
        })?;
        if !host.reachable {
            return Err(IpcError::new(
                codes::E_HOST_OFFLINE,
                format!("{role} host {alias} is not reachable"),
            ));
        }
        if role == "target" && !host.provisioned && alias != LOCAL {
            return Err(IpcError::new(
                codes::E_INVALID_STATE,
                format!("target host {alias} is not provisioned; run provision_hosts first"),
            ));
        }
    }
    if !args.keep_source {
        crate::service::sessions::guard_not_controller(
            s.get_controller()?.as_ref(),
            &row.host_alias,
            &row.tmux_name,
            false,
        )?;
    }
    let (owner, repo) = crate::service::sessions::fetch_owner_repo(s, project_id)?;
    crate::validate::path_component("owner", &owner)?;
    crate::validate::path_component("repo", &repo)?;
    let local_layout_paths = crate::service::sessions::remote_project_path(
        &crate::service::projects::local_projects_root(s).to_string_lossy(),
        crate::service::projects::layout(s),
        &owner,
        &repo,
        Some(&wt.name),
    );
    let stored_transcript = s
        .session_transcript_path(row.id)?
        .filter(|p| p.ends_with(&format!("/{claude_id}.jsonl")));
    let cap = max_transcript_bytes(Some(&crate::service::settings::get_string(
        s,
        SETTING_MAX_TRANSCRIPT_MB,
    )));
    let setting = |key: &str, default: u64| {
        crate::service::settings::get_string(s, key)
            .parse::<u64>()
            .ok()
            .filter(|n| *n > 0)
            .unwrap_or(default)
    };
    let bundle_cap = setting(carry::SETTING_MAX_BUNDLE_MB, carry::DEFAULT_MAX_BUNDLE_MB)
        .saturating_mul(1024 * 1024);
    let ignored_entry_kb = setting(
        carry::SETTING_IGNORED_ENTRY_KB,
        carry::DEFAULT_IGNORED_ENTRY_KB,
    );
    let ignored_total_kb = setting(
        carry::SETTING_IGNORED_TOTAL_MB,
        carry::DEFAULT_IGNORED_TOTAL_MB,
    )
    .saturating_mul(1024);
    let session_state_cap = setting(
        claude_state::SETTING_MAX_SESSION_STATE_MB,
        claude_state::DEFAULT_MAX_SESSION_STATE_MB,
    )
    .saturating_mul(1024 * 1024);
    let target_taken = s
        .list_sessions_for_host(target)?
        .into_iter()
        .map(|r| r.tmux_name)
        .collect();
    Ok(Snapshot {
        claude_id,
        branch,
        project_id,
        worktree_id,
        worktree_name: wt.name,
        worktree_path: s.worktree_path(worktree_id)?,
        owner,
        repo,
        project_base: s.project_base_path(project_id)?,
        stored_transcript,
        cap,
        bundle_cap,
        ignored_entry_kb,
        ignored_total_kb,
        session_state_cap,
        target_projects_root: crate::service::projects::project_base_for(s, target),
        layout: crate::service::projects::layout(s),
        target_taken,
        local_layout_paths,
        row,
    })
}

fn too_large(bytes: u64, cap: u64) -> IpcError {
    IpcError::new(
        codes::E_MOVE_TOO_LARGE,
        format!(
            "the transcript is {bytes} bytes, over the {} MiB cap ({SETTING_MAX_TRANSCRIPT_MB}); raise the setting or /compact the session first",
            cap / (1024 * 1024)
        ),
    )
    .with_details(
        serde_json::json!({ "bytes": bytes, "cap_bytes": cap, "payload": "transcript" }),
    )
}

fn bundle_too_large(bytes: u64, cap: u64) -> IpcError {
    IpcError::new(
        codes::E_MOVE_TOO_LARGE,
        format!(
            "the git bundle is {bytes} bytes, over the {} MiB cap ({}); push the branch first or raise the setting",
            cap / (1024 * 1024),
            carry::SETTING_MAX_BUNDLE_MB
        ),
    )
    .with_details(serde_json::json!({ "bytes": bytes, "cap_bytes": cap, "payload": "bundle" }))
}

/// The target worktree holds uncommitted changes. Raised from two places —
/// the prep step (behind the source HEAD and dirty) and the apply — with
/// one message, because from the user's side it is one situation. The third
/// case is the common one after this branch: a move leaves its work in the
/// source worktree, so moving the session BACK finds that copy waiting.
fn target_dirty(cwd: &str, target: &str) -> IpcError {
    IpcError::new(
        codes::E_MOVE_TARGET_DIRTY,
        format!(
            "move_session: the target worktree {cwd} on {target} has uncommitted changes — its own, work carried by an earlier move attempt that did not finish, or the copy left behind when this session was moved away from this host; the move never overwrites them, so inspect them there and commit or discard them before retrying (the source session was not touched)"
        ),
    )
}

/// A carry step failed before the target started.
fn carry_err(step: &str, what: &str, stderr: &str) -> IpcError {
    carry_err_cause(step, what, stderr, None)
}

/// [`carry_err`] keeping the code of an underlying failure in
/// `details.cause_code`.
fn carry_err_cause(step: &str, what: &str, stderr: &str, cause: Option<&str>) -> IpcError {
    IpcError::new(
        codes::E_MOVE_CARRY,
        format!(
            "move_session: carrying the work failed at {step}: {what} (the source session was not touched)"
        ),
    )
    .with_details(
        serde_json::json!({ "step": step, "stderr": stderr, "cause_code": cause }),
    )
}

/// The transport itself failed on a carry step (`E_SSH`, `E_SSH_TIMEOUT`).
/// That is still a carry failure — the caller needs `details.step` and the
/// promise that the source is untouched — so the original code travels in
/// the message and in `details.cause_code`, where a timeout stays tellable
/// from a parse failure.
fn carry_transport(step: &str, e: IpcError) -> IpcError {
    carry_err_cause(
        step,
        &format!("{}: {}", e.code, e.message),
        "",
        Some(&e.code),
    )
}

fn partial(step: &str, target: &str, name: &str, target_id: Option<i64>, e: &IpcError) -> IpcError {
    IpcError::new(
        codes::E_MOVE_PARTIAL,
        format!(
            "move_session stopped after the target session {name} was started on {target}: {step} failed ({}: {}). Both sessions were left as they are; the source is untouched — check the target, then kill whichever one you do not want",
            e.code, e.message
        ),
    )
    .with_details(serde_json::json!({
        "step": step,
        "target_host": target,
        "target_tmux_name": name,
        "target_session_id": target_id,
        "cause_code": e.code,
    }))
}

/// Prefix a pre-target error with where it happened; the code is kept.
fn before_target(step: &str, e: IpcError) -> IpcError {
    let details = e.details.clone();
    let mut out = IpcError::new(
        &e.code,
        format!(
            "move_session: {step}: {} (the source session was not touched)",
            e.message
        ),
    );
    out.details = details;
    out
}

/// Timeline event recorded instead of [`EVENT_MOVED`] when a move stops after
/// the target started (`E_MOVE_PARTIAL`): on the source, and on the target
/// row when it was registered. Carries the failing step.
pub const EVENT_MOVE_PARTIAL: &str = "session_move_partial";

/// Run [`locate_script`] on `host` and parse it (the post-copy checks).
async fn locate_on(
    ssh: &dyn SshExec,
    host: &str,
    stored_path: Option<&str>,
    claude_id: &str,
) -> Result<Located, IpcError> {
    let out = sh(
        ssh,
        host,
        &locate_script(stored_path, claude_id),
        GIT_TIMEOUT,
    )
    .await?;
    if !out.status.success() {
        return Err(IpcError::new(
            codes::E_SHELL,
            format!(
                "re-locating the source transcript failed: {}",
                stderr_of(&out)
            ),
        ));
    }
    parse_locate(&String::from_utf8_lossy(&out.stdout))
}

/// Record [`EVENT_MOVE_PARTIAL`] for a partial move. Best effort.
fn record_partial(store: &Mutex<Store>, source_id: i64, to_host: &str, e: &IpcError) {
    let d = e.details.clone().unwrap_or_default();
    let target_id = d["target_session_id"].as_i64();
    let detail = serde_json::json!({
        "step": d["step"],
        "to_host": to_host,
        "from_session_id": source_id,
        "to_session_id": target_id,
        "cause_code": d["cause_code"],
    })
    .to_string();
    let Ok(s) = store.lock() else { return };
    for sid in std::iter::once(source_id).chain(target_id) {
        if let Err(err) = s.insert_session_event(sid, EVENT_MOVE_PARTIAL, Some(&detail)) {
            tracing::warn!(
                kind = EVENT_MOVE_PARTIAL,
                session_id = sid,
                error = %err,
                "[event] insert failed"
            );
        }
    }
}

/// [`move_session`] over any transport and hooks. A partial move
/// (`E_MOVE_PARTIAL`) is recorded on the timeline as [`EVENT_MOVE_PARTIAL`].
pub async fn move_session_with(
    args: MoveSessionArgs,
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    hooks: &dyn MoveHooks,
    opts: MoveOptions,
) -> Result<MoveReport, IpcError> {
    let source_id = args.session_id;
    let to_host = args.target_host_alias.clone();
    move_session_steps(args, store, ssh, hooks, opts)
        .await
        .inspect_err(|e| {
            if e.code == codes::E_MOVE_PARTIAL {
                record_partial(store, source_id, &to_host, e);
            }
        })
}

/// The move's steps, with the carry cleanup always run afterwards — on
/// success and on every failure path once the carry began.
async fn move_session_steps(
    args: MoveSessionArgs,
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    hooks: &dyn MoveHooks,
    opts: MoveOptions,
) -> Result<MoveReport, IpcError> {
    let mut cleanup = CarryCleanup::default();
    let result = move_session_inner(args, store, ssh, hooks, opts, &mut cleanup).await;
    cleanup.run(ssh).await;
    result
}

/// The move's steps (see the module docs).
async fn move_session_inner(
    args: MoveSessionArgs,
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    hooks: &dyn MoveHooks,
    opts: MoveOptions,
    cleanup: &mut CarryCleanup,
) -> Result<MoveReport, IpcError> {
    crate::validate::host_alias(&args.target_host_alias)?;
    let _claim = MoveClaim::acquire(store, args.session_id)?;
    let snap = {
        let s = lock(store)?;
        snapshot(&s, &args)?
    };
    let src = snap.row.host_alias.clone();
    // The source row is looked up by id, not validated as an alias: a `local`
    // row on a hub without a local host is refused here, before any step.
    crate::service::hub::ensure_local_allowed(&src)?;
    let target = args.target_host_alias.clone();
    let id = snap.claude_id.clone();
    let mut warnings: Vec<String> = Vec::new();

    // 0. The source Claude must be idle NOW: reconcile its host so the status
    //    is fresh, then refuse a turn in progress or an unknown status.
    hooks.refresh_host(store, &src).await?;
    let status = {
        let s = lock(store)?;
        s.get_session_by_id(snap.row.id)?
            .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, "session not found"))?
            .claude_status
    };
    require_source_idle(status.as_deref())?;

    // 1. Source git state: on its branch, nothing in progress — and what the
    //    carry has to take along (strict: today's clean + pushed refusals).
    let hint = hooks.source_cwd_hint(store, &snap.row).await;
    let out = sh(
        ssh,
        &src,
        &inspect_script(&snap.row.tmux_name, hint.as_deref(), &snap.branch),
        GIT_TIMEOUT,
    )
    .await?;
    if !out.status.success() {
        let err = stderr_of(&out);
        let msg = if err.contains(NO_WORKTREE) {
            format!(
                "could not locate the source worktree on {src} (no live pane cwd, hint {hint:?})"
            )
        } else {
            format!("source inspection failed on {src}: {err}")
        };
        return Err(IpcError::new(codes::E_GIT, msg));
    }
    let state = parse_inspection(&String::from_utf8_lossy(&out.stdout))?;
    if args.strict {
        preflight_verdict(&state, &snap.branch)?;
    } else {
        carry_verdict(&state, &snap.branch)?;
    }
    let mut carried = carry::CarryReport {
        dirty_entries: state.dirty.clone(),
        ..Default::default()
    };

    // 2. Transcript: locate, cap, read (whole lines only).
    let out = sh(
        ssh,
        &src,
        &locate_script(snap.stored_transcript.as_deref(), &id),
        GIT_TIMEOUT,
    )
    .await?;
    if !out.status.success() {
        let err = stderr_of(&out);
        let code = if err.contains(NO_TRANSCRIPT) {
            codes::E_NO_TRANSCRIPT
        } else {
            codes::E_SHELL
        };
        return Err(IpcError::new(
            code,
            format!("no transcript to move for claude session {id} on {src}: {err}"),
        ));
    }
    let located = parse_locate(&String::from_utf8_lossy(&out.stdout))?;
    if located.size > snap.cap {
        return Err(too_large(located.size, snap.cap));
    }
    // Exactly the located bytes: the end-of-move check compares against
    // this size / mtime, so the copy and the check describe the same file.
    let out = sh(
        ssh,
        &src,
        &read_script(&located.path, located.size),
        COPY_TIMEOUT,
    )
    .await?;
    if !out.status.success() {
        return Err(IpcError::new(
            codes::E_SHELL,
            format!(
                "reading the transcript on {src} failed: {}",
                stderr_of(&out)
            ),
        ));
    }
    let mut bytes = out.stdout;
    if bytes.len() as u64 > snap.cap {
        return Err(too_large(bytes.len() as u64, snap.cap));
    }
    trim_to_last_newline(&mut bytes);
    if bytes.is_empty() {
        return Err(IpcError::new(
            codes::E_NO_TRANSCRIPT,
            format!("the transcript for claude session {id} on {src} is empty"),
        ));
    }
    let copied = bytes.len() as u64;

    // 3. Target workspace: refresh origin/<branch>, create/repair the
    //    worktree, fast-forward to the source HEAD, resolve the transcript path.
    let (project_root, cwd_hint) = if target == LOCAL {
        // The rows' own paths when present, else the layout-derived ones.
        let (layout_root, layout_cwd) = snap.local_layout_paths.clone();
        (
            snap.project_base.clone().unwrap_or(layout_root),
            snap.worktree_path.clone().unwrap_or(layout_cwd),
        )
    } else {
        let home = ssh
            .remote_home(&target)
            .await
            .map_err(|e| before_target("resolving the target $HOME", e))?;
        let root = crate::service::projects::expand_home(&snap.target_projects_root, &home);
        crate::service::sessions::remote_project_path(
            &root,
            snap.layout,
            &snap.owner,
            &snap.repo,
            Some(&snap.worktree_name),
        )
    };
    // 3a. Seed: the target needs a main clone before anything can be fetched
    //     into it. Never prompts; falls back to `git init` without origin.
    let clone_url = crate::repo_url::clone_url_for(&snap.owner, &snap.repo);
    let out = sh_long(ssh, &target, &carry::seed_script(&project_root, &clone_url))
        .await
        .map_err(|e| carry_transport("seed", e))?;
    if !out.status.success() {
        return Err(carry_err(
            "seed",
            &format!("preparing the clone at {project_root} on {target}"),
            &stderr_of(&out),
        ));
    }
    carried.target_seeded =
        carry::parse_seed(&String::from_utf8_lossy(&out.stdout)).map_err(|e| {
            carry_err(
                "seed",
                &format!("reading the seed output from {target}: {}", e.message),
                "",
            )
        })?;

    // Best effort: a failed fetch surfaces as the workspace step's error.
    let _ = sh(
        ssh,
        &target,
        &prefetch_script(&project_root, &snap.branch),
        GIT_TIMEOUT,
    )
    .await;

    // 3b. Carry the git state: snapshot + thin bundle on the source, relayed
    //     through this process, fetched into the target's main clone.
    //     `haves_script` creates the target's transfer dir, so the cleanup
    //     must already know about it when the call goes out: a transport
    //     failure after the remote `mkdir` would otherwise strand it.
    cleanup.id = id.clone();
    cleanup.target = Some((target.clone(), project_root.clone()));
    let out = sh(
        ssh,
        &target,
        &carry::haves_script(&project_root, &id, &snap.branch),
        GIT_TIMEOUT,
    )
    .await
    .map_err(|e| carry_transport("haves", e))?;
    if !out.status.success() {
        return Err(carry_err(
            "haves",
            &format!("listing refs in {project_root} on {target}"),
            &stderr_of(&out),
        ));
    }
    let (target_dir, haves) =
        carry::parse_haves(&String::from_utf8_lossy(&out.stdout)).map_err(|e| {
            carry_err(
                "haves",
                &format!("reading the ref listing from {target}: {}", e.message),
                "",
            )
        })?;
    // The snapshot is the first write on the source.
    cleanup.source = Some((src.clone(), state.worktree.clone()));

    let out = sh_long(
        ssh,
        &src,
        &carry::snapshot_script(&state.worktree, &id, &haves, snap.bundle_cap),
    )
    .await
    .map_err(|e| carry_transport("snapshot", e))?;
    if !out.status.success() {
        let err = stderr_of(&out);
        if let Some(rest) = err.split(carry::BUNDLE_TOO_LARGE).nth(1) {
            let bytes = rest
                .split_whitespace()
                .next()
                .and_then(|n| n.parse().ok())
                .unwrap_or(0);
            return Err(bundle_too_large(bytes, snap.bundle_cap));
        }
        return Err(carry_err(
            "snapshot",
            &format!("snapshotting {} on {src}", state.worktree),
            &err,
        ));
    }
    let bundle = carry::parse_snapshot(&String::from_utf8_lossy(&out.stdout)).map_err(|e| {
        carry_err(
            "snapshot",
            &format!("reading the snapshot output from {src}: {}", e.message),
            "",
        )
    })?;
    // The source script enforces the cap too; this is the orchestrator not
    // taking a host's word for the size of what it is about to relay.
    if bundle.bytes > snap.bundle_cap {
        return Err(bundle_too_large(bundle.bytes, snap.bundle_cap));
    }
    carried.commits = bundle.commits;
    carried.bundle_bytes = bundle.bytes;
    if bundle.submodules {
        warnings.push("the repository has submodules; their contents were not carried".into());
    }
    if bundle.lfs {
        warnings.push("the repository uses Git LFS; LFS objects were not carried".into());
    }
    let target_bundle = format!("{target_dir}/carry.bundle");
    {
        let local = download(ssh, &src, &bundle.path, bundle.bytes, "bundle").await?;
        put_file(ssh, &target, &local.0, &target_bundle)
            .await
            .map_err(|e| {
                carry_err(
                    "upload",
                    &format!("writing {target_bundle} on {target}: {}", e.message),
                    "",
                )
            })?;
    }
    let out = sh_long(
        ssh,
        &target,
        &carry::fetch_script(&project_root, &target_bundle, &id, &snap.branch),
    )
    .await
    .map_err(|e| carry_transport("fetch", e))?;
    if !out.status.success() {
        return Err(carry_err(
            "fetch",
            &format!("fetching the bundle into {project_root} on {target}"),
            &stderr_of(&out),
        ));
    }

    let tmux_name = pick_target_name(&snap.row.tmux_name, &snap.target_taken)?;
    crate::validate::tmux_name(&tmux_name)?;
    let pane_cmd = crate::service::sessions::recreate_pane_command("work", Some(&id), &tmux_name);
    let cwd = hooks
        .ensure_target_workspace(
            store,
            TargetWorkspace {
                host_alias: &target,
                project_id: snap.project_id,
                worktree_id: snap.worktree_id,
                tmux_name: &tmux_name,
                pane_cmd: &pane_cmd,
                cwd_hint: &cwd_hint,
            },
        )
        .await
        .map_err(|e| before_target("preparing the target worktree", e))?;

    let out = sh(
        ssh,
        &target,
        &target_prep_script(&cwd, &state.head, &snap.branch, &id),
        GIT_TIMEOUT,
    )
    .await
    .map_err(|e| before_target("verifying the target worktree", e))?;
    if !out.status.success() {
        let err = stderr_of(&out);
        // Behind the source HEAD and dirty: the target's own uncommitted
        // work, not a divergence.
        if err.contains(carry::TARGET_DIRTY) {
            return Err(target_dirty(&cwd, &target));
        }
        let msg = if err.contains(DIVERGED) {
            format!(
                "the target worktree {cwd} on {target} has diverged from the source HEAD {}; reconcile the branch there first",
                state.head
            )
        } else if err.contains(NO_CWD) {
            format!("the target worktree {cwd} does not exist on {target}")
        } else {
            format!("target preparation failed on {target}: {err}")
        };
        return Err(before_target(
            "verifying the target worktree",
            IpcError::new(codes::E_GIT, msg),
        ));
    }
    let prep = parse_target_prep(&String::from_utf8_lossy(&out.stdout), &id)
        .map_err(|e| before_target("verifying the target worktree", e))?;
    if prep.head != state.head {
        warnings.push(format!(
            "the target worktree is at {} (newer on origin) while the source was at {}",
            prep.head, state.head
        ));
    }
    if prep.existing > copied as i64 {
        return Err(before_target(
            "copying the transcript",
            IpcError::new(
                codes::E_INVALID_STATE,
                format!(
                    "{target} already has a larger transcript for this conversation at {} ({} > {copied} bytes); it may hold turns taken there — move it aside first",
                    prep.path, prep.existing
                ),
            ),
        ));
    }
    if prep.existing >= 0 {
        warnings.push(format!(
            "replaced an existing {}-byte transcript at {}",
            prep.existing, prep.path
        ));
    }
    // 3c. Replay the uncommitted work and check the claim: the target's
    //     porcelain must equal the source's.
    if prep.head != state.head {
        // Today's "newer on origin" case. A clean source has nothing to
        // replay; a dirty one cannot be replayed onto another base.
        if !state.dirty.is_empty() {
            return Err(carry_err(
                "apply",
                &format!(
                    "the target worktree is at {} while the source is at {}; uncommitted work cannot be replayed onto a different commit",
                    prep.head, state.head
                ),
                "",
            ));
        }
    } else {
        let out = sh(
            ssh,
            &target,
            &carry::apply_script(&cwd, &id, &state.head),
            GIT_TIMEOUT,
        )
        .await
        .map_err(|e| carry_transport("apply", e))?;
        if !out.status.success() {
            let err = stderr_of(&out);
            if err.contains(carry::TARGET_DIRTY) {
                return Err(target_dirty(&cwd, &target));
            }
            return Err(carry_err(
                "apply",
                &format!("replaying the work in {cwd} on {target}"),
                &err,
            ));
        }
        let stdout = String::from_utf8_lossy(&out.stdout);
        let porcelain = carry::parse_apply(&stdout).map_err(|e| {
            carry_err(
                "apply",
                &format!(
                    "reading the replayed state of {cwd} on {target}: {}",
                    e.message
                ),
                "",
            )
        })?;
        // `" M"` (unstaged) and `"M "` (staged) differ only in the two status
        // columns, so they are compared verbatim.
        let line_set = |files: &[DirtyFile]| -> std::collections::BTreeSet<String> {
            files
                .iter()
                .map(|d| format!("{}\t{}", d.status, d.path))
                .collect()
        };
        let (want, got) = (
            line_set(&state.dirty),
            line_set(&parse_porcelain(porcelain)),
        );
        if want != got {
            return Err(IpcError::new(
                codes::E_MOVE_CARRY,
                format!(
                    "move_session: after replaying the work, {cwd} on {target} does not match the source (the source session was not touched)"
                ),
            )
            .with_details(serde_json::json!({ "step": "verify", "source": want, "target": got })));
        }
    }
    if !state.dirty.is_empty() {
        warnings.push(format!(
            "the source worktree {} on {src} still holds a copy of the uncommitted work",
            state.worktree
        ));
    }
    if carried.target_seeded == carry::TargetSeed::Initialized {
        warnings.push(format!(
            "origin was unreachable from {target}; the clone was initialised from the bundle and cannot fetch or push until origin is reachable"
        ));
    }

    // 3d. Small git-ignored files. Never fails the move.
    match carry_ignored(
        ssh,
        &src,
        &target,
        &state.worktree,
        &cwd,
        &target_dir,
        &id,
        &snap,
    )
    .await
    {
        Ok((kept, left)) => {
            carried.ignored_carried = kept;
            carried.ignored_left_behind = left;
        }
        Err((left, why)) => {
            carried.ignored_left_behind = left;
            warnings.push(format!("ignored files were not carried: {why}"));
        }
    }

    // 3e. The Claude-side state: the per-session directory and the project's
    //     memory. Both only ever add on the target, and neither can fail the
    //     move — the transcript alone is all `--resume` needs.
    let parent = |p: &str| {
        std::path::Path::new(p)
            .parent()
            .map(|d| d.to_string_lossy().into_owned())
    };
    match (parent(&located.path), parent(&prep.path)) {
        (Some(src_dir), Some(tgt_dir)) => {
            match carry_session_state(
                ssh,
                &src,
                &target,
                &src_dir,
                &tgt_dir,
                &target_dir,
                &id,
                snap.session_state_cap,
            )
            .await
            {
                Ok(r) => carried.session_state = r,
                Err((r, why)) => {
                    carried.session_state = r;
                    warnings.push(format!("session state was not carried: {why}"));
                }
            }
        }
        _ => warnings
            .push("session state was not carried: the transcript has no parent directory".into()),
    }
    match carry_memory(
        ssh,
        &src,
        &target,
        &state.worktree,
        &project_root,
        &target_dir,
        &id,
    )
    .await
    {
        Ok(r) => carried.memory = r,
        Err((r, why)) => {
            carried.memory = r;
            warnings.push(format!("project memory was not carried: {why}"));
        }
    }

    put(ssh, &target, &prep.path, &bytes)
        .await
        .map_err(|e| before_target("copying the transcript", e))?;
    drop(bytes);

    // 4. Start the target. A failure here leaves nothing running on it.
    hooks
        .start_target(&target, &tmux_name, &cwd, &pane_cmd)
        .await
        .map_err(|e| {
            before_target(
                &format!(
                    "starting {tmux_name} on {target} (fleet only checked its own rows for that name, so a live tmux session called {tmux_name} may already exist on {target} outside fleet; the worktree and transcript copy on the target were left in place)"
                ),
                e,
            )
        })?;

    // From here on the target session exists: every failure is PARTIAL.
    // `pick_target_name` only avoids names live on the target, so the one it
    // chose may be a name fleet killed there moments ago; the refresh below
    // has to be free to insert its row.
    crate::service::sessions::record_tmux_created(store, &target, &tmux_name);
    hooks
        .refresh_host(store, &target)
        .await
        .map_err(|e| partial("reconciling the target host", &target, &tmux_name, None, &e))?;
    let new_row = {
        let s = lock(store)?;
        let row = s.get_session(&tmux_name, &target)?.ok_or_else(|| {
            partial(
                "registering the target row",
                &target,
                &tmux_name,
                None,
                &IpcError::new(codes::E_NOTFOUND, "no row after reconcile"),
            )
        })?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        // Soft-fail like new_session: the session is live either way.
        if let Err(e) = s.set_claude_session_id(row.id, &id) {
            tracing::warn!(
                session_id = row.id,
                error = %e,
                "[move_session] storing claude_session_id failed"
            );
        }
        // Usage (G1): the target's transcript is a whole-line prefix copy of
        // the source's, so its usage cursor starts where the source's stands
        // and the copied history is not counted twice.
        if let Err(e) = s.inherit_usage_cursor(
            row.id,
            snap.row.id,
            i64::try_from(copied).unwrap_or(i64::MAX),
        ) {
            tracing::warn!(
                "move_session: inheriting the usage cursor on {} failed: {e}",
                row.id
            );
        }
        if let Err(e) = s.set_started_at(row.id, now) {
            tracing::warn!(
                session_id = row.id,
                error = %e,
                "[move_session] storing started_at failed"
            );
        }
        if let Some(f) = snap.row.friendly_name.as_deref() {
            if let Err(e) = s.set_friendly_name(&target, &tmux_name, Some(f)) {
                tracing::warn!(
                    session_id = row.id,
                    error = %e,
                    "[move_session] copying friendly_name failed"
                );
            }
        }
        s.set_parent_session_id(row.id, Some(snap.row.id))?;
        row
    };

    // 5. Confirm: the row is running and the transcript is in place.
    let deadline = tokio::time::Instant::now() + opts.confirm_timeout;
    let confirmed = loop {
        let size_ok = match sh(ssh, &target, &size_script(&prep.path), GIT_TIMEOUT).await {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
                .trim()
                .parse::<i64>()
                .is_ok_and(|n| n >= copied as i64),
            _ => false,
        };
        let row = {
            let s = lock(store)?;
            s.get_session_by_id(new_row.id)?
        };
        if size_ok && row.as_ref().is_some_and(|r| r.status == "running") {
            break row;
        }
        let now = tokio::time::Instant::now();
        if now >= deadline {
            break None;
        }
        tokio::time::sleep(opts.poll.min(deadline - now)).await;
        let _ = hooks.refresh_host(store, &target).await;
    };
    let Some(target_row) = confirmed else {
        return Err(partial(
            "confirming the target is running",
            &target,
            &tmux_name,
            Some(new_row.id),
            &IpcError::new(
                codes::E_TIMEOUT,
                format!(
                    "not running with its transcript within {}s",
                    opts.confirm_timeout.as_secs()
                ),
            ),
        ));
    };

    // 6. The source: check it wrote nothing since the copy, kill it (unless
    //    keep_source), and only then record the move.
    let mut source_killed = false;
    if !args.keep_source {
        // The source kept running through the copy: if its transcript moved
        // on since, it took a turn the target does not have. Killing it would
        // lose that turn, so stop here with both sessions alive.
        let recheck = locate_on(ssh, &src, snap.stored_transcript.as_deref(), &id)
            .await
            .map_err(|e| {
                partial(
                    "re-checking the source transcript",
                    &target,
                    &tmux_name,
                    Some(target_row.id),
                    &e,
                )
            })?;
        if recheck.size != located.size || recheck.mtime != located.mtime {
            return Err(partial(
                "source transcript changed after copy",
                &target,
                &tmux_name,
                Some(target_row.id),
                &IpcError::new(
                    codes::E_INVALID_STATE,
                    format!(
                        "the source transcript went from {} to {} bytes (mtime {} -> {}) after it was copied, so the source took a turn the target does not have; kill the target session {tmux_name} on {target} and retry move_session once the source is idle",
                        located.size, recheck.size, located.mtime, recheck.mtime
                    ),
                ),
            ));
        }
        // Snapshot the source's lifetime usage and usage cursor before the
        // kill drops its row.
        let (source_usage, source_cursor) = store
            .lock()
            .ok()
            .map(|s| {
                (
                    s.get_session_by_id(snap.row.id)
                        .ok()
                        .flatten()
                        .map(|r| r.usage),
                    s.usage_cursor(snap.row.id).ok().flatten(),
                )
            })
            .unwrap_or((None, None));
        hooks
            .kill_source(store, &src, &snap.row.tmux_name)
            .await
            .map_err(|e| {
                partial(
                    &format!("killing the source {} on {src}", snap.row.tmux_name),
                    &target,
                    &tmux_name,
                    Some(target_row.id),
                    &e,
                )
            })?;
        source_killed = true;
        // One last look: a write between the final check and the kill means
        // the target may lack the source's last turn. Nothing is left to undo,
        // so say so in the report.
        match locate_on(ssh, &src, snap.stored_transcript.as_deref(), &id).await {
            Ok(after) if after.size != recheck.size || after.mtime != recheck.mtime => {
                warnings.push(format!(
                    "the source wrote after the final check and before the kill (transcript {} -> {} bytes, mtime {} -> {}); the target may be missing its last turn — compare the two with session_transcript",
                    recheck.size, after.size, recheck.mtime, after.mtime
                ));
            }
            Ok(_) => {}
            Err(e) => warnings.push(format!(
                "could not re-check the source transcript after the kill ({}: {}); the target may be missing its last turn",
                e.code, e.message
            )),
        }
        // The spend follows the session. Never under keep_source: both rows
        // stay live there and would report it twice.
        if let Ok(s) = store.lock() {
            if let Some(u) = source_usage.as_ref() {
                if let Err(e) = s.add_usage_totals(target_row.id, u) {
                    tracing::warn!(
                        "move_session: carrying usage totals to {} failed: {e}",
                        target_row.id
                    );
                }
            }
            // A source usage pass between the inherit and the kill counted
            // lines the target's cursor still points before: catch up.
            if let Some(c) = source_cursor.as_ref() {
                if let Err(e) = s.raise_usage_cursor(target_row.id, c) {
                    tracing::warn!(
                        "move_session: raising the usage cursor on {} failed: {e}",
                        target_row.id
                    );
                }
            }
        }
    }

    // 7. The move is complete: record it on both rows.
    let detail = serde_json::json!({
        "from_host": src,
        "to_host": target,
        "from_session_id": snap.row.id,
        "to_session_id": target_row.id,
        "claude_session_id": id,
        "branch": snap.branch,
        "bytes": copied,
        "kept_source": args.keep_source,
        "source_killed": source_killed,
        "carried": &carried,
    })
    .to_string();
    if let Ok(s) = store.lock() {
        for sid in [snap.row.id, target_row.id] {
            if let Err(e) = s.insert_session_event(sid, EVENT_MOVED, Some(&detail)) {
                tracing::warn!(
                    kind = EVENT_MOVED,
                    session_id = sid,
                    error = %e,
                    "[event] insert failed"
                );
            }
        }
    }

    Ok(MoveReport {
        source_session_id: snap.row.id,
        target_session_id: target_row.id,
        from_host: src,
        to_host: target,
        tmux_name,
        claude_session_id: id,
        branch: snap.branch,
        target_cwd: cwd,
        transcript_bytes: copied,
        source_killed,
        warnings,
        carried,
        target: target_row,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ssh_fake::{FakeSsh, Match, Reply};
    use crate::tmux::{RemoteTmux, TmuxExec};

    const SID: &str = "550e8400-e29b-41d4-a716-446655440000";
    const HEAD: &str = "1111111111111111111111111111111111111111";
    const SRC_PATH: &str = "/home/a/.claude/projects/-home-a-p-o-r--claude-worktrees-feat/550e8400-e29b-41d4-a716-446655440000.jsonl";
    const TGT_CWD: &str = "/home/b/p/o/r/.claude/worktrees/feat";
    /// The source worktree the inspection reports — the directory the source
    /// memory listing is keyed by, as both its repo hint and its fallback.
    const SRC_WORKTREE: &str = "/home/a/p/o/r/.claude/worktrees/feat";
    const TGT_ENC: &str = "-home-b-p-o-r--claude-worktrees-feat";
    const TRANSCRIPT: &str =
        "{\"type\":\"user\",\"message\":{\"content\":\"hi\"}}\n{\"type\":\"assistant\"}\n";
    /// Stand-in for the git bundle's bytes.
    const BUNDLE: &str = "FAKE-BUNDLE-BYTES";
    const SRC_DIR: &str =
        "/home/a/.cache/claude-fleet/transfer/550e8400-e29b-41d4-a716-446655440000";
    const TGT_DIR: &str =
        "/home/b/.cache/claude-fleet/transfer/550e8400-e29b-41d4-a716-446655440000";
    /// The source's Claude project directory: the parent of [`SRC_PATH`], and
    /// so the directory the per-session state is packed from.
    const SRC_PROJECT_DIR: &str = "/home/a/.claude/projects/-home-a-p-o-r--claude-worktrees-feat";
    /// The target project root the memory listing is keyed by (the parent of
    /// [`TGT_CWD`]'s `.claude/worktrees/`).
    const TGT_ROOT: &str = "/home/b/p/o/r";
    const SRC_MEMORY: &str = "/home/a/.claude/projects/-home-a-p-o-r/memory";
    const TGT_MEMORY: &str = "/home/b/.claude/projects/-home-b-p-o-r/memory";
    /// The source's `MEMORY.md`: one line per memory file.
    const SRC_INDEX: &str =
        "- [New thing](new.md) - what it is\n- [Differs](differs.md) - theirs\n";

    fn tgt_path() -> String {
        format!("/home/b/.claude/projects/{TGT_ENC}/{SID}.jsonl")
    }

    /// The target's Claude project directory: the parent of [`tgt_path`], and
    /// so the directory the per-session state is merged into.
    fn tgt_project_dir() -> String {
        format!("/home/b/.claude/projects/{TGT_ENC}")
    }

    /// A listing script's stdout: the marker, then NUL-terminated records.
    fn records_out(records: &[String]) -> Reply {
        let mut stdout = out("").into_bytes();
        for r in records {
            stdout.extend_from_slice(r.as_bytes());
            stdout.push(0);
        }
        Reply::Exit {
            code: 0,
            stdout,
            stderr: Vec::new(),
        }
    }

    /// Both halves of the Claude-side state answering as a real pair of hosts
    /// would: a session directory with two files (one of which the target
    /// already has a bigger copy of), and a memory directory with one file
    /// the target lacks, one it has in another version, and an index.
    fn with_claude_state(f: &Fixture) {
        f.fake
            .on_host(
                "alpha",
                Match::script_contains("# cf-carry:state-list"),
                records_out(&[
                    "550\tsubagents/agent-aa.jsonl".into(),
                    "12\ttool-results/out1.txt".into(),
                ]),
            )
            .on_host(
                "alpha",
                Match::script_contains("# cf-carry:state-pack"),
                Reply::ok(&out(&format!("{}\t{SRC_DIR}/state.tgz\n", BUNDLE.len()))),
            )
            .on_host(
                "beta",
                Match::script_contains("# cf-carry:state-merge"),
                Reply::ok(&out(
                    "carried\t550\tsubagents/agent-aa.jsonl\nkept\ttool-results/out1.txt\n",
                )),
            )
            .on_host(
                "alpha",
                Match::script_contains("# cf-carry:memory-list"),
                records_out(&[
                    format!("dir\t{SRC_MEMORY}\t1"),
                    "h-new\t20\tnew.md".into(),
                    "h-source\t30\tdiffers.md".into(),
                    "h-index\t40\tMEMORY.md".into(),
                ]),
            )
            .on_host(
                "beta",
                Match::script_contains("# cf-carry:memory-list"),
                records_out(&[
                    format!("dir\t{TGT_MEMORY}\t1"),
                    "h-target\t31\tdiffers.md".into(),
                ]),
            )
            .on_host(
                "alpha",
                Match::script_contains("# cf-carry:pack"),
                Reply::ok(&out(&format!("{}\t{SRC_DIR}/memory.tgz\n", BUNDLE.len()))),
            )
            .on_host(
                "beta",
                Match::script_contains("# cf-carry:memory-extract"),
                Reply::ok("ok\n"),
            )
            .on_host(
                "alpha",
                Match::script_contains("# cf-carry:memory-index"),
                Reply::ok(&out(&format!("present\n{SRC_INDEX}"))),
            )
            .on_host(
                "beta",
                Match::script_contains("# cf-carry:memory-index"),
                Reply::ok(&out("absent\n")),
            )
            .on_host(
                "beta",
                Match::script_contains("# cf-carry:memory-append"),
                Reply::ok("ok\n"),
            );
    }

    /// Every `bash -lc` script `host` was sent that carries `marker`.
    fn scripts_with(f: &Fixture, host: &str, marker: &str) -> Vec<String> {
        f.fake
            .calls_for(host)
            .into_iter()
            .filter_map(|c| c.script())
            .filter(|s| s.contains(marker))
            .collect()
    }

    /// A carry script's stdout as the real scripts print it: marker line,
    /// then the payload.
    fn out(payload: &str) -> String {
        format!("{}\n{payload}", carry::OUT_MARKER)
    }

    fn snapshot_out(bytes: usize, commits: u32) -> String {
        format!("{bytes}\t{commits}\t0\t0\t{SRC_DIR}/carry.bundle\n")
    }

    /// Test hooks: the workspace step returns a fixed cwd, tmux runs through
    /// `RemoteTmux` over the same `FakeSsh` (so a scripted `new-session`
    /// failure really fails), and "reconcile" registers started sessions.
    struct FakeHooks {
        fake: FakeSsh,
        project_id: i64,
        worktree_id: i64,
        /// Status the target row gets on reconcile.
        target_status: &'static str,
        /// `kill_source` fails.
        kill_fails: bool,
        /// Starting the target makes the source transcript grow (a turn
        /// taken on the source after the copy).
        grow_source_on_start: bool,
        /// Killing the source makes its transcript grow (a write that
        /// slipped in after the final check).
        grow_source_on_kill: bool,
        /// Whether `session_moved` was already on the source when the kill
        /// ran (`None` = no kill).
        moved_at_kill: Mutex<Option<bool>>,
        started: Mutex<Vec<(String, String)>>,
        log: Mutex<Vec<String>>,
    }

    impl FakeHooks {
        fn new(fake: &FakeSsh, project_id: i64, worktree_id: i64) -> Self {
            Self {
                fake: fake.clone(),
                project_id,
                worktree_id,
                target_status: "running",
                kill_fails: false,
                grow_source_on_start: false,
                grow_source_on_kill: false,
                moved_at_kill: Mutex::new(None),
                started: Mutex::new(Vec::new()),
                log: Mutex::new(Vec::new()),
            }
        }
        fn log(&self) -> Vec<String> {
            self.log.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl MoveHooks for FakeHooks {
        async fn source_cwd_hint(&self, _: &Mutex<Store>, _: &SessionRow) -> Option<String> {
            Some("/home/a/p/o/r/.claude/worktrees/feat".into())
        }
        async fn ensure_target_workspace(
            &self,
            _: &Mutex<Store>,
            w: TargetWorkspace<'_>,
        ) -> Result<String, IpcError> {
            self.log
                .lock()
                .unwrap()
                .push(format!("ensure {} hint={}", w.host_alias, w.cwd_hint));
            Ok(TGT_CWD.into())
        }
        async fn start_target(
            &self,
            host: &str,
            name: &str,
            cwd: &str,
            pane_cmd: &str,
        ) -> Result<(), IpcError> {
            RemoteTmux {
                client: self.fake.clone(),
                host: host.to_string(),
            }
            .new_session(name, std::path::Path::new(cwd), pane_cmd)
            .await?;
            if self.grow_source_on_start {
                self.fake.on_host(
                    "alpha",
                    Match::script_contains("# cf-move:locate"),
                    Reply::ok(&locate_out(TRANSCRIPT.len() + 40, MTIME + 3)),
                );
            }
            self.started
                .lock()
                .unwrap()
                .push((host.to_string(), name.to_string()));
            Ok(())
        }
        async fn refresh_host(&self, store: &Mutex<Store>, host: &str) -> Result<(), IpcError> {
            let started = self.started.lock().unwrap().clone();
            let s = store.lock().unwrap();
            for (h, n) in started.iter().filter(|(h, _)| h == host) {
                s.upsert_session(
                    n,
                    h,
                    Some(self.project_id),
                    Some(self.worktree_id),
                    1,
                    1,
                    self.target_status,
                    None,
                )?;
            }
            Ok(())
        }
        async fn kill_source(
            &self,
            store: &Mutex<Store>,
            host: &str,
            name: &str,
        ) -> Result<(), IpcError> {
            if self.kill_fails {
                return Err(IpcError::new(codes::E_TMUX, "can't find session"));
            }
            {
                let s = store.lock().unwrap();
                let id = s.get_session(name, host).unwrap().map(|r| r.id);
                let moved = id.is_some_and(|id| {
                    s.list_session_events(id, 50)
                        .unwrap()
                        .iter()
                        .any(|e| e.kind == EVENT_MOVED)
                });
                *self.moved_at_kill.lock().unwrap() = Some(moved);
            }
            if self.grow_source_on_kill {
                self.fake.on_host(
                    "alpha",
                    Match::script_contains("# cf-move:locate"),
                    Reply::ok(&locate_out(TRANSCRIPT.len() + 99, MTIME + 9)),
                );
            }
            self.log.lock().unwrap().push(format!("kill {host} {name}"));
            Ok(())
        }
    }

    struct Fixture {
        store: Mutex<Store>,
        fake: FakeSsh,
        source_id: i64,
        project_id: i64,
        worktree_id: i64,
    }

    const MTIME: usize = 1_789_000_000;

    fn locate_out(size: usize, mtime: usize) -> String {
        format!("{size}\t{mtime}\t{SRC_PATH}\n")
    }

    fn inspection(porcelain: &str, rsha: &str, ahead: &str) -> String {
        inspection_midop(porcelain, rsha, ahead, "")
    }

    fn inspection_midop(porcelain: &str, rsha: &str, ahead: &str, midop: &str) -> String {
        format!(
            "/home/a/p/o/r/.claude/worktrees/feat\x1e{porcelain}\x1e{HEAD}\x1efeat\x1e{rsha}\x1e{ahead}\x1e{midop}"
        )
    }

    /// alpha → beta, source clean + pushed, a small transcript.
    fn fixture() -> Fixture {
        let s = Store::open_in_memory().unwrap();
        for h in ["alpha", "beta"] {
            s.insert_host(h, None).unwrap();
            s.update_host_probe(h, true, None, None, 1).unwrap();
            s.set_host_provisioned(h, true).unwrap();
        }
        let pid = s.upsert_project("o", "r", "/local/o/r").unwrap();
        let wid = s
            .upsert_worktree(
                pid,
                "feat",
                "/local/o/r/.claude/worktrees/feat",
                Some("feat"),
            )
            .unwrap();
        let id = s
            .upsert_session(
                "dev-o-r--feat",
                "alpha",
                Some(pid),
                Some(wid),
                1,
                1,
                "running",
                None,
            )
            .unwrap();
        s.set_claude_session_id(id, SID).unwrap();
        s.set_claude_status_by_session_id(SID, "idle").unwrap();
        s.set_friendly_name("alpha", "dev-o-r--feat", Some("Feat work"))
            .unwrap();
        // beta's projects root → the layout hint is exactly TGT_CWD.
        s.set_setting("projects.base_path", r#"{"beta":"~/p"}"#)
            .unwrap();
        let fake = FakeSsh::new();
        fake.with_home("/home/b");
        fake.on_host(
            "alpha",
            Match::script_contains("# cf-move:inspect"),
            Reply::ok(&inspection("", HEAD, "0")),
        )
        .on_host(
            "alpha",
            Match::script_contains("# cf-move:locate"),
            Reply::ok(&locate_out(TRANSCRIPT.len(), MTIME)),
        )
        .on_host(
            "alpha",
            Match::script_contains("# cf-move:read"),
            Reply::ok(TRANSCRIPT),
        )
        .on_host(
            "beta",
            Match::script_contains("# cf-move:prep"),
            Reply::ok(&format!("{HEAD}\t{TGT_ENC}\t{}\t-1\n", tgt_path())),
        )
        .on_host(
            "beta",
            Match::script_contains("# cf-move:size"),
            Reply::ok(&format!("{}\n", TRANSCRIPT.len())),
        )
        // The carry: a target that already has the clone, nothing unpushed,
        // a clean replay and no git-ignored files worth carrying.
        .on_host(
            "beta",
            Match::script_contains("# cf-carry:seed"),
            Reply::ok(&out("existing\n")),
        )
        .on_host(
            "beta",
            Match::script_contains("# cf-carry:haves"),
            Reply::ok(&out(&format!("{TGT_DIR}\n{HEAD}\n"))),
        )
        .on_host(
            "alpha",
            Match::script_contains("# cf-carry:snapshot"),
            Reply::ok(&out(&snapshot_out(BUNDLE.len(), 0))),
        )
        .on_host(
            "alpha",
            Match::script_contains("# cf-carry:chunk"),
            Reply::ok(&out(BUNDLE)),
        )
        .on_host(
            "beta",
            Match::script_contains("# cf-carry:fetch"),
            Reply::ok("ok\n"),
        )
        .on_host(
            "beta",
            Match::script_contains("# cf-carry:apply"),
            Reply::ok(&out("")),
        )
        .on_host(
            "alpha",
            Match::script_contains("# cf-carry:ignored-list"),
            Reply::ok(&out("")),
        )
        // …and no Claude-side state either: no per-session directory, and a
        // project whose memory directory does not exist.
        .on_host(
            "alpha",
            Match::script_contains("# cf-carry:state-list"),
            Reply::ok(&out("")),
        )
        .on_host(
            "alpha",
            Match::script_contains("# cf-carry:memory-list"),
            records_out(&[format!("dir\t{SRC_MEMORY}\t0")]),
        );
        Fixture {
            store: Mutex::new(s),
            fake,
            source_id: id,
            project_id: pid,
            worktree_id: wid,
        }
    }

    fn fast() -> MoveOptions {
        MoveOptions {
            confirm_timeout: Duration::from_millis(150),
            poll: Duration::from_millis(10),
        }
    }

    fn args(f: &Fixture, keep_source: bool) -> MoveSessionArgs {
        MoveSessionArgs {
            session_id: f.source_id,
            target_host_alias: "beta".into(),
            keep_source,
            strict: false,
        }
    }

    async fn run(f: &Fixture, hooks: &FakeHooks, keep: bool) -> Result<MoveReport, IpcError> {
        move_session_with(args(f, keep), &f.store, &f.fake, hooks, fast()).await
    }

    fn events(f: &Fixture, id: i64) -> Vec<(String, Option<String>)> {
        f.store
            .lock()
            .unwrap()
            .list_session_events(id, 50)
            .unwrap()
            .into_iter()
            .map(|e| (e.kind, e.detail))
            .collect()
    }

    fn assert_source_untouched(f: &Fixture, hooks: &FakeHooks) {
        assert!(
            !hooks.log().iter().any(|l| l.starts_with("kill")),
            "source must not be killed: {:?}",
            hooks.log()
        );
        let s = f.store.lock().unwrap();
        let row = s.get_session_by_id(f.source_id).unwrap().unwrap();
        assert_eq!(row.status, "running");
        assert_eq!(row.host_alias, "alpha");
        drop(s);
        assert!(
            events(f, f.source_id).iter().all(|(k, _)| k != EVENT_MOVED),
            "no session_moved on a failed move"
        );
        assert!(
            f.fake
                .calls_for("alpha")
                .iter()
                .all(|c| !c.command().contains("kill-session")),
            "no tmux kill on the source"
        );
    }

    #[tokio::test]
    async fn the_target_inherits_the_usage_cursor_and_carries_totals_only_when_the_source_dies() {
        for keep_source in [false, true] {
            let f = fixture();
            f.store
                .lock()
                .unwrap()
                .apply_usage(
                    f.source_id,
                    "alpha",
                    &crate::store::UsageDelta {
                        reset: false,
                        totals: crate::store::UsageTotals {
                            input_tokens: 40,
                            cost_micros: 200,
                            ..Default::default()
                        },
                        model: Some("claude-opus-5".into()),
                        offset: 3,
                        source: "older.jsonl".into(),
                        last_msg_id: Some("msg_1".into()),
                        last_msg_usage: Some("40,0,0,0,0".into()),
                        now: 1,
                    },
                )
                .unwrap();
            let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
            let rep = run(&f, &hooks, keep_source).await.expect("move");
            let s = f.store.lock().unwrap();
            let c = s
                .list_usage_cursors("beta")
                .unwrap()
                .into_iter()
                .find(|c| c.session_id == rep.target_session_id)
                .expect("target cursor");
            // The source was reading another file: the target starts after
            // the copied prefix, so only lines appended after the move count.
            assert_eq!(c.offset_bytes, TRANSCRIPT.len() as i64, "{keep_source}");
            assert_eq!(c.source.as_deref(), Some(format!("{SID}.jsonl").as_str()));
            let usage = s
                .get_session_by_id(rep.target_session_id)
                .unwrap()
                .unwrap()
                .usage;
            let expected = if keep_source { 0 } else { 40 };
            assert_eq!(usage.usage_input_tokens, expected, "{keep_source}");
        }
    }

    #[tokio::test]
    async fn happy_path_copies_the_transcript_resumes_on_target_and_kills_the_source() {
        let f = fixture();
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let rep = run(&f, &hooks, false).await.expect("move");

        assert_eq!(rep.from_host, "alpha");
        assert_eq!(rep.to_host, "beta");
        assert_eq!(rep.tmux_name, "dev-o-r--feat");
        assert_eq!(rep.transcript_bytes, TRANSCRIPT.len() as u64);
        assert_eq!(rep.target_cwd, TGT_CWD);
        assert!(rep.source_killed);
        assert!(rep.warnings.is_empty(), "{:?}", rep.warnings);

        // The transcript went to the target through upload_file stdin, at the
        // path derived from the TARGET cwd.
        let uploads: Vec<_> = f
            .fake
            .calls_for("beta")
            .into_iter()
            .filter(|c| c.stdin.is_some())
            .collect();
        assert_eq!(uploads.len(), 2, "the bundle, then the transcript");
        assert_eq!(
            uploads[0].command(),
            format!("cat > {}", quote(&format!("{TGT_DIR}/carry.bundle")))
        );
        // The marker line was stripped, not relayed into the bundle.
        assert_eq!(uploads[0].stdin_str().as_deref(), Some(BUNDLE));
        assert_eq!(
            uploads[1].command(),
            format!("cat > {}", quote(&tgt_path()))
        );
        assert_eq!(uploads[1].stdin_str().as_deref(), Some(TRANSCRIPT));
        assert_eq!(rep.carried.commits, 0);

        // tmux on the target resumes the SAME conversation in the target cwd.
        let start = f
            .fake
            .calls_for("beta")
            .into_iter()
            .find_map(|c| c.script().filter(|s| s.contains("tmux new-session")))
            .expect("tmux new-session on beta");
        assert!(
            start.contains(&format!("--resume '\\''{SID}'\\''")),
            "{start}"
        );
        assert!(start.contains(&quote(TGT_CWD)), "{start}");
        // The workspace step got the layout-derived hint for the target.
        assert!(hooks.log().contains(&format!("ensure beta hint={TGT_CWD}")));

        // New row: parent = source, same claude id, friendly name carried.
        let s = f.store.lock().unwrap();
        let t = s.get_session_by_id(rep.target_session_id).unwrap().unwrap();
        assert_eq!(t.host_alias, "beta");
        assert_eq!(t.parent_session_id, Some(f.source_id));
        assert_eq!(t.claude_session_id.as_deref(), Some(SID));
        assert_eq!(t.friendly_name.as_deref(), Some("Feat work"));
        assert!(t.started_at.is_some());
        drop(s);

        // session_moved on both rows, with hosts, new id and byte count.
        for id in [f.source_id, rep.target_session_id] {
            let ev = events(&f, id);
            let (_, detail) = ev
                .iter()
                .find(|(k, _)| k == EVENT_MOVED)
                .unwrap_or_else(|| panic!("session_moved on {id}: {ev:?}"));
            let d: serde_json::Value = serde_json::from_str(detail.as_deref().unwrap()).unwrap();
            assert_eq!(d["from_host"], "alpha");
            assert_eq!(d["to_host"], "beta");
            assert_eq!(d["to_session_id"], rep.target_session_id);
            assert_eq!(d["bytes"], TRANSCRIPT.len() as u64);
        }
        // Source killed through the normal path, after the target confirmed —
        // and session_moved was recorded only after that kill succeeded.
        assert_eq!(hooks.log().last().unwrap(), "kill alpha dev-o-r--feat");
        assert_eq!(
            *hooks.moved_at_kill.lock().unwrap(),
            Some(false),
            "session_moved must not be recorded before the kill"
        );
    }

    #[tokio::test]
    async fn a_write_between_the_final_check_and_the_kill_is_a_warning() {
        let f = fixture();
        let mut hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        hooks.grow_source_on_kill = true;
        let rep = run(&f, &hooks, false)
            .await
            .expect("the move still completes");
        assert!(rep.source_killed);
        assert!(
            rep.warnings
                .iter()
                .any(|w| w.contains("after the final check") && w.contains("last turn")),
            "{:?}",
            rep.warnings
        );
        assert!(events(&f, f.source_id)
            .iter()
            .any(|(k, _)| k == EVENT_MOVED));
    }

    #[tokio::test]
    async fn a_partial_move_records_session_move_partial_not_session_moved() {
        for case in ["kill_fails", "source_changed", "never_confirmed"] {
            let f = fixture();
            let mut hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
            match case {
                "kill_fails" => hooks.kill_fails = true,
                "source_changed" => hooks.grow_source_on_start = true,
                _ => hooks.target_status = "ghost",
            }
            let err = run(&f, &hooks, false).await.unwrap_err();
            assert_eq!(err.code, codes::E_MOVE_PARTIAL, "{case}: {}", err.message);
            let details = err.details.clone().expect("details");
            let tid = details["target_session_id"].as_i64().expect("target id");
            for id in [f.source_id, tid] {
                let ev = events(&f, id);
                assert!(
                    ev.iter().all(|(k, _)| k != EVENT_MOVED),
                    "{case}: no session_moved on {id}: {ev:?}"
                );
                let (_, detail) = ev
                    .iter()
                    .find(|(k, _)| k == EVENT_MOVE_PARTIAL)
                    .unwrap_or_else(|| panic!("{case}: session_move_partial on {id}: {ev:?}"));
                let d: serde_json::Value =
                    serde_json::from_str(detail.as_deref().unwrap()).unwrap();
                assert_eq!(d["step"], details["step"], "{case}");
                assert_eq!(d["to_session_id"], tid);
                assert_eq!(d["to_host"], "beta");
            }
        }
    }

    #[tokio::test]
    async fn keep_source_leaves_the_source_running() {
        let f = fixture();
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let rep = run(&f, &hooks, true).await.expect("move");
        assert!(!rep.source_killed);
        assert!(!hooks.log().iter().any(|l| l.starts_with("kill")));
        let ev = events(&f, f.source_id);
        let (_, detail) = ev
            .iter()
            .find(|(k, _)| k == EVENT_MOVED)
            .expect("session_moved");
        let d: serde_json::Value = serde_json::from_str(detail.as_deref().unwrap()).unwrap();
        assert_eq!(d["kept_source"], true);
    }

    /// Strict mode only: the carry takes an unpushed branch along.
    #[tokio::test]
    async fn unpushed_branch_is_refused_before_the_target_is_touched() {
        let f = fixture();
        f.fake.on_host(
            "alpha",
            Match::script_contains("# cf-move:inspect"),
            Reply::ok(&inspection("", "", "-1")),
        );
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let strict = |f: &Fixture| MoveSessionArgs {
            strict: true,
            ..args(f, false)
        };
        let err = move_session_with(strict(&f), &f.store, &f.fake, &hooks, fast())
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::E_MOVE_UNPUSHED, "{}", err.message);
        assert!(
            err.message.contains("git push -u origin feat"),
            "{}",
            err.message
        );
        assert!(err.message.contains("never pushes"));
        assert!(f.fake.calls_for("beta").is_empty(), "target untouched");
        assert!(hooks.log().is_empty());
        assert_source_untouched(&f, &hooks);

        // Pushed but ahead of origin: also refused.
        f.fake.on_host(
            "alpha",
            Match::script_contains("# cf-move:inspect"),
            Reply::ok(&inspection("", HEAD, "2")),
        );
        let err = move_session_with(strict(&f), &f.store, &f.fake, &hooks, fast())
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::E_MOVE_UNPUSHED);
        assert!(err.message.contains("2 commit(s)"), "{}", err.message);
        // The inspection never pushes anything itself.
        assert!(f
            .fake
            .calls_for("alpha")
            .iter()
            .all(|c| !c.script().unwrap_or_default().contains("git push")));
    }

    /// Strict mode only: the carry takes uncommitted work along.
    #[tokio::test]
    async fn dirty_source_is_refused_and_lists_the_files() {
        let f = fixture();
        f.fake.on_host(
            "alpha",
            Match::script_contains("# cf-move:inspect"),
            Reply::ok(&inspection(" M src/lib.rs\n?? notes.txt", HEAD, "0")),
        );
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let err = move_session_with(
            MoveSessionArgs {
                strict: true,
                ..args(&f, false)
            },
            &f.store,
            &f.fake,
            &hooks,
            fast(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_MOVE_DIRTY);
        assert!(err.message.contains("src/lib.rs") && err.message.contains("notes.txt"));
        let details = err.details.expect("dirty_files details");
        assert_eq!(details["dirty_files"].as_array().unwrap().len(), 2);
        assert!(f.fake.calls_for("beta").is_empty());
        assert_source_untouched(&f, &hooks);
    }

    // ── the carry ──

    #[tokio::test]
    async fn a_dirty_unpushed_source_is_carried_and_reported() {
        let f = fixture();
        let porcelain = " M src/lib.rs\n?? notes.txt";
        let mut listed = out("").into_bytes();
        listed.extend_from_slice(b"4\t.env\0-1\tnode_modules/\0");
        f.fake
            .on_host(
                "alpha",
                Match::script_contains("# cf-move:inspect"),
                Reply::ok(&inspection(porcelain, "", "-1")),
            )
            .on_host(
                "alpha",
                Match::script_contains("# cf-carry:snapshot"),
                Reply::ok(&out(&snapshot_out(BUNDLE.len(), 2))),
            )
            .on_host(
                "beta",
                Match::script_contains("# cf-carry:seed"),
                Reply::ok(&out("initialized\n")),
            )
            .on_host(
                "beta",
                Match::script_contains("# cf-carry:apply"),
                Reply::ok(&out(&format!("{porcelain}\n"))),
            )
            .on_host(
                "alpha",
                Match::script_contains("# cf-carry:ignored-list"),
                Reply::Exit {
                    code: 0,
                    stdout: listed,
                    stderr: Vec::new(),
                },
            )
            .on_host(
                "alpha",
                Match::script_contains("# cf-carry:ignored-pack"),
                Reply::ok(&out(&format!("{}\t{SRC_DIR}/ignored.tgz\n", BUNDLE.len()))),
            )
            .on_host(
                "beta",
                Match::script_contains("# cf-carry:ignored-extract"),
                Reply::ok("ok\n"),
            );
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let rep = run(&f, &hooks, false)
            .await
            .expect("a dirty, unpushed source now moves");

        assert_eq!(rep.carried.commits, 2);
        assert_eq!(rep.carried.bundle_bytes, BUNDLE.len() as u64);
        assert_eq!(rep.carried.dirty_entries.len(), 2);
        assert_eq!(rep.carried.target_seeded, carry::TargetSeed::Initialized);
        assert_eq!(rep.carried.ignored_carried[0].path, ".env");
        assert_eq!(rep.carried.ignored_left_behind[0].path, "node_modules/");
        assert!(
            rep.warnings
                .iter()
                .any(|w| w.contains("still holds a copy of the uncommitted work")),
            "{:?}",
            rep.warnings
        );
        assert!(
            rep.warnings
                .iter()
                .any(|w| w.contains("origin was unreachable")),
            "{:?}",
            rep.warnings
        );

        // The bundle reached the target's transfer dir through upload_file.
        let uploads: Vec<String> = f
            .fake
            .calls_for("beta")
            .into_iter()
            .filter(|c| c.stdin.is_some())
            .map(|c| c.command())
            .collect();
        assert_eq!(
            uploads[0],
            format!("cat > {}", quote(&format!("{TGT_DIR}/carry.bundle")))
        );
        // Nothing was pushed, committed or stashed on the source.
        for c in f.fake.calls_for("alpha") {
            let s = c.script().unwrap_or_default();
            assert!(
                !s.contains("git push") && !s.contains("git stash") && !s.contains("git commit "),
                "{s}"
            );
        }
        // Both sides were cleaned up, and the event carries the report.
        for host in ["alpha", "beta"] {
            assert!(
                f.fake
                    .calls_for(host)
                    .iter()
                    .any(|c| c.script().is_some_and(|s| s.contains("# cf-carry:cleanup"))),
                "{host}"
            );
        }
        let ev = events(&f, rep.target_session_id);
        let detail = ev
            .iter()
            .find(|(k, _)| k == EVENT_MOVED)
            .unwrap()
            .1
            .clone()
            .unwrap();
        let d: serde_json::Value = serde_json::from_str(&detail).unwrap();
        assert_eq!(d["carried"]["commits"], 2);
    }

    #[tokio::test]
    async fn strict_still_refuses_dirty_and_unpushed_before_the_target_is_touched() {
        let f = fixture();
        f.fake.on_host(
            "alpha",
            Match::script_contains("# cf-move:inspect"),
            Reply::ok(&inspection(" M a.rs", HEAD, "0")),
        );
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let mut a = args(&f, false);
        a.strict = true;
        let err = move_session_with(a, &f.store, &f.fake, &hooks, fast())
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::E_MOVE_DIRTY);
        assert!(f.fake.calls_for("beta").is_empty(), "target untouched");
        assert_source_untouched(&f, &hooks);
    }

    #[tokio::test]
    async fn a_midop_source_is_refused_before_anything_else_runs() {
        let f = fixture();
        f.fake.on_host(
            "alpha",
            Match::script_contains("# cf-move:inspect"),
            Reply::ok(&inspection_midop("UU a.rs", HEAD, "0", "merge")),
        );
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let err = run(&f, &hooks, false).await.unwrap_err();
        assert_eq!(err.code, codes::E_MOVE_MIDOP);
        assert!(f.fake.calls_for("beta").is_empty());
        assert_source_untouched(&f, &hooks);
    }

    #[tokio::test]
    async fn carry_failures_leave_the_source_untouched_and_still_clean_up() {
        struct Case {
            host: &'static str,
            /// The carry script whose reply is replaced.
            marker: &'static str,
            /// What distinguishes this case when the marker repeats.
            what: &'static str,
            reply: Reply,
            code: &'static str,
            step: Option<&'static str>,
            /// The transport code kept in `details.cause_code`, when the
            /// failure was one (a timeout must stay tellable from a parse
            /// failure even though both are `E_MOVE_CARRY`).
            cause: Option<&'static str>,
            /// Hosts that must be sent a cleanup script — and, just as
            /// importantly, the only ones: a host nothing was created on
            /// must never get one (an empty transfer id above all).
            cleaned: &'static [&'static str],
            /// Shorten the fake's wall clock, for a reply that never comes.
            wall_clock: Option<Duration>,
        }
        let case = |host, marker, what, reply, code, step, cleaned| Case {
            host,
            marker,
            what,
            reply,
            code,
            step,
            cause: None,
            cleaned,
            wall_clock: None,
        };
        let failed = |what: &str| format!("{} {what}", carry::FAILED);
        let cases = vec![
            // Seeding is the first carry step: nothing exists to clean yet.
            case(
                "beta",
                "# cf-carry:seed",
                "exit",
                Reply::fail(5, &failed("init")),
                codes::E_MOVE_CARRY,
                Some("seed"),
                &[][..],
            ),
            case(
                "beta",
                "# cf-carry:seed",
                "no marker",
                Reply::ok("cloned\n"),
                codes::E_MOVE_CARRY,
                Some("seed"),
                &[][..],
            ),
            // The haves script creates the target's transfer dir, so a
            // failure there — including one where the reply never arrives —
            // must still clean the target, and only the target.
            case(
                "beta",
                "# cf-carry:haves",
                "exit",
                Reply::fail(5, &failed("mkdir")),
                codes::E_MOVE_CARRY,
                Some("haves"),
                &["beta"][..],
            ),
            case(
                "beta",
                "# cf-carry:haves",
                "no marker",
                Reply::ok(&format!("{TGT_DIR}\n{HEAD}\n")),
                codes::E_MOVE_CARRY,
                Some("haves"),
                &["beta"][..],
            ),
            case(
                "beta",
                "# cf-carry:haves",
                "unreachable",
                Reply::Unreachable,
                codes::E_MOVE_CARRY,
                Some("haves"),
                &["beta"][..],
            ),
            // A transport failure on a carry step is still a carry failure:
            // the caller needs `details.step` and "the source was not
            // touched", with the original code kept in `cause_code`.
            Case {
                cause: Some("E_SSH"),
                ..case(
                    "beta",
                    "# cf-carry:haves",
                    "transport",
                    Reply::SpawnError {
                        message: "No such file or directory (os error 2)".into(),
                    },
                    codes::E_MOVE_CARRY,
                    Some("haves"),
                    &["beta"][..],
                )
            },
            Case {
                cause: Some("E_SSH_TIMEOUT"),
                wall_clock: Some(Duration::from_millis(50)),
                ..case(
                    "beta",
                    "# cf-carry:haves",
                    "never answers",
                    Reply::hang(),
                    codes::E_MOVE_CARRY,
                    Some("haves"),
                    &["beta"][..],
                )
            },
            // The same for a step that runs on the long wall clock
            // (`sh_long`, a different transport call).
            Case {
                cause: Some("E_SSH"),
                ..case(
                    "alpha",
                    "# cf-carry:snapshot",
                    "transport",
                    Reply::SpawnError {
                        message: "No such file or directory (os error 2)".into(),
                    },
                    codes::E_MOVE_CARRY,
                    Some("snapshot"),
                    &["alpha", "beta"][..],
                )
            },
            case(
                "alpha",
                "# cf-carry:snapshot",
                "exit",
                Reply::fail(5, &failed("bundle")),
                codes::E_MOVE_CARRY,
                Some("snapshot"),
                &["alpha", "beta"][..],
            ),
            case(
                "alpha",
                "# cf-carry:snapshot",
                "no marker",
                Reply::ok(&snapshot_out(BUNDLE.len(), 0)),
                codes::E_MOVE_CARRY,
                Some("snapshot"),
                &["alpha", "beta"][..],
            ),
            case(
                "alpha",
                "# cf-carry:snapshot",
                "over the cap",
                Reply::fail(8, &format!("{} 999999999", carry::BUNDLE_TOO_LARGE)),
                codes::E_MOVE_TOO_LARGE,
                None,
                &["alpha", "beta"][..],
            ),
            // No marker at all, and a marker with an empty payload: both are
            // a chunk that carried nothing.
            case(
                "alpha",
                "# cf-carry:chunk",
                "no marker",
                Reply::ok(""),
                codes::E_MOVE_CARRY,
                Some("download"),
                &["alpha", "beta"][..],
            ),
            case(
                "alpha",
                "# cf-carry:chunk",
                "empty payload",
                Reply::ok(&out("")),
                codes::E_MOVE_CARRY,
                Some("download"),
                &["alpha", "beta"][..],
            ),
            case(
                "beta",
                "# cf-carry:fetch",
                "exit",
                Reply::fail(5, &failed("verify")),
                codes::E_MOVE_CARRY,
                Some("fetch"),
                &["alpha", "beta"][..],
            ),
            case(
                "beta",
                "# cf-carry:apply",
                "target dirty",
                Reply::fail(9, carry::TARGET_DIRTY),
                codes::E_MOVE_TARGET_DIRTY,
                None,
                &["alpha", "beta"][..],
            ),
            case(
                "beta",
                "# cf-carry:apply",
                "porcelain mismatch",
                Reply::ok(&out("?? surprise.txt\n")),
                codes::E_MOVE_CARRY,
                Some("verify"),
                &["alpha", "beta"][..],
            ),
        ];
        for c in cases {
            let name = format!("{} ({})", c.marker, c.what);
            let f = fixture();
            f.fake
                .on_host(c.host, Match::script_contains(c.marker), c.reply);
            if let Some(w) = c.wall_clock {
                f.fake.set_wall_clock(w);
            }
            let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
            let err = run(&f, &hooks, false).await.unwrap_err();
            assert_eq!(err.code, c.code, "{name}: {}", err.message);
            if let Some(step) = c.step {
                assert_eq!(err.details.as_ref().unwrap()["step"], step, "{name}");
                assert!(
                    err.message.contains("the source session was not touched"),
                    "{name}: {}",
                    err.message
                );
            }
            match c.cause {
                Some(cause) => {
                    assert_eq!(
                        err.details.as_ref().unwrap()["cause_code"],
                        cause,
                        "{name}: {}",
                        err.message
                    );
                    assert!(err.message.contains(cause), "{name}: {}", err.message);
                }
                None => assert!(
                    err.details
                        .as_ref()
                        .is_none_or(|d| d["cause_code"].is_null()),
                    "{name}: no transport cause to report"
                ),
            }
            if c.code == codes::E_MOVE_TOO_LARGE {
                assert_eq!(err.details.as_ref().unwrap()["payload"], "bundle");
            }
            assert!(
                !f.fake
                    .calls_for("beta")
                    .iter()
                    .any(|c| c.script().is_some_and(|s| s.contains("tmux new-session"))),
                "{name}: the target session never started"
            );
            assert_source_untouched(&f, &hooks);
            for host in ["alpha", "beta"] {
                let cleaned = f
                    .fake
                    .calls_for(host)
                    .iter()
                    .any(|c| c.script().is_some_and(|s| s.contains("# cf-carry:cleanup")));
                assert_eq!(
                    cleaned,
                    c.cleaned.contains(&host),
                    "{name}: cleanup on {host}"
                );
                // The Claude-side state comes after every step in this table,
                // so none of its scripts may have gone out either.
                for marker in [
                    "# cf-carry:state-list",
                    "# cf-carry:state-pack",
                    "# cf-carry:state-merge",
                    "# cf-carry:memory-list",
                    "# cf-carry:pack",
                    "# cf-carry:memory-extract",
                    "# cf-carry:memory-index",
                    "# cf-carry:memory-append",
                ] {
                    assert!(
                        scripts_with(&f, host, marker).is_empty(),
                        "{name}: {marker} must not run on {host}"
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn a_clean_source_with_a_newer_target_skips_the_apply_and_warns() {
        let f = fixture();
        let newer = "2222222222222222222222222222222222222222";
        f.fake.on_host(
            "beta",
            Match::script_contains("# cf-move:prep"),
            Reply::ok(&format!("{newer}\t{TGT_ENC}\t{}\t-1\n", tgt_path())),
        );
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let rep = run(&f, &hooks, false).await.expect("move");
        assert!(rep.warnings.iter().any(|w| w.contains("newer on origin")));
        assert!(!f
            .fake
            .calls_for("beta")
            .iter()
            .any(|c| c.script().is_some_and(|s| s.contains("# cf-carry:apply"))));

        // The same with a dirty source cannot replay onto another base.
        let f = fixture();
        f.fake
            .on_host(
                "alpha",
                Match::script_contains("# cf-move:inspect"),
                Reply::ok(&inspection(" M a.rs", HEAD, "0")),
            )
            .on_host(
                "beta",
                Match::script_contains("# cf-move:prep"),
                Reply::ok(&format!("{newer}\t{TGT_ENC}\t{}\t-1\n", tgt_path())),
            );
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let err = run(&f, &hooks, false).await.unwrap_err();
        assert_eq!(err.code, codes::E_MOVE_CARRY);
        assert_eq!(err.details.unwrap()["step"], "apply");
    }

    #[tokio::test]
    async fn an_ignored_files_failure_is_a_warning_not_a_failed_move() {
        let f = fixture();
        let mut listed = out("").into_bytes();
        listed.extend_from_slice(b"4\t.env\0");
        f.fake
            .on_host(
                "alpha",
                Match::script_contains("# cf-carry:ignored-list"),
                Reply::Exit {
                    code: 0,
                    stdout: listed,
                    stderr: Vec::new(),
                },
            )
            .on_host(
                "alpha",
                Match::script_contains("# cf-carry:ignored-pack"),
                Reply::fail(5, &format!("{} tar", carry::FAILED)),
            );
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let rep = run(&f, &hooks, false)
            .await
            .expect("the move still succeeds");
        assert!(rep.carried.ignored_carried.is_empty());
        assert!(
            rep.warnings
                .iter()
                .any(|w| w.contains("ignored files were not carried")),
            "{:?}",
            rep.warnings
        );
    }

    /// Both halves of the Claude-side state, end to end: the per-session
    /// directory is packed from the source's Claude project dir and merged
    /// into the target's, and the memory adds only the file the target lacks
    /// — together with that file's index line, and no other.
    #[tokio::test]
    async fn the_claude_side_state_travels_and_is_reported() {
        let f = fixture();
        with_claude_state(&f);
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let rep = run(&f, &hooks, false).await.expect("move");
        assert!(rep.warnings.is_empty(), "{:?}", rep.warnings);

        let st = &rep.carried.session_state;
        assert_eq!(
            st.carried,
            vec![carry::IgnoredEntry {
                path: "subagents/agent-aa.jsonl".into(),
                bytes: 550
            }]
        );
        assert_eq!(st.kept_target, vec!["tool-results/out1.txt".to_string()]);
        assert!(st.left_behind.is_empty(), "{:?}", st.left_behind);

        let mem = &rep.carried.memory;
        assert_eq!(
            mem.carried,
            vec![carry::IgnoredEntry {
                path: "new.md".into(),
                bytes: 20
            }]
        );
        assert_eq!(mem.kept_target, vec!["differs.md".to_string()]);
        assert_eq!(mem.identical, 0);
        assert_eq!(mem.index_lines_added, 1);

        // The session directory is read where the transcript lives and merged
        // beside the target's copy of it.
        let packs = scripts_with(&f, "alpha", "# cf-carry:state-pack");
        assert_eq!(packs.len(), 1, "one pack of the session directory");
        assert!(
            packs[0].contains(&format!("d={}", quote(SRC_PROJECT_DIR))),
            "{}",
            packs[0]
        );
        let merges = scripts_with(&f, "beta", "# cf-carry:state-merge");
        assert_eq!(merges.len(), 1, "one merge on the target");
        assert!(
            merges[0].contains(&format!("d={}", quote(&tgt_project_dir()))),
            "{}",
            merges[0]
        );
        // The target's memory is keyed by the target project root.
        let listings = scripts_with(&f, "beta", "# cf-carry:memory-list");
        assert_eq!(listings.len(), 1, "one memory listing on the target");
        assert!(
            listings[0].contains(&format!("r={}", quote(TGT_ROOT))),
            "{}",
            listings[0]
        );
        // (E.2) The SOURCE's memory is keyed by the source WORKTREE in both
        // slots: `r=` (whose repo root the script resolves) and `fb=` (the
        // fallback tried when that root has no `memory/`). The target gets no
        // fallback at all.
        let src_listings = scripts_with(&f, "alpha", "# cf-carry:memory-list");
        assert_eq!(src_listings.len(), 1, "one memory listing on the source");
        assert!(
            src_listings[0].contains(&format!(
                "r={}\nfb={}\n",
                quote(SRC_WORKTREE),
                quote(SRC_WORKTREE)
            )),
            "{}",
            src_listings[0]
        );
        assert!(
            listings[0].contains(&format!("fb={}\n", quote(""))),
            "the target has no fallback: {}",
            listings[0]
        );
        // Exactly one append, carrying the carried file's line and no other.
        let appends = scripts_with(&f, "beta", "# cf-carry:memory-append");
        assert_eq!(appends.len(), 1, "exactly one index append");
        assert!(appends[0].contains("(new.md)"), "{}", appends[0]);
        assert!(!appends[0].contains("differs.md"), "{}", appends[0]);

        // Both reports travel on the timeline with the move.
        let ev = events(&f, f.source_id);
        let (_, detail) = ev
            .iter()
            .find(|(k, _)| k == EVENT_MOVED)
            .expect("session_moved");
        let d: serde_json::Value = serde_json::from_str(detail.as_deref().unwrap()).unwrap();
        assert_eq!(
            d["carried"]["session_state"]["carried"][0]["path"],
            "subagents/agent-aa.jsonl"
        );
        assert_eq!(
            d["carried"]["session_state"]["kept_target"][0],
            "tool-results/out1.txt"
        );
        assert_eq!(d["carried"]["memory"]["carried"][0]["path"], "new.md");
        assert_eq!(d["carried"]["memory"]["index_lines_added"], 1);
    }

    /// Neither half may fail a move, and neither may take the other down
    /// with it: whatever breaks, the move completes with exactly one warning
    /// and the other half's report is whole.
    #[tokio::test]
    async fn each_half_failing_is_a_warning_and_the_other_half_still_runs() {
        const STATE: &str = "session state was not carried:";
        const MEMORY: &str = "project memory was not carried:";
        struct Case {
            host: &'static str,
            /// The Claude-state script whose reply is replaced.
            marker: &'static str,
            /// What distinguishes this case when the marker repeats.
            what: &'static str,
            reply: Reply,
            /// The warning this row must produce — and the only one.
            half: &'static str,
        }
        let case = |host, marker, what, reply, half| Case {
            host,
            marker,
            what,
            reply,
            half,
        };
        let failed = |what: &str| format!("{} {what}", carry::FAILED);
        let cases = vec![
            case(
                "alpha",
                "# cf-carry:state-list",
                "exit",
                Reply::fail(5, &failed("cd")),
                STATE,
            ),
            // The transport itself: still only a warning.
            case(
                "alpha",
                "# cf-carry:state-list",
                "unreachable",
                Reply::Unreachable,
                STATE,
            ),
            case(
                "alpha",
                "# cf-carry:state-pack",
                "exit",
                Reply::fail(5, &failed("tar")),
                STATE,
            ),
            case(
                "beta",
                "# cf-carry:state-merge",
                "exit",
                Reply::fail(5, &failed("extract")),
                STATE,
            ),
            case(
                "beta",
                "# cf-carry:state-merge",
                "no marker",
                Reply::ok("carried\t550\tsubagents/agent-aa.jsonl\n"),
                STATE,
            ),
            // A file the merge could not place is a warning of its own, even
            // though the script itself succeeded.
            case(
                "beta",
                "# cf-carry:state-merge",
                "a file could not be placed",
                Reply::ok(&out(
                    "carried\t550\tsubagents/agent-aa.jsonl\nfailed\ttool-results/out1.txt\n",
                )),
                STATE,
            ),
            case(
                "alpha",
                "# cf-carry:memory-list",
                "exit",
                Reply::fail(5, &failed("not-a-repo")),
                MEMORY,
            ),
            case(
                "beta",
                "# cf-carry:memory-list",
                "exit",
                Reply::fail(5, &failed("cd")),
                MEMORY,
            ),
            case(
                "alpha",
                "# cf-carry:pack",
                "exit",
                Reply::fail(5, &failed("tar")),
                MEMORY,
            ),
            case(
                "beta",
                "# cf-carry:memory-extract",
                "exit",
                Reply::fail(5, &failed("corrupt archive")),
                MEMORY,
            ),
            case(
                "beta",
                "# cf-carry:memory-append",
                "exit",
                Reply::fail(5, &failed("append")),
                MEMORY,
            ),
        ];
        for c in cases {
            let name = format!("{} ({})", c.marker, c.what);
            let f = fixture();
            with_claude_state(&f);
            f.fake
                .on_host(c.host, Match::script_contains(c.marker), c.reply);
            let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
            let rep = run(&f, &hooks, false)
                .await
                .unwrap_or_else(|e| panic!("{name}: the move must still succeed: {}", e.message));
            let other = if c.half == STATE { MEMORY } else { STATE };
            assert_eq!(
                rep.warnings
                    .iter()
                    .filter(|w| w.starts_with(c.half))
                    .count(),
                1,
                "{name}: {:?}",
                rep.warnings
            );
            assert!(
                !rep.warnings.iter().any(|w| w.starts_with(other)),
                "{name}: {:?}",
                rep.warnings
            );
            if c.half == STATE {
                assert_eq!(rep.carried.memory.carried.len(), 1, "{name}");
                assert_eq!(rep.carried.memory.kept_target.len(), 1, "{name}");
                assert_eq!(rep.carried.memory.index_lines_added, 1, "{name}");
            } else {
                assert_eq!(rep.carried.session_state.carried.len(), 1, "{name}");
                assert_eq!(rep.carried.session_state.kept_target.len(), 1, "{name}");
            }
        }
    }

    /// The common case: no per-session directory and no project memory is not
    /// a problem to report, and nothing is packed or relayed for it.
    #[tokio::test]
    async fn a_source_with_no_claude_state_sends_no_pack_and_warns_nothing() {
        let f = fixture();
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let rep = run(&f, &hooks, false).await.expect("move");
        assert!(rep.warnings.is_empty(), "{:?}", rep.warnings);
        assert_eq!(
            rep.carried.session_state,
            carry::SessionStateReport::default()
        );
        assert_eq!(rep.carried.memory, carry::MemoryReport::default());
        for marker in [
            "# cf-carry:state-pack",
            "# cf-carry:state-merge",
            "# cf-carry:pack",
            "# cf-carry:memory-extract",
            "# cf-carry:memory-index",
            "# cf-carry:memory-append",
        ] {
            for host in ["alpha", "beta"] {
                assert!(
                    scripts_with(&f, host, marker).is_empty(),
                    "{marker} must not run on {host}"
                );
            }
        }
    }

    /// An index too large to read is left alone — but the files it describes
    /// have already travelled, and stay reported as carried.
    #[tokio::test]
    async fn an_index_over_the_read_bound_is_left_alone_with_a_warning() {
        let f = fixture();
        with_claude_state(&f);
        let huge = "x".repeat(claude_state::INDEX_READ_MAX_BYTES as usize + 1);
        f.fake.on_host(
            "beta",
            Match::script_contains("# cf-carry:memory-index"),
            Reply::ok(&out(&format!("present\n{huge}"))),
        );
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let rep = run(&f, &hooks, false)
            .await
            .expect("the move still succeeds");
        assert_eq!(rep.carried.memory.carried.len(), 1);
        assert_eq!(rep.carried.memory.index_lines_added, 0);
        assert!(
            scripts_with(&f, "beta", "# cf-carry:memory-append").is_empty(),
            "the index is left alone"
        );
        let warned: Vec<&String> = rep
            .warnings
            .iter()
            .filter(|w| w.starts_with("project memory was not carried:"))
            .collect();
        assert_eq!(warned.len(), 1, "{:?}", rep.warnings);
        assert!(warned[0].contains("index"), "{}", warned[0]);
    }

    /// (A.2) The merge's report must account for every path the selection
    /// chose to carry. A reply that simply omits one — an archive without
    /// `./<id>` whose `cd` failed, a file that vanished between the listing
    /// and the pack, a collateral `[!/]` exclude — is a warning, and the
    /// files that DID land stay reported.
    #[tokio::test]
    async fn a_merge_that_omits_a_selected_path_warns_and_still_reports_what_landed() {
        let f = fixture();
        with_claude_state(&f);
        // The selection chose both files; the merge mentions only one.
        f.fake.on_host(
            "beta",
            Match::script_contains("# cf-carry:state-merge"),
            Reply::ok(&out("carried\t550\tsubagents/agent-aa.jsonl\n")),
        );
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let rep = run(&f, &hooks, false)
            .await
            .expect("the move still succeeds");
        let warned: Vec<&String> = rep
            .warnings
            .iter()
            .filter(|w| w.starts_with("session state was not carried:"))
            .collect();
        assert_eq!(warned.len(), 1, "{:?}", rep.warnings);
        assert!(
            warned[0].contains("tool-results/out1.txt") && warned[0].contains("1 selected"),
            "{}",
            warned[0]
        );
        assert_eq!(
            rep.carried.session_state.carried,
            vec![carry::IgnoredEntry {
                path: "subagents/agent-aa.jsonl".into(),
                bytes: 550
            }],
            "what did land is still reported"
        );
        // The memory half is untouched by it.
        assert_eq!(rep.carried.memory.carried.len(), 1);
    }

    /// (A.2, the other direction) An empty merge report for a selection that
    /// carried nothing at all — the shape an archive without `./<id>`
    /// produces — warns instead of returning `Ok` with an empty report.
    #[tokio::test]
    async fn a_merge_that_reports_nothing_at_all_warns() {
        let f = fixture();
        with_claude_state(&f);
        f.fake.on_host(
            "beta",
            Match::script_contains("# cf-carry:state-merge"),
            Reply::ok(&out("")),
        );
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let rep = run(&f, &hooks, false).await.expect("move");
        let warned: Vec<&String> = rep
            .warnings
            .iter()
            .filter(|w| w.starts_with("session state was not carried:"))
            .collect();
        assert_eq!(warned.len(), 1, "{:?}", rep.warnings);
        assert!(warned[0].contains("2 selected"), "{}", warned[0]);
        assert!(rep.carried.session_state.carried.is_empty());
    }

    /// (A.3 / H1) An exit-0 listing WITHOUT the marker is not "there is no
    /// session directory" — the script prints the marker on every successful
    /// path, including that one. Something swallowed the output, so the half
    /// warns rather than silently carrying nothing.
    #[tokio::test]
    async fn a_marker_less_state_listing_warns_instead_of_carrying_nothing() {
        let f = fixture();
        with_claude_state(&f);
        f.fake.on_host(
            "alpha",
            Match::script_contains("# cf-carry:state-list"),
            Reply::ok("550\tsubagents/agent-aa.jsonl\0"),
        );
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let rep = run(&f, &hooks, false).await.expect("move");
        let warned: Vec<&String> = rep
            .warnings
            .iter()
            .filter(|w| w.starts_with("session state was not carried:"))
            .collect();
        assert_eq!(warned.len(), 1, "{:?}", rep.warnings);
        assert!(warned[0].contains("said nothing"), "{}", warned[0]);
        for marker in ["# cf-carry:state-pack", "# cf-carry:state-merge"] {
            for host in ["alpha", "beta"] {
                assert!(
                    scripts_with(&f, host, marker).is_empty(),
                    "{marker} must not run on {host}"
                );
            }
        }
        // The memory half still ran in full.
        assert_eq!(rep.carried.memory.carried.len(), 1);
    }

    /// (B / H2) Neither half takes the source's word for the size of what it
    /// is about to pull: an announced archive over the bound is a warning
    /// and NOTHING is downloaded — no `# cf-carry:chunk` script is sent.
    #[tokio::test]
    async fn an_oversized_announced_archive_is_never_downloaded() {
        struct Case {
            half: &'static str,
            marker: &'static str,
            bytes: u64,
            warning: &'static str,
        }
        let cases = [
            Case {
                half: "session",
                marker: "# cf-carry:state-pack",
                // the default 200 MiB cap plus more than the tar allowance
                bytes: 200 * 1024 * 1024 + claude_state::PACK_OVERHEAD_ALLOWANCE_BYTES + 1,
                warning: "session state was not carried:",
            },
            Case {
                half: "memory",
                marker: "# cf-carry:pack",
                bytes: claude_state::MEMORY_TOTAL_MAX_BYTES
                    + claude_state::PACK_OVERHEAD_ALLOWANCE_BYTES
                    + 1,
                warning: "project memory was not carried:",
            },
        ];
        for c in cases {
            let f = fixture();
            with_claude_state(&f);
            let archive = if c.half == "session" {
                format!("{SRC_DIR}/state.tgz")
            } else {
                format!("{SRC_DIR}/memory.tgz")
            };
            f.fake.on_host(
                "alpha",
                Match::script_contains(c.marker),
                Reply::ok(&out(&format!("{}\t{archive}\n", c.bytes))),
            );
            let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
            let rep = run(&f, &hooks, false).await.unwrap_or_else(|e| {
                panic!("{}: the move must still succeed: {}", c.half, e.message)
            });
            let warned: Vec<&String> = rep
                .warnings
                .iter()
                .filter(|w| w.starts_with(c.warning))
                .collect();
            assert_eq!(warned.len(), 1, "{}: {:?}", c.half, rep.warnings);
            assert!(
                warned[0].contains("over the") && warned[0].contains("nothing was relayed"),
                "{}: {}",
                c.half,
                warned[0]
            );
            // The git bundle is relayed earlier in the same move, so chunk
            // scripts do go out — none of them may name THIS archive.
            let quoted = quote(&archive);
            assert!(
                scripts_with(&f, "alpha", "# cf-carry:chunk")
                    .iter()
                    .all(|s| !s.contains(&quoted)),
                "{}: {archive} must never be downloaded",
                c.half
            );
        }
    }

    /// The cap is the setting, in MiB: over it, the largest files stay behind
    /// as `--exclude`s and are reported.
    #[tokio::test]
    async fn the_session_state_cap_comes_from_the_setting() {
        const BIG: u64 = 2 * 1024 * 1024;
        let f = fixture();
        with_claude_state(&f);
        f.store
            .lock()
            .unwrap()
            .set_setting(claude_state::SETTING_MAX_SESSION_STATE_MB, "1")
            .unwrap();
        f.fake
            .on_host(
                "alpha",
                Match::script_contains("# cf-carry:state-list"),
                records_out(&[
                    format!("{BIG}\tsubagents/big.jsonl"),
                    "10240\tcustom-title.json".into(),
                ]),
            )
            .on_host(
                "beta",
                Match::script_contains("# cf-carry:state-merge"),
                Reply::ok(&out("carried\t10240\tcustom-title.json\n")),
            );
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let rep = run(&f, &hooks, false).await.expect("move");
        let packs = scripts_with(&f, "alpha", "# cf-carry:state-pack");
        assert_eq!(packs.len(), 1);
        assert!(
            packs[0].contains(&quote(&format!("--exclude=./{SID}/subagents/big.jsonl"))),
            "{}",
            packs[0]
        );
        assert!(!packs[0].contains("custom-title.json"), "{}", packs[0]);
        assert_eq!(
            rep.carried.session_state.left_behind,
            vec![carry::LeftBehind {
                path: "subagents/big.jsonl".into(),
                bytes: Some(BIG),
                reason: carry::LeftReason::OverCap,
            }]
        );
        assert_eq!(
            rep.carried.session_state.carried,
            vec![carry::IgnoredEntry {
                path: "custom-title.json".into(),
                bytes: 10240
            }]
        );
    }

    /// (E.1) The half's stated central invariant: every `Err` carries the
    /// best report built so far, `left_behind` above all. With the cap
    /// forcing an exclusion AND the pack then failing, the over-cap file
    /// must still be reported as left behind — otherwise the warning says
    /// "not carried" and the report silently forgets what was dropped and
    /// why.
    #[tokio::test]
    async fn a_pack_failure_still_reports_what_the_cap_left_behind() {
        const BIG: u64 = 2 * 1024 * 1024;
        let f = fixture();
        with_claude_state(&f);
        f.store
            .lock()
            .unwrap()
            .set_setting(claude_state::SETTING_MAX_SESSION_STATE_MB, "1")
            .unwrap();
        f.fake
            .on_host(
                "alpha",
                Match::script_contains("# cf-carry:state-list"),
                records_out(&[
                    format!("{BIG}\tsubagents/big.jsonl"),
                    "10240\tcustom-title.json".into(),
                ]),
            )
            .on_host(
                "alpha",
                Match::script_contains("# cf-carry:state-pack"),
                Reply::fail(5, &format!("{} tar", carry::FAILED)),
            );
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let rep = run(&f, &hooks, false)
            .await
            .expect("the move still succeeds");
        let warned: Vec<&String> = rep
            .warnings
            .iter()
            .filter(|w| w.starts_with("session state was not carried:"))
            .collect();
        assert_eq!(warned.len(), 1, "{:?}", rep.warnings);
        assert_eq!(
            rep.carried.session_state.left_behind,
            vec![carry::LeftBehind {
                path: "subagents/big.jsonl".into(),
                bytes: Some(BIG),
                reason: carry::LeftReason::OverCap,
            }],
            "the over-cap file is still reported although the pack failed"
        );
        assert!(rep.carried.session_state.carried.is_empty());
        assert!(rep.carried.session_state.kept_target.is_empty());
    }

    /// The relay loop itself: a payload larger than one chunk is pulled in
    /// order, the last request asks only for what is left, a chunk shorter
    /// than requested resumes from where it really ended (no skipped bytes),
    /// and more bytes than announced is a failure rather than a silent
    /// oversized file.
    #[tokio::test]
    async fn the_chunked_download_walks_the_payload_in_order() {
        const PAYLOAD: &str = "ABCDEFGHIJKLM"; // 13 bytes
        const REMOTE: &str = "/tmp/carry.bundle";
        /// Answer the request at `offset` (as the script spells it) with
        /// `reply` bytes.
        fn at(fake: &FakeSsh, offset: u64, reply: &str) {
            fake.on_host(
                "h",
                Match::script_contains(&format!("tail -c +{} ", offset + 1)),
                Reply::ok(&out(reply)),
            );
        }
        let chunk_scripts = |fake: &FakeSsh| -> Vec<String> {
            fake.calls_for("h")
                .iter()
                .filter_map(|c| c.script())
                .filter(|s| s.contains("# cf-carry:chunk"))
                .collect()
        };
        let expected = |pairs: &[(u64, u64)]| -> Vec<String> {
            pairs
                .iter()
                .map(|(off, len)| carry::chunk_script(REMOTE, *off, *len))
                .collect()
        };

        // Whole chunks, then a short final one asking for the remainder.
        let fake = FakeSsh::new();
        at(&fake, 0, "ABCDE");
        at(&fake, 5, "FGHIJ");
        at(&fake, 10, "KLM");
        let got = download_chunked(&fake, "h", REMOTE, PAYLOAD.len() as u64, "bundle", 5)
            .await
            .expect("the whole payload");
        assert_eq!(std::fs::read_to_string(&got.0).unwrap(), PAYLOAD);
        assert_eq!(
            chunk_scripts(&fake),
            expected(&[(0, 5), (5, 5), (10, 3)]),
            "offsets 0, 5, 10 and a 3-byte tail"
        );

        // A chunk shorter than requested: the next one resumes at 2, not 5.
        let fake = FakeSsh::new();
        at(&fake, 0, "AB");
        at(&fake, 2, "CDEFG");
        at(&fake, 7, "HIJKL");
        at(&fake, 12, "M");
        let got = download_chunked(&fake, "h", REMOTE, PAYLOAD.len() as u64, "bundle", 5)
            .await
            .expect("the whole payload");
        assert_eq!(std::fs::read_to_string(&got.0).unwrap(), PAYLOAD);
        assert_eq!(
            chunk_scripts(&fake),
            expected(&[(0, 5), (2, 5), (7, 5), (12, 1)])
        );

        // More than was announced: refused, not written off as fine.
        let fake = FakeSsh::new();
        at(&fake, 0, PAYLOAD);
        // (`TempFile` is deliberately not `Debug`, so no `unwrap_err`.)
        let err = match download_chunked(&fake, "h", REMOTE, 5, "bundle", 5).await {
            Ok(_) => panic!("more bytes than announced must be refused"),
            Err(e) => e,
        };
        assert_eq!(err.code, codes::E_MOVE_CARRY, "{}", err.message);
        assert_eq!(err.details.unwrap()["step"], "download");
        assert!(
            err.message.contains("gave 13 bytes, expected 5"),
            "{}",
            err.message
        );
    }

    /// Every carry script runs under `bash -lc`, so a login profile can print
    /// a banner before the script body ever does: the parsers anchor on the
    /// output marker, and the relayed bundle must be the payload alone.
    /// (Only the carry scripts are marker-protected; inspect, locate and
    /// read are not — pre-existing, and out of this test's scope.)
    #[tokio::test]
    async fn a_login_banner_before_every_carry_script_does_not_break_the_move() {
        let f = fixture();
        let banner = |payload: &str| format!("Welcome to alpha!\n{}", out(payload));
        f.fake
            .on_host(
                "beta",
                Match::script_contains("# cf-carry:seed"),
                Reply::ok(&banner("existing\n")),
            )
            .on_host(
                "beta",
                Match::script_contains("# cf-carry:haves"),
                Reply::ok(&banner(&format!("{TGT_DIR}\n{HEAD}\n"))),
            )
            .on_host(
                "alpha",
                Match::script_contains("# cf-carry:snapshot"),
                Reply::ok(&banner(&snapshot_out(BUNDLE.len(), 0))),
            )
            .on_host(
                "alpha",
                Match::script_contains("# cf-carry:chunk"),
                Reply::ok(&banner(BUNDLE)),
            )
            .on_host(
                "beta",
                Match::script_contains("# cf-carry:apply"),
                Reply::ok(&banner("")),
            )
            .on_host(
                "alpha",
                Match::script_contains("# cf-carry:ignored-list"),
                Reply::ok(&banner("")),
            );
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let rep = run(&f, &hooks, false).await.expect("move");
        assert_eq!(rep.carried.commits, 0);
        assert_eq!(rep.carried.bundle_bytes, BUNDLE.len() as u64);
        assert_eq!(rep.carried.target_seeded, carry::TargetSeed::Existing);
        let uploads: Vec<_> = f
            .fake
            .calls_for("beta")
            .into_iter()
            .filter(|c| c.stdin.is_some())
            .collect();
        assert_eq!(uploads[0].stdin_str().as_deref(), Some(BUNDLE));
    }

    #[tokio::test]
    async fn transcript_over_the_cap_is_refused_before_it_is_read() {
        let f = fixture();
        f.store
            .lock()
            .unwrap()
            .set_setting(SETTING_MAX_TRANSCRIPT_MB, "1")
            .unwrap();
        f.fake.on_host(
            "alpha",
            Match::script_contains("# cf-move:locate"),
            Reply::ok(&locate_out(2 * 1024 * 1024, MTIME)),
        );
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let err = run(&f, &hooks, false).await.unwrap_err();
        assert_eq!(err.code, codes::E_MOVE_TOO_LARGE);
        let details = err.details.unwrap();
        assert_eq!(details["cap_bytes"], 1024 * 1024);
        // The same code now also reports an over-cap bundle.
        assert_eq!(details["payload"], "transcript");
        assert!(
            f.fake
                .calls_for("alpha")
                .iter()
                .all(|c| !c.script().unwrap_or_default().contains("# cf-move:read")),
            "the body is never read"
        );
        assert!(f.fake.calls_for("beta").is_empty());
        assert_source_untouched(&f, &hooks);
    }

    #[tokio::test]
    async fn target_that_fails_to_start_leaves_the_source_untouched() {
        let f = fixture();
        f.fake.on_host(
            "beta",
            Match::script_contains("tmux new-session"),
            Reply::fail(1, "duplicate session: dev-o-r--feat"),
        );
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let err = run(&f, &hooks, false).await.unwrap_err();
        assert_eq!(err.code, "E_TMUX", "{}", err.message);
        assert!(
            err.message.contains("source session was not touched"),
            "{}",
            err.message
        );
        assert_source_untouched(&f, &hooks);
        let s = f.store.lock().unwrap();
        assert!(s.get_session("dev-o-r--feat", "beta").unwrap().is_none());
    }

    #[tokio::test]
    async fn target_never_confirmed_is_partial_and_keeps_both() {
        let f = fixture();
        let mut hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        hooks.target_status = "ghost";
        let err = run(&f, &hooks, false).await.unwrap_err();
        assert_eq!(err.code, codes::E_MOVE_PARTIAL, "{}", err.message);
        let d = err.details.expect("details");
        assert!(d["target_session_id"].is_i64());
        assert_eq!(d["target_host"], "beta");
        assert!(err.message.contains("source is untouched"));
        assert_source_untouched(&f, &hooks);
    }

    #[tokio::test]
    async fn kill_failure_is_partial_with_the_target_id() {
        let f = fixture();
        let mut hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        hooks.kill_fails = true;
        let err = run(&f, &hooks, false).await.unwrap_err();
        assert_eq!(err.code, codes::E_MOVE_PARTIAL, "{}", err.message);
        let d = err.details.expect("details");
        let tid = d["target_session_id"].as_i64().expect("target id set");
        assert_eq!(d["cause_code"], "E_TMUX");
        assert!(d["step"]
            .as_str()
            .unwrap()
            .starts_with("killing the source"));
        // The target is registered and running; the source row is intact.
        let s = f.store.lock().unwrap();
        let t = s.get_session_by_id(tid).unwrap().unwrap();
        assert_eq!(t.host_alias, "beta");
        assert_eq!(t.status, "running");
        assert!(s.get_session_by_id(f.source_id).unwrap().is_some());
    }

    #[tokio::test]
    async fn source_that_wrote_after_the_copy_is_not_killed() {
        let f = fixture();
        let mut hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        hooks.grow_source_on_start = true;
        let err = run(&f, &hooks, false).await.unwrap_err();
        assert_eq!(err.code, codes::E_MOVE_PARTIAL, "{}", err.message);
        let d = err.details.expect("details");
        assert_eq!(d["step"], "source transcript changed after copy");
        assert!(d["target_session_id"].is_i64());
        assert!(
            err.message.contains("retry move_session"),
            "{}",
            err.message
        );
        assert!(
            !hooks.log().iter().any(|l| l.starts_with("kill")),
            "the source must stay alive: {:?}",
            hooks.log()
        );
        // keep_source never re-checks: the user keeps both on purpose.
        let f = fixture();
        let mut hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        hooks.grow_source_on_start = true;
        assert!(run(&f, &hooks, true).await.is_ok());
    }

    #[tokio::test]
    async fn busy_or_unknown_source_status_is_refused_before_anything_runs() {
        for status in ["working", "blocked"] {
            let f = fixture();
            f.store
                .lock()
                .unwrap()
                .set_claude_status_by_session_id(SID, status)
                .unwrap();
            let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
            let err = run(&f, &hooks, false).await.unwrap_err();
            assert_eq!(
                err.code,
                codes::E_INVALID_STATE,
                "{status}: {}",
                err.message
            );
            assert!(err.message.contains("not idle"), "{}", err.message);
            assert!(f.fake.calls().is_empty(), "{status}: no ssh at all");
        }
        assert!(require_source_idle(None)
            .unwrap_err()
            .message
            .contains("unknown"));
        for ok in ["idle", "completed", "stopped", "failed"] {
            assert!(require_source_idle(Some(ok)).is_ok(), "{ok}");
        }
    }

    #[tokio::test]
    async fn diverged_target_worktree_is_refused_before_the_copy() {
        let f = fixture();
        f.fake.on_host(
            "beta",
            Match::script_contains("# cf-move:prep"),
            Reply::fail(
                6,
                &format!("{DIVERGED} 2222222222222222222222222222222222222222"),
            ),
        );
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let err = run(&f, &hooks, false).await.unwrap_err();
        assert_eq!(err.code, codes::E_GIT, "{}", err.message);
        assert!(err.message.contains("diverged"), "{}", err.message);
        assert!(err.message.contains("source session was not touched"));
        let beta = f.fake.calls_for("beta");
        // The carry's bundle goes up before prep; the transcript never does.
        assert!(
            !beta
                .iter()
                .any(|c| c.stdin.is_some() && c.command().contains(".jsonl")),
            "the transcript was never copied"
        );
        assert!(beta
            .iter()
            .all(|c| !c.script().unwrap_or_default().contains("tmux new-session")));
        assert_source_untouched(&f, &hooks);
    }

    /// (I5 minimum) A target that is merely BEHIND the source and has
    /// uncommitted changes is not diverged — most often it is the copy this
    /// very session left behind when it moved away from that host. Telling
    /// the user to "reconcile the branch" sends them after a problem they
    /// do not have.
    #[tokio::test]
    async fn a_behind_and_dirty_target_is_reported_as_dirty_not_diverged() {
        let f = fixture();
        f.fake.on_host(
            "beta",
            Match::script_contains("# cf-move:prep"),
            Reply::fail(9, carry::TARGET_DIRTY),
        );
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let err = run(&f, &hooks, false).await.unwrap_err();
        assert_eq!(err.code, codes::E_MOVE_TARGET_DIRTY, "{}", err.message);
        assert!(
            err.message.contains("moved away from this host"),
            "the third case is named: {}",
            err.message
        );
        assert!(
            err.message.contains("never overwrites")
                && err.message.contains("source session was not touched"),
            "{}",
            err.message
        );
        assert!(!err.message.contains("diverged"), "{}", err.message);
        assert_source_untouched(&f, &hooks);
    }

    /// The same verdict through the real `target_prep_script`: behind and
    /// dirty is `TARGET_DIRTY`, a genuine divergence is still `DIVERGED`,
    /// and a clean target that is merely behind still fast-forwards.
    #[test]
    fn target_prep_script_separates_a_dirty_target_from_a_diverged_one() {
        if !carry::tests::require(&["git", "bash"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();

        let git = |args: &[&str]| -> std::process::Output {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(&repo)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .env("HOME", &home)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            out
        };
        let sha = |rev: &str| {
            String::from_utf8_lossy(&git(&["rev-parse", rev]).stdout)
                .trim()
                .to_string()
        };
        let prep = |want: &str| -> std::process::Output {
            std::process::Command::new("bash")
                .arg("-c")
                .arg(target_prep_script(
                    repo.to_str().unwrap(),
                    want,
                    "feat",
                    SID,
                ))
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("HOME", &home)
                .output()
                .unwrap()
        };

        git(&["init", "-q", "-b", "feat"]);
        std::fs::write(repo.join("f.txt"), "one\n").unwrap();
        git(&["add", "f.txt"]);
        git(&["commit", "-q", "-m", "one"]);
        let first = sha("HEAD");
        std::fs::write(repo.join("f.txt"), "two\n").unwrap();
        git(&["commit", "-q", "-am", "two"]);
        let want = sha("HEAD");
        git(&["branch", "later"]); // keep `want` reachable

        // Clean and behind: still fast-forwarded, as before.
        git(&["checkout", "-q", "-B", "feat", &first]);
        let out = prep(&want);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(sha("HEAD"), want, "a clean behind target fast-forwards");

        // Behind AND dirty: the target's own uncommitted work, not a
        // divergence — and nothing is merged over it.
        git(&["checkout", "-q", "-B", "feat", &first]);
        std::fs::write(repo.join("f.txt"), "local edit\n").unwrap();
        let out = prep(&want);
        let err = String::from_utf8_lossy(&out.stderr).into_owned();
        assert_eq!(out.status.code(), Some(9), "{err}");
        assert!(err.contains(carry::TARGET_DIRTY), "{err}");
        assert!(!err.contains(DIVERGED), "{err}");
        assert_eq!(sha("HEAD"), first, "nothing was merged onto the dirty tree");
        assert_eq!(
            std::fs::read_to_string(repo.join("f.txt")).unwrap(),
            "local edit\n"
        );

        // A real divergence keeps DIVERGED.
        git(&["commit", "-q", "-am", "local"]);
        let out = prep(&want);
        let err = String::from_utf8_lossy(&out.stderr).into_owned();
        assert_eq!(out.status.code(), Some(6), "{err}");
        assert!(err.contains(DIVERGED), "{err}");
    }

    /// The source script enforces `move.max_bundle_mb`, but the orchestrator
    /// must not take a source's word for it.
    #[tokio::test]
    async fn a_bundle_over_the_cap_is_refused_by_the_orchestrator_too() {
        let f = fixture();
        f.store
            .lock()
            .unwrap()
            .set_setting(carry::SETTING_MAX_BUNDLE_MB, "1")
            .unwrap();
        f.fake.on_host(
            "alpha",
            Match::script_contains("# cf-carry:snapshot"),
            Reply::ok(&out(&snapshot_out(2 * 1024 * 1024, 0))),
        );
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let err = run(&f, &hooks, false).await.unwrap_err();
        assert_eq!(err.code, codes::E_MOVE_TOO_LARGE, "{}", err.message);
        let d = err.details.expect("details");
        assert_eq!(d["payload"], "bundle");
        assert_eq!(d["cap_bytes"], 1024 * 1024);
        // Nothing of the bundle was pulled down or pushed up.
        assert!(
            !f.fake
                .calls_for("alpha")
                .iter()
                .any(|c| c.script().is_some_and(|s| s.contains("# cf-carry:chunk"))),
            "the bundle is never downloaded"
        );
        assert_source_untouched(&f, &hooks);
    }

    #[tokio::test]
    async fn a_second_concurrent_move_of_the_same_session_is_refused() {
        let f = fixture();
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let claim = MoveClaim::acquire(&f.store, f.source_id).unwrap();
        let err = run(&f, &hooks, false).await.unwrap_err();
        assert_eq!(err.code, codes::E_INVALID_STATE);
        assert!(
            err.message.contains("already in progress"),
            "{}",
            err.message
        );
        assert!(f.fake.calls().is_empty());
        drop(claim);
        // Released on drop, including after a finished move.
        run(&f, &hooks, true).await.expect("move after release");
        assert!(MoveClaim::acquire(&f.store, f.source_id).is_ok());
    }

    #[tokio::test]
    async fn preflight_refuses_wrong_kinds_and_offline_or_unprovisioned_targets() {
        let f = fixture();
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        {
            let s = f.store.lock().unwrap();
            s.set_host_provisioned("beta", false).unwrap();
        }
        let err = run(&f, &hooks, false).await.unwrap_err();
        assert_eq!(err.code, codes::E_INVALID_STATE);
        assert!(err.message.contains("provision"));
        {
            let s = f.store.lock().unwrap();
            s.set_host_provisioned("beta", true).unwrap();
            s.update_host_probe("beta", false, None, None, 2).unwrap();
        }
        assert_eq!(
            run(&f, &hooks, false).await.unwrap_err().code,
            codes::E_HOST_OFFLINE
        );
        // Same host → E_INVALID.
        let same = MoveSessionArgs {
            session_id: f.source_id,
            target_host_alias: "alpha".into(),
            keep_source: false,
            strict: false,
        };
        assert_eq!(
            move_session_with(same, &f.store, &f.fake, &hooks, fast())
                .await
                .unwrap_err()
                .code,
            codes::E_INVALID
        );
        // A shell session has no conversation to carry.
        f.store
            .lock()
            .unwrap()
            .set_session_kind(f.source_id, "shell", None)
            .unwrap();
        assert_eq!(
            run(&f, &hooks, false).await.unwrap_err().code,
            codes::E_INVALID_STATE
        );
        assert!(f.fake.calls().iter().all(|c| c.host != "beta"));
    }

    #[tokio::test]
    async fn a_larger_existing_target_transcript_is_never_overwritten() {
        let f = fixture();
        f.fake.on_host(
            "beta",
            Match::script_contains("# cf-move:prep"),
            Reply::ok(&format!("{HEAD}\t{TGT_ENC}\t{}\t99999\n", tgt_path())),
        );
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let err = run(&f, &hooks, false).await.unwrap_err();
        assert_eq!(err.code, codes::E_INVALID_STATE, "{}", err.message);
        // The carry's bundle goes up before prep; the transcript never does.
        assert!(
            !f.fake
                .calls_for("beta")
                .iter()
                .any(|c| c.stdin.is_some() && c.command().contains(".jsonl")),
            "the transcript was never copied"
        );
        assert_source_untouched(&f, &hooks);
    }

    // ── pure helpers ──

    #[test]
    fn encoding_matches_claude_code() {
        assert_eq!(
            encode_project_dir("/home/b/p/o/r/.claude/worktrees/feat"),
            TGT_ENC
        );
    }

    #[test]
    fn cap_defaults_and_parses() {
        assert_eq!(max_transcript_bytes(None), 200 * 1024 * 1024);
        assert_eq!(max_transcript_bytes(Some("0")), 200 * 1024 * 1024);
        assert_eq!(max_transcript_bytes(Some("junk")), 200 * 1024 * 1024);
        assert_eq!(max_transcript_bytes(Some(" 5 ")), 5 * 1024 * 1024);
    }

    #[test]
    fn partial_last_line_is_dropped() {
        let mut b = b"{\"a\":1}\n{\"b\":".to_vec();
        trim_to_last_newline(&mut b);
        assert_eq!(b, b"{\"a\":1}\n");
        let mut none = b"no newline".to_vec();
        trim_to_last_newline(&mut none);
        assert_eq!(none, b"no newline");
    }

    #[test]
    fn target_name_avoids_collisions() {
        assert_eq!(pick_target_name("x", &[]).unwrap(), "x");
        assert_eq!(pick_target_name("x", &["x".into()]).unwrap(), "x-moved");
        let taken: Vec<String> = ["x", "x-moved", "x-moved-2"].map(String::from).to_vec();
        assert_eq!(pick_target_name("x", &taken).unwrap(), "x-moved-3");
    }

    #[test]
    fn target_prep_output_is_validated() {
        let ok =
            parse_target_prep(&format!("{HEAD}\t{TGT_ENC}\t{}\t-1\n", tgt_path()), SID).unwrap();
        assert_eq!(ok.path, tgt_path());
        assert_eq!(ok.existing, -1);
        // A path that is not under ~/.claude/projects/<enc>/<id>.jsonl.
        assert!(parse_target_prep(&format!("{HEAD}\t{TGT_ENC}\t/etc/passwd\t-1"), SID).is_err());
        assert!(parse_target_prep(
            &format!("{HEAD}\t../x\t/h/.claude/projects/../x/{SID}.jsonl\t-1"),
            SID
        )
        .is_err());
        // Over-long encodings are shortened by Claude: refuse.
        let long = "-x".repeat(101);
        let err = parse_target_prep(
            &format!("{HEAD}\t{long}\t/h/.claude/projects/{long}/{SID}.jsonl\t-1"),
            SID,
        )
        .unwrap_err();
        assert!(err.message.contains("hash"));
    }

    #[test]
    fn inspection_parses_and_verdicts() {
        let st = parse_inspection(&inspection(" M a.rs", HEAD, "0")).unwrap();
        assert_eq!(st.dirty.len(), 1);
        assert_eq!(st.remote_sha.as_deref(), Some(HEAD));
        assert_eq!(
            preflight_verdict(&st, "feat").unwrap_err().code,
            codes::E_MOVE_DIRTY
        );
        let clean = parse_inspection(&inspection("", HEAD, "0")).unwrap();
        assert!(preflight_verdict(&clean, "feat").is_ok());
        assert_eq!(
            preflight_verdict(&clean, "other").unwrap_err().code,
            codes::E_INVALID_STATE
        );
        assert!(parse_inspection("garbage").is_err());
    }

    #[test]
    fn carry_verdict_accepts_dirty_and_unpushed_but_refuses_midop_and_wrong_branch() {
        let dirty_unpushed = parse_inspection(&inspection(" M a.rs\n?? n.txt", "", "-1")).unwrap();
        assert!(carry_verdict(&dirty_unpushed, "feat").is_ok());
        assert_eq!(
            preflight_verdict(&dirty_unpushed, "feat").unwrap_err().code,
            codes::E_MOVE_DIRTY,
            "strict still refuses"
        );

        let mid = parse_inspection(&inspection_midop("", HEAD, "0", "rebase")).unwrap();
        assert_eq!(mid.midop.as_deref(), Some("rebase"));
        for verdict in [carry_verdict(&mid, "feat"), preflight_verdict(&mid, "feat")] {
            let e = verdict.unwrap_err();
            assert_eq!(e.code, codes::E_MOVE_MIDOP);
            assert!(e.message.contains("rebase"), "{}", e.message);
            assert_eq!(e.details.unwrap()["operation"], "rebase");
        }

        let clean = parse_inspection(&inspection("", HEAD, "0")).unwrap();
        assert_eq!(clean.midop, None);
        assert_eq!(
            carry_verdict(&clean, "other").unwrap_err().code,
            codes::E_INVALID_STATE
        );
    }

    #[test]
    fn inspect_script_probes_every_in_progress_operation() {
        let s = inspect_script("n", None, "feat");
        for marker in [
            "MERGE_HEAD",
            "CHERRY_PICK_HEAD",
            "REVERT_HEAD",
            "BISECT_LOG",
            "rebase-merge",
            "rebase-apply",
        ] {
            assert!(s.contains(marker), "{marker}: {s}");
        }
    }

    /// Runs the generated `inspect_script` through real `git`/`bash` against a
    /// temp repo that is genuinely mid-merge (two branches editing the same
    /// line, `git merge` left conflicted), then again after `git merge
    /// --abort`. Nobody had run the probe against real git before this test.
    #[test]
    fn inspect_script_probe_detects_a_real_mid_merge() {
        if !carry::tests::require(&["git", "bash"]) {
            return;
        }

        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();

        let git = |args: &[&str]| -> std::process::Output {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&repo)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("HOME", &home)
                .output()
                .unwrap()
        };

        assert!(git(&["init", "-q", "-b", "main"]).status.success());
        assert!(git(&["config", "user.email", "a@example.com"])
            .status
            .success());
        assert!(git(&["config", "user.name", "A"]).status.success());
        std::fs::write(repo.join("f.txt"), "one\n").unwrap();
        assert!(git(&["add", "f.txt"]).status.success());
        assert!(git(&["commit", "-q", "-m", "base"]).status.success());
        assert!(git(&["checkout", "-q", "-b", "other"]).status.success());
        std::fs::write(repo.join("f.txt"), "other\n").unwrap();
        assert!(git(&["commit", "-q", "-am", "other change"])
            .status
            .success());
        assert!(git(&["checkout", "-q", "main"]).status.success());
        std::fs::write(repo.join("f.txt"), "main\n").unwrap();
        assert!(git(&["commit", "-q", "-am", "main change"])
            .status
            .success());
        // Both branches touched the same line, so this merge conflicts and
        // leaves MERGE_HEAD in place instead of completing.
        let merge = git(&["merge", "other"]);
        assert!(
            !merge.status.success(),
            "merge must conflict to leave MERGE_HEAD: {}",
            String::from_utf8_lossy(&merge.stderr)
        );

        let probe = || -> SourceState {
            // Empty tmux name: the script skips the tmux lookup and locates
            // the worktree from `hint` alone.
            let script = inspect_script("", Some(repo.to_str().unwrap()), "main");
            let out = std::process::Command::new("bash")
                .arg("-c")
                .arg(&script)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("HOME", &home)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            parse_inspection(&String::from_utf8_lossy(&out.stdout)).unwrap()
        };

        assert_eq!(probe().midop.as_deref(), Some("merge"));

        assert!(git(&["merge", "--abort"]).status.success());
        assert_eq!(probe().midop, None);
    }

    #[test]
    fn scripts_quote_every_interpolated_value() {
        let evil = "x'; rm -rf / #";
        let i = inspect_script(evil, Some(evil), evil);
        // Exact-match target: never a prefix-matched sibling like `<name>-bar`.
        assert!(i.contains(r#"-t "=$name:""#), "{i}");
        assert!(i.contains("name='x'\\''; rm -rf / #'"), "{i}");
        assert!(i.contains("hint='x'\\''; rm -rf / #'"));
        assert!(i.contains("br='x'\\''; rm -rf / #'"));
        let l = locate_script(Some(evil), SID);
        assert!(l.contains("sp='x'\\''; rm -rf / #'"));
        assert!(read_script(evil, 10).contains("head -c 10 -- 'x'\\''; rm -rf / #'"));
        let p = target_prep_script(evil, evil, evil, SID);
        assert!(p.contains("cwd='x'\\''; rm -rf / #'"));
        assert!(p.contains("want='x'\\''; rm -rf / #'"));
        assert!(
            p.contains("sed 's/[^A-Za-z0-9]/-/g'"),
            "host encoding mirrors encode_project_dir"
        );
        assert!(prefetch_script(evil, evil).contains("r='x'\\''; rm -rf / #'"));
        assert!(size_script(evil).contains("f='x'\\''; rm -rf / #'"));
    }

    #[test]
    fn locate_script_prefers_the_stored_path_then_finds_by_id() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let dir = home.join(".claude/projects/-some-dir");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{SID}.jsonl")), "abc\n").unwrap();
        let run = |script: String| {
            std::process::Command::new("bash")
                .arg("-c")
                .arg(script)
                .env("HOME", &home)
                .output()
                .unwrap()
        };
        let out = run(locate_script(None, SID));
        let text = String::from_utf8_lossy(&out.stdout);
        let loc = parse_locate(&text).expect("parses");
        assert_eq!(loc.size, 4, "{text}");
        assert!(loc.mtime > 0, "stat gave an mtime: {text}");
        assert!(loc.path.ends_with(&format!("-some-dir/{SID}.jsonl")));
        let stored = tmp.path().join("stored.jsonl");
        std::fs::write(&stored, "stored!\n").unwrap();
        let out = run(locate_script(Some(&stored.to_string_lossy()), SID));
        assert_eq!(
            parse_locate(&String::from_utf8_lossy(&out.stdout))
                .unwrap()
                .size,
            8
        );
        let empty = tmp.path().join("none");
        let out = std::process::Command::new("bash")
            .arg("-c")
            .arg(locate_script(None, SID))
            .env("HOME", &empty)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(4));
        assert!(String::from_utf8_lossy(&out.stderr).contains(NO_TRANSCRIPT));
    }
}
