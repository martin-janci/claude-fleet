//! Self-repairing workspaces: make sure a session's directory exists, is a
//! registered git worktree on the right branch, and that its tmux pane runs
//! there — on create, recreate, restart, attach, and on explicit request.
//!
//! The design is probe → plan → apply → verify:
//!
//! 1. **Probe** — ONE `bash -lc` script per host (every value `quote`d)
//!    prints a small `key=value` status block plus the
//!    `git worktree list --porcelain` dump, with every compared path
//!    canonicalized ON THE HOST (`pwd -P`). Nothing in the probe writes.
//! 2. **Plan** — [`plan`] is a pure function of the probe and a [`Policy`]:
//!    the ordered [`Step`]s that make the workspace healthy, or a refusal
//!    with an `E_*` code when a fix would need a guess.
//! 3. **Apply** — the git steps run as one script, then the probe runs again
//!    to **verify**; only then is tmux touched.
//! 4. **Record** — rows are corrected (within the policy) and one
//!    `workspace_repaired` / `workspace_repair_failed` event is written.
//!
//! **Automatic vs explicit.** Automatic repair (new session, restart,
//! recreate, attach, the opt-in reconcile tick — [`Policy::Auto`]) may only
//! CREATE what is confirmed missing: `git worktree add` from an existing local
//! or remote-tracking branch into a target that is absent on disk, and a tmux
//! session that `tmux has-session` confirms is dead. The one removal it may
//! make is this worktree's OWN stale registration (`git worktree remove
//! --force -- <path>`, never a prune) right before that add, and only when
//! every [`VanishedGuard`] condition holds: the parent directory's `dev:inode`
//! matches the one recorded while the worktree was healthy (re-checked inside
//! the apply script), the directory is confirmed absent,
//! its parent exists on the project root's filesystem (no unmounted or other
//! volume), the repository is usable, the path
//! is under the project root, the entry is not locked, and no other session
//! maps to it. Anything else that can destroy, unregister, redirect or
//! rebranch — the same removal when a guard fails,
//! adopting a checkout elsewhere (and re-pathing the row), recreating a
//! branch from its base, `git worktree repair`, respawning a live pane — is
//! [`Policy::Explicit`] only (the Repair workspace button, or the
//! confirm-gated MCP tool). An automatic run that needs such a step returns a
//! warning with `needs_explicit_repair`; an ambiguous probe (unknown tmux
//! state, unreadable pane cwd, a failed fetch) never causes an action.
//!
//! A healthy workspace costs exactly one probe and no writes.
//! Spec: `docs/specs/2026-09-11-session-worktree-repair.md`.

use crate::ipc_error::{codes, IpcError};
use crate::shell::quote;
use crate::ssh::{SshClient, SshExec};
use crate::store::{SessionRow, Store};
use crate::tmux::TmuxExec;
use async_trait::async_trait;
use serde::Serialize;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// `session_events.kind` written when a repair changed something.
pub const EVENT_REPAIRED: &str = "workspace_repaired";
/// `session_events.kind` written when a repair was refused or failed.
pub const EVENT_REPAIR_FAILED: &str = "workspace_repair_failed";
/// stderr marker of the apply script's re-check right before it removes our
/// stale registration: the directory came back (or its parent vanished)
/// after the probe, so nothing was removed.
pub const UNREGISTER_REFUSED: &str = "reappeared or parent missing; not removing";
/// stderr marker of the apply script's fingerprint re-check right before the
/// re-add: the parent changed after the stale entry was removed.
pub const ADD_REFUSED: &str = "parent changed since the check; not re-adding";

/// `"dev:inode"` → `(dev, inode)`; `None` for anything else.
pub fn parse_fp(s: &str) -> Option<(u64, u64)> {
    let (d, i) = s.trim().split_once(':')?;
    Some((d.parse().ok()?, i.parse().ok()?))
}

/// `(dev, inode)` → `"dev:inode"` (the stored and scripted form).
pub fn fp_string(fp: (u64, u64)) -> String {
    format!("{}:{}", fp.0, fp.1)
}
/// Part of the `E_REPAIR_FAILED` message of an apply that was cut off.
pub const PARTIALLY_APPLIED: &str = "the repair may be partially applied";

/// ssh `ConnectTimeout` for the read-only probe on a remote host. The client
/// bounds the whole command by `SshClient::default_wall_clock` (3× this, at
/// least 30 s): 45 s, the same bound as [`PROBE_WALL_CLOCK`] on `local`.
const PROBE_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// ssh `ConnectTimeout` for the apply script; its 3× bound is the 180 s
/// [`APPLY_WALL_CLOCK`].
const APPLY_CONNECT_TIMEOUT: Duration = Duration::from_secs(60);
/// Wall clock for the read-only probe on `local`.
const PROBE_WALL_CLOCK: Duration = Duration::from_secs(45);
/// Wall clock for the apply script on `local`. A fetch plus `worktree add` on
/// a large repository can take a while; hitting this bound (local or remote)
/// means the repair may be partially applied.
const APPLY_WALL_CLOCK: Duration = Duration::from_secs(180);

// ---------------------------------------------------------------------------
// Spec: what the workspace SHOULD look like
// ---------------------------------------------------------------------------

/// Where a session's pane must live. Built from the store by
/// [`spec_for_session`] (existing rows) or by hand in `new_session` (the row
/// does not exist yet).
#[derive(Debug, Clone)]
pub struct WorkspaceSpec {
    pub host_alias: String,
    pub tmux_name: String,
    /// Absolute path of the project's main checkout on `host_alias`.
    pub project_root: String,
    /// `None` for a main-checkout session; `Some` for a linked worktree.
    pub worktree: Option<WorktreeSpec>,
    /// Branch to fork from when an explicit repair has to recreate the
    /// worktree's branch (`None` = the repo's default branch).
    pub base_branch: Option<String>,
    /// Pane command used when tmux must be created or respawned.
    pub pane_cmd: String,
    /// Session row to record events on / link the worktree to. `None` while
    /// the session is still being created.
    pub session_id: Option<i64>,
    pub project_id: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct WorktreeSpec {
    /// Worktree name (the `worktrees.name` / `sessions.worktree_key` value).
    pub name: String,
    /// Expected absolute path on the host (the user-facing form).
    pub path: String,
    /// Branch that must be checked out there.
    pub branch: String,
    /// `true` when `path` was derived from the layout convention rather than
    /// read from a row or seen on disk; the plan may then prefer the
    /// project's existing `.worktrees/` layout over `.claude/worktrees/`.
    pub path_is_guess: bool,
    /// Whether the local `worktrees` table row may be written. Only `local`
    /// paths belong in that table.
    pub row_is_local: bool,
}

/// How much a repair may do. See the module doc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// A lifecycle side effect. Only creates what is confirmed missing (see
    /// the module doc). With `create_dead_tmux` it also creates a tmux
    /// session that is confirmed dead (the attach path); without it the
    /// caller owns tmux (new session, restart, recreate).
    Auto { create_dead_tmux: bool },
    /// The Repair workspace button, or the confirm-gated MCP tool.
    Explicit,
}

/// The entry points that run a repair. [`policy_for`] is the single mapping
/// from an entry point to what it may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    NewSession,
    /// `spawn_review`: a new session in the source session's workspace.
    SpawnReview,
    Restart,
    Recreate,
    Attach,
    Explicit,
}

pub fn policy_for(entry: Entry) -> Policy {
    match entry {
        Entry::NewSession | Entry::SpawnReview | Entry::Restart | Entry::Recreate => Policy::Auto {
            create_dead_tmux: false,
        },
        Entry::Attach => Policy::Auto {
            create_dead_tmux: true,
        },
        Entry::Explicit => Policy::Explicit,
    }
}

// ---------------------------------------------------------------------------
// Probe: what the workspace DOES look like
// ---------------------------------------------------------------------------

/// One entry of `git worktree list --porcelain`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RegisteredWorktree {
    /// The path exactly as git prints it (git records realpaths, so under a
    /// symlinked root this differs from the path the user and the rows use).
    pub path: String,
    /// The same path canonicalized ON THE HOST by the probe (`pwd -P` of the
    /// nearest existing ancestor + the missing remainder). `None` when the
    /// probe did not report one (older / hand-written output).
    pub canon: Option<String>,
    /// `refs/heads/` stripped. `None` when detached or bare.
    pub branch: Option<String>,
    pub prunable: bool,
    pub locked: bool,
    pub bare: bool,
    /// The probe's `test -e || test -L` on this registration's path. `None`
    /// when the probe did not report it.
    pub present: Option<bool>,
}

impl RegisteredWorktree {
    /// The form every comparison uses: canonical when known, else git's path.
    pub fn key(&self) -> &str {
        self.canon.as_deref().unwrap_or(&self.path)
    }
}

/// Parsed probe output. Every boolean defaults to `false`, so a truncated
/// or failed probe reads as "nothing is there / nothing confirmed" and the
/// plan refuses or waits rather than assuming health or death.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Probe {
    pub root_exists: bool,
    pub root_git: bool,
    pub root_gitdir_ok: bool,
    /// Host-canonical project root / worktree path (symlinks resolved, e.g.
    /// macOS `/var` → `/private/var`, or `~/projects` → `/mnt/…`). Only for
    /// comparisons; rows and reports keep the user-facing path.
    pub root_canon: Option<String>,
    pub wt_canon: Option<String>,
    /// The worktree directory the probe used (user-facing form). For a
    /// convention-derived (guessed) path it is resolved on the host: the
    /// first of `.worktrees/<name>`, `.claude/worktrees/<name>` that exists
    /// or is registered with git, else the guess.
    pub wt_path: Option<String>,
    /// `<root>/.worktrees` exists (the alternate layout).
    pub layout_dot_worktrees: bool,
    pub wt_exists: bool,
    pub wt_git: bool,
    pub wt_empty: bool,
    /// `git rev-parse --git-dir` succeeds inside the worktree dir.
    pub wt_gitdir_ok: bool,
    pub index_lock: bool,
    pub branch_local: bool,
    pub branch_remote: bool,
    pub default_branch: Option<String>,
    /// `tmux has-session` succeeded.
    pub tmux_alive: bool,
    /// `tmux has-session` failed while the server answered (or no server
    /// runs): the session is CONFIRMED gone. `tmux_alive == tmux_dead ==
    /// false` means "unknown" (tmux missing / not answering) — no action.
    pub tmux_dead: bool,
    /// The live pane's cwd. `None` when tmux returned nothing: unknown.
    pub tmux_cwd: Option<String>,
    /// Only meaningful with `tmux_cwd`: that directory exists on the host.
    pub tmux_cwd_exists: bool,
    /// `test -e || test -L` on the worktree path. `None` when the probe did
    /// not report it (truncated or older output): never read as "absent".
    pub wt_entry_exists: Option<bool>,
    /// The worktree path's parent directory exists. A missing parent (an
    /// unmounted volume, a vanished mountpoint) blocks the automatic removal.
    pub wt_parent_exists: bool,
    /// Device id of the project root (`stat -L`), `None` when stat failed.
    pub root_dev: Option<String>,
    /// Device id of the worktree path's parent, `None` when stat failed. It
    /// must equal `root_dev` for the automatic removal (another volume, an
    /// autofs mountpoint).
    pub wt_parent_dev: Option<String>,
    /// `(dev, inode)` of the worktree's canonical parent directory (`stat -L`),
    /// `None` when stat failed. Recorded while the worktree is healthy; an
    /// automatic removal requires the current value to match it.
    pub wt_parent_fp: Option<(u64, u64)>,
    pub worktrees: Vec<RegisteredWorktree>,
}

/// Separator between the `key=value` block and the porcelain dump.
const WORKTREES_MARKER: &str = "@@worktrees";

/// The read-only probe script. Every interpolated value is `quote`d; the
/// script never exits non-zero on a missing file (that is a finding, not a
/// failure) — only a transport error is an error.
pub fn probe_script(spec: &WorkspaceSpec) -> String {
    let root = quote(&spec.project_root);
    let wt_path = spec
        .worktree
        .as_ref()
        .map(|w| w.path.as_str())
        .unwrap_or(&spec.project_root);
    let wt = quote(wt_path);
    let br = quote(
        spec.worktree
            .as_ref()
            .map(|w| w.branch.as_str())
            .unwrap_or(""),
    );
    let sess = quote(&spec.tmux_name);
    let name = quote(
        spec.worktree
            .as_ref()
            .map(|w| w.name.as_str())
            .unwrap_or(""),
    );
    let guess = if spec.worktree.as_ref().is_some_and(|w| w.path_is_guess) {
        "1"
    } else {
        "0"
    };
    format!(
        r#"set +e
root={root}
wt={wt}
br={br}
sess={sess}
name={name}
guess={guess}
yn() {{ if "$@" >/dev/null 2>&1; then echo 1; else echo 0; fi; }}
# Canonical path on THIS host: `pwd -P` of the nearest existing ancestor plus
# the missing remainder (realpath is absent on stock macOS < 13).
canon() {{
  [ -z "$1" ] && {{ echo; return; }}
  _p="$1"; _r=""
  while [ "$_p" != "/" ] && [ "$_p" != "." ] && [ ! -d "$_p" ]; do
    _r="/$(basename -- "$_p")$_r"
    _p="$(dirname -- "$_p")"
  done
  _c="$(CDPATH= cd -P -- "$_p" 2>/dev/null && pwd -P)" || _c="$_p"
  [ -z "$_c" ] && _c="$_p"
  if [ "$_c" = "/" ] && [ -n "$_r" ]; then _c=""; fi
  printf '%s%s\n' "$_c" "$_r"
}}
# A convention-derived worktree path is only a guess: hosts use either
# layout. Take the first of `.worktrees/<name>`, `.claude/worktrees/<name>`
# that exists or that git has registered (in either path form).
if [ "$guess" = 1 ] && [ -n "$name" ]; then
  for cand in "$root/.worktrees/$name" "$root/.claude/worktrees/$name"; do
    if [ -d "$cand" ] || git -C "$root" worktree list --porcelain 2>/dev/null | grep -Fxq -e "worktree $cand" -e "worktree $(canon "$cand")"; then
      wt="$cand"
      break
    fi
  done
fi
echo "wt_path=$wt"
echo "root_exists=$(yn test -d "$root")"
echo "root_git=$(yn test -e "$root/.git")"
echo "root_gitdir_ok=$(yn git -C "$root" rev-parse --git-dir)"
echo "root_canon=$(canon "$root")"
echo "wt_canon=$(canon "$wt")"
echo "layout_dot_worktrees=$(yn test -d "$root/.worktrees")"
echo "wt_exists=$(yn test -d "$wt")"
if [ -e "$wt" ] || [ -L "$wt" ]; then echo wt_entry_exists=1; else echo wt_entry_exists=0; fi
echo "wt_parent_exists=$(yn test -d "$(dirname -- "$wt")")"
# Device ids (GNU/BusyBox `stat -c`, else BSD/macOS `stat -f`); empty on failure.
if stat -L -c %d / >/dev/null 2>&1; then devof() {{ stat -L -c %d -- "$1" 2>/dev/null; }}; else devof() {{ stat -L -f %d -- "$1" 2>/dev/null; }}; fi
echo "root_dev=$(devof "$root")"
echo "wt_parent_dev=$(devof "$(dirname -- "$wt")")"
# Parent fingerprint `dev:inode` of the canonical parent; empty on failure.
if stat -L -c %d / >/dev/null 2>&1; then fpof() {{ stat -L -c '%d:%i' -- "$1" 2>/dev/null; }}; else fpof() {{ stat -L -f '%d:%i' -- "$1" 2>/dev/null; }}; fi
echo "wt_parent_fp=$(fpof "$(dirname -- "$(canon "$wt")")")"
echo "wt_git=$(yn test -e "$wt/.git")"
if [ -d "$wt" ] && [ -z "$(ls -A "$wt" 2>/dev/null)" ]; then echo wt_empty=1; else echo wt_empty=0; fi
echo "wt_gitdir_ok=$(yn git -C "$wt" rev-parse --git-dir)"
gd="$(git -C "$wt" rev-parse --absolute-git-dir 2>/dev/null)"
if [ -n "$gd" ] && [ -e "$gd/index.lock" ]; then echo index_lock=1; else echo index_lock=0; fi
if [ -n "$br" ]; then
  echo "branch_local=$(yn git -C "$root" show-ref --verify --quiet "refs/heads/$br")"
  echo "branch_remote=$(yn git -C "$root" show-ref --verify --quiet "refs/remotes/origin/$br")"
fi
def="$(git -C "$root" symbolic-ref --short refs/remotes/origin/HEAD 2>/dev/null | sed 's#^origin/##')"
[ -z "$def" ] && def="$(git -C "$root" rev-parse --abbrev-ref HEAD 2>/dev/null)"
echo "default_branch=$def"
if ! command -v tmux >/dev/null 2>&1; then
  echo tmux_alive=0
  echo tmux_dead=0
elif tmux has-session -t "=$sess" >/dev/null 2>&1; then
  echo tmux_alive=1
  echo tmux_dead=0
  cwd="$(tmux display-message -p -t "=$sess" '#{{pane_current_path}}' 2>/dev/null)"
  echo "tmux_cwd=$cwd"
  if [ -n "$cwd" ]; then echo "tmux_cwd_exists=$(yn test -d "$cwd")"; fi
else
  echo tmux_alive=0
  err="$(tmux list-sessions 2>&1 >/dev/null)"; rc=$?
  if [ "$rc" -eq 0 ] || printf '%s' "$err" | grep -qiE 'no server running|error connecting to'; then
    echo tmux_dead=1
  else
    echo tmux_dead=0
  fi
fi
echo '{marker}'
git -C "$root" worktree list --porcelain 2>/dev/null | while IFS= read -r l; do
  printf '%s\n' "$l"
  case "$l" in "worktree "*) echo "canon $(canon "${{l#worktree }}")"; _w="${{l#worktree }}"; if [ -e "$_w" ] || [ -L "$_w" ]; then echo "present 1"; else echo "present 0"; fi;; esac
done
exit 0
"#,
        marker = WORKTREES_MARKER,
    )
}

/// Parse the probe script's stdout.
pub fn parse_probe(stdout: &str) -> Probe {
    let mut p = Probe::default();
    let (kv, porcelain) = match stdout.find(WORKTREES_MARKER) {
        Some(i) => (&stdout[..i], &stdout[i + WORKTREES_MARKER.len()..]),
        None => (stdout, ""),
    };
    let flag = |v: &str| v.trim() == "1";
    for line in kv.lines() {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        match k.trim() {
            "root_exists" => p.root_exists = flag(v),
            "root_git" => p.root_git = flag(v),
            "root_gitdir_ok" => p.root_gitdir_ok = flag(v),
            "layout_dot_worktrees" => p.layout_dot_worktrees = flag(v),
            "wt_exists" => p.wt_exists = flag(v),
            "wt_git" => p.wt_git = flag(v),
            "wt_empty" => p.wt_empty = flag(v),
            "wt_entry_exists" => p.wt_entry_exists = Some(flag(v)),
            "wt_parent_exists" => p.wt_parent_exists = flag(v),
            "root_dev" => {
                let v = v.trim();
                p.root_dev = (!v.is_empty()).then(|| v.to_string());
            }
            "wt_parent_dev" => {
                let v = v.trim();
                p.wt_parent_dev = (!v.is_empty()).then(|| v.to_string());
            }
            "wt_parent_fp" => p.wt_parent_fp = parse_fp(v),
            "wt_gitdir_ok" => p.wt_gitdir_ok = flag(v),
            "index_lock" => p.index_lock = flag(v),
            "branch_local" => p.branch_local = flag(v),
            "branch_remote" => p.branch_remote = flag(v),
            "default_branch" => {
                let v = v.trim();
                p.default_branch = (!v.is_empty()).then(|| v.to_string());
            }
            "tmux_alive" => p.tmux_alive = flag(v),
            "tmux_dead" => p.tmux_dead = flag(v),
            "tmux_cwd" => {
                let v = v.trim();
                p.tmux_cwd = (!v.is_empty()).then(|| v.to_string());
            }
            "tmux_cwd_exists" => p.tmux_cwd_exists = flag(v),
            "root_canon" => p.root_canon = (!v.is_empty()).then(|| v.to_string()),
            "wt_canon" => p.wt_canon = (!v.is_empty()).then(|| v.to_string()),
            "wt_path" => p.wt_path = (!v.is_empty()).then(|| v.to_string()),
            _ => {}
        }
    }
    p.worktrees = parse_porcelain(porcelain);
    p
}

/// Parse `git worktree list --porcelain` (entries separated by blank lines).
pub fn parse_porcelain(input: &str) -> Vec<RegisteredWorktree> {
    let mut out = Vec::new();
    let mut cur: Option<RegisteredWorktree> = None;
    for line in input.lines() {
        let line = line.trim_end();
        if let Some(rest) = line.strip_prefix("worktree ") {
            if let Some(w) = cur.take() {
                out.push(w);
            }
            cur = Some(RegisteredWorktree {
                path: rest.to_string(),
                ..Default::default()
            });
            continue;
        }
        let Some(w) = cur.as_mut() else {
            continue;
        };
        if let Some(rest) = line.strip_prefix("present ") {
            // Emitted by the probe: does that registered path exist on disk?
            w.present = Some(rest.trim() == "1");
        } else if let Some(rest) = line.strip_prefix("canon ") {
            // Emitted by the probe right after each `worktree` line.
            if !rest.is_empty() {
                w.canon = Some(rest.to_string());
            }
        } else if let Some(rest) = line.strip_prefix("branch ") {
            w.branch = Some(rest.trim_start_matches("refs/heads/").to_string());
        } else if line == "bare" {
            w.bare = true;
        } else if line == "prunable" || line.starts_with("prunable ") {
            w.prunable = true;
        } else if line == "locked" || line.starts_with("locked ") {
            w.locked = true;
        }
    }
    if let Some(w) = cur {
        out.push(w);
    }
    out
}

// ---------------------------------------------------------------------------
// Plan: the minimal ordered fix
// ---------------------------------------------------------------------------

/// Where the branch for a recreated worktree comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum BranchSource {
    /// `refs/heads/<branch>` exists: check it out. (automatic)
    Local,
    /// Only `refs/remotes/origin/<branch>` exists: create a tracking branch.
    /// (automatic)
    Remote,
    /// Neither exists locally. Explicit only: ask origin with
    /// `git ls-remote --exit-code`; if origin has it, fetch it and track it;
    /// if origin confirms it is gone (or there is no origin), fork a NEW
    /// branch from `base` (local, then `origin/<base>`), then `default`, then
    /// `HEAD`. Any other ls-remote / fetch error aborts the repair.
    FetchOrBase { base: String, default: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Step {
    /// `git worktree remove --force -- <path>` — drop OUR registration whose
    /// directory is gone (`path` exactly as git lists it). Explicit only, and
    /// never a repo-wide `git worktree prune`: that would also discard every
    /// other worktree's stale registration (another session's, or one on an
    /// unmounted volume).
    Unregister { path: String },
    /// `git worktree repair -- <path>` — re-link a moved / stale checkout.
    /// Explicit only.
    RepairLinks { path: String },
    /// `git worktree add -- <path> …` with the branch resolved per `from`.
    AddWorktree {
        path: String,
        branch: String,
        from: BranchSource,
    },
    /// The branch is checked out in another linked worktree: use that
    /// directory (and re-path the row). Explicit only; guarded against
    /// checkouts that belong to another fleet workspace.
    AdoptPath { path: String },
    /// `tmux new-session -c <cwd>` — the session is confirmed gone.
    TmuxCreate { cwd: String },
    /// `tmux respawn-pane -k -c <cwd>` — the live pane's cwd was reported and
    /// is confirmed missing. Explicit only (it restarts the pane's process).
    TmuxRespawn { cwd: String },
}

impl Step {
    /// Human-readable one-liner for the report / event detail.
    pub fn describe(&self) -> String {
        match self {
            Step::Unregister { path } => format!("git worktree remove --force -- {path}"),
            Step::RepairLinks { path } => format!("git worktree repair -- {path}"),
            Step::AddWorktree { path, branch, from } => match from {
                BranchSource::Local => format!("git worktree add -- {path} {branch}"),
                BranchSource::Remote => {
                    format!("git worktree add --track -b {branch} -- {path} origin/{branch}")
                }
                BranchSource::FetchOrBase { base, .. } => format!(
                    "git worktree add -- {path} (origin/{branch} if origin has it, else a new branch {branch} from {base})"
                ),
            },
            Step::AdoptPath { path } => format!("adopt existing checkout at {path}"),
            Step::TmuxCreate { cwd } => format!("tmux new-session -c {cwd}"),
            Step::TmuxRespawn { cwd } => format!("tmux respawn-pane -k -c {cwd}"),
        }
    }

    fn is_git(&self) -> bool {
        matches!(
            self,
            Step::Unregister { .. } | Step::RepairLinks { .. } | Step::AddWorktree { .. }
        )
    }
}

/// The plan for one workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// Steps to apply now, in order.
    pub steps: Vec<Step>,
    /// The directory the pane must run in once the steps are applied (the
    /// user-facing form).
    pub cwd: String,
    /// Branch actually checked out at `cwd` when it differs from the spec
    /// (the user switched branches inside the worktree). Reported, never
    /// "fixed".
    pub branch_drift: Option<String>,
    pub warnings: Vec<String>,
    /// Automatic policy only: the workspace needs a step that only an
    /// explicit repair may take; nothing git-side is applied.
    pub needs_explicit_repair: bool,
    /// What an explicit repair would do (for the warning / report).
    pub deferred: Vec<Step>,
    /// The vanished-directory guard, evaluated when our registration's
    /// directory was found missing ([`VanishedGuard`]).
    pub vanished_guard: Option<VanishedGuard>,
}

impl Plan {
    pub fn is_noop(&self) -> bool {
        self.steps.is_empty()
    }
    fn has_git_steps(&self) -> bool {
        self.steps.iter().any(Step::is_git)
    }
}

fn norm(p: &str) -> &str {
    let t = p.trim_end_matches('/');
    if t.is_empty() {
        "/"
    } else {
        t
    }
}

fn basename(p: &str) -> &str {
    norm(p).rsplit('/').next().unwrap_or("")
}

fn refuse(code: &str, msg: impl Into<String>) -> IpcError {
    IpcError::new(code, msg)
}

const INDEX_LOCK_WARNING: &str = "index.lock present (a git operation may be running)";

/// What an automatic run knows beyond the probe (from the store). The
/// default ("unknown") always blocks the guarded automatic removal.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AutoContext {
    /// Another live or ghost session on the host maps to this worktree (same
    /// project and `worktree_key`, or a `worktree_id` whose row has that name
    /// or canonical path). `None` = unknown.
    pub other_sessions_mapped: Option<bool>,
    /// May this run drop our own stale registration automatically at all?
    /// Every store-backed run (the reconcile tick and the click-driven entry
    /// points) passes `true`: the removal then still requires every
    /// [`VanishedGuard`] condition, above all the parent fingerprint match.
    /// Only a context-free plan (plain [`plan`]) leaves it false.
    pub allow_auto_unregister: bool,
    /// The parent `(dev, inode)` recorded while this worktree was healthy
    /// (keyed by host + canonical path). `None`: never recorded.
    pub recorded_parent_fp: Option<(u64, u64)>,
}

/// When an AUTOMATIC run may drop this worktree's own stale registration
/// (`git worktree remove --force -- <path>`, never a prune) and re-add it:
/// the directory vanished, and nothing suggests an unmounted volume or
/// someone else's checkout. Recorded in the `workspace_repaired` detail.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct VanishedGuard {
    /// Non-empty canonical path, and `test -e` / `test -L` false for it.
    pub dir_absent: bool,
    /// The worktree path's parent directory exists.
    pub parent_exists: bool,
    /// The parent's device id equals the project root's (not another volume
    /// or an autofs mountpoint). Unknown (either stat failed) fails.
    pub same_filesystem: bool,
    /// The project root exists and `git rev-parse --git-dir` works there.
    pub repo_ok: bool,
    /// The canonical path lies strictly under the canonical project root
    /// (which covers `.worktrees/` and `.claude/worktrees/`), no `.` / `..`.
    pub under_root: bool,
    /// The registration is not locked.
    pub not_locked: bool,
    /// No other live or ghost session maps to the worktree.
    pub no_other_session: bool,
    /// Every OTHER registered worktree under the same parent still exists.
    /// An unmounted volume makes all of its worktrees vanish at once; an
    /// `rm -rf` leaves the others in place. A sole worktree passes.
    pub siblings_present: bool,
    /// The parent's current `dev:inode` equals the one recorded while this
    /// worktree was healthy. The primary unmounted / remounted discriminator.
    pub fingerprint_matches: bool,
    /// `match` / `mismatch` / `missing` (never recorded) / `stat_failed`.
    pub fingerprint_check: &'static str,
}

impl VanishedGuard {
    pub fn holds(&self) -> bool {
        self.failed().is_empty()
    }

