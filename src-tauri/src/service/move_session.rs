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
//!    provisioned; the source worktree is clean (`E_MOVE_DIRTY`) and its
//!    branch is on origin with nothing unpushed (`E_MOVE_UNPUSHED`). Nothing
//!    is ever pushed or stashed on the user's behalf.
//! 2. **Transcript** — the source JSONL is located (stored hook path, else
//!    `~/.claude/projects/*/<id>.jsonl`), size-checked against
//!    `move.max_transcript_mb` (`E_MOVE_TOO_LARGE`), read over ssh and
//!    written to the target's `~/.claude/projects/<encoded target cwd>/<id>.jsonl`
//!    through `SshExec::upload_file` stdin.
//! 3. **Target** — the worktree is created (or repaired, create-only) with
//!    `repair::ensure_for_new_session` from the same branch, fast-forwarded to
//!    the source HEAD when it lags, and tmux starts `cl --resume <id>` there
//!    (the recreate pane command). The move then waits, bounded, for the row
//!    to be `running` and the transcript to be in place.
//! 4. **Source** — only once the target is confirmed (the new row's
//!    `parent_session_id` is the source): unless `keep_source`, and only if
//!    the source transcript still has the size and mtime the copy was taken
//!    at, the normal kill path, followed by one more look (a write that
//!    slipped in before the kill becomes a report warning). Then — and only
//!    then — a `session_moved` event on both rows. A partial move records
//!    `session_move_partial` (with the failing step) instead.
//!
//! Failure handling: every step before the target tmux session starts leaves
//! the source untouched and returns the step's own error. Any failure after
//! the target session exists returns `E_MOVE_PARTIAL` (with the new session
//! id in `details`) and leaves BOTH sessions alive.
//!
//! The lifecycle steps that need the real `SshClient` (workspace repair,
//! tmux start, reconcile, kill) sit behind [`MoveHooks`]; every data-plane
//! step (git inspection, transcript copy, target verification) goes through
//! `&dyn SshExec`, so the whole flow runs end-to-end over `FakeSsh` in tests.

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

/// Longest encoded project directory Claude Code uses verbatim; longer names
/// are truncated with a hash suffix we cannot reproduce, so `--resume` would
/// not find a copy written under the full name.
const MAX_ENCODED_DIR: usize = 200;

const NO_TRANSCRIPT: &str = "__CF_NO_TRANSCRIPT__";
const NO_WORKTREE: &str = "__CF_NO_WORKTREE__";
const NO_CWD: &str = "__CF_NO_CWD__";
const DIVERGED: &str = "__CF_DIVERGED__";

#[derive(Debug, Clone, Deserialize)]
pub struct MoveSessionArgs {
    pub session_id: i64,
    pub target_host_alias: String,
    /// Leave the source session running after the target is confirmed.
    #[serde(default)]
    pub keep_source: bool,
}

/// What a completed move did.
#[derive(Debug, Clone, Serialize)]
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
}

/// Parse the inspect script's `\x1e`-separated output.
pub fn parse_inspection(stdout: &str) -> Result<SourceState, IpcError> {
    let parts: Vec<&str> = stdout.split('\x1e').collect();
    if parts.len() != 6 {
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
    })
}

/// Refuse a source that would lose work or land on the wrong code.
pub fn preflight_verdict(state: &SourceState, branch: &str) -> Result<(), IpcError> {
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
/// branch, origin sha of `branch`, ahead count. Tries the pane's cwd first,
/// then `hint`.
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
porcelain=$(git -C "$wt" status --porcelain=v1 2>/dev/null)
head=$(git -C "$wt" rev-parse HEAD 2>/dev/null)
cur=$(git -C "$wt" rev-parse --abbrev-ref HEAD 2>/dev/null)
rsha=$(git -C "$wt" ls-remote --heads origin "refs/heads/$br" 2>/dev/null | cut -f1 | head -n1)
ahead=-1
if [ -n "$rsha" ]; then
  git -C "$wt" cat-file -e "$rsha^{{commit}}" 2>/dev/null || git -C "$wt" fetch -q origin "refs/heads/$br" >/dev/null 2>&1
  ahead=$(git -C "$wt" rev-list --count "$rsha..HEAD" 2>/dev/null)
  if [ -z "$ahead" ]; then ahead=-1; fi
fi
printf '%s\036%s\036%s\036%s\036%s\036%s' "$wt" "$porcelain" "$head" "$cur" "$rsha" "$ahead"
"#,
        name = quote(tmux_name),
        hint = quote(hint.unwrap_or("")),
        br = quote(branch),
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
/// size or -1>`.
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
  elif git merge-base --is-ancestor HEAD "$want" 2>/dev/null && [ -z "$(git status --porcelain)" ]; then
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
    if host == LOCAL {
        let child = tokio::process::Command::new("bash")
            .args(["-lc", script])
            .output();
        let wall = crate::ssh::SshClient::default_wall_clock(timeout);
        return match tokio::time::timeout(wall, child).await {
            Ok(res) => res.map_err(|e| IpcError::new(codes::E_SHELL, format!("spawn bash: {e}"))),
            Err(_) => Err(IpcError::new(
                codes::E_TIMEOUT,
                format!("local script exceeded {}s", wall.as_secs()),
            )),
        };
    }
    ssh.run(host, &["bash", "-lc", &quote(script)], timeout)
        .await
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
    fn write(bytes: &[u8]) -> Result<Self, IpcError> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "claude-fleet-move-{}-{seq}-{nanos}.jsonl",
            std::process::id()
        ));
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        let guard = TempFile(path);
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
        return tokio::fs::write(path, bytes)
            .await
            .map_err(|e| IpcError::new(codes::E_UPLOAD, format!("write {path}: {e}")));
    }
    let tmp = TempFile::write(bytes)?;
    ssh.upload_file(host, &tmp.0, path, COPY_TIMEOUT).await
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
    .with_details(serde_json::json!({ "bytes": bytes, "cap_bytes": cap }))
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