    /// The conditions that failed, human-readable.
    pub fn failed(&self) -> Vec<&'static str> {
        let fingerprint = match self.fingerprint_check {
            "missing" => "no parent fingerprint was recorded while the worktree was healthy",
            "stat_failed" => "could not read the parent's dev:inode",
            _ => "the parent's dev:inode differs from the one recorded while healthy (remounted or replaced?)",
        };
        [
            (self.fingerprint_matches, fingerprint),
            // A missing sibling blocks only alongside a failed fingerprint;
            // with a match it is a stale registration, reported as a warning.
            (
                self.siblings_present || self.fingerprint_matches,
                "another worktree under the same parent is missing too (unmounted volume?)",
            ),
            (self.dir_absent, "directory not confirmed absent"),
            (
                self.parent_exists,
                "parent directory missing (unmounted volume?)",
            ),
            (
                self.same_filesystem,
                "parent not confirmed on the project root's filesystem (mount point?)",
            ),
            (self.repo_ok, "project repository not usable"),
            (self.under_root, "path not under the project root"),
            (self.not_locked, "registration is locked"),
            (
                self.no_other_session,
                "another session maps to this worktree",
            ),
        ]
        .into_iter()
        .filter(|(ok, _)| !ok)
        .map(|(_, why)| why)
        .collect()
    }
}

/// A usable canonical path: absolute, not `/`, no `.` / `..` component.
fn clean_canon(c: &str) -> Option<&str> {
    let c = norm(c.trim());
    (c.starts_with('/') && c != "/" && !c.split('/').any(|seg| seg == "." || seg == ".."))
        .then_some(c)
}

/// Parent directory of a canonical path (`/` for a top-level entry).
fn parent_of(c: &str) -> &str {
    let c = norm(c);
    match c.rfind('/') {
        Some(0) => "/",
        Some(i) => &c[..i],
        None => "",
    }
}

/// Other registered worktrees under our parent that are not confirmed
/// present on disk (git's path form, for the refusal message).
fn missing_siblings(p: &Probe) -> Vec<String> {
    let Some(w) = p.wt_canon.as_deref().and_then(clean_canon) else {
        return Vec::new();
    };
    let parent = parent_of(w);
    p.worktrees
        .iter()
        .filter(|o| !o.bare && norm(o.key()) != w && parent_of(o.key()) == parent)
        .filter(|o| o.present != Some(true))
        .map(|o| o.path.clone())
        .collect()
}

/// Pure: evaluate the [`VanishedGuard`] for our registration `r`.
fn vanished_guard(p: &Probe, r: &RegisteredWorktree, ctx: AutoContext) -> VanishedGuard {
    let wt = p.wt_canon.as_deref().and_then(clean_canon);
    let root = p.root_canon.as_deref().and_then(clean_canon);
    let fingerprint_check = match (p.wt_parent_fp, ctx.recorded_parent_fp) {
        (None, _) => "stat_failed",
        (Some(_), None) => "missing",
        (Some(now), Some(then)) if now == then => "match",
        _ => "mismatch",
    };
    VanishedGuard {
        siblings_present: missing_siblings(p).is_empty(),
        fingerprint_matches: fingerprint_check == "match",
        fingerprint_check,
        dir_absent: wt.is_some() && p.wt_entry_exists == Some(false) && !p.wt_exists,
        parent_exists: p.wt_parent_exists,
        same_filesystem: matches!(
            (p.root_dev.as_deref(), p.wt_parent_dev.as_deref()),
            (Some(a), Some(b)) if a == b
        ),
        repo_ok: p.root_exists && p.root_gitdir_ok,
        under_root: match (wt, root) {
            (Some(w), Some(rt)) => w
                .strip_prefix(rt)
                .is_some_and(|rest| rest.len() > 1 && rest.starts_with('/')),
            _ => false,
        },
        not_locked: !r.locked,
        no_other_session: ctx.other_sessions_mapped == Some(false),
    }
}

/// Decide the fix. Pure: every branch of this function is covered by a unit
/// test with a hand-built [`Probe`]. Refusals (`Err`) apply in every policy;
/// explicit-only steps are deferred under [`Policy::Auto`]. Without an
/// [`AutoContext`] the guarded automatic removal never applies. Test-only:
/// production always plans with the store context ([`plan_with`]).
#[cfg(test)]
pub fn plan(spec: &WorkspaceSpec, p: &Probe, policy: Policy) -> Result<Plan, IpcError> {
    plan_with(spec, p, policy, AutoContext::default())
}

/// [`plan`] with what the store knows about the workspace; this is what
/// [`ensure_workspace`] runs. Pure.
pub fn plan_with(
    spec: &WorkspaceSpec,
    p: &Probe,
    policy: Policy,
    ctx: AutoContext,
) -> Result<Plan, IpcError> {
    if !p.root_exists || !p.root_git || !p.root_gitdir_ok {
        return Err(refuse(
            codes::E_REPO_MISSING,
            format!(
                "project repository is missing or not a git checkout at {} on {} — \
                 it is never recreated by mkdir; restore or re-clone it (a new session \
                 on a remote host clones automatically)",
                spec.project_root, spec.host_alias
            ),
        ));
    }
    let explicit = policy == Policy::Explicit;
    // (step, may an automatic repair take it?)
    let mut git: Vec<(Step, bool)> = Vec::new();
    let mut warnings = Vec::new();
    let mut branch_drift = None;
    let mut vanished: Option<VanishedGuard> = None;
    if p.index_lock {
        warnings.push(INDEX_LOCK_WARNING.to_string());
    }
    // For a guessed (convention-derived) path the probe resolved the real
    // layout on the host — `.worktrees/<name>` first, then
    // `.claude/worktrees/<name>`, the first that exists or is registered.
    let user_wt_path: Option<String> = spec.worktree.as_ref().map(|w| {
        if w.path_is_guess {
            p.wt_path.clone().unwrap_or_else(|| w.path.clone())
        } else {
            w.path.clone()
        }
    });

    let cwd = match &spec.worktree {
        None => spec.project_root.clone(),
        Some(w0) => {
            // Every `w.path` below is the resolved, user-facing path.
            let resolved = WorktreeSpec {
                path: user_wt_path.clone().unwrap_or_else(|| w0.path.clone()),
                ..w0.clone()
            };
            let w = &resolved;
            // Compare canonical to canonical: git prints realpaths while the
            // row keeps the user-facing path. A raw comparison under a
            // symlinked root would miss our own registration and "adopt" it,
            // or miss the main checkout and adopt THAT.
            let expected_key = p.wt_canon.clone().unwrap_or_else(|| w.path.clone());
            let expected = norm(&expected_key);
            let root_key = p
                .root_canon
                .clone()
                .unwrap_or_else(|| spec.project_root.clone());
            let registered = p.worktrees.iter().find(|r| norm(r.key()) == expected);
            // (o) A locked worktree whose directory is gone: real git lists it
            // as `locked`, not `prunable`. Refused first, in every policy.
            if let Some(r) = registered {
                if r.locked && !p.wt_exists {
                    return Err(refuse(
                        codes::E_WORKSPACE_LOCKED,
                        format!(
                            "worktree {} is registered but its directory is missing and \
                             it is locked (git worktree unlock -- {} to allow repair)",
                            r.path, r.path
                        ),
                    ));
                }
            }
            let elsewhere = p.worktrees.iter().find(|r| {
                norm(r.key()) != expected && !r.prunable && r.branch.as_deref() == Some(&w.branch)
            });
            if let Some(other) = elsewhere {
                // git refuses a second checkout of a branch. The main checkout
                // is the user's: never hijack it.
                if norm(other.key()) == norm(&root_key) {
                    return Err(refuse(
                        codes::E_BRANCH_CHECKED_OUT,
                        format!(
                            "branch {} is checked out in the main checkout {}; \
                             switch it to another branch (or move your work) before repairing",
                            w.branch, other.path
                        ),
                    ));
                }
                // A linked worktree elsewhere may be this workspace moved, or
                // someone else's: ambiguous, so explicit-only (and guarded in
                // `ensure_workspace`).
                warnings.push(format!(
                    "branch {} is checked out at {}, not at {}; only an explicit repair adopts it",
                    w.branch, other.path, w.path
                ));
                git.push((
                    Step::AdoptPath {
                        path: other.path.clone(),
                    },
                    false,
                ));
                other.path.clone()
            } else {
                let from = if p.branch_local {
                    BranchSource::Local
                } else if p.branch_remote {
                    BranchSource::Remote
                } else {
                    let default = p
                        .default_branch
                        .clone()
                        .unwrap_or_else(|| "HEAD".to_string());
                    BranchSource::FetchOrBase {
                        base: spec
                            .base_branch
                            .clone()
                            .filter(|b| !b.trim().is_empty())
                            .unwrap_or_else(|| default.clone()),
                        default,
                    }
                };
                let from_existing_branch = !matches!(from, BranchSource::FetchOrBase { .. });
                if !from_existing_branch {
                    warnings.push(format!(
                        "branch {} exists neither locally nor as origin/{}; only an explicit \
                         repair asks origin for it (and recreates it from the base branch if \
                         origin confirms it is gone)",
                        w.branch, w.branch
                    ));
                }
                // Where a fresh checkout goes: the expected path, unless the
                // path was only guessed and the project already uses the
                // `.worktrees/` layout.
                let add_path = if w.path_is_guess
                    && p.layout_dot_worktrees
                    && !p.wt_exists
                    && registered.is_none()
                {
                    format!("{}/.worktrees/{}", spec.project_root, w.name)
                } else {
                    w.path.clone()
                };
                let add = Step::AddWorktree {
                    path: add_path.clone(),
                    branch: w.branch.clone(),
                    from,
                };
                match registered {
                    Some(r) if p.wt_exists && p.wt_gitdir_ok && !r.prunable => {
                        // Healthy. Note branch drift, never touch it.
                        if r.branch.as_deref() != Some(&w.branch) {
                            branch_drift = r.branch.clone();
                            warnings.push(format!(
                                "worktree has {} checked out, row says {}",
                                r.branch.as_deref().unwrap_or("(detached)"),
                                w.branch
                            ));
                        }
                        w.path.clone()
                    }
                    Some(_) if p.wt_exists && !p.wt_empty && p.wt_git => {
                        // (c) registered and present but the `.git` link is
                        // stale (moved repo / rewritten admin dir).
                        warnings.push(format!(
                            "{} is registered but its git link is stale; only an explicit \
                             repair runs git worktree repair",
                            w.path
                        ));
                        git.push((
                            Step::RepairLinks {
                                path: w.path.clone(),
                            },
                            false,
                        ));
                        w.path.clone()
                    }
                    Some(_) if p.wt_exists && !p.wt_empty => {
                        return Err(refuse(
                            codes::E_REPAIR_FAILED,
                            format!(
                                "{} exists, is not empty and has no .git link; move it aside \
                                 (it is never deleted automatically) and repair again",
                                w.path
                            ),
                        ));
                    }
                    Some(r) => {
                        // (a)/(d) registered, directory gone (prunable, or git
                        // predates the flag) or left empty. Dropping our own
                        // registration is automatic only when the directory is
                        // confirmed vanished, every [`VanishedGuard`] condition
                        // holds (above all: the parent's dev:inode matches the
                        // one recorded while healthy) and the branch already
                        // exists; otherwise it is explicit-only.
                        let guard = vanished_guard(p, r, ctx);
                        let auto =
                            ctx.allow_auto_unregister && guard.holds() && from_existing_branch;
                        if !auto {
                            // Without a store context: exactly #49's warning.
                            let failed = if ctx.allow_auto_unregister {
                                guard.failed()
                            } else {
                                Vec::new()
                            };
                            let siblings = if ctx.allow_auto_unregister {
                                missing_siblings(p)
                            } else {
                                Vec::new()
                            };
                            warnings.push(format!(
                                "git still lists {} but its directory is gone; only an explicit \
                                 repair unregisters that entry and re-adds the worktree{}{}",
                                r.path,
                                if failed.is_empty() {
                                    String::new()
                                } else {
                                    format!(" (not automatic: {})", failed.join(", "))
                                },
                                if siblings.is_empty() {
                                    String::new()
                                } else {
                                    format!(
                                        "; also missing (stale registrations cause this too — \
                                         prune them if they are gone for good): {}",
                                        siblings.join(", ")
                                    )
                                }
                            ));
                        } else {
                            // The fingerprint matched, so a missing sibling is a
                            // stale registration: say so, but do not block.
                            let siblings = missing_siblings(p);
                            if !siblings.is_empty() {
                                warnings.push(format!(
                                    "git also lists {} under the same parent, missing on disk \
                                     (not blocking: the parent fingerprint matches; prune them \
                                     if they are gone for good)",
                                    siblings.join(", ")
                                ));
                            }
                        }
                        vanished = Some(guard);
                        git.push((
                            Step::Unregister {
                                path: r.path.clone(),
                            },
                            auto,
                        ));
                        git.push((add, auto));
                        add_path
                    }
                    None if !p.wt_exists => {
                        // (b) absent on disk and not registered: a pure
                        // create — automatic when the branch already exists.
                        git.push((add, from_existing_branch));
                        add_path
                    }
                    None if p.wt_empty => {
                        // (l) empty leftover from an interrupted add. git
                        // accepts an empty target; not "absent on disk", so
                        // explicit only. TOCTOU: if the dir fills between the
                        // probe and the add, git refuses a non-empty target —
                        // benign, nothing deletes anything.
                        git.push((add, false));
                        add_path
                    }
                    None if p.wt_git => {
                        // (c) a checkout with a `.git` file that git no
                        // longer lists: explicit re-link; verify decides.
                        warnings.push(format!(
                            "{} has a .git link git no longer lists; only an explicit repair \
                             runs git worktree repair",
                            w.path
                        ));
                        git.push((
                            Step::RepairLinks {
                                path: w.path.clone(),
                            },
                            false,
                        ));
                        w.path.clone()
                    }
                    None => {
                        return Err(refuse(
                            codes::E_REPAIR_FAILED,
                            format!(
                                "{} exists, is not empty and is not a git worktree; move it \
                                 aside (it is never deleted automatically) and repair again",
                                w.path
                            ),
                        ));
                    }
                }
            }
        }
    };

    let blocked = !explicit && git.iter().any(|(_, auto)| !*auto);
    let mut steps = Vec::new();
    let mut deferred = Vec::new();
    let cwd = if blocked {
        deferred.extend(git.into_iter().map(|(s, _)| s));
        // Nothing moves: the pane's directory stays the expected one.
        user_wt_path.clone().unwrap_or(cwd)
    } else {
        steps.extend(git.into_iter().map(|(s, _)| s));
        cwd
    };

    // ── tmux: act only on confirmed facts ───────────────────────────────
    let pane_cwd_missing = p.tmux_alive && p.tmux_cwd.is_some() && !p.tmux_cwd_exists;
    if p.tmux_alive && p.tmux_cwd.is_none() {
        warnings.push("could not read the pane's working directory; not respawning it".into());
    }
    if !p.tmux_alive && !p.tmux_dead {
        warnings.push(
            "tmux state unknown (tmux missing or not answering); not creating a session".into(),
        );
    }
    match policy {
        Policy::Explicit => {
            if p.tmux_dead {
                steps.push(Step::TmuxCreate { cwd: cwd.clone() });
            } else if pane_cwd_missing {
                // A pane whose cwd was deleted keeps the dead inode even after
                // the path is recreated; respawn into the verified directory.
                steps.push(Step::TmuxRespawn { cwd: cwd.clone() });
            }
        }
        Policy::Auto { create_dead_tmux } => {
            if create_dead_tmux && p.tmux_dead && !blocked {
                steps.push(Step::TmuxCreate { cwd: cwd.clone() });
            }
            if pane_cwd_missing {
                warnings.push(format!(
                    "the pane's directory {} no longer exists; Repair workspace (or Restart) \
                     respawns it",
                    p.tmux_cwd.as_deref().unwrap_or_default()
                ));
                deferred.push(Step::TmuxRespawn { cwd: cwd.clone() });
            }
        }
    }

    Ok(Plan {
        steps,
        cwd,
        branch_drift,
        warnings,
        needs_explicit_repair: blocked,
        deferred,
        vanished_guard: vanished,
    })
}

/// Render the git steps of a plan into one `bash` script. Prints
/// `outcome=<...>` lines for the parts whose result is only known at run
/// time (which branch source the add used). `--` separates options from
/// paths / refs wherever git accepts it. Test-only: production renders
/// through [`render_git_script_expecting`] (with or without an expected pair).
#[cfg(test)]
pub fn render_git_script(root: &str, steps: &[Step]) -> String {
    render_git_script_expecting(root, steps, None)
}

/// [`render_git_script`] for an automatic removal: `parent_fp` is the parent
/// `(dev, inode)` the plan matched against the recorded fingerprint. The
/// script re-checks it in the same shell right before `worktree remove` and
/// again before `worktree add`, closing the probe-to-apply window (a volume
/// unmounting or remounting in between refuses before the next git step).
pub fn render_git_script_expecting(
    root: &str,
    steps: &[Step],
    parent_fp: Option<(u64, u64)>,
) -> String {
    let rq = quote(root);
    let mut s = String::from("set -e\n");
    if let Some(fp) = parent_fp {
        s.push_str(&format!(
            "if stat -L -c %d / >/dev/null 2>&1; then fpof() {{ stat -L -c '%d:%i' -- \"$1\" 2>/dev/null; }}; \
             else fpof() {{ stat -L -f '%d:%i' -- \"$1\" 2>/dev/null; }}; fi\n\
             fp_expect={}\n",
            quote(&fp_string(fp))
        ));
    }
    for step in steps {
        match step {
            Step::Unregister { path } => {
                // TOCTOU: the probe ran one round trip ago. A late-mounting
                // path (autofs, NFS) may be back now, and `remove --force`
                // would delete whatever is there. Re-check in this shell,
                // right before the remove: anything but an empty directory,
                // a symlink, or a missing parent refuses before any git step.
                let pq = quote(path);
                s.push_str(&format!("p={pq}\n"));
                if parent_fp.is_some() {
                    s.push_str(&format!(
                        "if [ \"$(fpof \"$(dirname -- \"$p\")\")\" != \"$fp_expect\" ]; \
                         then echo \"repair: $p parent changed since the check; {UNREGISTER_REFUSED}\" >&2; exit 1; fi\n"
                    ));
                }
                s.push_str(&format!(
                    "if [ -L \"$p\" ] || [ ! -d \"$(dirname -- \"$p\")\" ] || \
                     {{ [ -e \"$p\" ] && [ -n \"$(ls -A -- \"$p\" 2>/dev/null || echo x)\" ]; }}; \
                     then echo \"repair: $p {UNREGISTER_REFUSED}\" >&2; exit 1; fi\n\
                     git -C {rq} worktree remove --force -- {pq} 1>&2\n"
                ));
            }
            Step::RepairLinks { path } => s.push_str(&format!(
                "git -C {rq} worktree repair -- {} 1>&2\n",
                quote(path)
            )),
            Step::AddWorktree { path, branch, from } => {
                let pq = quote(path);
                let bq = quote(branch);
                if parent_fp.is_some() {
                    s.push_str(&format!(
                        "if [ \"$(fpof \"$(dirname -- {pq})\")\" != \"$fp_expect\" ]; \
                         then echo \"repair: {ADD_REFUSED}\" >&2; exit 1; fi\n"
                    ));
                }
                match from {
                    BranchSource::Local => {
                        s.push_str(&format!(
                            "git -C {rq} worktree add -- {pq} {bq} 1>&2\necho outcome=branch_local\n"
                        ));
                    }
                    BranchSource::Remote => {
                        s.push_str(&format!(
                            "b={bq}\ngit -C {rq} worktree add --track -b \"$b\" -- {pq} \"origin/$b\" 1>&2\necho outcome=branch_remote\n"
                        ));
                    }
                    BranchSource::FetchOrBase { base, default } => {
                        let baseq = quote(base);
                        let defq = quote(default);
                        s.push_str(&format!(
                            "b={bq}\n\
                             if git -C {rq} remote get-url origin >/dev/null 2>&1; then\n\
                             \x20 lr=0; git -C {rq} ls-remote --exit-code --heads -- origin \"refs/heads/$b\" >/dev/null 2>&1 || lr=$?\n\
                             else\n\
                             \x20 lr=2\n\
                             fi\n\
                             case \"$lr\" in\n\
                             0)\n\
                             \x20 if ! git -C {rq} fetch -- origin \"+refs/heads/$b:refs/remotes/origin/$b\" 1>&2; then\n\
                             \x20   echo \"repair: fetching origin/$b failed; not recreating the branch\" >&2; exit 1\n\
                             \x20 fi\n\
                             \x20 git -C {rq} worktree add --track -b \"$b\" -- {pq} \"origin/$b\" 1>&2\n\
                             \x20 echo outcome=branch_remote\n\
                             \x20 ;;\n\
                             2)\n\
                             \x20 basebr={baseq}\n\
                             \x20 defbr={defq}\n\
                             \x20 if git -C {rq} show-ref --verify --quiet \"refs/heads/$basebr\"; then start=\"$basebr\"\n\
                             \x20 elif git -C {rq} show-ref --verify --quiet \"refs/remotes/origin/$basebr\"; then start=\"origin/$basebr\"\n\
                             \x20 elif git -C {rq} show-ref --verify --quiet \"refs/heads/$defbr\"; then start=\"$defbr\"\n\
                             \x20 else start=HEAD\n\
                             \x20 fi\n\
                             \x20 git -C {rq} worktree add -b \"$b\" -- {pq} \"$start\" 1>&2\n\
                             \x20 echo \"outcome=branch_from_base:$start\"\n\
                             \x20 ;;\n\
                             *)\n\
                             \x20 echo \"repair: cannot confirm whether origin still has $b (git ls-remote exit $lr); not recreating it\" >&2; exit 1\n\
                             \x20 ;;\n\
                             esac\n"
                        ));
                    }
                }
            }
            Step::AdoptPath { .. } | Step::TmuxCreate { .. } | Step::TmuxRespawn { .. } => {}
        }
    }
    s
}

// ---------------------------------------------------------------------------
// Executor abstraction (real host vs. scripted fake in tests)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct ScriptOutput {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Everything a repair needs from a host. Production: [`HostExec`]. Tests
/// script the outputs and record every call.
#[async_trait]
pub trait RepairExec: Send + Sync {
    /// Run the read-only probe script (`bash -lc`) on the host.
    async fn run_script(&self, script: &str) -> Result<ScriptOutput, IpcError>;
    /// Run the git apply script. Separate so production can give it a longer
    /// wall clock; test doubles share one queue with `run_script`.
    async fn apply_script(&self, script: &str) -> Result<ScriptOutput, IpcError> {
        self.run_script(script).await
    }
    async fn tmux_new_session(&self, name: &str, cwd: &str, pane_cmd: &str)
        -> Result<(), IpcError>;
    async fn tmux_respawn(&self, name: &str, cwd: &str, pane_cmd: &str) -> Result<(), IpcError>;
}

/// Production executor: local `bash -lc`, or `ssh <host> bash -lc '<script>'`
/// through any [`SshExec`] (the shared ControlMaster client in production,
/// `FakeSsh` in tests); tmux through the same `TmuxExec` the lifecycle
/// commands use. Every script is bounded by a wall clock.
pub struct HostExec<'a> {
    host: String,
    ssh: &'a dyn SshExec,
    tmux: Box<dyn TmuxExec>,
}

impl<'a> HostExec<'a> {
    /// Production: the shared client, tmux via `sessions::exec_for`.
    pub fn new(host: &str, ssh: &'a Arc<SshClient>) -> Self {
        Self::with(host, &**ssh, crate::service::sessions::exec_for(host, ssh))
    }

    /// Any [`SshExec`] (callers pass `&*ssh`) and an explicit tmux executor.
    pub fn with(host: &str, ssh: &'a dyn SshExec, tmux: Box<dyn TmuxExec>) -> Self {
        Self {
            host: host.to_string(),
            ssh,
            tmux,
        }
    }

    async fn run_bash(
        &self,
        script: &str,
        local_wall: Duration,
        connect: Duration,
    ) -> Result<ScriptOutput, IpcError> {
        let out = if self.host == "local" {
            let child = tokio::process::Command::new("bash")
                .args(["-lc", script])
                .kill_on_drop(true)
                .output();
            tokio::time::timeout(local_wall, child)
                .await
                .map_err(|_| {
                    IpcError::new(
                        codes::E_TIMEOUT,
                        format!("local bash exceeded {local_wall:?}"),
                    )
                })?
                .map_err(|e| IpcError::new(codes::E_SHELL, format!("spawn bash: {e}")))?
        } else {
            // Quote the WHOLE script so it crosses the ssh argv-join as one
            // word (see RemoteTmux::remote_bash). The client bounds the whole
            // command by `SshClient::default_wall_clock(connect)`.
            let out = self
                .ssh
                .run(&self.host, &["bash", "-lc", &quote(script)], connect)
                .await?;
            // `SshExec` contract: an unreachable host is ssh exiting 255 with
            // the connect error on stderr — `Ok`, not `Err`. Surface it as a
            // transport failure, so the probe reports E_HOST_OFFLINE and an
            // interrupted apply says it may be partially applied.
            if out.status.code() == Some(255) {
                return Err(IpcError::new(
                    codes::E_SSH,
                    format!(
                        "ssh {} failed: {}",
                        self.host,
                        String::from_utf8_lossy(&out.stderr).trim()
                    ),
                ));
            }
            out
        };
        Ok(ScriptOutput {
            ok: out.status.success(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

#[async_trait]
impl RepairExec for HostExec<'_> {
    async fn run_script(&self, script: &str) -> Result<ScriptOutput, IpcError> {
        self.run_bash(script, PROBE_WALL_CLOCK, PROBE_CONNECT_TIMEOUT)
            .await
    }
    async fn apply_script(&self, script: &str) -> Result<ScriptOutput, IpcError> {
        self.run_bash(script, APPLY_WALL_CLOCK, APPLY_CONNECT_TIMEOUT)
            .await
    }
    async fn tmux_new_session(
        &self,
        name: &str,
        cwd: &str,
        pane_cmd: &str,
    ) -> Result<(), IpcError> {
        self.tmux
            .new_session(name, std::path::Path::new(cwd), pane_cmd)
            .await
    }
    async fn tmux_respawn(&self, name: &str, cwd: &str, pane_cmd: &str) -> Result<(), IpcError> {
        self.tmux
            .respawn_pane_in(name, std::path::Path::new(cwd), pane_cmd)
            .await
    }
}

// ---------------------------------------------------------------------------
// Report
// ---------------------------------------------------------------------------

/// What [`ensure_workspace`] found and did. Serialized to the UI / MCP.
#[derive(Debug, Clone, Serialize)]
pub struct RepairReport {
    pub session_id: Option<i64>,
    pub host_alias: String,
    pub tmux_name: String,
    /// The project root this repair resolved on the host. On `E_REPO_MISSING`
    /// the error message carries the same value, so the user can see which
    /// base-path setting / convention produced it.
    pub project_root: String,
    /// The directory the pane runs (or must run) in — user-facing form, and
    /// always one the probe resolved on the host (never a bare layout guess).
    pub cwd: String,
    /// The same directory as the host resolves it (`pwd -P`), when known.
    pub cwd_physical: Option<String>,
    /// `true` when nothing needed doing.
    pub healthy: bool,
    /// Ordered, human-readable actions that were applied.
    pub actions: Vec<String>,
    pub warnings: Vec<String>,
    /// Automatic run only: the workspace needs an explicit repair; nothing
    /// git-side was applied. `deferred` says what the explicit repair would do.
    pub needs_explicit_repair: bool,
    pub deferred: Vec<String>,
    /// `branch_local` | `branch_remote` | `branch_from_base:<start>` when a
    /// worktree was (re)created.
    pub branch_source: Option<String>,
    /// `created` | `respawned` when tmux was touched.
    pub tmux: Option<String>,
    /// tmux state after the repair (callers that own tmux read these).
    pub tmux_alive: bool,
    /// The session is confirmed gone (not merely unknown).
    pub tmux_dead: bool,
    /// A live pane's reported cwd no longer exists (or the dir under it was
    /// recreated); it runs in a dead inode until respawned.
    pub tmux_cwd_stale: bool,
    pub worktree_row_updated: bool,
    /// Alive sessions on the same host sharing this workspace (reviews, twins)
    /// whose panes may also need a respawn.
    pub sibling_session_ids: Vec<i64>,
    /// Set when our registered worktree directory was found missing: which
    /// conditions of the automatic stale-entry removal held.
    pub vanished_guard: Option<VanishedGuard>,
}

// ---------------------------------------------------------------------------
// ensure_workspace: probe → plan → apply → verify → record
// ---------------------------------------------------------------------------

/// Probe-time transport failure: nothing was changed.
fn host_offline(spec: &WorkspaceSpec, e: IpcError) -> IpcError {
    if e.code == codes::E_SSH || e.code == codes::E_SSH_TIMEOUT {
        IpcError::new(
            codes::E_HOST_OFFLINE,
            format!(
                "host {} unreachable during workspace repair ({}); nothing was changed",
                spec.host_alias, e.message
            ),
        )
    } else {
        e
    }
}

/// Apply-time transport failure / timeout: git steps may have run.
fn apply_interrupted(spec: &WorkspaceSpec, e: IpcError) -> IpcError {
    if e.code == codes::E_SSH || e.code == codes::E_SSH_TIMEOUT || e.code == codes::E_TIMEOUT {
        IpcError::new(
            codes::E_REPAIR_FAILED,
            format!(
                "lost {} while applying the workspace repair ({}); {PARTIALLY_APPLIED} \
                 — run Repair workspace again",
                spec.host_alias, e.message
            ),
        )
    } else {
        e
    }
}

async fn run_probe(exec: &dyn RepairExec, spec: &WorkspaceSpec) -> Result<Probe, IpcError> {
    let out = exec
        .run_script(&probe_script(spec))
        .await
        .map_err(|e| host_offline(spec, e))?;
    if !out.ok {
        return Err(host_offline(
            spec,
            IpcError::new(
                codes::E_SHELL,
                format!("workspace probe failed: {}", out.stderr.trim()),
            ),
        ));
    }
    Ok(parse_probe(&out.stdout))
}

/// Best-effort timeline event; never fails the caller.
fn record_event(store: &Mutex<Store>, session_id: Option<i64>, kind: &str, detail: &str) {
    let Some(id) = session_id else {
        return;
    };
    if let Ok(s) = store.lock() {
        if let Err(e) = s.insert_session_event(id, kind, Some(detail)) {
            tracing::warn!("[repair] event insert failed for session {id}: {e}");
        }
    }
}

fn fail(store: &Mutex<Store>, spec: &WorkspaceSpec, e: IpcError) -> IpcError {
    record_event(
        store,
        spec.session_id,
        EVENT_REPAIR_FAILED,
        &format!("{}: {}", e.code, e.message),
    );
    e
}

fn healthy_after(spec: &WorkspaceSpec, cwd: &str, p: &Probe) -> bool {
    if !(p.root_exists && p.root_git && p.root_gitdir_ok) {
        return false;
    }
    match &spec.worktree {
        None => true,
        Some(_) => {
            p.wt_exists
                && p.wt_gitdir_ok
                && p.worktrees.iter().any(|r| {
                    norm(r.key()) == norm(p.wt_canon.as_deref().unwrap_or(cwd)) && !r.prunable
                })
        }
    }
}

/// Canonicalize a LOCAL path the way the probe does on a host: resolve the
/// nearest existing ancestor, append the missing remainder.
fn local_canon(p: &str) -> String {
    let mut cur = std::path::PathBuf::from(p);
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    loop {
        if let Ok(c) = std::fs::canonicalize(&cur) {
            let mut out = c;
            for r in rest.iter().rev() {
                out.push(r);
            }
            return out.to_string_lossy().into_owned();
        }
        match (cur.file_name().map(|f| f.to_os_string()), cur.parent()) {
            (Some(f), Some(parent)) => {
                rest.push(f);
                cur = parent.to_path_buf();
            }
            _ => return p.to_string(),
        }
    }
}

/// Explicit adoption guard: `Some(reason)` when the checkout at `adopt_key`
/// (host-canonical) belongs to another workspace fleet knows about — another
/// worktree row on the same branch, or any other running / not-yet-dismissed
/// ghost session mapped to that checkout (by portable key, or by canonical
/// local path). Two rows sharing one path would let a safe-kill of either
/// delete the other's tree.
fn adoption_conflict(
    s: &Store,
    spec: &WorkspaceSpec,
    w: &WorktreeSpec,
    adopt_key: &str,
) -> Result<Option<String>, IpcError> {
    let Some(pid) = spec.project_id else {
        return Ok(None);
    };
    let rows = s.list_worktrees_for_project(pid)?;
    if let Some(other) = rows
        .iter()
        .find(|r| r.name != w.name && r.branch.as_deref() == Some(w.branch.as_str()))
    {
        return Ok(Some(format!("fleet tracks it as worktree {}", other.name)));
    }
    let local = spec.host_alias == "local";
    let adopt_name = basename(adopt_key);
    let target = if local {
        local_canon(adopt_key)
    } else {
        adopt_key.to_string()
    };
    for o in s.list_sessions_for_host(&spec.host_alias)? {
        if Some(o.id) == spec.session_id || o.kind == "bg" || o.project_id != Some(pid) {
            continue;
        }
        // A key equal to ours is our own workspace group (reviews, twins).
        let by_key = adopt_name != w.name && o.worktree_key.as_deref() == Some(adopt_name);
        let by_row = local
            && o.worktree_id
                .and_then(|wid| rows.iter().find(|r| r.id == wid))
                .is_some_and(|r| r.name != w.name && norm(&local_canon(&r.path)) == norm(&target));
        if by_key || by_row {
            return Ok(Some(format!(
                "session {} ({}) uses it",
                o.tmux_name, o.status
            )));
        }
    }
    Ok(None)
}

/// Make the workspace described by `spec` healthy within `policy`.
/// Idempotent: a healthy workspace costs one probe and writes nothing.
///
/// Errors: `E_HOST_OFFLINE` (probe could not reach the host — no change),
/// `E_REPO_MISSING`, `E_BRANCH_CHECKED_OUT`, `E_WORKSPACE_LOCKED`,
/// `E_REPAIR_FAILED` (a git step failed, the result did not verify, or the
/// apply was interrupted — "may be partially applied"), `E_TMUX`. Under
/// [`Policy::Auto`] a workspace that needs an explicit step is NOT an error:
/// the report carries `needs_explicit_repair` (see [`require_no_explicit`]).
/// A failure never deletes or ghosts the session row.
pub async fn ensure_workspace(
    spec: &WorkspaceSpec,
    policy: Policy,
    siblings: Vec<i64>,
    store: &Mutex<Store>,
    exec: &dyn RepairExec,
) -> Result<RepairReport, IpcError> {
    // Click-driven entry points may drop our own stale registration too, but
    // only under the full [`VanishedGuard`], i.e. with a matching parent
    // fingerprint recorded while the worktree was healthy.
    ensure_workspace_with(spec, policy, siblings, store, exec, true).await
}

/// [`ensure_workspace`] with an explicit permission to drop our own stale
/// registration automatically ([`AutoContext::allow_auto_unregister`]); the
/// [`VanishedGuard`] (with the parent fingerprint) still decides.
pub async fn ensure_workspace_with(
    spec: &WorkspaceSpec,
    policy: Policy,
    siblings: Vec<i64>,
    store: &Mutex<Store>,
    exec: &dyn RepairExec,
    allow_auto_unregister: bool,
) -> Result<RepairReport, IpcError> {
    let probe = run_probe(exec, spec).await?;
    record_healthy_fingerprint(store, spec, &probe);
    let ctx = {
        let s = store.lock().map_err(|_| IpcError::lock())?;
        AutoContext {
            allow_auto_unregister,
            ..auto_context(&s, spec, &probe)
        }
    };
    let fix = plan_with(spec, &probe, policy, ctx).map_err(|e| fail(store, spec, e))?;
    let explicit = policy == Policy::Explicit;

    let mut report = RepairReport {
        session_id: spec.session_id,
        host_alias: spec.host_alias.clone(),
        tmux_name: spec.tmux_name.clone(),
        project_root: spec.project_root.clone(),
        cwd: fix.cwd.clone(),
        cwd_physical: if spec.worktree.is_some() {
            probe.wt_canon.clone()
        } else {
            probe.root_canon.clone()
        },
        healthy: fix.is_noop() && !fix.needs_explicit_repair,
        actions: Vec::new(),
        warnings: fix.warnings.clone(),
        needs_explicit_repair: fix.needs_explicit_repair,
        deferred: fix.deferred.iter().map(Step::describe).collect(),
        branch_source: None,
        tmux: None,
        tmux_alive: probe.tmux_alive,
        tmux_dead: probe.tmux_dead,
        tmux_cwd_stale: probe.tmux_alive && probe.tmux_cwd.is_some() && !probe.tmux_cwd_exists,
        worktree_row_updated: false,
        sibling_session_ids: siblings,
        vanished_guard: fix.vanished_guard,
    };
    if fix.needs_explicit_repair {
        // Nothing git-side runs automatically; the caller decides.
        return Ok(report);
    }

    // ── adoption guard (explicit only: AdoptPath is never automatic) ────
    let adopt_path = fix.steps.iter().find_map(|s| match s {
        Step::AdoptPath { path } => Some(path.clone()),
        _ => None,
    });
    if let (Some(path), Some(w)) = (&adopt_path, &spec.worktree) {
        let adopt_key = probe
            .worktrees
            .iter()
            .find(|r| &r.path == path)
            .map(|r| r.key().to_string())
            .unwrap_or_else(|| path.clone());
        let conflict = {
            let s = store.lock().map_err(|_| IpcError::lock())?;
            adoption_conflict(&s, spec, w, &adopt_key)?
        };
        if let Some(reason) = conflict {
            return Err(fail(
                store,
                spec,
                IpcError::new(
                    codes::E_BRANCH_CHECKED_OUT,
                    format!(
                        "branch {} is checked out at {}, but {reason}; not adopting another \
                         workspace's checkout (repair or remove that one first)",
                        w.branch, path
                    ),
                ),
            ));
        }
    }
    let adopting = adopt_path.is_some();

    // ── apply git steps, then verify (adoption is verified too) ─────────
    let mut dir_changed = false;
    let mut final_branch: Option<String> = fix.branch_drift.clone();
    if fix.has_git_steps() || adopting {
        let git_steps: Vec<Step> = fix.steps.iter().filter(|s| s.is_git()).cloned().collect();
        if !git_steps.is_empty() {
            // An automatic removal re-checks the matched parent fingerprint
            // inside the script, before the remove and before the add.
            let expect_fp = if !explicit
                && git_steps
                    .iter()
                    .any(|s| matches!(s, Step::Unregister { .. }))
            {
                probe.wt_parent_fp
            } else {
                None
            };
            let script = render_git_script_expecting(&spec.project_root, &git_steps, expect_fp);
            let out = exec
                .apply_script(&script)
                .await
                .map_err(|e| fail(store, spec, apply_interrupted(spec, e)))?;
            if !out.ok {
                // The re-check before `remove --force` refused: the directory
                // reappeared between the probe and the apply. Nothing ran; a
                // refusal (not transient), so the tick backs off.
                let e = if out.stderr.contains(ADD_REFUSED) {
                    IpcError::new(
                        codes::E_REPAIR_REQUIRED,
                        format!(
                            "the parent of {} changed between the check and the re-add on {} \
                             (remounted?); the stale entry was removed but nothing was \
                             re-added. Check it, then use Repair workspace ({})",
                            fix.cwd,
                            spec.host_alias,
                            out.stderr.trim()
                        ),
                    )
                } else if out.stderr.contains(UNREGISTER_REFUSED) {
                    IpcError::new(
                        codes::E_REPAIR_REQUIRED,
                        format!(
                            "the worktree directory at {} reappeared (or its parent vanished) \
                             between the check and the repair on {}; nothing was removed. \
                             Check it, then use Repair workspace ({})",
                            fix.cwd,
                            spec.host_alias,
                            out.stderr.trim()
                        ),
                    )
                } else {
                    IpcError::new(
                        codes::E_REPAIR_FAILED,
                        format!(
                            "workspace repair step failed on {}: {}",
                            spec.host_alias,
                            out.stderr.trim()
                        ),
                    )
                };
                return Err(fail(store, spec, e));
            }
            for step in &git_steps {
                report.actions.push(step.describe());
            }
            report.branch_source = out
                .stdout
                .lines()
                .filter_map(|l| l.trim().strip_prefix("outcome="))
                .next_back()
                .map(str::to_string);
        }
        // Verify against the (possibly re-pathed or adopted) cwd. An adopted
        // checkout must exist and be healthy before tmux is moved there:
        // older git does not flag a missing checkout as `prunable`.
        let mut vspec = spec.clone();
        if let Some(w) = vspec.worktree.as_mut() {
            // Verify exactly the planned directory; no layout re-guessing.
            w.path = fix.cwd.clone();
            w.path_is_guess = false;
        }
        let after = run_probe(exec, &vspec)
            .await
            .map_err(|e| fail(store, spec, e))?;
        if !healthy_after(spec, &fix.cwd, &after) {
            return Err(fail(
                store,
                spec,
                IpcError::new(
                    codes::E_REPAIR_FAILED,
                    format!(
                        "workspace at {} on {} is still not a healthy worktree after repair \
                         (steps: {})",
                        fix.cwd,
                        spec.host_alias,
                        report.actions.join("; ")
                    ),
                ),
            ));
        }
        if final_branch.is_none() {
            final_branch = after
                .worktrees
                .iter()
                .find(|r| norm(r.key()) == norm(after.wt_canon.as_deref().unwrap_or(&fix.cwd)))
                .and_then(|r| r.branch.clone());
        }
        report.tmux_alive = after.tmux_alive;
        report.tmux_dead = after.tmux_dead;
        report.cwd_physical = after.wt_canon.clone();
        // The repaired worktree is healthy: refresh its parent fingerprint.
        record_healthy_fingerprint(store, spec, &after);
        dir_changed = true;
    }
    if adopting {
        report
            .actions
            .push(format!("adopt existing checkout at {}", fix.cwd));
    }

    // ── tmux ────────────────────────────────────────────────────────────
    for step in &fix.steps {
        let res = match step {
            Step::TmuxCreate { cwd } => exec
                .tmux_new_session(&spec.tmux_name, cwd, &spec.pane_cmd)
                .await
                .map(|_| "created"),
            Step::TmuxRespawn { cwd } => exec
                .tmux_respawn(&spec.tmux_name, cwd, &spec.pane_cmd)
                .await
                .map(|_| "respawned"),
            _ => continue,
        };
        let what = res.map_err(|e| fail(store, spec, e))?;
        report.tmux = Some(what.to_string());
        report.tmux_alive = true;
        report.tmux_dead = false;
        report.tmux_cwd_stale = false;
        report.actions.push(step.describe());
    }
    if dir_changed && report.tmux.is_none() && report.tmux_alive {
        // The directory was (re)created under a live pane left to the caller.
        report.tmux_cwd_stale = true;
    }

    // ── record: rows + timeline (one lock, no awaits) ───────────────────
    {
        let s = store.lock().map_err(|_| IpcError::lock())?;
        if let (Some(w), Some(pid)) = (&spec.worktree, spec.project_id) {
            let branch = final_branch.clone().unwrap_or_else(|| w.branch.clone());
            let existing = s
                .list_worktrees_for_project(pid)?
                .into_iter()
                .find(|r| r.name == w.name);
            // `worktrees.path` is a LOCAL path: a remote repair never writes
            // it. Re-pathing an existing row happens only on explicit
            // adoption; the branch field follows the checkout only on an
            // explicit repair. A missing row is recorded in both policies
            // (creation, not rewrite); refresh_projects later stores git's
            // own (resolved) form, and all comparisons are canonical anyway.
            let write: Option<(String, String)> = match (&existing, w.row_is_local) {
                (Some(row), true) if explicit && adopting && norm(&row.path) != norm(&fix.cwd) => {
                    Some((fix.cwd.clone(), branch.clone()))
                }
                (Some(row), _) if explicit && row.branch.as_deref() != Some(branch.as_str()) => {
                    Some((row.path.clone(), branch.clone()))
                }
                (None, true) => Some((fix.cwd.clone(), branch.clone())),
                _ => None,
            };
            let mut wt_id = existing.as_ref().map(|r| r.id);
            if let Some((path, branch)) = write {
                wt_id = Some(s.upsert_worktree(pid, &w.name, &path, Some(&branch))?);
                report.worktree_row_updated = true;
            }
            // Link the session to its worktree row (reconcile only ever sets
            // `worktree_key`, never the FK).
            if let (Some(sid), Some(wid)) = (spec.session_id, wt_id) {
                if let Some(row) = s.get_session_by_id(sid)? {
                    if row.worktree_id != Some(wid) {
                        s.conn_ref().execute(
                            "UPDATE sessions SET worktree_id=?1 WHERE id=?2",
                            rusqlite::params![wid, sid],
                        )?;
                        // Re-stamp the (unchanged) key so `session:updated`
                        // carries the new FK to the UI.
                        s.set_worktree_key(sid, Some(&w.name))?;
                    }
                }
            }
        }
        if let Some(sid) = spec.session_id {
            if report.tmux.as_deref() == Some("created") {
                s.restore_session(sid)?;
            }
            if !report.actions.is_empty() {
                if let Err(e) =
                    s.insert_session_event(sid, EVENT_REPAIRED, Some(&event_detail(&report)))
                {
                    tracing::warn!("[repair] event insert failed for session {sid}: {e}");
                }
            }
        }
    }
    report.healthy = report.actions.is_empty();
    Ok(report)
}

/// The `workspace_repaired` event detail. Always carries `branch_source`, so
/// a branch recreated from its base is visible on the timeline.
pub fn event_detail(report: &RepairReport) -> String {
    let mut v = serde_json::json!({
        "cwd": report.cwd,
        "actions": report.actions,
        "branch_source": report.branch_source,
        "tmux": report.tmux,
        "warnings": report.warnings,
    });
    if let Some(g) = &report.vanished_guard {
        v["vanished_guard"] = serde_json::to_value(g).unwrap_or_default();
    }
    v.to_string()
}

/// A registered, healthy worktree: remember its canonical parent's
/// `dev:inode`, the evidence a later automatic removal must match.
/// Best-effort; never fails the caller.
fn record_healthy_fingerprint(store: &Mutex<Store>, spec: &WorkspaceSpec, p: &Probe) {
    if spec.worktree.is_none() || !(p.wt_exists && p.wt_gitdir_ok) {
        return;
    }
    let (Some(canon), Some(fp)) = (p.wt_canon.as_deref().and_then(clean_canon), p.wt_parent_fp)
    else {
        return;
    };
    let registered = p
        .worktrees
        .iter()
        .any(|r| norm(r.key()) == canon && !r.prunable && !r.bare);
    if !registered {
        return;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    if let Ok(s) = store.lock() {
        if let Err(e) = s.record_parent_fingerprint(&spec.host_alias, canon, &fp_string(fp), now) {
            tracing::warn!("[repair] recording the parent fingerprint of {canon} failed: {e}");
        }
    }
}

/// What the store says about this workspace, for the [`VanishedGuard`]:
/// other sessions mapped to it, and the parent fingerprint recorded while it
/// was healthy. No worktree / project id, or a store error, reads as
/// "unknown", which blocks the automatic removal.
fn auto_context(s: &Store, spec: &WorkspaceSpec, probe: &Probe) -> AutoContext {
    let (Some(w), Some(pid)) = (&spec.worktree, spec.project_id) else {
        return AutoContext::default();
    };
    let recorded_parent_fp = probe
        .wt_canon
        .as_deref()
        .and_then(clean_canon)
        .and_then(|c| s.parent_fingerprint(&spec.host_alias, c).ok().flatten())
        .and_then(|v| parse_fp(&v));
    let (Ok(rows), Ok(sessions)) = (
        s.list_worktrees_for_project(pid),
        s.list_sessions_for_host(&spec.host_alias),
    ) else {
        return AutoContext::default();
    };
    let local = spec.host_alias == "local";
    let target = if local {
        local_canon(&w.path)
    } else {
        w.path.clone()
    };
    let mapped = sessions.iter().any(|o| {
        Some(o.id) != spec.session_id
            && o.kind != "bg"
            && o.project_id == Some(pid)
            && (o.worktree_key.as_deref() == Some(w.name.as_str())
                || o.worktree_id
                    .and_then(|wid| rows.iter().find(|r| r.id == wid))
                    .is_some_and(|r| {
                        r.name == w.name || (local && norm(&local_canon(&r.path)) == norm(&target))
                    }))
    });
    AutoContext {
        other_sessions_mapped: Some(mapped),
        // Set by the caller (`ensure_workspace_with`).
        allow_auto_unregister: false,
        recorded_parent_fp,
    }
}

/// Lifecycle helpers (new session, restart, recreate) must not start or
/// respawn tmux in a workspace that still needs an explicit step: turn that
/// into `E_REPAIR_REQUIRED`, naming what the explicit repair would do.
pub fn require_no_explicit(report: RepairReport) -> Result<RepairReport, IpcError> {
    if !report.needs_explicit_repair {
        return Ok(report);
    }
    Err(IpcError::new(
        codes::E_REPAIR_REQUIRED,
        format!(
            "the workspace at {} needs an explicit repair: {}. Use Repair workspace (it would: {})",
            report.cwd,
            report.warnings.join("; "),
            report.deferred.join("; ")
        ),
    ))
}

// ---------------------------------------------------------------------------
// Building a spec from an existing session row
// ---------------------------------------------------------------------------

/// `(project_root, cwd)` of a project (and optional worktree) on a REMOTE
/// host, resolved with the exact function `new_session_inner` uses
/// (`sessions::remote_project_path`), so repair never disagrees with where a
/// session is created. This is the ONLY place `repair` derives a remote
/// path. `root` / `layout` are the host's `projects.*` settings, snapshotted
/// under the store lock by the caller (`project_base_for` + `layout`); `root`
/// may start with `~/`, which is expanded against the remote `$HOME` here. The
/// local `worktrees.path` column is a local path and is never used for a
/// remote host.
async fn resolve_remote_paths(
    ssh: &dyn SshExec,
    host: &str,
    root: &str,
    layout: crate::projects::Layout,
    owner: &str,
    repo: &str,
    wt_name: Option<&str>,
) -> Result<(String, String), IpcError> {
    crate::validate::path_component("owner", owner)?;
    crate::validate::path_component("repo", repo)?;
    if let Some(name) = wt_name {
        crate::validate::path_component("worktree name", name)?;
    }
    let home = ssh.remote_home(host).await?;
    let root = crate::service::projects::expand_home(root, &home);
    Ok(crate::service::sessions::remote_project_path(
        &root, layout, owner, repo, wt_name,
    ))
}

/// Snapshot taken under the store lock; finalized off-lock (remote `$HOME`).
struct SpecSeed {
    row: SessionRow,
    owner: String,
    repo: String,
    base_path: String,
    /// The session host's projects root (`projects.base_path` entry, env var
    /// for `local`, or the layout default; unexpanded) and layout, taken under
    /// the lock for `resolve_remote_paths`.
    projects_root: String,
    layout: crate::projects::Layout,
    /// `(name, path-from-row, branch)` for a linked worktree.
    worktree: Option<(String, Option<String>, Option<String>)>,
    siblings: Vec<i64>,
}

fn seed_for_session(s: &Store, row: &SessionRow) -> Result<SpecSeed, IpcError> {
    let pid = row.project_id.ok_or_else(|| {
        IpcError::new(codes::E_NOREPO, "session has no project; nothing to repair")
    })?;
    let (owner, repo) = crate::service::sessions::fetch_owner_repo(s, pid)?;
    let base_path = s
        .project_base_path(pid)?
        .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("project {pid} not found")))?;
    let rows = s.list_worktrees_for_project(pid)?;
    // The FK wins; otherwise the portable key (set by reconcile).
    let wt_row = match row.worktree_id {
        Some(wid) => s.get_worktree_row(wid)?,
        None => None,
    };
    if let Some(ref wt) = wt_row {
        if wt.project_id != pid {
            return Err(IpcError::new(
                codes::E_INVALID_STATE,
                format!(
                    "session {} points at worktree {} of project {}, but belongs to project {}",
                    row.id, wt.id, wt.project_id, pid
                ),
            ));
        }
    }
    let name = wt_row
        .as_ref()
        .map(|w| w.name.clone())
        .or_else(|| row.worktree_key.clone());
    let worktree = match name {
        Some(n) if n != "main" => {
            let by_key = wt_row
                .clone()
                .or_else(|| rows.iter().find(|w| w.name == n).cloned());
            Some((
                n,
                by_key.as_ref().map(|w| w.path.clone()),
                by_key.and_then(|w| w.branch),
            ))
        }
        _ => None,
    };
    let siblings = s
        .list_sessions_for_host(&row.host_alias)?
        .into_iter()
        .filter(|o| {
            o.id != row.id
                && o.status == "running"
                && o.project_id == Some(pid)
                && o.worktree_key.is_some()
                && o.worktree_key == row.worktree_key
        })
        .map(|o| o.id)
        .collect();
    Ok(SpecSeed {
        row: row.clone(),
        owner,
        repo,
        base_path,
        projects_root: crate::service::projects::project_base_for(s, &row.host_alias),
        layout: crate::service::projects::layout(s),
        worktree,
        siblings,
    })
}

/// Build the [`WorkspaceSpec`] for an existing session row: paths from the
/// local `projects` / `worktrees` tables for `local`, from the
/// host's `projects.*` root and layout (plus the remote `$HOME`)
/// for any other host. Returns the spec and the ids of alive sibling
/// sessions sharing the workspace.
///
/// Errors: `E_NOREPO` (orphan session), `E_BG_SESSION`, `E_INVALID_STATE`
/// (worktree row belongs to another project).
pub async fn spec_for_session(
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    session_id: i64,
) -> Result<(WorkspaceSpec, Vec<i64>), IpcError> {
    let seed = {
        let s = store.lock().map_err(|_| IpcError::lock())?;
        let row = s
            .get_session_by_id(session_id)?
            .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, "session not found"))?;
        if row.kind == "bg" {
            return Err(IpcError::new(
                codes::E_BG_SESSION,
                "background sessions have no worktree or tmux pane to repair",
            ));
        }
        seed_for_session(&s, &row)?
    };
    let row = &seed.row;
    let pane_cmd = crate::service::sessions::recreate_pane_command(
        &row.kind,
        row.claude_session_id.as_deref(),
    );
    let is_local = row.host_alias == "local";
    let (project_root, worktree) = if is_local {
        let root = seed.base_path.clone();
        let wt = seed.worktree.as_ref().map(|(name, path, branch)| {
            let (path, guess) = match path {
                Some(p) => (p.clone(), false),
                None => (format!("{root}/.claude/worktrees/{name}"), true),
            };
            WorktreeSpec {
                name: name.clone(),
                path,
                branch: branch.clone().unwrap_or_else(|| name.clone()),
                path_is_guess: guess,
                row_is_local: true,
            }
        });
        (root, wt)
    } else {
        let wt_name = seed.worktree.as_ref().map(|(n, _, _)| n.as_str());
        let (root, cwd) = resolve_remote_paths(
            &**ssh,
            &row.host_alias,
            &seed.projects_root,
            seed.layout,
            &seed.owner,
            &seed.repo,
            wt_name,
        )
        .await?;
        let wt = seed
            .worktree
            .as_ref()
            .map(|(name, _, branch)| WorktreeSpec {
                name: name.clone(),
                path: cwd.clone(),
                branch: branch.clone().unwrap_or_else(|| name.clone()),
                path_is_guess: true,
                row_is_local: false,
            });
        (root, wt)
    };
    if let Some(w) = &worktree {
        crate::validate::git_ref(&w.branch)?;
    }
    Ok((
        WorkspaceSpec {
            host_alias: row.host_alias.clone(),
            tmux_name: row.tmux_name.clone(),
            project_root,
            worktree,
            base_branch: None,
            pane_cmd,
            session_id: Some(row.id),
            project_id: row.project_id,
        },
        seed.siblings,
    ))
}

/// What `new_session` has in hand for an EXISTING worktree row (or the main
/// checkout) right before `tmux new-session`.
pub struct NewSessionWorkspace<'a> {
    pub host_alias: &'a str,
    pub project_id: i64,
    pub worktree_id: Option<i64>,
    pub tmux_name: &'a str,
    pub pane_cmd: &'a str,
    /// The cwd `new_session` resolved: the row's path locally, the
    /// settings-derived `<root>/[<owner>/]<repo>[/.claude/worktrees/<name>]`
    /// remotely (`sessions::remote_project_path_for`).
    pub cwd: &'a str,
    pub base_branch: Option<&'a str>,
}

/// Pure half of [`ensure_for_new_session`]: build the spec from the rows.
/// `remote_root` is the project root on a remote host (`None` for `local`).
fn spec_for_new_session(
    s: &Store,
    w: &NewSessionWorkspace<'_>,
    remote_root: Option<String>,
) -> Result<WorkspaceSpec, IpcError> {
    let is_local = w.host_alias == "local";
    let project_root = match remote_root {
        Some(r) => r,
        None => s.project_base_path(w.project_id)?.ok_or_else(|| {
            IpcError::new(
                codes::E_NOTFOUND,
                format!("project {} not found", w.project_id),
            )
        })?,
    };
    let wt = match w.worktree_id {
        Some(wid) => s.get_worktree_row(wid)?,
        None => None,
    };
    let worktree = wt.filter(|r| r.name != "main").map(|r| WorktreeSpec {
        branch: r.branch.clone().unwrap_or_else(|| r.name.clone()),
        name: r.name,
        path: w.cwd.to_string(),
        path_is_guess: !is_local,
        row_is_local: is_local,
    });
    // Same validation as `spec_for_session`: these values reach git.
    if let Some(wt) = &worktree {
        crate::validate::git_ref(&wt.branch)?;
    }
    let base_branch = w
        .base_branch
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .map(str::to_string);
    if let Some(b) = &base_branch {
        crate::validate::git_ref(b)?;
    }
    Ok(WorkspaceSpec {
        host_alias: w.host_alias.to_string(),
        tmux_name: w.tmux_name.to_string(),
        project_root,
        worktree,
        base_branch,
        pane_cmd: w.pane_cmd.to_string(),
        session_id: None,
        project_id: Some(w.project_id),
    })
}