/// The move's steps (see the module docs).
async fn move_session_steps(
    args: MoveSessionArgs,
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    hooks: &dyn MoveHooks,
    opts: MoveOptions,
) -> Result<MoveReport, IpcError> {
    crate::validate::host_alias(&args.target_host_alias)?;
    let _claim = MoveClaim::acquire(store, args.session_id)?;
    let snap = {
        let s = store.lock().map_err(|_| IpcError::lock())?;
        snapshot(&s, &args)?
    };
    let src = snap.row.host_alias.clone();
    let target = args.target_host_alias.clone();
    let id = snap.claude_id.clone();
    let mut warnings: Vec<String> = Vec::new();

    // 0. The source Claude must be idle NOW: reconcile its host so the status
    //    is fresh, then refuse a turn in progress or an unknown status.
    hooks.refresh_host(store, &src).await?;
    let status = {
        let s = store.lock().map_err(|_| IpcError::lock())?;
        s.get_session_by_id(snap.row.id)?
            .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, "session not found"))?
            .claude_status
    };
    require_source_idle(status.as_deref())?;

    // 1. Source git state: clean, on its branch, fully pushed.
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
    preflight_verdict(&state, &snap.branch)?;

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
    // Best effort: a failed fetch surfaces as the workspace step's error.
    let _ = sh(
        ssh,
        &target,
        &prefetch_script(&project_root, &snap.branch),
        GIT_TIMEOUT,
    )
    .await;

    let tmux_name = pick_target_name(&snap.row.tmux_name, &snap.target_taken)?;
    crate::validate::tmux_name(&tmux_name)?;
    let pane_cmd = crate::service::sessions::recreate_pane_command("work", Some(&id));
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
    hooks
        .refresh_host(store, &target)
        .await
        .map_err(|e| partial("reconciling the target host", &target, &tmux_name, None, &e))?;
    let new_row = {
        let s = store.lock().map_err(|_| IpcError::lock())?;
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
            let s = store.lock().map_err(|_| IpcError::lock())?;
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
    const TGT_ENC: &str = "-home-b-p-o-r--claude-worktrees-feat";
    const TRANSCRIPT: &str =
        "{\"type\":\"user\",\"message\":{\"content\":\"hi\"}}\n{\"type\":\"assistant\"}\n";

    fn tgt_path() -> String {
        format!("/home/b/.claude/projects/{TGT_ENC}/{SID}.jsonl")
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
                return Err(IpcError::new("E_TMUX", "can't find session"));
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
        format!(
            "/home/a/p/o/r/.claude/worktrees/feat\x1e{porcelain}\x1e{HEAD}\x1efeat\x1e{rsha}\x1e{ahead}"
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
        assert_eq!(uploads.len(), 1);
        assert_eq!(
            uploads[0].command(),
            format!("cat > {}", quote(&tgt_path()))
        );
        assert_eq!(uploads[0].stdin_str().as_deref(), Some(TRANSCRIPT));

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

    #[tokio::test]
    async fn unpushed_branch_is_refused_before_the_target_is_touched() {
        let f = fixture();
        f.fake.on_host(
            "alpha",
            Match::script_contains("# cf-move:inspect"),
            Reply::ok(&inspection("", "", "-1")),
        );
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let err = run(&f, &hooks, false).await.unwrap_err();
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
        let err = run(&f, &hooks, false).await.unwrap_err();
        assert_eq!(err.code, codes::E_MOVE_UNPUSHED);
        assert!(err.message.contains("2 commit(s)"), "{}", err.message);
        // The inspection never pushes anything itself.
        assert!(f
            .fake
            .calls_for("alpha")
            .iter()
            .all(|c| !c.script().unwrap_or_default().contains("git push")));
    }

    #[tokio::test]
    async fn dirty_source_is_refused_and_lists_the_files() {
        let f = fixture();
        f.fake.on_host(
            "alpha",
            Match::script_contains("# cf-move:inspect"),
            Reply::ok(&inspection(" M src/lib.rs\n?? notes.txt", HEAD, "0")),
        );
        let hooks = FakeHooks::new(&f.fake, f.project_id, f.worktree_id);
        let err = run(&f, &hooks, false).await.unwrap_err();
        assert_eq!(err.code, codes::E_MOVE_DIRTY);
        assert!(err.message.contains("src/lib.rs") && err.message.contains("notes.txt"));
        let details = err.details.expect("dirty_files details");
        assert_eq!(details["dirty_files"].as_array().unwrap().len(), 2);
        assert!(f.fake.calls_for("beta").is_empty());
        assert_source_untouched(&f, &hooks);
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
        assert_eq!(err.details.unwrap()["cap_bytes"], 1024 * 1024);
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
        assert!(beta.iter().all(|c| c.stdin.is_none()), "no upload");
        assert!(beta
            .iter()
            .all(|c| !c.script().unwrap_or_default().contains("tmux new-session")));
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
        assert!(f.fake.calls_for("beta").iter().all(|c| c.stdin.is_none()));
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