/// `new_session`'s pre-tmux check for an existing worktree row / main
/// checkout ([`Entry::NewSession`]: automatic). Returns the path tmux must
/// start in, or `E_REPAIR_REQUIRED` when only an explicit repair can make
/// it usable. No row exists yet, so nothing is recorded on a timeline here —
/// the caller does that once the row appears.
pub async fn ensure_for_new_session(
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    w: NewSessionWorkspace<'_>,
) -> Result<RepairReport, IpcError> {
    let remote_root = if w.host_alias == "local" {
        None
    } else {
        let (owner, repo, root, layout) = {
            let s = store.lock().map_err(|_| IpcError::lock())?;
            let (owner, repo) = crate::service::sessions::fetch_owner_repo(&s, w.project_id)?;
            (
                owner,
                repo,
                crate::service::projects::project_base_for(&s, w.host_alias),
                crate::service::projects::layout(&s),
            )
        };
        Some(
            resolve_remote_paths(&**ssh, w.host_alias, &root, layout, &owner, &repo, None)
                .await?
                .0,
        )
    };
    let spec = {
        let s = store.lock().map_err(|_| IpcError::lock())?;
        spec_for_new_session(&s, &w, remote_root)?
    };
    let exec = HostExec::new(w.host_alias, ssh);
    let report = ensure_workspace(
        &spec,
        policy_for(Entry::NewSession),
        Vec::new(),
        store,
        &exec,
    )
    .await?;
    require_no_explicit(report)
}

/// Repair of one session: [`Entry::Explicit`] (the Repair workspace button,
/// the confirm-gated MCP tool) or [`Entry::Attach`] (the pre-attach check,
/// automatic: never respawns a live pane, never unregisters / adopts /
/// rebranches). An attach that finds work for an explicit repair returns
/// `Ok` with `needs_explicit_repair` so the UI can say so and still attach.
/// Refuses on an unreachable host (`E_HOST_OFFLINE`) before touching anything.
pub async fn repair_session(
    session_id: i64,
    explicit: bool,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<RepairReport, IpcError> {
    {
        let s = store.lock().map_err(|_| IpcError::lock())?;
        let row = s
            .get_session_by_id(session_id)?
            .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, "session not found"))?;
        let host = s
            .get_host_row(&row.host_alias)?
            .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, "host not found"))?;
        if !host.reachable {
            return Err(IpcError::new(
                codes::E_HOST_OFFLINE,
                format!(
                    "host {} is not reachable; workspace repair skipped",
                    host.alias
                ),
            ));
        }
    }
    let (spec, siblings) = spec_for_session(store, ssh, session_id).await?;
    let exec = HostExec::new(&spec.host_alias, ssh);
    let entry = if explicit {
        Entry::Explicit
    } else {
        Entry::Attach
    };
    ensure_workspace(&spec, policy_for(entry), siblings, store, &exec).await
}

/// The automatic pre-check for lifecycle callers that own tmux (restart,
/// recreate). Returns the verified cwd inside the report, or
/// `E_REPAIR_REQUIRED` when only an explicit repair can make the workspace
/// usable.
pub async fn ensure_session_workspace(
    session_id: i64,
    entry: Entry,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<RepairReport, IpcError> {
    let (spec, siblings) = spec_for_session(store, ssh, session_id).await?;
    let exec = HostExec::new(&spec.host_alias, ssh);
    let report = ensure_workspace(&spec, policy_for(entry), siblings, store, &exec).await?;
    require_no_explicit(report)
}

/// The opt-in reconcile tick's repair (`repair.auto_on_tick`): the same
/// automatic pre-check as [`ensure_session_workspace`]; dropping a stale
/// registration still needs the full [`VanishedGuard`] (parent fingerprint).
pub async fn ensure_session_workspace_for_tick(
    session_id: i64,
    entry: Entry,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<RepairReport, IpcError> {
    let (spec, siblings) = spec_for_session(store, ssh, session_id).await?;
    let exec = HostExec::new(&spec.host_alias, ssh);
    let report =
        ensure_workspace_with(&spec, policy_for(entry), siblings, store, &exec, true).await?;
    require_no_explicit(report)
}

#[cfg(test)]
mod tests {
    use super::plan as make_plan;
    use super::*;
    use std::collections::VecDeque;

    /// Lifecycle side effect that owns tmux (new session, restart, recreate).
    const AUTO: Policy = Policy::Auto {
        create_dead_tmux: false,
    };
    /// The attach path: automatic, may create a confirmed-dead session.
    const ATTACH: Policy = Policy::Auto {
        create_dead_tmux: true,
    };

    // ── fixtures ──────────────────────────────────────────────────────────

    fn spec(worktree: bool) -> WorkspaceSpec {
        WorkspaceSpec {
            host_alias: "local".into(),
            tmux_name: "dev-x".into(),
            project_root: "/repo".into(),
            worktree: worktree.then(|| WorktreeSpec {
                name: "feat".into(),
                path: "/repo/.claude/worktrees/feat".into(),
                branch: "feat".into(),
                path_is_guess: false,
                row_is_local: true,
            }),
            base_branch: None,
            pane_cmd: "cl".into(),
            session_id: None,
            project_id: None,
        }
    }

    fn main_wt() -> RegisteredWorktree {
        RegisteredWorktree {
            path: "/repo".into(),
            branch: Some("main".into()),
            ..Default::default()
        }
    }

    fn feat_wt() -> RegisteredWorktree {
        RegisteredWorktree {
            path: "/repo/.claude/worktrees/feat".into(),
            branch: Some("feat".into()),
            ..Default::default()
        }
    }

    /// A fully healthy probe for `spec(true)`.
    fn healthy() -> Probe {
        Probe {
            root_exists: true,
            root_git: true,
            root_gitdir_ok: true,
            wt_exists: true,
            wt_git: true,
            wt_gitdir_ok: true,
            branch_local: true,
            default_branch: Some("main".into()),
            tmux_alive: true,
            tmux_cwd: Some("/repo/.claude/worktrees/feat".into()),
            tmux_cwd_exists: true,
            worktrees: vec![main_wt(), feat_wt()],
            ..Default::default()
        }
    }

    fn add_local() -> Step {
        Step::AddWorktree {
            path: "/repo/.claude/worktrees/feat".into(),
            branch: "feat".into(),
            from: BranchSource::Local,
        }
    }

    // ── probe parsing ─────────────────────────────────────────────────────

    #[test]
    fn probe_script_quotes_every_value_and_never_fails() {
        let mut s = spec(true);
        s.project_root = "/re po's".into();
        s.tmux_name = "a;b".into();
        let script = probe_script(&s);
        assert!(script.starts_with("set +e"));
        assert!(script.contains("root='/re po'\\''s'"), "{script}");
        assert!(script.contains("sess='a;b'"), "{script}");
        assert!(script.contains("br='feat'"));
        assert!(script.contains("tmux has-session -t \"=$sess\""));
        assert!(script.contains("#{pane_current_path}"));
        assert!(script.contains("worktree list --porcelain"));
        assert!(script.contains("then echo wt_entry_exists=1; else echo wt_entry_exists=0; fi"));
        assert!(script.contains("wt_parent_exists=$(yn test -d \"$(dirname -- \"$wt\")\")"));
        assert!(script.contains("if stat -L -c %d / >/dev/null 2>&1; then"));
        assert!(script.contains("stat -L -f %d -- \"$1\""));
        assert!(script.contains("echo \"root_dev=$(devof \"$root\")\""));
        assert!(script.contains("echo \"wt_parent_dev=$(devof \"$(dirname -- \"$wt\")\")\""));
        assert!(script.trim_end().ends_with("exit 0"));
    }

    #[test]
    fn parse_probe_reads_the_vanished_guard_keys_and_never_assumes_absence() {
        let p = parse_probe("wt_entry_exists=0\nwt_parent_exists=1\n@@worktrees\n");
        assert_eq!(p.wt_entry_exists, Some(false));
        assert!(p.wt_parent_exists);
        let p = parse_probe("wt_entry_exists=1\n");
        assert_eq!(p.wt_entry_exists, Some(true));
        // Missing keys: unknown, not "absent".
        let p = parse_probe("wt_exists=0\n");
        assert_eq!(p.wt_entry_exists, None);
        assert!(!p.wt_parent_exists);
    }

    #[test]
    fn probe_script_for_main_checkout_probes_root_and_no_branch() {
        let script = probe_script(&spec(false));
        assert!(script.contains("wt='/repo'"));
        assert!(script.contains("br=''"));
    }

    #[test]
    fn parse_probe_reads_flags_values_and_porcelain() {
        let out = "root_exists=1\nroot_git=1\nroot_gitdir_ok=1\nlayout_dot_worktrees=0\n\
                   wt_exists=0\nwt_git=0\nwt_empty=0\nwt_gitdir_ok=0\nindex_lock=0\n\
                   branch_local=1\nbranch_remote=0\ndefault_branch=main\n\
                   tmux_alive=1\ntmux_cwd=/repo/x=y\ntmux_cwd_exists=0\n@@worktrees\n\
                   worktree /repo\nHEAD abc\nbranch refs/heads/main\n\n\
                   worktree /repo/.claude/worktrees/feat\nHEAD def\nbranch refs/heads/feat\n\
                   prunable gitdir file points to non-existent location\n\n\
                   worktree /repo/.worktrees/other\nHEAD 123\ndetached\nlocked\n";
        let p = parse_probe(out);
        assert!(p.root_exists && p.root_git && p.root_gitdir_ok);
        assert!(!p.wt_exists);
        assert!(p.branch_local && !p.branch_remote);
        assert_eq!(p.default_branch.as_deref(), Some("main"));
        assert!(p.tmux_alive);
        assert_eq!(p.tmux_cwd.as_deref(), Some("/repo/x=y"));
        assert!(!p.tmux_cwd_exists);
        assert_eq!(p.worktrees.len(), 3);
        assert_eq!(p.worktrees[0].branch.as_deref(), Some("main"));
        assert!(p.worktrees[1].prunable);
        assert_eq!(p.worktrees[1].branch.as_deref(), Some("feat"));
        assert!(p.worktrees[2].locked);
        assert_eq!(p.worktrees[2].branch, None);
    }

    #[test]
    fn parse_probe_of_garbage_is_all_false() {
        let p = parse_probe("");
        assert_eq!(p, Probe::default());
        let p = parse_probe("ssh: connect to host failed");
        assert!(!p.root_exists && p.worktrees.is_empty());
    }

    // ── plan: every inventory case ────────────────────────────────────────

    #[test]
    fn healthy_workspace_is_a_noop_under_both_policies() {
        for policy in [AUTO, Policy::Explicit] {
            let p = make_plan(&spec(true), &healthy(), policy).unwrap();
            assert!(p.is_noop(), "{policy:?}: {:?}", p.steps);
            assert_eq!(p.cwd, "/repo/.claude/worktrees/feat");
            assert!(p.warnings.is_empty());
        }
    }

    #[test]
    fn case_a_dir_deleted_tmux_alive_unregisters_adds_and_respawns() {
        let mut p = healthy();
        p.wt_exists = false;
        p.wt_git = false;
        p.wt_gitdir_ok = false;
        p.tmux_cwd_exists = false;
        p.worktrees[1].prunable = true; // git already flags it
        let plan = make_plan(&spec(true), &p, Policy::Explicit).unwrap();
        assert_eq!(
            plan.steps,
            vec![
                Step::Unregister {
                    path: "/repo/.claude/worktrees/feat".into()
                },
                add_local(),
                Step::TmuxRespawn {
                    cwd: "/repo/.claude/worktrees/feat".into()
                }
            ]
        );
    }

    #[test]
    fn case_b_dir_and_tmux_gone_adds_and_creates_tmux() {
        let mut p = healthy();
        p.wt_exists = false;
        p.wt_git = false;
        p.wt_gitdir_ok = false;
        p.tmux_alive = false;
        p.tmux_dead = true;
        p.tmux_cwd = None;
        p.tmux_cwd_exists = false;
        p.worktrees.truncate(1); // already pruned
        let plan = make_plan(&spec(true), &p, Policy::Explicit).unwrap();
        assert_eq!(
            plan.steps,
            vec![
                add_local(),
                Step::TmuxCreate {
                    cwd: "/repo/.claude/worktrees/feat".into()
                }
            ]
        );
        // Leave: the caller creates tmux. Not registered ⇒ nothing to unregister.
        let plan = make_plan(&spec(true), &p, AUTO).unwrap();
        assert_eq!(plan.steps, vec![add_local()]);
    }

    #[test]
    fn case_c_dir_present_but_unregistered_or_stale_link_is_repaired() {
        // Unregistered: `.git` file present, git no longer lists it.
        let mut p = healthy();
        p.wt_gitdir_ok = false;
        p.worktrees.truncate(1);
        let plan = make_plan(&spec(true), &p, Policy::Explicit).unwrap();
        assert_eq!(
            plan.steps,
            vec![Step::RepairLinks {
                path: "/repo/.claude/worktrees/feat".into()
            }]
        );
        // Registered but the checkout's link is stale.
        let mut p = healthy();
        p.wt_gitdir_ok = false;
        let plan = make_plan(&spec(true), &p, Policy::Explicit).unwrap();
        assert_eq!(
            plan.steps,
            vec![Step::RepairLinks {
                path: "/repo/.claude/worktrees/feat".into()
            }]
        );
        // Re-linking is explicit-only.
        let auto = make_plan(&spec(true), &p, AUTO).unwrap();
        assert!(auto.needs_explicit_repair && auto.steps.is_empty());
    }

    #[test]
    fn case_d_prunable_registration_is_unregistered_before_add() {
        let mut p = healthy();
        p.wt_exists = false;
        p.wt_git = false;
        p.wt_gitdir_ok = false;
        p.worktrees[1].prunable = true;
        let plan = make_plan(&spec(true), &p, Policy::Explicit).unwrap();
        assert_eq!(
            plan.steps,
            vec![
                Step::Unregister {
                    path: "/repo/.claude/worktrees/feat".into()
                },
                add_local()
            ]
        );
        // Automatic without a confirmed vanished-directory guard: reported,
        // not applied (see `each_vanished_guard_blocks_the_automatic_removal`).
        assert!(
            make_plan(&spec(true), &p, AUTO)
                .unwrap()
                .needs_explicit_repair
        );
        // Older git without the `prunable` flag: same fix.
        p.worktrees[1].prunable = false;
        let plan = make_plan(&spec(true), &p, Policy::Explicit).unwrap();
        assert_eq!(plan.steps.len(), 2);
        assert!(matches!(plan.steps[0], Step::Unregister { .. }));
    }

    #[test]
    fn unregister_is_scoped_to_our_own_registration() {
        // Another worktree's stale registration must be left for its owner:
        // no repo-wide prune.
        let mut p = healthy();
        p.wt_exists = false;
        p.wt_git = false;
        p.wt_gitdir_ok = false;
        p.worktrees[1].prunable = true;
        p.worktrees.push(RegisteredWorktree {
            path: "/repo/.worktrees/other".into(),
            branch: Some("other".into()),
            prunable: true,
            ..Default::default()
        });
        let plan = make_plan(&spec(true), &p, Policy::Explicit).unwrap();
        assert_eq!(
            plan.steps,
            vec![
                Step::Unregister {
                    path: "/repo/.claude/worktrees/feat".into()
                },
                add_local()
            ]
        );
        let script = render_git_script("/repo", &plan.steps);
        assert!(
            script.contains("worktree remove --force -- '/repo/.claude/worktrees/feat'"),
            "{script}"
        );
        assert!(!script.contains("prune"), "{script}");
        assert!(!script.contains("other"), "{script}");
    }

    #[test]
    fn create_only_policy_creates_dead_sessions_but_never_respawns() {
        // Live pane in a vanished cwd: left alone (no kill on attach).
        let mut p = healthy();
        p.tmux_cwd_exists = false;
        assert!(make_plan(&spec(true), &p, ATTACH).unwrap().is_noop());
        // Dead session: created.
        let mut p = healthy();
        p.tmux_alive = false;
        p.tmux_dead = true;
        assert_eq!(
            make_plan(&spec(true), &p, ATTACH).unwrap().steps,
            vec![Step::TmuxCreate {
                cwd: "/repo/.claude/worktrees/feat".into()
            }]
        );
    }

    #[test]
    fn case_e_branch_only_on_remote_creates_tracking_branch() {
        let mut p = healthy();
        p.wt_exists = false;
        p.wt_git = false;
        p.wt_gitdir_ok = false;
        p.branch_local = false;
        p.branch_remote = true;
        p.worktrees.truncate(1);
        let plan = make_plan(&spec(true), &p, AUTO).unwrap();
        assert_eq!(
            plan.steps[0],
            Step::AddWorktree {
                path: "/repo/.claude/worktrees/feat".into(),
                branch: "feat".into(),
                from: BranchSource::Remote,
            }
        );
    }

    #[test]
    fn case_f_branch_gone_everywhere_fetches_then_forks_from_base() {
        let mut p = healthy();
        p.wt_exists = false;
        p.wt_git = false;
        p.wt_gitdir_ok = false;
        p.branch_local = false;
        p.branch_remote = false;
        p.worktrees.truncate(1);
        // Default branch from the repo…
        let plan1 = make_plan(&spec(true), &p, Policy::Explicit).unwrap();
        assert_eq!(
            plan1.steps[0],
            Step::AddWorktree {
                path: "/repo/.claude/worktrees/feat".into(),
                branch: "feat".into(),
                from: BranchSource::FetchOrBase {
                    base: "main".into(),
                    default: "main".into(),
                },
            }
        );
        // …or the caller's explicit base, with the default as fallback.
        let mut s = spec(true);
        s.base_branch = Some("dev".into());
        let plan2 = make_plan(&s, &p, Policy::Explicit).unwrap();
        assert!(matches!(
            &plan2.steps[0],
            Step::AddWorktree {
                from: BranchSource::FetchOrBase { base, default },
                ..
            } if base == "dev" && default == "main"
        ));
    }

    #[test]
    fn case_g_branch_in_another_linked_worktree_is_adopted() {
        let mut p = healthy();
        p.wt_exists = false;
        p.wt_git = false;
        p.wt_gitdir_ok = false;
        p.tmux_cwd_exists = false;
        p.worktrees[1].path = "/repo/.worktrees/feat".into(); // other layout
        let plan = make_plan(&spec(true), &p, Policy::Explicit).unwrap();
        assert_eq!(plan.cwd, "/repo/.worktrees/feat");
        assert_eq!(
            plan.steps,
            vec![
                Step::AdoptPath {
                    path: "/repo/.worktrees/feat".into()
                },
                Step::TmuxRespawn {
                    cwd: "/repo/.worktrees/feat".into()
                }
            ]
        );
    }

    #[test]
    fn case_g_branch_in_main_checkout_is_refused() {
        let mut p = healthy();
        p.wt_exists = false;
        p.wt_git = false;
        p.wt_gitdir_ok = false;
        p.worktrees = vec![RegisteredWorktree {
            path: "/repo".into(),
            branch: Some("feat".into()),
            ..Default::default()
        }];
        let err = make_plan(&spec(true), &p, Policy::Explicit).unwrap_err();
        assert_eq!(err.code, codes::E_BRANCH_CHECKED_OUT);
    }

    #[test]
    fn case_h_and_m_missing_repo_is_reported_never_faked() {
        for wt in [true, false] {
            let mut p = healthy();
            p.root_exists = false;
            p.root_git = false;
            p.root_gitdir_ok = false;
            let err = make_plan(&spec(wt), &p, Policy::Explicit).unwrap_err();
            assert_eq!(err.code, codes::E_REPO_MISSING);
            assert!(err.message.contains("never recreated by mkdir"));
            // The resolved root is named so the user can fix the right setting.
            assert!(err.message.contains("/repo on local"), "{}", err.message);
        }
        // Dir exists but is not a git checkout (base path moved elsewhere).
        let mut p = healthy();
        p.root_git = false;
        p.root_gitdir_ok = false;
        assert_eq!(
            make_plan(&spec(false), &p, AUTO).unwrap_err().code,
            codes::E_REPO_MISSING
        );
    }

    #[test]
    fn case_j_tmux_cwd_gone_but_dir_healthy_respawns_only() {
        let mut p = healthy();
        p.tmux_cwd = Some("/repo/.claude/worktrees/feat".into());
        p.tmux_cwd_exists = false;
        let plan = make_plan(&spec(true), &p, Policy::Explicit).unwrap();
        assert_eq!(
            plan.steps,
            vec![Step::TmuxRespawn {
                cwd: "/repo/.claude/worktrees/feat".into()
            }]
        );
        // A pane that merely `cd`ed elsewhere is left alone.
        let mut p = healthy();
        p.tmux_cwd = Some("/repo/.claude/worktrees/feat/src".into());
        assert!(make_plan(&spec(true), &p, Policy::Explicit)
            .unwrap()
            .is_noop());
    }

    #[test]
    fn case_l_partial_add_empty_dir_is_reused_non_empty_is_refused() {
        // Empty leftover dir: git accepts it, so an explicit repair just adds;
        // it is not "absent on disk", so an automatic one only reports it.
        let mut p = healthy();
        p.wt_git = false;
        p.wt_gitdir_ok = false;
        p.wt_empty = true;
        p.worktrees.truncate(1);
        let plan1 = make_plan(&spec(true), &p, Policy::Explicit).unwrap();
        assert_eq!(plan1.steps, vec![add_local()]);
        assert!(
            make_plan(&spec(true), &p, AUTO)
                .unwrap()
                .needs_explicit_repair
        );
        // Non-empty dir without `.git`: refuse, never delete user files.
        p.wt_empty = false;
        let err = make_plan(&spec(true), &p, AUTO).unwrap_err();
        assert_eq!(err.code, codes::E_REPAIR_FAILED);
        assert!(err.message.contains("never deleted"));
    }

    #[test]
    fn case_m_main_checkout_session_is_healthy_when_root_is_a_repo() {
        let plan = make_plan(&spec(false), &healthy(), Policy::Explicit).unwrap();
        assert!(plan.is_noop());
        assert_eq!(plan.cwd, "/repo");
        // …and gets its tmux recreated when that is gone.
        let mut p = healthy();
        p.tmux_alive = false;
        p.tmux_dead = true;
        let plan = make_plan(&spec(false), &p, Policy::Explicit).unwrap();
        assert_eq!(
            plan.steps,
            vec![Step::TmuxCreate {
                cwd: "/repo".into()
            }]
        );
    }

    #[test]
    fn case_o_locked_missing_worktree_is_refused_and_index_lock_is_a_warning() {
        let mut p = healthy();
        p.wt_exists = false;
        p.wt_git = false;
        p.wt_gitdir_ok = false;
        // Real git lists a locked worktree whose dir is gone as `locked`,
        // NOT `prunable` (verified on git 2.54): refused in every policy.
        p.worktrees[1].prunable = false;
        p.worktrees[1].locked = true;
        for policy in [AUTO, ATTACH, Policy::Explicit] {
            assert_eq!(
                make_plan(&spec(true), &p, policy).unwrap_err().code,
                codes::E_WORKSPACE_LOCKED
            );
        }
        let err = make_plan(&spec(true), &p, AUTO).unwrap_err();
        assert_eq!(err.code, codes::E_WORKSPACE_LOCKED);
        assert!(err.message.contains("git worktree unlock"));
        // A healthy worktree with an index.lock is reported, not touched.
        let mut p = healthy();
        p.index_lock = true;
        let plan = make_plan(&spec(true), &p, Policy::Explicit).unwrap();
        assert!(plan.is_noop());
        assert!(plan.warnings.iter().any(|w| w.contains("index.lock")));
    }

    #[test]
    fn branch_drift_is_reported_not_fixed() {
        let mut p = healthy();
        p.worktrees[1].branch = Some("feat-v2".into());
        let plan = make_plan(&spec(true), &p, Policy::Explicit).unwrap();
        assert!(plan.is_noop());
        assert_eq!(plan.branch_drift.as_deref(), Some("feat-v2"));
        assert!(!plan.warnings.is_empty());
    }

    #[test]
    fn guessed_path_prefers_existing_dot_worktrees_layout() {
        let mut s = spec(true);
        s.worktree.as_mut().unwrap().path_is_guess = true;
        let mut p = healthy();
        p.wt_exists = false;
        p.wt_git = false;
        p.wt_gitdir_ok = false;
        p.layout_dot_worktrees = true;
        p.worktrees.truncate(1);
        let plan = make_plan(&s, &p, AUTO).unwrap();
        assert_eq!(plan.cwd, "/repo/.worktrees/feat");
        assert!(
            matches!(&plan.steps[0], Step::AddWorktree { path, .. } if path == "/repo/.worktrees/feat")
        );
    }

    // ── script rendering ─────────────────────────────────────────────────

    #[test]
    fn git_script_renders_steps_in_order_with_quoting() {
        let script = render_git_script(
            "/re po",
            &[
                Step::Unregister {
                    path: "/re po/.claude/worktrees/it's".into(),
                },
                Step::AddWorktree {
                    path: "/re po/.claude/worktrees/it's".into(),
                    branch: "feat".into(),
                    from: BranchSource::Local,
                },
            ],
        );
        assert!(script.starts_with("set -e\n"));
        let unregister = script
            .find("worktree remove --force -- '/re po/.claude/worktrees/it'\\''s'")
            .unwrap_or_else(|| panic!("scoped, quoted unregister: {script}"));
        let add = script.find("worktree add").unwrap();
        assert!(unregister < add, "unregister must precede add: {script}");
        assert!(
            !script.contains("prune"),
            "never a repo-wide prune: {script}"
        );
        assert!(
            script.contains(
                "git -C '/re po' worktree add -- '/re po/.claude/worktrees/it'\\''s' 'feat'"
            ),
            "{script}"
        );
        assert!(script.contains("echo outcome=branch_local"));
    }

    #[test]
    fn git_script_remote_and_fetch_or_base_variants() {
        let remote = render_git_script(
            "/repo",
            &[Step::AddWorktree {
                path: "/repo/w".into(),
                branch: "feat".into(),
                from: BranchSource::Remote,
            }],
        );
        assert!(remote.contains("b='feat'\n"), "{remote}");
        assert!(
            remote.contains("worktree add --track -b \"$b\" -- '/repo/w' \"origin/$b\""),
            "{remote}"
        );
        let fob = render_git_script(
            "/repo",
            &[
                Step::RepairLinks {
                    path: "/repo/w".into(),
                },
                Step::AddWorktree {
                    path: "/repo/w".into(),
                    branch: "feat".into(),
                    from: BranchSource::FetchOrBase {
                        base: "dev".into(),
                        default: "main".into(),
                    },
                },
            ],
        );
        assert!(fob.contains("worktree repair -- '/repo/w'"), "{fob}");
        assert!(fob.contains("ls-remote --exit-code --heads -- origin \"refs/heads/$b\""));
        assert!(fob.contains("fetch -- origin \"+refs/heads/$b:refs/remotes/origin/$b\""));
        assert!(fob.contains("basebr='dev'"));
        assert!(fob.contains("defbr='main'"));
        assert!(fob.contains("else start=HEAD"));
        assert!(fob.contains("worktree add -b \"$b\" -- '/repo/w' \"$start\""));
        assert!(fob.contains("outcome=branch_from_base:$start"));
        // A fetch / ls-remote failure aborts; it never falls through to a fork.
        assert!(fob.contains("exit 1"));
        assert!(!fob.contains("|| true"));
    }

    // ── end-to-end with a scripted executor ───────────────────────────────

    /// Scripted host: `outputs` are consumed by `run_script` in order; every
    /// script and tmux call is recorded so a test can assert the exact
    /// sequence of side effects.
    struct FakeExec {
        outputs: Mutex<VecDeque<Result<ScriptOutput, IpcError>>>,
        scripts: Mutex<Vec<String>>,
        tmux_calls: Mutex<Vec<String>>,
        tmux_fail: bool,
    }

    impl FakeExec {
        fn new(outputs: Vec<Result<ScriptOutput, IpcError>>) -> Self {
            Self {
                outputs: Mutex::new(outputs.into()),
                scripts: Mutex::new(Vec::new()),
                tmux_calls: Mutex::new(Vec::new()),
                tmux_fail: false,
            }
        }
        fn scripts(&self) -> Vec<String> {
            self.scripts.lock().unwrap().clone()
        }
        fn tmux_calls(&self) -> Vec<String> {
            self.tmux_calls.lock().unwrap().clone()
        }
    }

    fn ok(stdout: &str) -> Result<ScriptOutput, IpcError> {
        Ok(ScriptOutput {
            ok: true,
            stdout: stdout.into(),
            stderr: String::new(),
        })
    }

    #[async_trait]
    impl RepairExec for FakeExec {
        async fn run_script(&self, script: &str) -> Result<ScriptOutput, IpcError> {
            self.scripts.lock().unwrap().push(script.to_string());
            self.outputs
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| panic!("unexpected script call: {script}"))
        }
        async fn tmux_new_session(&self, name: &str, cwd: &str, cmd: &str) -> Result<(), IpcError> {
            self.tmux_calls
                .lock()
                .unwrap()
                .push(format!("new {name} -c {cwd} {cmd}"));
            if self.tmux_fail {
                return Err(IpcError::new(codes::E_TMUX, "boom"));
            }
            Ok(())
        }
        async fn tmux_respawn(&self, name: &str, cwd: &str, cmd: &str) -> Result<(), IpcError> {
            self.tmux_calls
                .lock()
                .unwrap()
                .push(format!("respawn {name} -c {cwd} {cmd}"));
            if self.tmux_fail {
                return Err(IpcError::new(codes::E_TMUX, "boom"));
            }
            Ok(())
        }
    }

    const HEALTHY_OUT: &str =
        "root_exists=1\nroot_git=1\nroot_gitdir_ok=1\nwt_exists=1\nwt_git=1\n\
        wt_gitdir_ok=1\nbranch_local=1\ndefault_branch=main\ntmux_alive=1\n\
        tmux_cwd=/repo/.claude/worktrees/feat\ntmux_cwd_exists=1\n@@worktrees\n\
        worktree /repo\nHEAD a\nbranch refs/heads/main\n\n\
        worktree /repo/.claude/worktrees/feat\nHEAD b\nbranch refs/heads/feat\n";

    const DIR_GONE_OUT: &str =
        "root_exists=1\nroot_git=1\nroot_gitdir_ok=1\nwt_exists=0\nwt_git=0\n\
        wt_gitdir_ok=0\nbranch_local=1\ndefault_branch=main\ntmux_alive=1\n\
        tmux_cwd=/repo/.claude/worktrees/feat\ntmux_cwd_exists=0\n@@worktrees\n\
        worktree /repo\nHEAD a\nbranch refs/heads/main\n\n\
        worktree /repo/.claude/worktrees/feat\nHEAD b\nbranch refs/heads/feat\nprunable gone\n";

    #[tokio::test]
    async fn repair_remote_paths_follow_projects_base_path() {
        use crate::service::settings;
        use crate::ssh_fake::FakeSsh;
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("vps").unwrap();
        let pid = s.upsert_project("o", "r", "/local/o/r").unwrap();
        let sid = s
            .upsert_session("dev-x", "vps", Some(pid), None, 1, 1, "running", None)
            .unwrap();
        s.set_worktree_key(sid, Some("feat")).unwrap();
        let fake = FakeSsh::new();
        fake.with_home("/home/me");
        // Mirrors `spec_for_session`: seed under the lock, resolve off-lock.
        async fn resolve(s: &Store, sid: i64, fake: &FakeSsh) -> (String, String) {
            let row = s.get_session_by_id(sid).unwrap().unwrap();
            let seed = seed_for_session(s, &row).unwrap();
            let wt = seed.worktree.as_ref().map(|(n, _, _)| n.as_str());
            resolve_remote_paths(
                fake,
                &row.host_alias,
                &seed.projects_root,
                seed.layout,
                &seed.owner,
                &seed.repo,
                wt,
            )
            .await
            .unwrap()
        }

        // No setting: exactly the pre-setting convention.
        let (root, cwd) = resolve(&s, sid, &fake).await;
        assert_eq!(root, "/home/me/projects/github.com/o/r");
        assert_eq!(
            cwd,
            "/home/me/projects/github.com/o/r/.claude/worktrees/feat"
        );

        // The host's `projects.base_path` entry + flat layout.
        settings::set(&s, settings::PROJECTS_BASE_PATH, r#"{"vps":"~/code"}"#).unwrap();
        settings::set(&s, settings::PROJECTS_LAYOUT, "flat").unwrap();
        let (root, cwd) = resolve(&s, sid, &fake).await;
        assert_eq!(root, "/home/me/code/r");
        assert_eq!(cwd, "/home/me/code/r/.claude/worktrees/feat");

        // An absolute root under the github layout.
        settings::set(&s, settings::PROJECTS_BASE_PATH, r#"{"vps":"/srv/git"}"#).unwrap();
        settings::set(&s, settings::PROJECTS_LAYOUT, "github").unwrap();
        let (root, _) = resolve(&s, sid, &fake).await;
        assert_eq!(root, "/srv/git/o/r");
    }

    /// Store with one project + worktree row + running session; returns
    /// `(store, session_id, project_id)`.
    fn seeded_store(wt_path: &str) -> (Mutex<Store>, i64, i64) {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let pid = s.upsert_project("o", "r", "/repo").unwrap();
        let wid = s
            .upsert_worktree(pid, "feat", wt_path, Some("feat"))
            .unwrap();
        let sid = s
            .upsert_session(
                "dev-x",
                "local",
                Some(pid),
                Some(wid),
                1,
                1,
                "running",
                None,
            )
            .unwrap();
        s.set_worktree_key(sid, Some("feat")).unwrap();
        (Mutex::new(s), sid, pid)
    }

    fn spec_with_ids(sid: i64, pid: i64) -> WorkspaceSpec {
        let mut s = spec(true);
        s.session_id = Some(sid);
        s.project_id = Some(pid);
        s
    }

    #[tokio::test]
    async fn healthy_workspace_runs_one_probe_and_writes_nothing() {
        let (store, sid, pid) = seeded_store("/repo/.claude/worktrees/feat");
        let exec = FakeExec::new(vec![ok(HEALTHY_OUT)]);
        let rep = ensure_workspace(
            &spec_with_ids(sid, pid),
            Policy::Explicit,
            vec![],
            &store,
            &exec,
        )
        .await
        .unwrap();
        assert!(rep.healthy);
        assert!(rep.actions.is_empty());
        assert_eq!(rep.cwd, "/repo/.claude/worktrees/feat");
        assert_eq!(rep.project_root, "/repo", "resolved root is reported");
        assert_eq!(exec.scripts().len(), 1, "exactly one probe");
        assert!(exec.tmux_calls().is_empty());
        let s = store.lock().unwrap();
        assert!(s.list_session_events(sid, 10).unwrap().is_empty());
        assert!(!rep.worktree_row_updated);
    }

    #[tokio::test]
    async fn dir_deleted_end_to_end_prunes_adds_verifies_respawns_and_records() {
        let (store, sid, pid) = seeded_store("/repo/.claude/worktrees/feat");
        let exec = FakeExec::new(vec![
            ok(DIR_GONE_OUT),             // probe
            ok("outcome=branch_local\n"), // git steps
            ok(HEALTHY_OUT),              // verify
        ]);
        let rep = ensure_workspace(
            &spec_with_ids(sid, pid),
            Policy::Explicit,
            vec![7],
            &store,
            &exec,
        )
        .await
        .unwrap();
        assert!(!rep.healthy);
        let scripts = exec.scripts();
        assert_eq!(scripts.len(), 3);
        assert!(scripts[0].contains("worktree list --porcelain"));
        // Exact apply script: this entry only, then the add.
        assert_eq!(
            scripts[1],
            render_git_script(
                "/repo",
                &[
                    Step::Unregister {
                        path: "/repo/.claude/worktrees/feat".into()
                    },
                    add_local()
                ]
            )
        );
        assert!(scripts[2].contains("worktree list --porcelain"));
        assert_eq!(
            exec.tmux_calls(),
            vec!["respawn dev-x -c /repo/.claude/worktrees/feat cl"]
        );
        assert_eq!(rep.tmux.as_deref(), Some("respawned"));
        assert_eq!(rep.branch_source.as_deref(), Some("branch_local"));
        assert_eq!(rep.sibling_session_ids, vec![7]);
        assert_eq!(rep.actions.len(), 3);
        let s = store.lock().unwrap();
        let ev = s.list_session_events(sid, 10).unwrap();
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].kind, EVENT_REPAIRED);
        assert!(ev[0]
            .detail
            .as_deref()
            .unwrap()
            .contains("worktree remove --force"));
        let row = s.get_session_by_id(sid).unwrap().unwrap();
        assert_eq!(row.status, "running");
        assert!(row.worktree_id.is_some());
    }

    #[tokio::test]
    async fn tmux_gone_is_created_and_row_restored_from_ghost() {
        let (store, sid, pid) = seeded_store("/repo/.claude/worktrees/feat");
        store
            .lock()
            .unwrap()
            .conn_ref()
            .execute(
                "UPDATE sessions SET status='ghost', lost_at=5 WHERE id=?1",
                rusqlite::params![sid],
            )
            .unwrap();
        let out = HEALTHY_OUT.replace("tmux_alive=1", "tmux_alive=0\ntmux_dead=1");
        let exec = FakeExec::new(vec![ok(&out)]);
        let rep = ensure_workspace(
            &spec_with_ids(sid, pid),
            Policy::Explicit,
            vec![],
            &store,
            &exec,
        )
        .await
        .unwrap();
        assert_eq!(rep.tmux.as_deref(), Some("created"));
        assert_eq!(
            exec.tmux_calls(),
            vec!["new dev-x -c /repo/.claude/worktrees/feat cl"]
        );
        let s = store.lock().unwrap();
        let row = s.get_session_by_id(sid).unwrap().unwrap();
        assert_eq!(row.status, "running");
        assert!(row.lost_at.is_none());
    }

    #[tokio::test]
    async fn leave_policy_reports_stale_tmux_instead_of_touching_it() {
        let (store, sid, pid) = seeded_store("/repo/.claude/worktrees/feat");
        let exec = FakeExec::new(vec![
            ok(DIR_GONE_OUT),
            ok("outcome=branch_local\n"),
            ok(HEALTHY_OUT),
        ]);
        let rep = ensure_workspace(&spec_with_ids(sid, pid), AUTO, vec![], &store, &exec)
            .await
            .unwrap();
        // A stale registration is explicit-only: nothing applied, tmux untouched.
        assert_eq!(exec.scripts().len(), 1, "probe only");
        assert!(rep.needs_explicit_repair);
        assert!(exec.tmux_calls().is_empty());
        assert!(rep.tmux.is_none());
        assert!(rep.tmux_alive);
        assert!(rep.tmux_cwd_stale, "the pane's reported cwd is missing");
    }

    #[tokio::test]
    async fn adopting_another_path_rewrites_the_local_worktree_row() {
        let (store, sid, pid) = seeded_store("/repo/.claude/worktrees/feat");
        // The pane's cwd is REPORTED and missing, so the explicit repair may
        // respawn it (an unreported cwd would be "unknown": no respawn).
        let out = "root_exists=1\nroot_git=1\nroot_gitdir_ok=1\nwt_exists=0\nbranch_local=1\n\
                   tmux_alive=1\ntmux_cwd=/repo/.claude/worktrees/feat\ntmux_cwd_exists=0\n\
                   @@worktrees\nworktree /repo\nbranch refs/heads/main\n\n\
                   worktree /repo/.worktrees/feat\nbranch refs/heads/feat\n";
        // Probe, then the verify probe of the adopted checkout.
        let exec = FakeExec::new(vec![ok(out), ok(ADOPTED_HEALTHY_OUT)]);
        let rep = ensure_workspace(
            &spec_with_ids(sid, pid),
            Policy::Explicit,
            vec![],
            &store,
            &exec,
        )
        .await
        .unwrap();
        assert_eq!(rep.cwd, "/repo/.worktrees/feat");
        assert!(rep.worktree_row_updated);
        assert_eq!(
            exec.tmux_calls(),
            vec!["respawn dev-x -c /repo/.worktrees/feat cl"]
        );
        let s = store.lock().unwrap();
        let wt = s
            .list_worktrees_for_project(pid)
            .unwrap()
            .into_iter()
            .find(|w| w.name == "feat")
            .unwrap();
        assert_eq!(wt.path, "/repo/.worktrees/feat");
        assert_eq!(wt.branch.as_deref(), Some("feat"));
    }

    #[tokio::test]
    async fn remote_repair_never_writes_a_remote_path_into_the_local_row() {
        let (store, sid, pid) = seeded_store("/Users/me/repo/.claude/worktrees/feat");
        let mut s = spec_with_ids(sid, pid);
        s.host_alias = "mefistos".into();
        s.project_root = "/home/me/repo".into();
        let w = s.worktree.as_mut().unwrap();
        w.path = "/home/me/repo/.claude/worktrees/feat".into();
        w.row_is_local = false;
        let out = HEALTHY_OUT
            .replace("/repo/", "/home/me/repo/")
            .replace("worktree /repo\n", "worktree /home/me/repo\n");
        let exec = FakeExec::new(vec![ok(&out)]);
        let rep = ensure_workspace(&s, Policy::Explicit, vec![], &store, &exec)
            .await
            .unwrap();
        assert!(rep.healthy);
        let st = store.lock().unwrap();
        let wt = st
            .get_worktree_row(
                st.get_session_by_id(sid)
                    .unwrap()
                    .unwrap()
                    .worktree_id
                    .unwrap(),
            )
            .unwrap()
            .unwrap();
        assert_eq!(wt.path, "/Users/me/repo/.claude/worktrees/feat");
    }

    #[tokio::test]
    async fn worktree_key_only_session_gets_linked_and_row_created() {
        // Reconciled sessions carry only worktree_key; a repair creates the
        // worktrees row and sets the FK so later cwd resolution is direct.
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let pid = s.upsert_project("o", "r", "/repo").unwrap();
        let sid = s
            .upsert_session("dev-x", "local", Some(pid), None, 1, 1, "running", None)
            .unwrap();
        s.set_worktree_key(sid, Some("feat")).unwrap();
        let store = Mutex::new(s);
        let exec = FakeExec::new(vec![ok(HEALTHY_OUT)]);
        let rep = ensure_workspace(
            &spec_with_ids(sid, pid),
            Policy::Explicit,
            vec![],
            &store,
            &exec,
        )
        .await
        .unwrap();
        assert!(
            rep.healthy,
            "linking rows is bookkeeping, not a repair action"
        );
        assert!(rep.worktree_row_updated);
        let st = store.lock().unwrap();
        let row = st.get_session_by_id(sid).unwrap().unwrap();
        let wid = row.worktree_id.expect("linked");
        assert_eq!(
            st.worktree_path(wid).unwrap().as_deref(),
            Some("/repo/.claude/worktrees/feat")
        );
    }

    /// Probe of a workspace whose branch is checked out at another linked
    /// worktree (`/repo/.worktrees/feat`) — the adopt case.
    const ADOPT_OUT: &str =
        "root_exists=1\nroot_git=1\nroot_gitdir_ok=1\nwt_exists=0\nbranch_local=1\n\
        tmux_alive=1\ntmux_cwd_exists=0\n@@worktrees\nworktree /repo\nbranch refs/heads/main\n\n\
        worktree /repo/.worktrees/feat\nbranch refs/heads/feat\n";
    /// Verify probe of the adopted checkout: present and healthy.
    const ADOPTED_HEALTHY_OUT: &str = "root_exists=1\nroot_git=1\nroot_gitdir_ok=1\nwt_exists=1\n\
        wt_git=1\nwt_gitdir_ok=1\nbranch_local=1\ntmux_alive=1\ntmux_cwd_exists=0\n@@worktrees\n\
        worktree /repo\nbranch refs/heads/main\n\n\
        worktree /repo/.worktrees/feat\nbranch refs/heads/feat\n";

    #[tokio::test]
    async fn adoption_is_verified_before_tmux_moves_there() {
        // Older git does not print `prunable`, so the checkout "elsewhere"
        // may be a missing directory. The verify probe catches it before any
        // respawn and before the row is re-pathed.
        let (store, sid, pid) = seeded_store("/repo/.claude/worktrees/feat");
        let gone = ADOPTED_HEALTHY_OUT.replace(
            "wt_exists=1\nwt_git=1\nwt_gitdir_ok=1",
            "wt_exists=0\nwt_git=0\nwt_gitdir_ok=0",
        );
        let exec = FakeExec::new(vec![ok(ADOPT_OUT), ok(&gone)]);
        let err = ensure_workspace(
            &spec_with_ids(sid, pid),
            Policy::Explicit,
            vec![],
            &store,
            &exec,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_REPAIR_FAILED);
        assert_eq!(exec.scripts().len(), 2, "probe + verify, no git steps");
        assert!(exec.tmux_calls().is_empty());
        let s = store.lock().unwrap();
        let wt = s
            .list_worktrees_for_project(pid)
            .unwrap()
            .into_iter()
            .find(|w| w.name == "feat")
            .unwrap();
        assert_eq!(wt.path, "/repo/.claude/worktrees/feat", "row not re-pathed");
    }

    #[tokio::test]
    async fn adoption_is_refused_when_another_worktree_row_owns_the_branch() {
        let (store, sid, pid) = seeded_store("/repo/.claude/worktrees/feat");
        store
            .lock()
            .unwrap()
            .upsert_worktree(pid, "feat-old", "/repo/.worktrees/feat", Some("feat"))
            .unwrap();
        let exec = FakeExec::new(vec![ok(ADOPT_OUT)]);
        let err = ensure_workspace(
            &spec_with_ids(sid, pid),
            Policy::Explicit,
            vec![],
            &store,
            &exec,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_BRANCH_CHECKED_OUT);
        assert!(err.message.contains("feat-old"), "{}", err.message);
        assert_eq!(exec.scripts().len(), 1, "refused before any change");
        assert!(exec.tmux_calls().is_empty());
        let s = store.lock().unwrap();
        assert!(s.get_session_by_id(sid).unwrap().is_some());
        assert_eq!(
            s.list_session_events(sid, 10).unwrap()[0].kind,
            EVENT_REPAIR_FAILED
        );
    }

    #[tokio::test]
    async fn create_only_policy_repairs_the_dir_but_never_kills_a_live_pane() {
        // Attach path: dir deleted under a live Claude pane while git still
        // lists it. Nothing is unregistered or respawned automatically; the
        // report says the pane is stale and an explicit repair is needed.
        let (store, sid, pid) = seeded_store("/repo/.claude/worktrees/feat");
        let exec = FakeExec::new(vec![ok(DIR_GONE_OUT)]);
        let rep = ensure_workspace(&spec_with_ids(sid, pid), ATTACH, vec![], &store, &exec)
            .await
            .unwrap();
        assert_eq!(exec.scripts().len(), 1, "probe only");
        assert!(rep.needs_explicit_repair);
        assert!(
            exec.tmux_calls().is_empty(),
            "live pane must not be respawned"
        );
        assert!(rep.tmux.is_none());
        assert!(rep.tmux_alive && rep.tmux_cwd_stale);
        // A dead session is still created.
        let out = HEALTHY_OUT.replace("tmux_alive=1", "tmux_alive=0\ntmux_dead=1");
        let exec = FakeExec::new(vec![ok(&out)]);
        let rep = ensure_workspace(&spec_with_ids(sid, pid), ATTACH, vec![], &store, &exec)
            .await
            .unwrap();
        assert_eq!(rep.tmux.as_deref(), Some("created"));
    }

    #[tokio::test]
    async fn host_unreachable_fails_clearly_and_changes_nothing() {
        let (store, sid, pid) = seeded_store("/repo/.claude/worktrees/feat");
        let before = store
            .lock()
            .unwrap()
            .get_session_by_id(sid)
            .unwrap()
            .unwrap();
        let exec = FakeExec::new(vec![Err(IpcError::new(codes::E_SSH_TIMEOUT, "ssh hung"))]);
        let err = ensure_workspace(
            &spec_with_ids(sid, pid),
            Policy::Explicit,
            vec![],
            &store,
            &exec,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_HOST_OFFLINE);
        assert!(err.message.contains("nothing was changed"));
        assert!(exec.tmux_calls().is_empty());
        let s = store.lock().unwrap();
        assert_eq!(s.get_session_by_id(sid).unwrap().unwrap(), before);
        assert!(s.list_session_events(sid, 10).unwrap().is_empty());
    }

    #[tokio::test]
    async fn git_step_failure_reports_e_repair_failed_and_keeps_the_row() {
        // (p) disk full / permissions: git fails once; we report and stop.
        let (store, sid, pid) = seeded_store("/repo/.claude/worktrees/feat");
        let exec = FakeExec::new(vec![
            ok(DIR_GONE_OUT),
            Ok(ScriptOutput {
                ok: false,
                stdout: String::new(),
                stderr: "fatal: could not create work tree dir: No space left on device".into(),
            }),
        ]);
        let err = ensure_workspace(
            &spec_with_ids(sid, pid),
            Policy::Explicit,
            vec![],
            &store,
            &exec,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_REPAIR_FAILED);
        assert!(err.message.contains("No space left"));
        assert_eq!(exec.scripts().len(), 2, "no retry loop");
        assert!(exec.tmux_calls().is_empty());
        let s = store.lock().unwrap();
        let row = s.get_session_by_id(sid).unwrap().expect("row survives");
        assert_eq!(row.status, "running", "never ghosted by a failed repair");
        let ev = s.list_session_events(sid, 10).unwrap();
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].kind, EVENT_REPAIR_FAILED);
    }

    #[tokio::test]
    async fn unverified_repair_is_a_failure() {
        let (store, sid, pid) = seeded_store("/repo/.claude/worktrees/feat");
        // git "succeeds" but the verify probe still sees nothing there.
        let exec = FakeExec::new(vec![ok(DIR_GONE_OUT), ok(""), ok(DIR_GONE_OUT)]);
        let err = ensure_workspace(
            &spec_with_ids(sid, pid),
            Policy::Explicit,
            vec![],
            &store,
            &exec,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_REPAIR_FAILED);
        assert!(err.message.contains("still not a healthy worktree"));
        assert!(
            exec.tmux_calls().is_empty(),
            "tmux is not touched on an unverified dir"
        );
    }

    #[tokio::test]
    async fn refusals_record_a_failed_event_and_keep_the_row() {
        let (store, sid, pid) = seeded_store("/repo/.claude/worktrees/feat");
        let out = "root_exists=0\nroot_git=0\nroot_gitdir_ok=0\n@@worktrees\n";
        let exec = FakeExec::new(vec![ok(out)]);
        let err = ensure_workspace(
            &spec_with_ids(sid, pid),
            Policy::Explicit,
            vec![],
            &store,
            &exec,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_REPO_MISSING);
        let s = store.lock().unwrap();
        assert!(s.get_session_by_id(sid).unwrap().is_some());
        let ev = s.list_session_events(sid, 10).unwrap();
        assert_eq!(ev[0].kind, EVENT_REPAIR_FAILED);
        assert!(ev[0]
            .detail
            .as_deref()
            .unwrap()
            .starts_with("E_REPO_MISSING"));
    }

    #[tokio::test]
    async fn tmux_failure_after_a_good_dir_surfaces_e_tmux_without_deleting() {
        let (store, sid, pid) = seeded_store("/repo/.claude/worktrees/feat");
        let out = HEALTHY_OUT.replace("tmux_alive=1", "tmux_alive=0\ntmux_dead=1");
        let mut exec = FakeExec::new(vec![ok(&out)]);
        exec.tmux_fail = true;
        let err = ensure_workspace(
            &spec_with_ids(sid, pid),
            Policy::Explicit,
            vec![],
            &store,
            &exec,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_TMUX);
        let s = store.lock().unwrap();
        assert!(s.get_session_by_id(sid).unwrap().is_some());
    }

    #[tokio::test]
    async fn new_session_spec_without_ids_records_no_events() {
        // `new_session` calls with session_id = None: the automatic create
        // runs (dir absent, entry gone, branch exists), nothing is attached to
        // a row (the row does not exist yet).
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let gone = DIR_GONE_OUT.replace(
            "worktree /repo/.claude/worktrees/feat\nHEAD b\nbranch refs/heads/feat\nprunable gone\n",
            "",
        );
        let exec = FakeExec::new(vec![
            ok(&gone),
            ok("outcome=branch_local\n"),
            ok(HEALTHY_OUT),
        ]);
        let rep = ensure_workspace(&spec(true), AUTO, vec![], &store, &exec)
            .await
            .unwrap();
        assert!(!rep.needs_explicit_repair);
        assert_eq!(rep.actions.len(), 1);
        assert_eq!(rep.cwd, "/repo/.claude/worktrees/feat");
    }

    // ── spec_for_session ──────────────────────────────────────────────────

    #[tokio::test]
    async fn spec_for_local_session_uses_rows_and_finds_siblings() {
        let (store, sid, pid) = seeded_store("/repo/.worktrees/feat");
        {
            let s = store.lock().unwrap();
            // A review sharing the worktree (same key) and an unrelated one.
            let rid = s
                .upsert_session(
                    "dev-x--review-1",
                    "local",
                    Some(pid),
                    None,
                    1,
                    1,
                    "running",
                    None,
                )
                .unwrap();
            s.set_worktree_key(rid, Some("feat")).unwrap();
            let oid = s
                .upsert_session("other", "local", Some(pid), None, 1, 1, "running", None)
                .unwrap();
            s.set_worktree_key(oid, Some("main")).unwrap();
        }
        let ssh = Arc::new(SshClient::new());
        let (spec, siblings) = spec_for_session(&store, &ssh, sid).await.unwrap();
        assert_eq!(spec.project_root, "/repo");
        let w = spec.worktree.unwrap();
        assert_eq!(w.path, "/repo/.worktrees/feat");
        assert_eq!(w.branch, "feat");
        assert!(!w.path_is_guess);
        assert!(w.row_is_local);
        assert_eq!(siblings.len(), 1);
        assert_eq!(spec.session_id, Some(sid));
    }

    #[tokio::test]
    async fn spec_for_key_only_session_guesses_the_path() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let pid = s.upsert_project("o", "r", "/repo").unwrap();
        let sid = s
            .upsert_session("dev-x", "local", Some(pid), None, 1, 1, "running", None)
            .unwrap();
        s.set_worktree_key(sid, Some("feat")).unwrap();
        let store = Mutex::new(s);
        let ssh = Arc::new(SshClient::new());
        let (spec, _) = spec_for_session(&store, &ssh, sid).await.unwrap();
        let w = spec.worktree.unwrap();
        assert_eq!(w.path, "/repo/.claude/worktrees/feat");
        assert!(w.path_is_guess);
        // "main" key → main checkout, no worktree spec.
        store
            .lock()
            .unwrap()
            .set_worktree_key(sid, Some("main"))
            .unwrap();
        let (spec, _) = spec_for_session(&store, &ssh, sid).await.unwrap();
        assert!(spec.worktree.is_none());
    }

    #[tokio::test]
    async fn spec_for_session_refuses_orphans_bg_and_cross_project_rows() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let orphan = s
            .upsert_session("orphan", "local", None, None, 1, 1, "running", None)
            .unwrap();
        s.upsert_bg_session("local", "bg:abc", None, "abc", None, 1)
            .unwrap();
        let bg = s.get_session("bg:abc", "local").unwrap().unwrap().id;
        let pa = s.upsert_project("o", "a", "/a").unwrap();
        let pb = s.upsert_project("o", "b", "/b").unwrap();
        let wid_b = s
            .upsert_worktree(pb, "feat", "/b/.worktrees/feat", None)
            .unwrap();
        let cross = s
            .upsert_session(
                "cross",
                "local",
                Some(pa),
                Some(wid_b),
                1,
                1,
                "running",
                None,
            )
            .unwrap();
        let store = Mutex::new(s);
        let ssh = Arc::new(SshClient::new());
        assert_eq!(
            spec_for_session(&store, &ssh, orphan)
                .await
                .unwrap_err()
                .code,
            codes::E_NOREPO
        );
        assert_eq!(
            spec_for_session(&store, &ssh, bg).await.unwrap_err().code,
            codes::E_BG_SESSION
        );
        assert_eq!(
            spec_for_session(&store, &ssh, cross)
                .await
                .unwrap_err()
                .code,
            codes::E_INVALID_STATE
        );
        assert_eq!(
            spec_for_session(&store, &ssh, 999).await.unwrap_err().code,
            codes::E_NOTFOUND
        );
    }

    #[test]
    fn spec_for_new_session_uses_row_locally_and_convention_remotely() {
        let s = Store::open_in_memory().unwrap();
        let pid = s.upsert_project("o", "r", "/repo").unwrap();
        let wid = s
            .upsert_worktree(pid, "feat", "/repo/.worktrees/feat", Some("feat-branch"))
            .unwrap();
        let main_id = s
            .upsert_worktree(pid, "main", "/repo", Some("main"))
            .unwrap();
        let w = NewSessionWorkspace {
            host_alias: "local",
            project_id: pid,
            worktree_id: Some(wid),
            tmux_name: "dev-x",
            pane_cmd: "cl",
            cwd: "/repo/.worktrees/feat",
            base_branch: Some("dev"),
        };
        let spec = spec_for_new_session(&s, &w, None).unwrap();
        assert_eq!(spec.project_root, "/repo");
        let wt = spec.worktree.unwrap();
        assert_eq!(wt.branch, "feat-branch");
        assert!(wt.row_is_local && !wt.path_is_guess);
        assert_eq!(spec.base_branch.as_deref(), Some("dev"));
        assert_eq!(spec.project_id, Some(pid));
        assert!(spec.session_id.is_none());
        // The "main" row is the main checkout, not a linked worktree.
        let w_main = NewSessionWorkspace {
            worktree_id: Some(main_id),
            cwd: "/repo",
            ..w
        };
        assert!(spec_for_new_session(&s, &w_main, None)
            .unwrap()
            .worktree
            .is_none());
        // Remote: root from the convention, path only guessed, row untouched.
        let w_remote = NewSessionWorkspace {
            host_alias: "mefistos",
            cwd: "/home/me/projects/github.com/o/r/.claude/worktrees/feat",
            ..w
        };
        let spec = spec_for_new_session(
            &s,
            &w_remote,
            Some("/home/me/projects/github.com/o/r".into()),
        )
        .unwrap();
        assert_eq!(spec.project_root, "/home/me/projects/github.com/o/r");
        let wt = spec.worktree.unwrap();
        assert!(wt.path_is_guess && !wt.row_is_local);
        assert_eq!(wt.path, w_remote.cwd);
    }

    #[tokio::test]
    async fn repair_session_refuses_offline_host_before_probing() {
        let (store, sid, _) = seeded_store("/repo/.worktrees/feat");
        store
            .lock()
            .unwrap()
            .conn_ref()
            .execute("UPDATE hosts SET reachable=0 WHERE alias='local'", [])
            .unwrap();
        let ssh = Arc::new(SshClient::new());
        let err = repair_session(sid, true, &store, &ssh).await.unwrap_err();
        assert_eq!(err.code, codes::E_HOST_OFFLINE);
    }

    // ── automatic vs explicit (review round 2) ────────────────────────────

    #[test]
    fn policy_for_each_entry_point() {
        let auto = Policy::Auto {
            create_dead_tmux: false,
        };
        assert_eq!(policy_for(Entry::NewSession), auto);
        assert_eq!(policy_for(Entry::SpawnReview), auto);
        assert_eq!(policy_for(Entry::Restart), auto);
        assert_eq!(policy_for(Entry::Recreate), auto);
        assert_eq!(
            policy_for(Entry::Attach),
            Policy::Auto {
                create_dead_tmux: true
            }
        );
        assert_eq!(policy_for(Entry::Explicit), Policy::Explicit);
        // Every automatic entry point (click-driven or the tick) drops our own
        // vanished registration ONLY with a matching parent fingerprint; with
        // none recorded or a mismatch it stays explicit-only.
        for entry in [
            Entry::NewSession,
            Entry::SpawnReview,
            Entry::Restart,
            Entry::Recreate,
            Entry::Attach,
        ] {
            for (ctx, why) in [
                (NO_FP, "no parent fingerprint was recorded"),
                (OTHER_FP, "differs from the one recorded"),
            ] {
                let p = plan_with(&spec(true), &vanished(), policy_for(entry), ctx).unwrap();
                assert!(
                    p.needs_explicit_repair && p.steps.is_empty(),
                    "{entry:?}: {:?}",
                    p.steps
                );
                assert!(
                    matches!(p.deferred.first(), Some(Step::Unregister { .. })),
                    "{entry:?}: {:?}",
                    p.deferred
                );
                assert!(
                    p.warnings.iter().any(|w| w.contains(why)),
                    "{entry:?}: {:?}",
                    p.warnings
                );
            }
            let t = plan_with(&spec(true), &vanished(), policy_for(entry), NO_OTHERS).unwrap();
            assert!(!t.needs_explicit_repair, "{entry:?}: {:?}", t.warnings);
            assert!(
                matches!(t.steps.first(), Some(Step::Unregister { .. })),
                "{entry:?}: {:?}",
                t.steps
            );
        }
        // Explicit is unchanged by the fingerprint and the flag.
        for ctx in [NO_FP, OTHER_FP, NO_OTHERS, AutoContext::default()] {
            let p = plan_with(&spec(true), &vanished(), Policy::Explicit, ctx).unwrap();
            assert_eq!(
                &p.steps[..2],
                &[
                    Step::Unregister {
                        path: VANISHED_WT.into()
                    },
                    add_local()
                ]
            );
        }
        // Without the store context (plain `plan`) no automatic entry point may.
        for entry in [Entry::NewSession, Entry::Restart, Entry::Attach] {
            let p = make_plan(&spec(true), &vanished(), policy_for(entry)).unwrap();
            assert!(p.needs_explicit_repair && p.steps.is_empty(), "{entry:?}");
        }
    }

    /// Wiring: each lifecycle call site passes its own entry point and starts
    /// tmux in the probe-resolved directory; only the explicit surfaces reach
    /// `Policy::Explicit`.
    #[test]
    fn call_sites_use_their_documented_entry_points() {
        let sessions = include_str!("sessions.rs");
        for needle in [
            "Entry::Restart",
            "Entry::Recreate",
            "Entry::SpawnReview",
            "repair::ensure_for_new_session(",
        ] {
            assert!(sessions.contains(needle), "sessions.rs must use {needle}");
        }
        assert!(
            !sessions.contains("Entry::Explicit"),
            "lifecycles are never explicit"
        );
        assert!(!sessions.contains("Entry::Attach"));
        // recreate + spawn_review use the resolved cwd; restart respawns into it.
        assert!(sessions.contains("Ok(rep) => rep.cwd"));
        assert!(sessions.contains("std::path::Path::new(&rep.cwd)"));
        let commands = include_str!("../commands/sessions.rs");
        assert!(commands.contains("repair::repair_session(args.session_id, args.explicit"));
        let tools = include_str!("../mcp/tools.rs");
        assert!(tools.contains("repair::repair_session(id, true"));
        assert!(tools.contains("\"repair_session\",\n            p.confirm_nonce.as_deref(),"));
        // Only the opt-in tick may drop a stale entry automatically.
        for (name, src) in [
            ("sessions.rs", sessions),
            ("commands/sessions.rs", commands),
            ("mcp/tools.rs", tools),
        ] {
            assert!(
                !src.contains("for_tick"),
                "{name} must not use the tick path"
            );
            assert!(!src.contains("ensure_workspace_with("), "{name}");
        }
        assert!(
            include_str!("repair_tick.rs").contains("repair::ensure_session_workspace_for_tick(")
        );
    }

    #[test]
    fn auto_policy_never_applies_explicit_only_steps() {
        // (a)/(d) stale registration without a confirmed vanished-directory
        // guard (this probe reports no canonical path / absence): automatic
        // runs report, never unregister.
        let mut p = healthy();
        p.wt_exists = false;
        p.wt_git = false;
        p.wt_gitdir_ok = false;
        p.tmux_cwd_exists = false;
        p.worktrees[1].prunable = true;
        for policy in [AUTO, ATTACH] {
            let plan = make_plan(&spec(true), &p, policy).unwrap();
            assert!(plan.steps.is_empty(), "{policy:?}: {:?}", plan.steps);
            assert!(plan.needs_explicit_repair);
            assert_eq!(
                plan.deferred[0],
                Step::Unregister {
                    path: "/repo/.claude/worktrees/feat".into()
                }
            );
            assert!(plan
                .deferred
                .iter()
                .any(|s| matches!(s, Step::TmuxRespawn { .. })));
            assert_eq!(plan.cwd, "/repo/.claude/worktrees/feat");
        }
        // (f) branch gone everywhere: recreating it is explicit-only.
        let mut p = healthy();
        p.wt_exists = false;
        p.wt_git = false;
        p.wt_gitdir_ok = false;
        p.branch_local = false;
        p.branch_remote = false;
        p.worktrees.truncate(1);
        let plan = make_plan(&spec(true), &p, AUTO).unwrap();
        assert!(plan.needs_explicit_repair && plan.steps.is_empty());
        // (g) branch checked out in another linked worktree: ambiguous.
        let mut p = healthy();
        p.wt_exists = false;
        p.wt_git = false;
        p.wt_gitdir_ok = false;
        p.worktrees[1].path = "/repo/.worktrees/feat".into();
        let plan = make_plan(&spec(true), &p, ATTACH).unwrap();
        assert!(plan.needs_explicit_repair && plan.steps.is_empty());
        assert!(matches!(&plan.deferred[0], Step::AdoptPath { .. }));
        // (l) empty leftover dir: not "absent on disk".
        let mut p = healthy();
        p.wt_git = false;
        p.wt_gitdir_ok = false;
        p.wt_empty = true;
        p.worktrees.truncate(1);
        assert!(
            make_plan(&spec(true), &p, AUTO)
                .unwrap()
                .needs_explicit_repair
        );
        // (c) stale link: re-linking is explicit-only.
        let mut p = healthy();
        p.wt_gitdir_ok = false;
        let plan = make_plan(&spec(true), &p, AUTO).unwrap();
        assert!(plan.needs_explicit_repair && plan.steps.is_empty());
    }

    #[test]
    fn auto_policy_creates_only_what_is_confirmed_missing() {
        // Absent on disk, unregistered, branch exists: automatic add.
        let mut p = healthy();
        p.wt_exists = false;
        p.wt_git = false;
        p.wt_gitdir_ok = false;
        p.tmux_alive = false;
        p.tmux_dead = true;
        p.tmux_cwd = None;
        p.worktrees.truncate(1);
        let plan = make_plan(&spec(true), &p, ATTACH).unwrap();
        assert!(!plan.needs_explicit_repair);
        assert_eq!(
            plan.steps,
            vec![
                add_local(),
                Step::TmuxCreate {
                    cwd: "/repo/.claude/worktrees/feat".into()
                }
            ]
        );
        // Callers that own tmux (restart / recreate / new session): the add only.
        assert_eq!(
            make_plan(&spec(true), &p, AUTO).unwrap().steps,
            vec![add_local()]
        );
        // tmux state unknown: no create, a warning.
        p.tmux_dead = false;
        let plan = make_plan(&spec(true), &p, ATTACH).unwrap();
        assert_eq!(plan.steps, vec![add_local()]);
        assert!(plan
            .warnings
            .iter()
            .any(|w| w.contains("tmux state unknown")));
    }

    #[test]
    fn empty_pane_cwd_is_unknown_and_never_respawns() {
        let mut p = healthy();
        p.tmux_cwd = None;
        p.tmux_cwd_exists = false;
        for policy in [AUTO, ATTACH, Policy::Explicit] {
            let plan = make_plan(&spec(true), &p, policy).unwrap();
            assert!(plan.is_noop(), "{policy:?}: {:?}", plan.steps);
            assert!(plan.deferred.is_empty());
            assert!(plan
                .warnings
                .iter()
                .any(|w| w.contains("could not read the pane")));
        }
        // A live pane on attach is never respawned, even when its cwd is gone.
        let mut p = healthy();
        p.tmux_cwd_exists = false;
        let plan = make_plan(&spec(true), &p, ATTACH).unwrap();
        assert!(plan.is_noop());
        assert!(
            !plan.needs_explicit_repair,
            "a stale pane does not block the dir"
        );
        assert_eq!(
            plan.deferred,
            vec![Step::TmuxRespawn {
                cwd: "/repo/.claude/worktrees/feat".into()
            }]
        );
    }

    #[test]
    fn parse_probe_reads_tmux_state_and_resolved_path() {
        let p = parse_probe("wt_path=/r/.worktrees/x\ntmux_alive=0\ntmux_dead=1\n@@worktrees\n");
        assert_eq!(p.wt_path.as_deref(), Some("/r/.worktrees/x"));
        assert!(!p.tmux_alive && p.tmux_dead);
        let p = parse_probe("tmux_alive=1\ntmux_dead=0\ntmux_cwd=\n");
        assert!(p.tmux_alive && p.tmux_cwd.is_none());
    }

    #[test]
    fn probe_script_confirms_tmux_death_and_resolves_both_layouts() {
        let mut s = spec(true);
        s.worktree.as_mut().unwrap().path_is_guess = true;
        let script = probe_script(&s);
        assert!(script.contains("guess=1"));
        assert!(script.contains("name='feat'"));
        assert!(
            script.contains(
                "for cand in \"$root/.worktrees/$name\" \"$root/.claude/worktrees/$name\"; do"
            ),
            "{script}"
        );
        assert!(script.contains("echo \"wt_path=$wt\""));
        assert!(script.contains("echo tmux_dead=1"));
        assert!(script.contains("no server running|error connecting to"));
        assert!(script.contains("if [ -n \"$cwd\" ]; then echo \"tmux_cwd_exists="));
        // Row-backed paths are never re-guessed.
        assert!(probe_script(&spec(true)).contains("guess=0"));
    }

    #[test]
    fn dot_worktrees_layout_on_a_guessed_path_resolves_and_is_healthy() {
        // mefistos: remote worktrees live under `.worktrees/<name>`; the spec
        // guessed `.claude/worktrees/<name>`; the probe resolved the real dir.
        let mut s = spec(true);
        s.worktree.as_mut().unwrap().path_is_guess = true;
        let mut p = healthy();
        p.wt_path = Some("/repo/.worktrees/feat".into());
        p.wt_canon = Some("/repo/.worktrees/feat".into());
        p.worktrees[1].path = "/repo/.worktrees/feat".into();
        p.tmux_cwd = Some("/repo/.worktrees/feat".into());
        for policy in [AUTO, ATTACH, Policy::Explicit] {
            let plan = make_plan(&s, &p, policy).unwrap();
            assert!(plan.is_noop(), "{policy:?}: {:?}", plan.steps);
            assert!(!plan.needs_explicit_repair, "{:?}", plan.warnings);
            assert_eq!(plan.cwd, "/repo/.worktrees/feat", "resolved, not the guess");
        }
    }

    #[test]
    fn mixed_logical_and_physical_path_forms_are_healthy() {
        // mefistos: rows and PWD use /home/me/projects/…, while git and
        // `pwd -P` say /mnt/sda4/projects/…; one worktree is even listed in
        // its logical form (it was added from a logical cwd).
        let mut s = spec(true);
        s.project_root = "/home/me/projects/o/r".into();
        s.worktree.as_mut().unwrap().path = "/home/me/projects/o/r/.worktrees/feat".into();
        let mut p = healthy();
        p.root_canon = Some("/mnt/sda4/projects/o/r".into());
        p.wt_canon = Some("/mnt/sda4/projects/o/r/.worktrees/feat".into());
        p.tmux_cwd = Some("/mnt/sda4/projects/o/r/.worktrees/feat".into());
        p.worktrees = vec![
            RegisteredWorktree {
                path: "/mnt/sda4/projects/o/r".into(),
                canon: Some("/mnt/sda4/projects/o/r".into()),
                branch: Some("main".into()),
                ..Default::default()
            },
            RegisteredWorktree {
                path: "/home/me/projects/o/r/.worktrees/feat".into(),
                canon: Some("/mnt/sda4/projects/o/r/.worktrees/feat".into()),
                branch: Some("feat".into()),
                ..Default::default()
            },
        ];
        for policy in [AUTO, Policy::Explicit] {
            let plan = make_plan(&s, &p, policy).unwrap();
            assert!(plan.is_noop(), "{policy:?}: {:?}", plan.steps);
            assert!(plan.warnings.is_empty(), "{:?}", plan.warnings);
            assert_eq!(plan.cwd, "/home/me/projects/o/r/.worktrees/feat");
        }
        // P1: the branch is in the MAIN checkout, reported physically while
        // the root is logical — recognised as the main checkout, never adopted.
        p.wt_exists = false;
        p.wt_git = false;
        p.wt_gitdir_ok = false;
        p.worktrees = vec![RegisteredWorktree {
            path: "/mnt/sda4/projects/o/r".into(),
            canon: Some("/mnt/sda4/projects/o/r".into()),
            branch: Some("feat".into()),
            ..Default::default()
        }];
        for policy in [AUTO, ATTACH, Policy::Explicit] {
            assert_eq!(
                make_plan(&s, &p, policy).unwrap_err().code,
                codes::E_BRANCH_CHECKED_OUT
            );
        }
    }

    #[test]
    fn git_script_is_exact_for_each_step_kind() {
        let steps = vec![
            Step::Unregister {
                path: "/r/.claude/worktrees/f".into(),
            },
            Step::AddWorktree {
                path: "/r/.claude/worktrees/f".into(),
                branch: "f".into(),
                from: BranchSource::Local,
            },
        ];
        assert_eq!(
            render_git_script("/r", &steps),
            "set -e\n\
             p='/r/.claude/worktrees/f'\n\
             if [ -L \"$p\" ] || [ ! -d \"$(dirname -- \"$p\")\" ] || { [ -e \"$p\" ] && [ -n \"$(ls -A -- \"$p\" 2>/dev/null || echo x)\" ]; }; then echo \"repair: $p reappeared or parent missing; not removing\" >&2; exit 1; fi\n\
             git -C '/r' worktree remove --force -- '/r/.claude/worktrees/f' 1>&2\n\
             git -C '/r' worktree add -- '/r/.claude/worktrees/f' 'f' 1>&2\n\
             echo outcome=branch_local\n"
        );
        assert_eq!(
            render_git_script(
                "/r",
                &[Step::RepairLinks {
                    path: "/r/w".into()
                }]
            ),
            "set -e\ngit -C '/r' worktree repair -- '/r/w' 1>&2\n"
        );
        assert_eq!(
            render_git_script(
                "/r",
                &[Step::AddWorktree {
                    path: "/r/w".into(),
                    branch: "f".into(),
                    from: BranchSource::Remote,
                }]
            ),
            "set -e\nb='f'\ngit -C '/r' worktree add --track -b \"$b\" -- '/r/w' \"origin/$b\" 1>&2\necho outcome=branch_remote\n"
        );
    }

    #[test]
    fn branch_recreation_asks_origin_and_aborts_on_errors() {
        let script = render_git_script(
            "/r",
            &[Step::AddWorktree {
                path: "/r/w".into(),
                branch: "f".into(),
                from: BranchSource::FetchOrBase {
                    base: "dev".into(),
                    default: "main".into(),
                },
            }],
        );
        for needle in [
            "b='f'\n",
            "if git -C '/r' remote get-url origin >/dev/null 2>&1; then\n",
            "  lr=0; git -C '/r' ls-remote --exit-code --heads -- origin \"refs/heads/$b\" >/dev/null 2>&1 || lr=$?\n",
            "  lr=2\n",
            "  if ! git -C '/r' fetch -- origin \"+refs/heads/$b:refs/remotes/origin/$b\" 1>&2; then\n",
            "  git -C '/r' worktree add --track -b \"$b\" -- '/r/w' \"origin/$b\" 1>&2\n",
            "  basebr='dev'\n",
            "  defbr='main'\n",
            "  git -C '/r' worktree add -b \"$b\" -- '/r/w' \"$start\" 1>&2\n",
            "  echo \"outcome=branch_from_base:$start\"\n",
            "*)\n  echo \"repair: cannot confirm whether origin still has $b",
        ] {
            assert!(script.contains(needle), "missing {needle:?} in:\n{script}");
        }
        // A fetch failure is fatal, never a silent fall-through to a fork.
        assert!(!script.contains("|| true"), "{script}");
    }

    #[tokio::test]
    async fn auto_add_runs_the_exact_script_then_verifies() {
        // (b) automatic: dir absent, unregistered, branch exists locally.
        let (store, sid, pid) = seeded_store("/repo/.claude/worktrees/feat");
        let gone = "root_exists=1\nroot_git=1\nroot_gitdir_ok=1\nwt_exists=0\nbranch_local=1\n\
            default_branch=main\ntmux_alive=1\ntmux_dead=0\ntmux_cwd=/repo\ntmux_cwd_exists=1\n\
            @@worktrees\nworktree /repo\nHEAD a\nbranch refs/heads/main\n";
        let exec = FakeExec::new(vec![
            ok(gone),
            ok("outcome=branch_local\n"),
            ok(HEALTHY_OUT),
        ]);
        let rep = ensure_workspace(&spec_with_ids(sid, pid), AUTO, vec![], &store, &exec)
            .await
            .unwrap();
        let scripts = exec.scripts();
        assert_eq!(scripts.len(), 3);
        assert_eq!(scripts[1], render_git_script("/repo", &[add_local()]));
        assert!(
            scripts[2].contains("worktree list --porcelain"),
            "verify probe"
        );
        assert!(exec.tmux_calls().is_empty(), "AUTO never touches tmux");
        assert_eq!(rep.branch_source.as_deref(), Some("branch_local"));
        assert!(!rep.needs_explicit_repair);
        let s = store.lock().unwrap();
        let ev = s.list_session_events(sid, 10).unwrap();
        assert_eq!(ev[0].kind, EVENT_REPAIRED);
        assert!(ev[0]
            .detail
            .as_deref()
            .unwrap()
            .contains("\"branch_source\":\"branch_local\""));
    }

    #[tokio::test]
    async fn auto_run_needing_explicit_steps_applies_nothing_and_lifecycles_refuse() {
        let (store, sid, pid) = seeded_store("/repo/.claude/worktrees/feat");
        let exec = FakeExec::new(vec![ok(DIR_GONE_OUT)]);
        let rep = ensure_workspace(&spec_with_ids(sid, pid), ATTACH, vec![], &store, &exec)
            .await
            .unwrap();
        assert_eq!(exec.scripts().len(), 1, "probe only");
        assert!(
            exec.tmux_calls().is_empty(),
            "attach never respawns a live pane"
        );
        assert!(rep.needs_explicit_repair && !rep.healthy);
        assert!(
            rep.deferred
                .iter()
                .any(|d| d.contains("worktree remove --force --")),
            "{:?}",
            rep.deferred
        );
        assert!(store
            .lock()
            .unwrap()
            .list_session_events(sid, 10)
            .unwrap()
            .is_empty());
        let err = require_no_explicit(rep).unwrap_err();
        assert_eq!(err.code, codes::E_REPAIR_REQUIRED);
        assert!(err.message.contains("Repair workspace"), "{}", err.message);
    }

    #[tokio::test]
    async fn fetch_or_ls_remote_failure_is_e_repair_failed() {
        let (store, sid, pid) = seeded_store("/repo/.claude/worktrees/feat");
        let gone = "root_exists=1\nroot_git=1\nroot_gitdir_ok=1\nwt_exists=0\nbranch_local=0\n\
            branch_remote=0\ndefault_branch=main\ntmux_alive=1\ntmux_dead=0\ntmux_cwd=/repo\n\
            tmux_cwd_exists=1\n@@worktrees\nworktree /repo\nbranch refs/heads/main\n";
        let exec = FakeExec::new(vec![
            ok(gone),
            Ok(ScriptOutput {
                ok: false,
                stdout: String::new(),
                stderr: "repair: cannot confirm whether origin still has feat (git ls-remote exit 128); not recreating it".into(),
            }),
        ]);
        let err = ensure_workspace(
            &spec_with_ids(sid, pid),
            Policy::Explicit,
            vec![],
            &store,
            &exec,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_REPAIR_FAILED);
        assert!(err.message.contains("cannot confirm whether origin"));
        assert_eq!(exec.scripts().len(), 2, "no verify, no retry");
        assert!(exec.tmux_calls().is_empty());
    }

    #[tokio::test]
    async fn interrupted_apply_says_it_may_be_partially_applied() {
        let (store, sid, pid) = seeded_store("/repo/.claude/worktrees/feat");
        let exec = FakeExec::new(vec![
            ok(DIR_GONE_OUT),
            Err(IpcError::new(codes::E_SSH_TIMEOUT, "wall clock")),
        ]);
        let err = ensure_workspace(
            &spec_with_ids(sid, pid),
            Policy::Explicit,
            vec![],
            &store,
            &exec,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_REPAIR_FAILED);
        assert!(err.message.contains("partially applied"), "{}", err.message);
        assert!(!err.message.contains("nothing was changed"));
        // A probe-time failure, by contrast, changed nothing.
        let exec = FakeExec::new(vec![Err(IpcError::new(codes::E_SSH_TIMEOUT, "wall clock"))]);
        let err = ensure_workspace(
            &spec_with_ids(sid, pid),
            Policy::Explicit,
            vec![],
            &store,
            &exec,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_HOST_OFFLINE);
    }

    #[tokio::test]
    async fn adoption_is_refused_when_another_session_uses_that_checkout() {
        let (store, sid, pid) = seeded_store("/repo/.claude/worktrees/feat");
        {
            let s = store.lock().unwrap();
            // A not-yet-dismissed ghost counts too.
            let other = s
                .upsert_session("other", "local", Some(pid), None, 1, 1, "ghost", None)
                .unwrap();
            s.set_worktree_key(other, Some("feat-moved")).unwrap();
        }
        let out = ADOPT_OUT.replace("/repo/.worktrees/feat", "/repo/.worktrees/feat-moved");
        let exec = FakeExec::new(vec![ok(&out)]);
        let err = ensure_workspace(
            &spec_with_ids(sid, pid),
            Policy::Explicit,
            vec![],
            &store,
            &exec,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_BRANCH_CHECKED_OUT);
        assert!(err.message.contains("session other"), "{}", err.message);
        assert_eq!(exec.scripts().len(), 1, "refused before any change");
        assert!(exec.tmux_calls().is_empty());
    }

    // ── HostExec over FakeSsh: the real transport, scripted ──────────────

    #[test]
    fn remote_wall_clocks_match_the_local_bounds() {
        assert_eq!(
            SshClient::default_wall_clock(PROBE_CONNECT_TIMEOUT),
            PROBE_WALL_CLOCK
        );
        assert_eq!(
            SshClient::default_wall_clock(APPLY_CONNECT_TIMEOUT),
            APPLY_WALL_CLOCK
        );
        assert!(APPLY_WALL_CLOCK >= Duration::from_secs(120));
    }

    /// A remote spec: paths under the host's home, layout only guessed.
    fn remote_spec() -> WorkspaceSpec {
        let mut s = spec(true);
        s.host_alias = "mefistos".into();
        s.project_root = "/home/me/projects/github.com/o/r".into();
        let w = s.worktree.as_mut().unwrap();
        w.path = "/home/me/projects/github.com/o/r/.claude/worktrees/feat".into();
        w.path_is_guess = true;
        w.row_is_local = false;
        s
    }

    /// Probe fixtures re-rooted under the remote project.
    fn remote_out(out: &str) -> String {
        out.replace("/repo", "/home/me/projects/github.com/o/r")
    }

    /// A `HostExec` whose ssh AND tmux both go through `fake`.
    fn fake_host_exec(fake: &Arc<crate::ssh_fake::FakeSsh>) -> HostExec<'_> {
        let tmux = Box::new(crate::tmux::RemoteTmux {
            client: Arc::clone(fake),
            host: "mefistos".to_string(),
        });
        HostExec::with("mefistos", &**fake, tmux)
    }

    #[tokio::test]
    async fn host_exec_probe_goes_over_ssh_as_one_quoted_script() {
        use crate::ssh_fake::{FakeSsh, Match, Reply};
        let fake = Arc::new(FakeSsh::new());
        fake.on_host(
            "mefistos",
            Match::script_contains("worktree list --porcelain"),
            Reply::ok(&remote_out(HEALTHY_OUT)),
        );
        let s = remote_spec();
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let exec = fake_host_exec(&fake);
        let rep = ensure_workspace(&s, Policy::Explicit, vec![], &store, &exec)
            .await
            .unwrap();
        assert!(rep.healthy, "{:?} {:?}", rep.actions, rep.warnings);
        let calls = fake.calls_for("mefistos");
        assert_eq!(
            calls.len(),
            1,
            "one probe, nothing else: {:?}",
            fake.commands()
        );
        assert_eq!(calls[0].args[0], "bash");
        assert_eq!(calls[0].args[1], "-lc");
        assert_eq!(
            calls[0].script().as_deref(),
            Some(probe_script(&s).as_str()),
            "the whole probe crosses ssh as one quoted word"
        );
    }

    #[tokio::test]
    async fn host_exec_explicit_repair_sends_probe_apply_verify_then_respawn() {
        use crate::ssh_fake::{FakeSsh, Match, Reply};
        let fake = Arc::new(FakeSsh::new());
        let root = "/home/me/projects/github.com/o/r";
        let wt = format!("{root}/.claude/worktrees/feat");
        // The first probe re-guesses the layout (guess=1); the verify probe
        // checks exactly the planned dir (guess=0).
        fake.on_host(
            "mefistos",
            Match::script_contains("guess=1"),
            Reply::ok(&remote_out(DIR_GONE_OUT)),
        );
        fake.on_host(
            "mefistos",
            Match::script_contains("guess=0"),
            Reply::ok(&remote_out(HEALTHY_OUT)),
        );
        fake.on_host(
            "mefistos",
            Match::script_contains("worktree remove --force"),
            Reply::ok("outcome=branch_local\n"),
        );
        let s = remote_spec();
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let exec = fake_host_exec(&fake);
        let rep = ensure_workspace(&s, Policy::Explicit, vec![], &store, &exec)
            .await
            .unwrap();
        assert_eq!(rep.cwd, wt);
        assert_eq!(rep.tmux.as_deref(), Some("respawned"));
        let scripts: Vec<String> = fake
            .calls_for("mefistos")
            .iter()
            .map(|c| c.script().unwrap_or_default())
            .collect();
        assert_eq!(
            scripts.len(),
            4,
            "probe, apply, verify, respawn: {scripts:?}"
        );
        assert!(scripts[0].contains("guess=1"));
        assert_eq!(
            scripts[1],
            render_git_script(
                root,
                &[
                    Step::Unregister { path: wt.clone() },
                    Step::AddWorktree {
                        path: wt.clone(),
                        branch: "feat".into(),
                        from: BranchSource::Local,
                    },
                ]
            )
        );
        assert!(scripts[2].contains("guess=0"));
        assert!(
            scripts[3].contains(&format!("respawn-pane -k -c {}", quote(&wt))),
            "respawned INTO the repaired dir: {}",
            scripts[3]
        );
    }

    #[tokio::test]
    async fn host_exec_unreachable_host_is_e_host_offline_and_changes_nothing() {
        use crate::ssh_fake::FakeSsh;
        let fake = Arc::new(FakeSsh::new());
        fake.unreachable("mefistos");
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let exec = fake_host_exec(&fake);
        let err = ensure_workspace(&remote_spec(), Policy::Explicit, vec![], &store, &exec)
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::E_HOST_OFFLINE, "{}", err.message);
        assert!(err.message.contains("nothing was changed"));
        assert_eq!(fake.calls_for("mefistos").len(), 1, "probe only");
    }

    #[tokio::test]
    async fn host_exec_interrupted_apply_may_be_partially_applied() {
        use crate::ssh_fake::{FakeSsh, Match, Reply};
        for apply_reply in [Reply::hang(), Reply::Unreachable] {
            let fake = Arc::new(FakeSsh::new());
            fake.set_wall_clock(Duration::from_millis(50));
            fake.on_host(
                "mefistos",
                Match::script_contains("guess=1"),
                Reply::ok(&remote_out(DIR_GONE_OUT)),
            );
            fake.on_host(
                "mefistos",
                Match::script_contains("worktree remove --force"),
                apply_reply.clone(),
            );
            let store = Mutex::new(Store::open_in_memory().unwrap());
            let exec = fake_host_exec(&fake);
            let err = ensure_workspace(&remote_spec(), Policy::Explicit, vec![], &store, &exec)
                .await
                .unwrap_err();
            assert_eq!(err.code, codes::E_REPAIR_FAILED, "{apply_reply:?}");
            assert!(
                err.message.contains("partially applied"),
                "{apply_reply:?}: {}",
                err.message
            );
            assert_eq!(
                fake.calls_for("mefistos").len(),
                2,
                "no verify, no tmux after a lost apply"
            );
        }
    }

    // ── symlinked roots (macOS /var → /private/var, ~/projects → /mnt/…) ──

    #[test]
    fn probe_script_canonicalizes_root_worktree_and_registrations() {
        let script = probe_script(&spec(true));
        assert!(script.contains("canon() {"), "{script}");
        assert!(script.contains("pwd -P"), "{script}");
        assert!(script.contains("echo \"root_canon=$(canon \"$root\")\""));
        assert!(script.contains("echo \"wt_canon=$(canon \"$wt\")\""));
        assert!(script.contains("echo \"canon $(canon \"${l#worktree }\")\""));
    }

    #[test]
    fn parse_probe_reads_canonical_paths() {
        let out = "root_canon=/private/var/r\nwt_canon=/private/var/r/w\n@@worktrees\n\
                   worktree /private/var/r\ncanon /private/var/r\nbranch refs/heads/main\n\n\
                   worktree /private/var/r/w\ncanon /private/var/r/w\nbranch refs/heads/w\n";
        let p = parse_probe(out);
        assert_eq!(p.root_canon.as_deref(), Some("/private/var/r"));
        assert_eq!(p.wt_canon.as_deref(), Some("/private/var/r/w"));
        assert_eq!(p.worktrees[1].key(), "/private/var/r/w");
        // No canon line → git's own path is the key.
        let p = parse_probe("@@worktrees\nworktree /a\n");
        assert_eq!(p.worktrees[0].key(), "/a");
    }

    /// `healthy()`, but the host resolves `/repo` to `/real/repo` and git
    /// prints the realpaths.
    fn healthy_under_symlink() -> Probe {
        let mut p = healthy();
        p.root_canon = Some("/real/repo".into());
        p.wt_canon = Some("/real/repo/.claude/worktrees/feat".into());
        for w in p.worktrees.iter_mut() {
            let real = w.path.replacen("/repo", "/real/repo", 1);
            w.path = real.clone();
            w.canon = Some(real);
        }
        p
    }

    #[test]
    fn symlinked_root_healthy_is_noop_and_deleted_dir_prunes_and_adds() {
        // Healthy: canonical-to-canonical match, nothing to do. (Comparing
        // raw paths would have "adopted" our own registration elsewhere.)
        let p = healthy_under_symlink();
        let plan = make_plan(&spec(true), &p, Policy::Explicit).unwrap();
        assert!(plan.is_noop(), "{:?}", plan.steps);
        assert_eq!(
            plan.cwd, "/repo/.claude/worktrees/feat",
            "user-facing path kept"
        );
        // Deleted dir: the prunable registration is still recognised as ours.
        let mut p = healthy_under_symlink();
        p.wt_exists = false;
        p.wt_git = false;
        p.wt_gitdir_ok = false;
        p.tmux_cwd_exists = false;
        p.worktrees[1].prunable = true;
        let plan = make_plan(&spec(true), &p, Policy::Explicit).unwrap();
        assert_eq!(
            plan.steps,
            vec![
                // git's own (realpath) form, so `worktree remove` matches it.
                Step::Unregister {
                    path: "/real/repo/.claude/worktrees/feat".into()
                },
                add_local(),
                Step::TmuxRespawn {
                    cwd: "/repo/.claude/worktrees/feat".into()
                }
            ]
        );
    }

    #[test]
    fn symlinked_root_main_checkout_guard_still_refuses() {
        // Branch checked out in the main checkout, reported via its realpath:
        // must be recognised as the main checkout (refuse), never adopted.
        let mut p = healthy_under_symlink();
        p.wt_exists = false;
        p.wt_git = false;
        p.wt_gitdir_ok = false;
        p.worktrees = vec![RegisteredWorktree {
            path: "/real/repo".into(),
            canon: Some("/real/repo".into()),
            branch: Some("feat".into()),
            ..Default::default()
        }];
        let err = make_plan(&spec(true), &p, Policy::Explicit).unwrap_err();
        assert_eq!(err.code, codes::E_BRANCH_CHECKED_OUT);
    }

    #[tokio::test]
    async fn host_reporting_private_var_for_a_var_row_is_healthy() {
        // macOS: /var -> /private/var. Row and spec say /var/…; `pwd -P` and
        // git say /private/var/…. Judged healthy: no prune, add or respawn,
        // and the row keeps its user-facing path.
        let wt_row = "/var/folders/x/repo/.claude/worktrees/feat";
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let pid = s.upsert_project("o", "r", "/var/folders/x/repo").unwrap();
        let wid = s
            .upsert_worktree(pid, "feat", wt_row, Some("feat"))
            .unwrap();
        let sid = s
            .upsert_session(
                "dev-x",
                "local",
                Some(pid),
                Some(wid),
                1,
                1,
                "running",
                None,
            )
            .unwrap();
        let store = Mutex::new(s);
        let mut sp = spec_with_ids(sid, pid);
        sp.project_root = "/var/folders/x/repo".into();
        sp.worktree.as_mut().unwrap().path = wt_row.into();
        let out = "root_exists=1\nroot_git=1\nroot_gitdir_ok=1\n\
            root_canon=/private/var/folders/x/repo\n\
            wt_canon=/private/var/folders/x/repo/.claude/worktrees/feat\n\
            wt_exists=1\nwt_git=1\nwt_gitdir_ok=1\nbranch_local=1\ndefault_branch=main\n\
            tmux_alive=1\ntmux_cwd=/private/var/folders/x/repo/.claude/worktrees/feat\n\
            tmux_cwd_exists=1\n@@worktrees\n\
            worktree /private/var/folders/x/repo\ncanon /private/var/folders/x/repo\n\
            HEAD a\nbranch refs/heads/main\n\n\
            worktree /private/var/folders/x/repo/.claude/worktrees/feat\n\
            canon /private/var/folders/x/repo/.claude/worktrees/feat\n\
            HEAD b\nbranch refs/heads/feat\n";
        let exec = FakeExec::new(vec![ok(out)]);
        let rep = ensure_workspace(&sp, Policy::Explicit, vec![], &store, &exec)
            .await
            .unwrap();
        assert!(rep.healthy, "{:?}", rep.actions);
        assert_eq!(exec.scripts().len(), 1, "one probe, no git steps");
        assert!(exec.tmux_calls().is_empty(), "no respawn");
        assert_eq!(rep.cwd, wt_row, "user-facing path stays the row's");
        assert!(!rep.worktree_row_updated);
        let st = store.lock().unwrap();
        assert_eq!(st.worktree_path(wid).unwrap().as_deref(), Some(wt_row));
        assert!(st.list_session_events(sid, 10).unwrap().is_empty());
    }

    // ── against a real git repo (local) ───────────────────────────────────

    /// Runs scripts with the real local `bash`; tmux must never be touched
    /// (callers use `AUTO`).
    struct LocalExec;

    #[async_trait]
    impl RepairExec for LocalExec {
        async fn run_script(&self, script: &str) -> Result<ScriptOutput, IpcError> {
            let out = tokio::process::Command::new("bash")
                .args(["-c", script])
                .output()
                .await
                .unwrap();
            Ok(ScriptOutput {
                ok: out.status.success(),
                stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            })
        }
        // Explicit real-git runs may create a confirmed-dead session; the
        // unique test session names never collide with a real one, and no
        // real tmux call is made from here.
        async fn tmux_new_session(&self, _: &str, _: &str, _: &str) -> Result<(), IpcError> {
            Ok(())
        }
        async fn tmux_respawn(&self, _: &str, _: &str, _: &str) -> Result<(), IpcError> {
            Ok(())
        }
    }

    fn git_in(dir: &std::path::Path, args: &[&str]) {
        let st = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .status()
            .unwrap();
        assert!(st.success(), "git {args:?}");
    }

    /// `git init` + one commit on `main` at `root`.
    fn init_repo(root: &std::path::Path) {
        std::fs::create_dir_all(root).unwrap();
        let st = std::process::Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .arg(root)
            .status()
            .unwrap();
        assert!(st.success(), "git init");
        git_in(root, &["config", "user.email", "t@t"]);
        git_in(root, &["config", "user.name", "t"]);
        std::fs::write(root.join("f"), "x").unwrap();
        git_in(root, &["add", "."]);
        git_in(root, &["commit", "-q", "-m", "init"]);
    }

    fn local_spec(root: &std::path::Path, wt: &std::path::Path, tag: &str) -> WorkspaceSpec {
        let mut s = spec(true);
        s.project_root = root.to_str().unwrap().to_string();
        s.tmux_name = format!("cf-repair-{tag}-{}", std::process::id());
        s.worktree.as_mut().unwrap().path = wt.to_str().unwrap().to_string();
        s
    }

    /// The owner's layout: the project lives under a symlinked directory
    /// (`~/projects` → `/mnt/…`), and on macOS every tempdir does too. A
    /// healthy worktree must be a no-op; a deleted one must be re-added (not
    /// adopted, not refused) and then be a no-op again.
    #[cfg(unix)]
    #[tokio::test]
    async fn real_git_under_a_symlinked_root_is_noop_when_healthy_and_repairs_when_deleted() {
        let base = tempfile::TempDir::new().unwrap();
        let real = base.path().join("real");
        std::fs::create_dir_all(&real).unwrap();
        let link = base.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let root = link.join("repo");
        init_repo(&root);
        let wt = root.join(".claude/worktrees/feat");
        git_in(
            &root,
            &["worktree", "add", "-q", wt.to_str().unwrap(), "-b", "feat"],
        );
        let s = local_spec(&root, &wt, "sym");
        let store = Mutex::new(Store::open_in_memory().unwrap());

        let rep = ensure_workspace(&s, AUTO, vec![], &store, &LocalExec)
            .await
            .unwrap();
        assert!(
            rep.healthy,
            "healthy worktree under a symlinked root must be a no-op: {:?}",
            rep.actions
        );
        assert_eq!(rep.cwd, wt.to_str().unwrap(), "user-facing path kept");

        std::fs::remove_dir_all(&wt).unwrap();
        // Automatic: git still lists the entry, so this is reported only.
        let rep = ensure_workspace(&s, AUTO, vec![], &store, &LocalExec)
            .await
            .unwrap();
        assert!(rep.needs_explicit_repair, "{:?}", rep.warnings);
        assert!(!wt.exists(), "nothing applied automatically");
        // Explicit: unregister this entry, re-add, verify (canonically).
        let rep = ensure_workspace(&s, Policy::Explicit, vec![], &store, &LocalExec)
            .await
            .unwrap();
        assert!(!rep.healthy);
        assert!(
            rep.actions.iter().any(|a| a.contains("worktree add")),
            "{:?}",
            rep.actions
        );
        assert!(
            !rep.actions.iter().any(|a| a.starts_with("adopt")),
            "must not adopt its own registration: {:?}",
            rep.actions
        );
        assert_eq!(rep.cwd, wt.to_str().unwrap());
        assert!(wt.join(".git").exists(), "worktree is back");

        let rep = ensure_workspace(&s, AUTO, vec![], &store, &LocalExec)
            .await
            .unwrap();
        assert!(rep.healthy, "{:?}", rep.actions);
    }

    /// P1 against real git, under a symlinked root: the branch is checked out
    /// in the MAIN checkout. Refused in both policies; the row is untouched
    /// and nothing is created.
    #[cfg(unix)]
    #[tokio::test]
    async fn real_git_branch_in_main_checkout_under_symlink_is_refused_and_row_untouched() {
        let base = tempfile::TempDir::new().unwrap();
        let real = base.path().join("real");
        std::fs::create_dir_all(&real).unwrap();
        let link = base.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let root = link.join("repo");
        init_repo(&root);
        git_in(&root, &["checkout", "-q", "-b", "feat"]);
        let wt = root.join(".claude/worktrees/feat");
        let wt_s = wt.to_str().unwrap().to_string();
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let pid = s.upsert_project("o", "r", root.to_str().unwrap()).unwrap();
        let wid = s.upsert_worktree(pid, "feat", &wt_s, Some("feat")).unwrap();
        let sid = s
            .upsert_session(
                "dev-x",
                "local",
                Some(pid),
                Some(wid),
                1,
                1,
                "running",
                None,
            )
            .unwrap();
        let store = Mutex::new(s);
        let mut sp = local_spec(&root, &wt, "p1");
        sp.session_id = Some(sid);
        sp.project_id = Some(pid);
        for policy in [AUTO, Policy::Explicit] {
            let err = ensure_workspace(&sp, policy, vec![], &store, &LocalExec)
                .await
                .unwrap_err();
            assert_eq!(
                err.code,
                codes::E_BRANCH_CHECKED_OUT,
                "{policy:?}: {}",
                err.message
            );
        }
        let st = store.lock().unwrap();
        assert_eq!(
            st.worktree_path(wid).unwrap().as_deref(),
            Some(wt_s.as_str()),
            "row untouched"
        );
        assert!(!wt.exists(), "nothing created");
    }

    /// `.worktrees` layout against real git: a key-only session whose path is
    /// only guessed (`.claude/worktrees/<name>`) resolves to the real
    /// `.worktrees/<name>` checkout — a no-op, never the missing guess.
    #[tokio::test]
    async fn real_git_guessed_path_resolves_the_dot_worktrees_layout() {
        let base = tempfile::TempDir::new().unwrap();
        let root = base.path().join("repo");
        init_repo(&root);
        let real_wt = root.join(".worktrees/feat");
        git_in(
            &root,
            &[
                "worktree",
                "add",
                "-q",
                real_wt.to_str().unwrap(),
                "-b",
                "feat",
            ],
        );
        let mut s = local_spec(&root, &root.join(".claude/worktrees/feat"), "layout");
        s.worktree.as_mut().unwrap().path_is_guess = true;
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let rep = ensure_workspace(&s, AUTO, vec![], &store, &LocalExec)
            .await
            .unwrap();
        assert!(rep.healthy, "{:?} {:?}", rep.actions, rep.warnings);
        assert_eq!(rep.cwd, real_wt.to_str().unwrap(), "the resolved dir");
        assert!(!root.join(".claude/worktrees/feat").exists());
    }

    /// Branch gone everywhere with an unreachable origin: automatic runs only
    /// report it, and the explicit repair aborts instead of silently forking
    /// a new branch from base.
    #[tokio::test]
    async fn real_git_unreachable_origin_aborts_branch_recreation() {
        let base = tempfile::TempDir::new().unwrap();
        let root = base.path().join("repo");
        init_repo(&root);
        let nope = base.path().join("nope.git");
        git_in(&root, &["remote", "add", "origin", nope.to_str().unwrap()]);
        let wt = root.join(".claude/worktrees/feat");
        let s = local_spec(&root, &wt, "fetchfail");
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let auto = ensure_workspace(&s, AUTO, vec![], &store, &LocalExec)
            .await
            .unwrap();
        assert!(
            auto.needs_explicit_repair,
            "automatic never recreates a branch"
        );
        let err = ensure_workspace(&s, Policy::Explicit, vec![], &store, &LocalExec)
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::E_REPAIR_FAILED, "{}", err.message);
        assert!(err.message.contains("cannot confirm"), "{}", err.message);
        assert!(!wt.exists());
        let branches = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["branch", "--list", "feat"])
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&branches.stdout).trim().is_empty(),
            "no branch created"
        );
    }

    /// Branch gone everywhere and no origin at all: the explicit repair
    /// recreates it from the base branch and records that.
    #[tokio::test]
    async fn real_git_no_origin_recreates_branch_from_base_explicitly() {
        let base = tempfile::TempDir::new().unwrap();
        let root = base.path().join("repo");
        init_repo(&root);
        let wt = root.join(".claude/worktrees/feat");
        let s = local_spec(&root, &wt, "frombase");
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let rep = ensure_workspace(&s, Policy::Explicit, vec![], &store, &LocalExec)
            .await
            .unwrap();
        assert_eq!(rep.branch_source.as_deref(), Some("branch_from_base:main"));
        assert!(wt.join(".git").exists());
    }

    /// Prune scope against real git: two worktrees are deleted; repairing
    /// ours must leave the other's stale registration for its owner.
    #[tokio::test]
    async fn real_git_repair_leaves_other_stale_registrations_alone() {
        let base = tempfile::TempDir::new().unwrap();
        let root = base.path().join("repo");
        init_repo(&root);
        let ours = root.join(".claude/worktrees/feat");
        let other = root.join(".claude/worktrees/other");
        git_in(
            &root,
            &[
                "worktree",
                "add",
                "-q",
                ours.to_str().unwrap(),
                "-b",
                "feat",
            ],
        );
        git_in(
            &root,
            &[
                "worktree",
                "add",
                "-q",
                other.to_str().unwrap(),
                "-b",
                "other",
            ],
        );
        std::fs::remove_dir_all(&ours).unwrap();
        std::fs::remove_dir_all(&other).unwrap();
        let s = local_spec(&root, &ours, "scope");
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let rep = ensure_workspace(&s, Policy::Explicit, vec![], &store, &LocalExec)
            .await
            .unwrap();
        assert!(ours.join(".git").exists(), "ours is back");
        assert!(
            rep.actions
                .iter()
                .any(|a| a.contains("worktree remove --force")),
            "{:?}",
            rep.actions
        );
        let list = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["worktree", "list", "--porcelain"])
            .output()
            .unwrap();
        let list = String::from_utf8_lossy(&list.stdout);
        assert!(
            list.contains("/.claude/worktrees/other"),
            "another worktree's stale registration must survive: {list}"
        );
    }

    /// Real `git` + a real `bash`, no tmux (policy Leave): delete a
    /// worktree directory and watch the repair bring it back on its branch.
    #[tokio::test]
    async fn real_git_repairs_a_deleted_worktree_directory() {
        use std::process::Command;
        // On macOS the tempdir itself sits under /var -> /private/var, so this
        // is also a symlinked-root test there.
        let base = tempfile::TempDir::new().unwrap();
        let root = base.path().join("repo");
        init_repo(&root);
        let wt = root.join(".claude/worktrees/feat");
        git_in(
            &root,
            &["worktree", "add", "-q", wt.to_str().unwrap(), "-b", "feat"],
        );
        // Simulate the user (or a cleanup job) deleting the directory.
        std::fs::remove_dir_all(&wt).unwrap();
        let s = local_spec(&root, &wt, "test");
        let store = Mutex::new(Store::open_in_memory().unwrap());
        // git still lists the deleted entry: that is an explicit repair.
        let rep = ensure_workspace(&s, Policy::Explicit, vec![], &store, &LocalExec)
            .await
            .unwrap();
        assert!(!rep.healthy);
        assert_eq!(rep.branch_source.as_deref(), Some("branch_local"));
        assert!(wt.join(".git").exists(), "worktree is back");
        let head = Command::new("git")
            .args([
                "-C",
                wt.to_str().unwrap(),
                "rev-parse",
                "--abbrev-ref",
                "HEAD",
            ])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&head.stdout).trim(), "feat");
        // Second run: healthy, no-op.
        let rep2 = ensure_workspace(&s, AUTO, vec![], &store, &LocalExec)
            .await
            .unwrap();
        assert!(rep2.healthy);
    }

    // ── automatic removal of our own vanished registration ────────────────

    const VANISHED_WT: &str = "/repo/.claude/worktrees/feat";
    /// A store-backed context with no other session and the parent
    /// fingerprint recorded while healthy matching `vanished()`'s.
    const NO_OTHERS: AutoContext = AutoContext {
        other_sessions_mapped: Some(false),
        allow_auto_unregister: true,
        recorded_parent_fp: Some((42, 7)),
    };
    /// Never recorded while healthy.
    const NO_FP: AutoContext = AutoContext {
        recorded_parent_fp: None,
        ..NO_OTHERS
    };
    /// Recorded, but the parent's inode differs now (remounted / replaced).
    const OTHER_FP: AutoContext = AutoContext {
        recorded_parent_fp: Some((42, 8)),
        ..NO_OTHERS
    };

    fn record_vanished_fp(store: &Mutex<Store>, fp: &str) {
        store
            .lock()
            .unwrap()
            .record_parent_fingerprint("local", VANISHED_WT, fp, 1)
            .unwrap();
    }

    #[tokio::test]
    async fn click_driven_entry_points_remove_only_with_a_matching_fingerprint() {
        // new_session, spawn_review, restart, recreate, and attach (also the
        // non-explicit repair_session Tauri call).
        for entry in [
            Entry::NewSession,
            Entry::SpawnReview,
            Entry::Restart,
            Entry::Recreate,
            Entry::Attach,
        ] {
            // No fingerprint recorded, or a different one: probe only,
            // E_REPAIR_REQUIRED naming why.
            for (recorded, why) in [
                (None, "no parent fingerprint was recorded"),
                (Some("42:8"), "differs from the one recorded"),
            ] {
                let (store, sid, pid) = seeded_store(VANISHED_WT);
                if let Some(fp) = recorded {
                    record_vanished_fp(&store, fp);
                }
                let exec = FakeExec::new(vec![ok(&vanished_out(true))]);
                let rep = ensure_workspace(
                    &spec_with_ids(sid, pid),
                    policy_for(entry),
                    vec![],
                    &store,
                    &exec,
                )
                .await
                .unwrap();
                assert_eq!(exec.scripts().len(), 1, "{entry:?}: probe only");
                assert!(exec.tmux_calls().is_empty(), "{entry:?}");
                assert!(rep.needs_explicit_repair, "{entry:?}");
                assert!(
                    rep.deferred
                        .iter()
                        .any(|d| d.contains("worktree remove --force")),
                    "{entry:?}: {:?}",
                    rep.deferred
                );
                let err = require_no_explicit(rep).unwrap_err();
                assert_eq!(err.code, codes::E_REPAIR_REQUIRED, "{entry:?}");
                assert!(err.message.contains(why), "{entry:?}: {}", err.message);
            }
            // The matching fingerprint: remove + re-add, re-checked in-script.
            let (store, sid, pid) = seeded_store(VANISHED_WT);
            record_vanished_fp(&store, "42:7");
            let exec = FakeExec::new(vec![
                ok(&vanished_out(true)),
                ok("outcome=branch_local\n"),
                ok(HEALTHY_OUT),
            ]);
            let rep = ensure_workspace(
                &spec_with_ids(sid, pid),
                policy_for(entry),
                vec![],
                &store,
                &exec,
            )
            .await
            .unwrap();
            let rep = require_no_explicit(rep).unwrap();
            assert!(
                rep.actions
                    .iter()
                    .any(|a| a.contains("worktree remove --force")),
                "{entry:?}: {:?}",
                rep.actions
            );
            assert!(
                exec.scripts()[1].contains("fp_expect='42:7'\n"),
                "{entry:?}"
            );
        }
    }

    /// `spec(true)`'s registered worktree whose directory vanished, with every
    /// guard condition confirmed by the probe.
    fn vanished() -> Probe {
        let mut p = healthy();
        p.wt_exists = false;
        p.wt_git = false;
        p.wt_gitdir_ok = false;
        p.tmux_cwd_exists = false;
        p.worktrees[1].prunable = true;
        p.root_canon = Some("/repo".into());
        p.wt_canon = Some(VANISHED_WT.into());
        p.wt_entry_exists = Some(false);
        p.wt_parent_exists = true;
        p.root_dev = Some("42".into());
        p.wt_parent_dev = Some("42".into());
        p.wt_parent_fp = Some((42, 7));
        p
    }

    /// `DIR_GONE_OUT` plus the canonical paths and the guard keys.
    fn vanished_out(parent_exists: bool) -> String {
        DIR_GONE_OUT.replacen(
            "@@worktrees\n",
            &format!(
                "root_canon=/repo\nwt_canon={VANISHED_WT}\nwt_entry_exists=0\n\
                 root_dev=42\nwt_parent_dev=42\nwt_parent_fp=42:7\nwt_parent_exists={}\n\
                 @@worktrees\n",
                u8::from(parent_exists)
            ),
            1,
        )
    }

    #[test]
    fn auto_removes_our_vanished_registration_when_every_guard_holds() {
        for policy in [AUTO, ATTACH] {
            let plan = plan_with(&spec(true), &vanished(), policy, NO_OTHERS).unwrap();
            assert!(
                !plan.needs_explicit_repair,
                "{policy:?}: {:?}",
                plan.warnings
            );
            assert_eq!(
                plan.steps,
                vec![
                    Step::Unregister {
                        path: VANISHED_WT.into()
                    },
                    add_local()
                ],
                "{policy:?}"
            );
            let g = plan.vanished_guard.expect("guard recorded");
            assert!(g.holds() && g.failed().is_empty());
            // A live pane is still never respawned automatically.
            assert!(plan
                .deferred
                .iter()
                .any(|s| matches!(s, Step::TmuxRespawn { .. })));
        }
        // Only on origin: the same removal, then a tracking add.
        let mut p = vanished();
        p.branch_local = false;
        p.branch_remote = true;
        let plan = plan_with(&spec(true), &p, AUTO, NO_OTHERS).unwrap();
        assert!(matches!(
            plan.steps[1],
            Step::AddWorktree {
                from: BranchSource::Remote,
                ..
            }
        ));
        // Branch nowhere: recreating it stays explicit, so nothing runs.
        let mut p = vanished();
        p.branch_local = false;
        let plan = plan_with(&spec(true), &p, AUTO, NO_OTHERS).unwrap();
        assert!(plan.needs_explicit_repair && plan.steps.is_empty());
    }

    /// Each guard alone keeps the automatic run explicit-only: no steps, and
    /// the warning (hence the `E_REPAIR_REQUIRED` message) names it.
    #[test]
    fn each_vanished_guard_blocks_the_automatic_removal() {
        let mut outside = vanished();
        outside.wt_canon = Some("/mnt/other/feat".into());
        outside.worktrees[1].canon = Some("/mnt/other/feat".into());
        let mut dotdot = vanished();
        dotdot.wt_canon = Some("/repo/x/../../etc/feat".into());
        dotdot.worktrees[1].canon = Some("/repo/x/../../etc/feat".into());
        let mapped = AutoContext {
            other_sessions_mapped: Some(true),
            ..NO_OTHERS
        };
        let cases: Vec<(&str, Probe, AutoContext, &str)> = vec![
            (
                "parent missing",
                Probe {
                    wt_parent_exists: false,
                    ..vanished()
                },
                NO_OTHERS,
                "parent directory missing",
            ),
            (
                "absence not reported",
                Probe {
                    wt_entry_exists: None,
                    ..vanished()
                },
                NO_OTHERS,
                "not confirmed absent",
            ),
            (
                "something still at the path",
                Probe {
                    wt_entry_exists: Some(true),
                    ..vanished()
                },
                NO_OTHERS,
                "not confirmed absent",
            ),
            (
                "no canonical path",
                Probe {
                    wt_canon: None,
                    ..vanished()
                },
                NO_OTHERS,
                "not confirmed absent",
            ),
            (
                "root canonical path unknown",
                Probe {
                    root_canon: None,
                    ..vanished()
                },
                NO_OTHERS,
                "not under the project root",
            ),
            (
                "outside the root",
                outside,
                NO_OTHERS,
                "not under the project root",
            ),
            ("dot-dot", dotdot, NO_OTHERS, "not under the project root"),
            ("another session", vanished(), mapped, "another session"),
            (
                "mapping unknown",
                vanished(),
                AutoContext {
                    other_sessions_mapped: None,
                    ..NO_OTHERS
                },
                "another session",
            ),
        ];
        for (name, probe, ctx, why) in cases {
            for policy in [AUTO, ATTACH] {
                let plan = plan_with(&spec(true), &probe, policy, ctx)
                    .unwrap_or_else(|e| panic!("{name}: {e}"));
                assert!(plan.steps.is_empty(), "{name} {policy:?}: {:?}", plan.steps);
                assert!(plan.needs_explicit_repair, "{name}");
                assert!(
                    plan.warnings.iter().any(|w| w.contains(why)),
                    "{name}: {:?}",
                    plan.warnings
                );
                assert!(!plan.vanished_guard.unwrap().holds(), "{name}");
            }
            // An explicit repair is unaffected by the guard.
            let ex = plan_with(&spec(true), &probe, Policy::Explicit, ctx).unwrap();
            assert!(
                matches!(ex.steps.first(), Some(Step::Unregister { .. })),
                "{name}"
            );
        }
        // Root missing / not a repository: refused before any guard, in every
        // policy (never faked).
        for probe in [
            Probe {
                root_exists: false,
                ..vanished()
            },
            Probe {
                root_gitdir_ok: false,
                ..vanished()
            },
        ] {
            let err = plan_with(&spec(true), &probe, AUTO, NO_OTHERS).unwrap_err();
            assert_eq!(err.code, codes::E_REPO_MISSING);
        }
        // Locked: refused first (`E_WORKSPACE_LOCKED`), in every policy; the
        // guard itself also names it.
        let mut locked = vanished();
        locked.worktrees[1].prunable = false;
        locked.worktrees[1].locked = true;
        assert_eq!(
            plan_with(&spec(true), &locked, AUTO, NO_OTHERS)
                .unwrap_err()
                .code,
            codes::E_WORKSPACE_LOCKED
        );
        let g = vanished_guard(
            &vanished(),
            &RegisteredWorktree {
                locked: true,
                ..feat_wt()
            },
            NO_OTHERS,
        );
        assert_eq!(g.failed(), vec!["registration is locked"]);
    }

    #[tokio::test]
    async fn auto_repair_of_a_vanished_registration_end_to_end_records_the_guard() {
        let (store, sid, pid) = seeded_store(VANISHED_WT);
        record_vanished_fp(&store, "42:7");
        let exec = FakeExec::new(vec![
            ok(&vanished_out(true)),      // probe
            ok("outcome=branch_local\n"), // git steps
            ok(HEALTHY_OUT),              // verify
        ]);
        let rep =
            ensure_workspace_with(&spec_with_ids(sid, pid), AUTO, vec![], &store, &exec, true)
                .await
                .unwrap();
        let rep = require_no_explicit(rep).unwrap();
        let scripts = exec.scripts();
        assert_eq!(scripts.len(), 3, "probe, apply, verify");
        assert_eq!(
            scripts[1],
            render_git_script_expecting(
                "/repo",
                &[
                    Step::Unregister {
                        path: VANISHED_WT.into()
                    },
                    add_local()
                ],
                Some((42, 7))
            )
        );
        assert!(!scripts[1].contains("prune"), "{}", scripts[1]);
        assert!(exec.tmux_calls().is_empty(), "AUTO never touches tmux");
        assert!(rep.tmux_cwd_stale, "the live pane is left to the caller");
        let ev = store.lock().unwrap().list_session_events(sid, 10).unwrap();
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].kind, EVENT_REPAIRED);
        let d: serde_json::Value = serde_json::from_str(ev[0].detail.as_deref().unwrap()).unwrap();
        for k in [
            "dir_absent",
            "parent_exists",
            "repo_ok",
            "under_root",
            "not_locked",
            "no_other_session",
            "same_filesystem",
            "siblings_present",
            "fingerprint_matches",
        ] {
            assert_eq!(d["vanished_guard"][k], true, "{k}: {d}");
        }
        assert_eq!(d["vanished_guard"]["fingerprint_check"], "match", "{d}");
    }

    #[tokio::test]
    async fn vanished_guard_failures_are_e_repair_required_and_apply_nothing() {
        // Parent missing (an unmounted volume looks exactly like this).
        let (store, sid, pid) = seeded_store(VANISHED_WT);
        let exec = FakeExec::new(vec![ok(&vanished_out(false))]);
        let rep =
            ensure_workspace_with(&spec_with_ids(sid, pid), AUTO, vec![], &store, &exec, true)
                .await
                .unwrap();
        assert_eq!(exec.scripts().len(), 1, "probe only");
        let err = require_no_explicit(rep).unwrap_err();
        assert_eq!(err.code, codes::E_REPAIR_REQUIRED);
        assert!(
            err.message.contains("parent directory missing"),
            "{}",
            err.message
        );

        // Another (ghost) session on the host maps to the same worktree.
        let (store, sid, pid) = seeded_store(VANISHED_WT);
        {
            let s = store.lock().unwrap();
            let other = s
                .upsert_session("rev-x", "local", Some(pid), None, 1, 1, "ghost", None)
                .unwrap();
            s.set_worktree_key(other, Some("feat")).unwrap();
        }
        let exec = FakeExec::new(vec![ok(&vanished_out(true))]);
        let rep =
            ensure_workspace_with(&spec_with_ids(sid, pid), AUTO, vec![], &store, &exec, true)
                .await
                .unwrap();
        assert_eq!(exec.scripts().len(), 1, "probe only");
        let err = require_no_explicit(rep).unwrap_err();
        assert_eq!(err.code, codes::E_REPAIR_REQUIRED);
        assert!(err.message.contains("another session"), "{}", err.message);
    }

    /// The owner's case against real git: `rm -rf` of a registered worktree
    /// directory. The automatic policy removes that one entry and re-adds it
    /// on the same branch; the second run is a no-op.
    #[tokio::test]
    async fn real_git_automatic_repair_recreates_a_vanished_registered_worktree() {
        use std::process::Command;
        let base = tempfile::TempDir::new().unwrap();
        let root = base.path().join("repo");
        init_repo(&root);
        let wt = root.join(".claude/worktrees/feat");
        git_in(
            &root,
            &["worktree", "add", "-q", wt.to_str().unwrap(), "-b", "feat"],
        );
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let pid = store
            .lock()
            .unwrap()
            .upsert_project("o", "r", root.to_str().unwrap())
            .unwrap();
        let mut s = local_spec(&root, &wt, "auto-vanished");
        s.project_id = Some(pid);
        // A healthy probe (any attach / restart) records the fingerprint.
        let healthy = ensure_workspace(&s, AUTO, vec![], &store, &LocalExec)
            .await
            .unwrap();
        assert!(healthy.healthy, "{:?}", healthy.warnings);
        // The owner's case: `rm -rf` of the ONLY worktree, then a
        // click-driven restart re-adds it (the fingerprint matches).
        std::fs::remove_dir_all(&wt).unwrap();
        let rep = ensure_workspace(&s, AUTO, vec![], &store, &LocalExec)
            .await
            .unwrap();
        let rep = require_no_explicit(rep).unwrap();
        assert!(
            rep.actions
                .iter()
                .any(|a| a.contains("worktree remove --force")),
            "{:?}",
            rep.actions
        );
        assert!(rep.vanished_guard.is_some_and(|g| g.holds()));
        assert_eq!(rep.branch_source.as_deref(), Some("branch_local"));
        assert!(wt.join(".git").exists(), "worktree is back");
        let head = Command::new("git")
            .args([
                "-C",
                wt.to_str().unwrap(),
                "rev-parse",
                "--abbrev-ref",
                "HEAD",
            ])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&head.stdout).trim(), "feat");
        let rep2 = ensure_workspace(&s, AUTO, vec![], &store, &LocalExec)
            .await
            .unwrap();
        assert!(
            rep2.healthy && rep2.actions.is_empty(),
            "{:?}",
            rep2.actions
        );
    }

    /// The unmounted-volume guard against real git: the whole worktrees dir
    /// is gone, so the parent is missing and nothing is removed.
    #[tokio::test]
    async fn real_git_missing_parent_blocks_the_automatic_removal() {
        let base = tempfile::TempDir::new().unwrap();
        let root = base.path().join("repo");
        init_repo(&root);
        let wt = root.join(".claude/worktrees/feat");
        git_in(
            &root,
            &["worktree", "add", "-q", wt.to_str().unwrap(), "-b", "feat"],
        );
        std::fs::remove_dir_all(root.join(".claude/worktrees")).unwrap();
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let pid = store
            .lock()
            .unwrap()
            .upsert_project("o", "r", root.to_str().unwrap())
            .unwrap();
        let mut s = local_spec(&root, &wt, "auto-unmounted");
        s.project_id = Some(pid);
        let rep = ensure_workspace_with(&s, AUTO, vec![], &store, &LocalExec, true)
            .await
            .unwrap();
        let err = require_no_explicit(rep).unwrap_err();
        assert_eq!(err.code, codes::E_REPAIR_REQUIRED);
        assert!(
            err.message.contains("parent directory missing"),
            "{}",
            err.message
        );
        let list = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["worktree", "list", "--porcelain"])
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&list.stdout).contains("/.claude/worktrees/feat"),
            "the registration is left alone"
        );
    }

    #[test]
    fn parse_probe_reads_device_ids_and_empty_means_unknown() {
        let p = parse_probe("root_dev=2049\nwt_parent_dev=2049\n");
        assert_eq!(p.root_dev.as_deref(), Some("2049"));
        assert_eq!(p.wt_parent_dev.as_deref(), Some("2049"));
        let p = parse_probe("root_dev=\nwt_parent_dev=\n");
        assert_eq!((p.root_dev, p.wt_parent_dev), (None, None));
    }

    /// Guard (g): a parent on another device (a mounted volume, an autofs
    /// mountpoint) or an unknown device id blocks the automatic removal.
    #[test]
    fn same_filesystem_guard_blocks_a_parent_on_another_device() {
        assert!(vanished_guard(&vanished(), &feat_wt(), NO_OTHERS).same_filesystem);
        let cases = [
            (
                "other device",
                Probe {
                    wt_parent_dev: Some("43".into()),
                    ..vanished()
                },
            ),
            (
                "parent stat failed",
                Probe {
                    wt_parent_dev: None,
                    ..vanished()
                },
            ),
            (
                "root stat failed",
                Probe {
                    root_dev: None,
                    ..vanished()
                },
            ),
        ];
        for (name, probe) in cases {
            for policy in [AUTO, ATTACH] {
                let plan = plan_with(&spec(true), &probe, policy, NO_OTHERS).unwrap();
                assert!(
                    plan.steps.is_empty() && plan.needs_explicit_repair,
                    "{name}: {:?}",
                    plan.steps
                );
                assert!(
                    plan.warnings
                        .iter()
                        .any(|w| w.contains("project root's filesystem")),
                    "{name}: {:?}",
                    plan.warnings
                );
                let g = plan.vanished_guard.unwrap();
                assert!(!g.same_filesystem && !g.holds(), "{name}");
            }
        }
    }

    #[tokio::test]
    async fn reappeared_refusal_from_the_apply_is_e_repair_required() {
        let (store, sid, pid) = seeded_store(VANISHED_WT);
        record_vanished_fp(&store, "42:7");
        let exec = FakeExec::new(vec![
            ok(&vanished_out(true)),
            Ok(ScriptOutput {
                ok: false,
                stdout: String::new(),
                stderr: format!("repair: {VANISHED_WT} {UNREGISTER_REFUSED}\n"),
            }),
        ]);
        let err =
            ensure_workspace_with(&spec_with_ids(sid, pid), AUTO, vec![], &store, &exec, true)
                .await
                .unwrap_err();
        assert_eq!(err.code, codes::E_REPAIR_REQUIRED, "{}", err.message);
        assert!(err.message.contains("reappeared"), "{}", err.message);
        assert_eq!(exec.scripts().len(), 2, "probe + refused apply, no verify");
        let ev = store.lock().unwrap().list_session_events(sid, 10).unwrap();
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].kind, EVENT_REPAIR_FAILED);
        assert!(ev[0]
            .detail
            .as_deref()
            .unwrap()
            .starts_with("E_REPAIR_REQUIRED"));
    }

    /// Real probe, then the directory reappears (with untracked work) right
    /// before the apply — an autofs / NFS path mounting late.
    struct ReappearExec {
        dir: std::path::PathBuf,
    }

    #[async_trait]
    impl RepairExec for ReappearExec {
        async fn run_script(&self, script: &str) -> Result<ScriptOutput, IpcError> {
            LocalExec.run_script(script).await
        }
        async fn apply_script(&self, script: &str) -> Result<ScriptOutput, IpcError> {
            std::fs::create_dir_all(&self.dir).unwrap();
            std::fs::write(self.dir.join("untracked.txt"), "work").unwrap();
            LocalExec.run_script(script).await
        }
        async fn tmux_new_session(&self, _: &str, _: &str, _: &str) -> Result<(), IpcError> {
            Ok(())
        }
        async fn tmux_respawn(&self, _: &str, _: &str, _: &str) -> Result<(), IpcError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn real_git_directory_reappearing_before_the_apply_is_never_removed() {
        let base = tempfile::TempDir::new().unwrap();
        let root = base.path().join("repo");
        init_repo(&root);
        let wt = root.join(".claude/worktrees/feat");
        git_in(
            &root,
            &["worktree", "add", "-q", wt.to_str().unwrap(), "-b", "feat"],
        );
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let pid = store
            .lock()
            .unwrap()
            .upsert_project("o", "r", root.to_str().unwrap())
            .unwrap();
        let mut s = local_spec(&root, &wt, "auto-reappear");
        s.project_id = Some(pid);
        // Healthy first, so the fingerprint is recorded and the guard holds.
        ensure_workspace(&s, AUTO, vec![], &store, &LocalExec)
            .await
            .unwrap();
        std::fs::remove_dir_all(&wt).unwrap();
        let exec = ReappearExec { dir: wt.clone() };
        let err = ensure_workspace_with(&s, AUTO, vec![], &store, &exec, true)
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::E_REPAIR_REQUIRED, "{}", err.message);
        assert!(err.message.contains("reappeared"), "{}", err.message);
        assert_eq!(
            std::fs::read_to_string(wt.join("untracked.txt")).unwrap(),
            "work",
            "the reappeared work is never deleted"
        );
        let list = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["worktree", "list", "--porcelain"])
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&list.stdout).contains("/.claude/worktrees/feat"),
            "nothing was removed"
        );
    }

    // ── the parent fingerprint ───────────────────────────────────────────

    #[test]
    fn fingerprint_parse_and_render_round_trip() {
        assert_eq!(parse_fp("42:7"), Some((42, 7)));
        assert_eq!(parse_fp(" 2049:131 \n"), Some((2049, 131)));
        for bad in ["", "42", "42:", ":7", "a:b", "42:7:1"] {
            assert_eq!(parse_fp(bad), None, "{bad:?}");
        }
        assert_eq!(fp_string((42, 7)), "42:7");
        let p = parse_probe("wt_parent_fp=42:7\n@@worktrees\n");
        assert_eq!(p.wt_parent_fp, Some((42, 7)));
        assert_eq!(parse_probe("wt_parent_fp=\n").wt_parent_fp, None);
    }

    #[test]
    fn probe_script_records_the_parent_fingerprint_and_sibling_presence() {
        let script = probe_script(&spec(true));
        assert!(script.contains("fpof() { stat -L -c '%d:%i' -- \"$1\" 2>/dev/null; }"));
        assert!(script.contains("fpof() { stat -L -f '%d:%i' -- \"$1\" 2>/dev/null; }"));
        assert!(
            script.contains("echo \"wt_parent_fp=$(fpof \"$(dirname -- \"$(canon \"$wt\")\")\")\"")
        );
        assert!(script.contains(
            "_w=\"${l#worktree }\"; if [ -e \"$_w\" ] || [ -L \"$_w\" ]; \
             then echo \"present 1\"; else echo \"present 0\"; fi;;"
        ));
        let p = parse_probe(
            "@@worktrees\nworktree /r/a\ncanon /r/a\npresent 0\n\nworktree /r/b\npresent 1\n",
        );
        assert_eq!(p.worktrees[0].present, Some(false));
        assert_eq!(p.worktrees[1].present, Some(true));
        assert_eq!(
            parse_probe("@@worktrees\nworktree /r\n").worktrees[0].present,
            None
        );
    }

    /// Match / mismatch / missing / failed stat: only a match lets an
    /// automatic removal run; the refusal names which.
    #[test]
    fn parent_fingerprint_gates_the_automatic_removal() {
        let g = vanished_guard(&vanished(), &feat_wt(), NO_OTHERS);
        assert_eq!(g.fingerprint_check, "match");
        assert!(g.fingerprint_matches && g.holds());
        let stat_failed = Probe {
            wt_parent_fp: None,
            ..vanished()
        };
        // The unmounted-volume shape: the leftover mountpoint dir lives on the
        // root's device, the recorded parent was the volume's (other device),
        // and the inode number happens to be equal. Must refuse.
        let other_device_same_inode = Probe {
            wt_parent_fp: Some((43, 7)),
            ..vanished()
        };
        for (name, probe, ctx, check, why) in [
            (
                "device differs, inode equal",
                other_device_same_inode,
                NO_OTHERS,
                "mismatch",
                "differs from the one recorded",
            ),
            (
                "mismatch",
                vanished(),
                OTHER_FP,
                "mismatch",
                "differs from the one recorded",
            ),
            (
                "missing",
                vanished(),
                NO_FP,
                "missing",
                "no parent fingerprint was recorded",
            ),
            (
                "stat failed",
                stat_failed,
                NO_OTHERS,
                "stat_failed",
                "could not read the parent's dev:inode",
            ),
        ] {
            let g = vanished_guard(&probe, &feat_wt(), ctx);
            assert_eq!(g.fingerprint_check, check, "{name}");
            assert!(!g.fingerprint_matches && !g.holds(), "{name}");
            for policy in [AUTO, ATTACH] {
                let plan = plan_with(&spec(true), &probe, policy, ctx).unwrap();
                assert!(
                    plan.steps.is_empty() && plan.needs_explicit_repair,
                    "{name}: {:?}",
                    plan.steps
                );
                assert!(
                    plan.warnings.iter().any(|w| w.contains(why)),
                    "{name}: {:?}",
                    plan.warnings
                );
            }
        }
    }

    /// (h): with a matching parent fingerprint a missing sibling is a stale
    /// registration, only a warning naming it; alongside a failed fingerprint
    /// it blocks and is named in the refusal.
    #[test]
    fn a_missing_sibling_is_a_warning_when_the_fingerprint_matches() {
        let mut p = vanished();
        p.worktrees.push(RegisteredWorktree {
            path: "/repo/.claude/worktrees/other".into(),
            branch: Some("other".into()),
            present: Some(false),
            ..Default::default()
        });
        let plan = plan_with(&spec(true), &p, AUTO, NO_OTHERS).unwrap();
        assert!(!plan.needs_explicit_repair, "{:?}", plan.warnings);
        assert!(matches!(plan.steps.first(), Some(Step::Unregister { .. })));
        let w = plan.warnings.join("\n");
        assert!(w.contains("not blocking"), "{w}");
        assert!(w.contains("/repo/.claude/worktrees/other"), "{w}");
        let g = plan.vanished_guard.unwrap();
        assert!(!g.siblings_present && g.holds(), "reported, not blocking");
        // Without the fingerprint it blocks and names the sibling.
        for ctx in [NO_FP, OTHER_FP] {
            let plan = plan_with(&spec(true), &p, AUTO, ctx).unwrap();
            assert!(plan.steps.is_empty() && plan.needs_explicit_repair);
            let w = plan.warnings.join("\n");
            assert!(w.contains("missing too"), "{w}");
            assert!(w.contains("/repo/.claude/worktrees/other"), "{w}");
        }
    }

    #[test]
    fn apply_script_re_checks_the_quoted_fingerprint_before_remove_and_add() {
        let steps = [
            Step::Unregister {
                path: "/r/.claude/worktrees/f".into(),
            },
            Step::AddWorktree {
                path: "/r/.claude/worktrees/f".into(),
                branch: "f".into(),
                from: BranchSource::Local,
            },
        ];
        let s = render_git_script_expecting("/r", &steps, Some((42, 7)));
        let fp = s.find("fp_expect='42:7'\n").expect("expected pair quoted");
        let before_rm = s
            .find(
                "if [ \"$(fpof \"$(dirname -- \"$p\")\")\" != \"$fp_expect\" ]; then echo \
                 \"repair: $p parent changed since the check; reappeared or parent missing; \
                 not removing\" >&2; exit 1; fi\n",
            )
            .expect("check before remove");
        let rm = s.find("worktree remove --force").unwrap();
        let before_add = s
            .find(
                "if [ \"$(fpof \"$(dirname -- '/r/.claude/worktrees/f')\")\" != \"$fp_expect\" ]; \
                 then echo \"repair: parent changed since the check; not re-adding\" >&2; exit 1; fi\n",
            )
            .expect("check before add");
        let add = s.find("worktree add --").unwrap();
        assert!(
            fp < before_rm && before_rm < rm && rm < before_add && before_add < add,
            "{s}"
        );
        // Without an expected pair (explicit) the script is unchanged.
        assert_eq!(
            render_git_script_expecting("/r", &steps, None),
            render_git_script("/r", &steps)
        );
        assert!(!render_git_script("/r", &steps).contains("fp_expect"));
    }

    #[tokio::test]
    async fn parent_changed_before_the_re_add_is_e_repair_required() {
        let (store, sid, pid) = seeded_store(VANISHED_WT);
        record_vanished_fp(&store, "42:7");
        let exec = FakeExec::new(vec![
            ok(&vanished_out(true)),
            Ok(ScriptOutput {
                ok: false,
                stdout: String::new(),
                stderr: format!("repair: {ADD_REFUSED}\n"),
            }),
        ]);
        let err = ensure_workspace(&spec_with_ids(sid, pid), AUTO, vec![], &store, &exec)
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::E_REPAIR_REQUIRED, "{}", err.message);
        assert!(
            err.message.contains("nothing was re-added"),
            "{}",
            err.message
        );
    }

    #[tokio::test]
    async fn fingerprint_is_recorded_only_for_a_healthy_registered_worktree() {
        let (store, _sid, _pid) = seeded_store(VANISHED_WT);
        let healthy = HEALTHY_OUT.replacen(
            "@@worktrees\n",
            &format!("wt_canon={VANISHED_WT}\nwt_parent_fp=42:7\n@@worktrees\n"),
            1,
        );
        let exec = FakeExec::new(vec![ok(&healthy)]);
        let rep = ensure_workspace(&spec(true), AUTO, vec![], &store, &exec)
            .await
            .unwrap();
        assert!(rep.healthy);
        {
            let s = store.lock().unwrap();
            assert_eq!(
                s.parent_fingerprint("local", VANISHED_WT)
                    .unwrap()
                    .as_deref(),
                Some("42:7")
            );
        }
        // A vanished worktree never overwrites what was recorded.
        let (store2, _, _) = seeded_store(VANISHED_WT);
        let exec = FakeExec::new(vec![ok(&vanished_out(true))]);
        ensure_workspace(&spec(true), AUTO, vec![], &store2, &exec)
            .await
            .unwrap();
        assert_eq!(
            store2
                .lock()
                .unwrap()
                .parent_fingerprint("local", VANISHED_WT)
                .unwrap(),
            None
        );
    }

    /// Against real git: a healthy probe records the canonical parent's
    /// real `dev:inode`.
    #[cfg(unix)]
    #[tokio::test]
    async fn real_git_healthy_probe_records_the_parent_dev_inode() {
        use std::os::unix::fs::MetadataExt;
        let base = tempfile::TempDir::new().unwrap();
        let root = base.path().join("repo");
        init_repo(&root);
        let wt = root.join(".claude/worktrees/feat");
        git_in(
            &root,
            &["worktree", "add", "-q", wt.to_str().unwrap(), "-b", "feat"],
        );
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let s = local_spec(&root, &wt, "fp-record");
        let rep = ensure_workspace(&s, AUTO, vec![], &store, &LocalExec)
            .await
            .unwrap();
        assert!(rep.healthy, "{:?}", rep.warnings);
        let canon = std::fs::canonicalize(&wt).unwrap();
        let parent = std::fs::metadata(canon.parent().unwrap()).unwrap();
        let recorded = store
            .lock()
            .unwrap()
            .parent_fingerprint("local", canon.to_str().unwrap())
            .unwrap();
        assert_eq!(recorded, Some(format!("{}:{}", parent.dev(), parent.ino())));
    }

    /// Against real git: the parent directory is replaced (a new inode, as a
    /// remount gives). Automatic repair refuses; the registration stays.
    #[tokio::test]
    async fn real_git_replaced_parent_directory_stays_explicit() {
        let base = tempfile::TempDir::new().unwrap();
        let root = base.path().join("repo");
        init_repo(&root);
        let wt = root.join(".claude/worktrees/feat");
        git_in(
            &root,
            &["worktree", "add", "-q", wt.to_str().unwrap(), "-b", "feat"],
        );
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let pid = store
            .lock()
            .unwrap()
            .upsert_project("o", "r", root.to_str().unwrap())
            .unwrap();
        let mut s = local_spec(&root, &wt, "fp-replaced");
        s.project_id = Some(pid);
        assert!(
            ensure_workspace(&s, AUTO, vec![], &store, &LocalExec)
                .await
                .unwrap()
                .healthy
        );
        // Move the old parent aside and create the new one WHILE the old
        // still exists (both inodes alive at once, so ext4 cannot hand the
        // freed inode straight back), and only then delete the old.
        let parent = root.join(".claude/worktrees");
        let old = root.join(".claude/worktrees.old");
        std::fs::rename(&parent, &old).unwrap();
        std::fs::create_dir(&parent).unwrap();
        std::fs::remove_dir_all(&old).unwrap();
        let rep = ensure_workspace(&s, AUTO, vec![], &store, &LocalExec)
            .await
            .unwrap();
        let err = require_no_explicit(rep).unwrap_err();
        assert_eq!(err.code, codes::E_REPAIR_REQUIRED);
        assert!(
            err.message.contains("differs from the one recorded"),
            "{}",
            err.message
        );
        assert!(!wt.exists(), "nothing was re-added under the new parent");
        assert!(worktree_list(&root).contains("/.claude/worktrees/feat"));
    }

    /// Against real git: two worktrees under one parent are deleted, but the
    /// parent is the same directory (its fingerprint matches), so the other
    /// is a stale registration: ours is re-added, the sibling only warned.
    #[tokio::test]
    async fn real_git_stale_sibling_is_only_a_warning_when_the_fingerprint_matches() {
        let base = tempfile::TempDir::new().unwrap();
        let root = base.path().join("repo");
        init_repo(&root);
        let wt = root.join(".claude/worktrees/feat");
        let other = root.join(".claude/worktrees/other");
        git_in(
            &root,
            &["worktree", "add", "-q", wt.to_str().unwrap(), "-b", "feat"],
        );
        git_in(
            &root,
            &[
                "worktree",
                "add",
                "-q",
                other.to_str().unwrap(),
                "-b",
                "other",
            ],
        );
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let pid = store
            .lock()
            .unwrap()
            .upsert_project("o", "r", root.to_str().unwrap())
            .unwrap();
        let mut s = local_spec(&root, &wt, "fp-siblings");
        s.project_id = Some(pid);
        assert!(
            ensure_workspace(&s, AUTO, vec![], &store, &LocalExec)
                .await
                .unwrap()
                .healthy
        );
        std::fs::remove_dir_all(&wt).unwrap();
        std::fs::remove_dir_all(&other).unwrap();
        let rep = ensure_workspace(&s, AUTO, vec![], &store, &LocalExec)
            .await
            .unwrap();
        let rep = require_no_explicit(rep).unwrap();
        assert!(wt.join(".git").exists(), "ours is re-added");
        let w = rep.warnings.join("\n");
        assert!(w.contains("not blocking"), "{w}");
        assert!(w.contains("/.claude/worktrees/other"), "{w}");
        assert!(!other.exists(), "the sibling is only reported");
    }

    fn worktree_list(root: &std::path::Path) -> String {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["worktree", "list", "--porcelain"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }
}
