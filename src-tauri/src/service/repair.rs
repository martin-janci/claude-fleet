//! Self-repairing workspaces: make sure a session's directory exists, is a
//! registered git worktree on the right branch, and that its tmux pane runs
//! there — on create, recreate, restart, attach, and on explicit request.
//!
//! The design is probe → plan → apply → verify:
//!
//! 1. **Probe** — ONE `bash -lc` script per host (every value `quote`d)
//!    prints a small `key=value` status block plus the raw
//!    `git worktree list --porcelain` dump. Nothing in the probe writes.
//! 2. **Plan** — [`plan`] is a pure function of the probe: it either returns
//!    the ordered [`Step`]s that make the workspace healthy, or refuses with
//!    an `E_*` code when a fix would need a guess (missing repo, branch
//!    checked out in the main checkout, a locked worktree, a non-empty dir
//!    that is not a worktree). Refusals never touch a row.
//! 3. **Apply** — the steps are rendered into one more script (git) and one
//!    or two tmux calls, then the probe runs once more to **verify**. A
//!    failed or unverifiable repair returns `E_REPAIR_FAILED` (or the tmux
//!    error) and never deletes or ghosts the session row.
//! 4. **Record** — the worktree row's path/branch are corrected, the session
//!    is linked to its worktree, and one `workspace_repaired` (or
//!    `workspace_repair_failed`) event lands on the session timeline.
//!
//! A healthy workspace costs exactly one probe and no writes, so every
//! lifecycle entry point can call [`ensure_workspace`] unconditionally.
//! Spec: `docs/specs/2026-09-11-session-worktree-repair.md`.

use crate::ipc_error::{codes, IpcError};
use crate::shell::quote;
use crate::ssh::SshClient;
use crate::store::{SessionRow, Store};
use crate::tmux::TmuxExec;
use async_trait::async_trait;
use serde::Serialize;
use std::sync::{Arc, Mutex};

/// `session_events.kind` written when a repair changed something.
pub const EVENT_REPAIRED: &str = "workspace_repaired";
/// `session_events.kind` written when a repair was refused or failed.
pub const EVENT_REPAIR_FAILED: &str = "workspace_repair_failed";

/// Connect budget for the probe / apply scripts on a remote host. The ssh
/// layer bounds the whole command by `default_wall_clock` on top, so a
/// wedged host surfaces as `E_SSH_TIMEOUT` → `E_HOST_OFFLINE` here.
const SCRIPT_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

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
    /// Branch to fork from when the worktree's branch has to be recreated
    /// (`None` = the repo's default branch).
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
    /// Expected absolute path on the host.
    pub path: String,
    /// Branch that must be checked out there.
    pub branch: String,
    /// `true` when `path` was derived from the layout convention rather than
    /// read from a row or seen on disk; the plan may then prefer the
    /// project's existing `.worktrees/` layout over `.claude/worktrees/`.
    pub path_is_guess: bool,
    /// Whether the local `worktrees` table row may be rewritten with the
    /// verified path. Only `local` paths belong in that table.
    pub row_is_local: bool,
}

/// What to do about tmux once the directory is healthy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TmuxPolicy {
    /// Fix the directory only; the caller creates / respawns tmux itself
    /// (`new_session`, `recreate_session`, `restart_session`).
    Leave,
    /// Also make sure the tmux session exists and its pane runs in the
    /// repaired directory (`repair_session`, the attach path).
    Ensure,
}

// ---------------------------------------------------------------------------
// Probe: what the workspace DOES look like
// ---------------------------------------------------------------------------

/// One entry of `git worktree list --porcelain`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RegisteredWorktree {
    pub path: String,
    /// `refs/heads/` stripped. `None` when detached or bare.
    pub branch: Option<String>,
    pub prunable: bool,
    pub locked: bool,
    pub bare: bool,
}

/// Parsed probe output. Every boolean defaults to `false`, so a truncated
/// or failed probe reads as "nothing is there" and the plan refuses rather
/// than assuming health.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Probe {
    pub root_exists: bool,
    pub root_git: bool,
    pub root_gitdir_ok: bool,
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
    pub tmux_alive: bool,
    pub tmux_cwd: Option<String>,
    pub tmux_cwd_exists: bool,
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
    format!(
        r#"set +e
root={root}
wt={wt}
br={br}
sess={sess}
yn() {{ if "$@" >/dev/null 2>&1; then echo 1; else echo 0; fi; }}
echo "root_exists=$(yn test -d "$root")"
echo "root_git=$(yn test -e "$root/.git")"
echo "root_gitdir_ok=$(yn git -C "$root" rev-parse --git-dir)"
echo "layout_dot_worktrees=$(yn test -d "$root/.worktrees")"
echo "wt_exists=$(yn test -d "$wt")"
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
if tmux has-session -t "=$sess" >/dev/null 2>&1; then
  echo tmux_alive=1
  cwd="$(tmux display-message -p -t "=$sess" '#{{pane_current_path}}' 2>/dev/null)"
  echo "tmux_cwd=$cwd"
  echo "tmux_cwd_exists=$(yn test -n "$cwd" -a -d "$cwd")"
else
  echo tmux_alive=0
fi
echo '{marker}'
git -C "$root" worktree list --porcelain 2>/dev/null
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
            "wt_gitdir_ok" => p.wt_gitdir_ok = flag(v),
            "index_lock" => p.index_lock = flag(v),
            "branch_local" => p.branch_local = flag(v),
            "branch_remote" => p.branch_remote = flag(v),
            "default_branch" => {
                let v = v.trim();
                p.default_branch = (!v.is_empty()).then(|| v.to_string());
            }
            "tmux_alive" => p.tmux_alive = flag(v),
            "tmux_cwd" => {
                let v = v.trim();
                p.tmux_cwd = (!v.is_empty()).then(|| v.to_string());
            }
            "tmux_cwd_exists" => p.tmux_cwd_exists = flag(v),
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
        if let Some(rest) = line.strip_prefix("branch ") {
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
    /// `refs/heads/<branch>` exists: check it out.
    Local,
    /// Only `refs/remotes/origin/<branch>` exists: create a tracking branch.
    Remote,
    /// Neither exists: `git fetch origin <branch>` first; use `origin/<branch>`
    /// if that produced it, else fork a NEW branch from `base` (local, then
    /// `origin/<base>`), falling back to `default` and finally the main
    /// checkout's `HEAD`.
    FetchOrBase { base: String, default: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Step {
    /// `git worktree prune` — drop registrations whose directory is gone.
    Prune,
    /// `git worktree repair <path>` — re-link a moved / stale checkout.
    RepairLinks { path: String },
    /// `git worktree add <path> …` with the branch resolved per `from`.
    AddWorktree {
        path: String,
        branch: String,
        from: BranchSource,
    },
    /// The branch is already checked out in another linked worktree: use
    /// that directory instead of creating a second checkout.
    AdoptPath { path: String },
    /// `tmux new-session -c <cwd>` — the session is gone.
    TmuxCreate { cwd: String },
    /// `tmux respawn-pane -k -c <cwd>` — the pane's cwd no longer exists
    /// (or was just recreated under it).
    TmuxRespawn { cwd: String },
}

impl Step {
    /// Human-readable one-liner for the report / event detail.
    pub fn describe(&self) -> String {
        match self {
            Step::Prune => "git worktree prune".into(),
            Step::RepairLinks { path } => format!("git worktree repair {path}"),
            Step::AddWorktree { path, branch, from } => match from {
                BranchSource::Local => format!("git worktree add {path} {branch}"),
                BranchSource::Remote => {
                    format!("git worktree add --track -b {branch} {path} origin/{branch}")
                }
                BranchSource::FetchOrBase { base, .. } => format!(
                    "git fetch origin {branch}; git worktree add {path} (origin/{branch} or new branch from {base})"
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
            Step::Prune | Step::RepairLinks { .. } | Step::AddWorktree { .. }
        )
    }
}

/// The plan for one workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub steps: Vec<Step>,
    /// The directory the pane must run in once the steps are applied.
    pub cwd: String,
    /// Branch actually checked out at `cwd` when it differs from the spec
    /// (the user switched branches inside the worktree). Reported and
    /// written back to the row; never "fixed".
    pub branch_drift: Option<String>,
    pub warnings: Vec<String>,
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

fn refuse(code: &str, msg: impl Into<String>) -> IpcError {
    IpcError::new(code, msg)
}

/// Decide the minimal fix. Pure: every branch of this function is covered
/// by a unit test with a hand-built [`Probe`].
pub fn plan(spec: &WorkspaceSpec, p: &Probe, policy: TmuxPolicy) -> Result<Plan, IpcError> {
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
    let mut steps = Vec::new();
    let mut warnings = Vec::new();
    let mut branch_drift = None;
    let mut recreated_dir = false;

    let cwd = match &spec.worktree {
        None => spec.project_root.clone(),
        Some(w) => {
            let expected = norm(&w.path);
            let registered = p.worktrees.iter().find(|r| norm(&r.path) == expected);
            let elsewhere = p.worktrees.iter().find(|r| {
                norm(&r.path) != expected && !r.prunable && r.branch.as_deref() == Some(&w.branch)
            });
            if let Some(other) = elsewhere {
                // The branch identifies the workspace; git refuses a second
                // checkout of it. A linked worktree elsewhere IS this
                // workspace (moved dir, other layout) — adopt it. The main
                // checkout is the user's, so refuse rather than hijack it.
                if norm(&other.path) == norm(&spec.project_root) {
                    return Err(refuse(
                        codes::E_BRANCH_CHECKED_OUT,
                        format!(
                            "branch {} is checked out in the main checkout {}; \
                             switch it to another branch (or move your work) before repairing",
                            w.branch, other.path
                        ),
                    ));
                }
                if p.index_lock {
                    warnings.push("index.lock present (a git operation may be running)".into());
                }
                steps.push(Step::AdoptPath {
                    path: other.path.clone(),
                });
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
                // Where a fresh checkout goes: the expected path, unless the
                // path was only guessed and the project already uses the
                // `.worktrees/` layout.
                let add_path = if w.path_is_guess && p.layout_dot_worktrees {
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
                    Some(r) if r.prunable => {
                        // (d) registered, directory gone.
                        if r.locked {
                            return Err(refuse(
                                codes::E_WORKSPACE_LOCKED,
                                format!(
                                    "worktree {} is registered but its directory is missing and \
                                     it is locked (git worktree unlock {} to allow repair)",
                                    r.path, r.path
                                ),
                            ));
                        }
                        steps.push(Step::Prune);
                        steps.push(add);
                        recreated_dir = true;
                        add_path
                    }
                    Some(r) if p.wt_exists && p.wt_gitdir_ok => {
                        // Healthy. Note branch drift, never touch it.
                        if r.branch.as_deref() != Some(&w.branch) {
                            branch_drift = r.branch.clone();
                            warnings.push(format!(
                                "worktree has {} checked out, row says {}",
                                r.branch.as_deref().unwrap_or("(detached)"),
                                w.branch
                            ));
                        }
                        if p.index_lock {
                            warnings
                                .push("index.lock present (a git operation may be running)".into());
                        }
                        w.path.clone()
                    }
                    Some(_) if p.wt_exists => {
                        // (c) registered and present but the `.git` link is
                        // stale (moved repo / rewritten admin dir).
                        steps.push(Step::RepairLinks {
                            path: w.path.clone(),
                        });
                        w.path.clone()
                    }
                    Some(_) => {
                        // Registered, not flagged prunable, directory gone —
                        // git just has not noticed yet.
                        steps.push(Step::Prune);
                        steps.push(add);
                        recreated_dir = true;
                        add_path
                    }
                    None if !p.wt_exists || p.wt_empty => {
                        // (a)/(b)/(l) nothing there, or an empty leftover
                        // from an interrupted add (git accepts an empty dir).
                        steps.push(Step::Prune);
                        steps.push(add);
                        recreated_dir = true;
                        add_path
                    }
                    None if p.wt_git => {
                        // (c) a checkout with a `.git` file that git no
                        // longer lists: try to re-link; verify decides.
                        steps.push(Step::RepairLinks {
                            path: w.path.clone(),
                        });
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

    if policy == TmuxPolicy::Ensure {
        if !p.tmux_alive {
            steps.push(Step::TmuxCreate { cwd: cwd.clone() });
        } else if recreated_dir || !p.tmux_cwd_exists {
            // A pane whose cwd was deleted keeps a dead inode even after the
            // path is recreated, so a recreated dir always means respawn.
            steps.push(Step::TmuxRespawn { cwd: cwd.clone() });
        }
    }

    Ok(Plan {
        steps,
        cwd,
        branch_drift,
        warnings,
    })
}

/// Render the git steps of a plan into one `bash` script. Prints
/// `outcome=<...>` lines for the parts whose result is only known at run
/// time (which branch source the add used).
pub fn render_git_script(root: &str, steps: &[Step]) -> String {
    let rq = quote(root);
    let mut s = String::from("set -e\n");
    for step in steps {
        match step {
            Step::Prune => s.push_str(&format!("git -C {rq} worktree prune 1>&2\n")),
            Step::RepairLinks { path } => s.push_str(&format!(
                "git -C {rq} worktree repair {} 1>&2\n",
                quote(path)
            )),
            Step::AddWorktree { path, branch, from } => {
                let pq = quote(path);
                let bq = quote(branch);
                match from {
                    BranchSource::Local => {
                        s.push_str(&format!(
                            "git -C {rq} worktree add {pq} {bq} 1>&2\necho outcome=branch_local\n"
                        ));
                    }
                    BranchSource::Remote => {
                        s.push_str(&format!(
                            "git -C {rq} worktree add --track -b {bq} {pq} origin/{bq} 1>&2\necho outcome=branch_remote\n"
                        ));
                    }
                    BranchSource::FetchOrBase { base, default } => {
                        let baseq = quote(base);
                        let defq = quote(default);
                        s.push_str(&format!(
                            "git -C {rq} fetch origin {bq} >/dev/null 2>&1 || true\n\
                             if git -C {rq} show-ref --verify --quiet refs/remotes/origin/{bq}; then\n\
                             \x20 git -C {rq} worktree add --track -b {bq} {pq} origin/{bq} 1>&2 || exit 1\n\
                             \x20 echo outcome=branch_remote\n\
                             else\n\
                             \x20 basebr={baseq}\n\
                             \x20 defbr={defq}\n\
                             \x20 if git -C {rq} show-ref --verify --quiet \"refs/heads/$basebr\"; then start=\"$basebr\"\n\
                             \x20 elif git -C {rq} show-ref --verify --quiet \"refs/remotes/origin/$basebr\"; then start=\"origin/$basebr\"\n\
                             \x20 elif git -C {rq} show-ref --verify --quiet \"refs/heads/$defbr\"; then start=\"$defbr\"\n\
                             \x20 else start=HEAD\n\
                             \x20 fi\n\
                             \x20 git -C {rq} worktree add {pq} -b {bq} \"$start\" 1>&2 || exit 1\n\
                             \x20 echo \"outcome=branch_from_base:$start\"\n\
                             fi\n"
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
    /// Run a bash script (`bash -lc`) on the host.
    async fn run_script(&self, script: &str) -> Result<ScriptOutput, IpcError>;
    async fn tmux_new_session(&self, name: &str, cwd: &str, pane_cmd: &str)
        -> Result<(), IpcError>;
    async fn tmux_respawn(&self, name: &str, cwd: &str, pane_cmd: &str) -> Result<(), IpcError>;
}

/// Production executor: local `bash -lc`, or `ssh <host> bash -lc '<script>'`
/// through the shared ControlMaster; tmux through the same `TmuxExec` the
/// lifecycle commands use.
pub struct HostExec {
    host: String,
    ssh: Arc<SshClient>,
    tmux: Box<dyn TmuxExec>,
}

impl HostExec {
    pub fn new(host: &str, ssh: &Arc<SshClient>) -> Self {
        Self {
            host: host.to_string(),
            ssh: Arc::clone(ssh),
            tmux: crate::service::sessions::exec_for(host, ssh),
        }
    }
}

#[async_trait]
impl RepairExec for HostExec {
    async fn run_script(&self, script: &str) -> Result<ScriptOutput, IpcError> {
        let out = if self.host == "local" {
            tokio::process::Command::new("bash")
                .args(["-lc", script])
                .output()
                .await
                .map_err(|e| IpcError::new(codes::E_SHELL, format!("spawn bash: {e}")))?
        } else {
            // Quote the WHOLE script so it crosses the ssh argv-join as one
            // word (see RemoteTmux::remote_bash).
            self.ssh
                .run(
                    &self.host,
                    &["bash", "-lc", &quote(script)],
                    SCRIPT_CONNECT_TIMEOUT,
                )
                .await?
        };
        Ok(ScriptOutput {
            ok: out.status.success(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
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
    /// The verified directory the pane runs (or must run) in.
    pub cwd: String,
    /// `true` when nothing needed doing.
    pub healthy: bool,
    /// Ordered, human-readable actions that were applied.
    pub actions: Vec<String>,
    pub warnings: Vec<String>,
    /// `branch_local` | `branch_remote` | `branch_from_base:<start>` when a
    /// worktree was (re)created.
    pub branch_source: Option<String>,
    /// `created` | `respawned` when tmux was touched.
    pub tmux: Option<String>,
    /// tmux state after the repair (for `TmuxPolicy::Leave` callers).
    pub tmux_alive: bool,
    /// The pane's cwd no longer exists or was recreated; a `Leave` caller
    /// must respawn / recreate with `cwd`.
    pub tmux_cwd_stale: bool,
    pub worktree_row_updated: bool,
    /// Alive sessions on the same host sharing this workspace (reviews, twins)
    /// whose panes may also need a respawn.
    pub sibling_session_ids: Vec<i64>,
}

// ---------------------------------------------------------------------------
// ensure_workspace: probe → plan → apply → verify → record
// ---------------------------------------------------------------------------

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
            eprintln!("[repair] event insert failed for session {id}: {e}");
        }
    }
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
                && p.worktrees
                    .iter()
                    .any(|r| norm(&r.path) == norm(cwd) && !r.prunable)
        }
    }
}

/// Make the workspace described by `spec` exist and be a healthy git
/// worktree; with [`TmuxPolicy::Ensure`] also make the tmux session run in
/// it. Idempotent: a healthy workspace costs one probe and writes nothing.
///
/// Errors: `E_HOST_OFFLINE` (probe could not reach the host — no change),
/// `E_REPO_MISSING`, `E_BRANCH_CHECKED_OUT`, `E_WORKSPACE_LOCKED`,
/// `E_REPAIR_FAILED` (a git step failed or the result did not verify),
/// `E_TMUX`. A failure never deletes or ghosts the session row.
pub async fn ensure_workspace(
    spec: &WorkspaceSpec,
    policy: TmuxPolicy,
    siblings: Vec<i64>,
    store: &Mutex<Store>,
    exec: &dyn RepairExec,
) -> Result<RepairReport, IpcError> {
    let probe = run_probe(exec, spec).await?;
    let fix = match plan(spec, &probe, policy) {
        Ok(p) => p,
        Err(e) => {
            record_event(
                store,
                spec.session_id,
                EVENT_REPAIR_FAILED,
                &format!("{}: {}", e.code, e.message),
            );
            return Err(e);
        }
    };

    let mut report = RepairReport {
        session_id: spec.session_id,
        host_alias: spec.host_alias.clone(),
        project_root: spec.project_root.clone(),
        tmux_name: spec.tmux_name.clone(),
        cwd: fix.cwd.clone(),
        healthy: fix.is_noop(),
        actions: Vec::new(),
        warnings: fix.warnings.clone(),
        branch_source: None,
        tmux: None,
        tmux_alive: probe.tmux_alive,
        tmux_cwd_stale: !probe.tmux_alive || !probe.tmux_cwd_exists,
        worktree_row_updated: false,
        sibling_session_ids: siblings,
    };

    // ── apply git steps, then verify ────────────────────────────────────
    let mut dir_changed = false;
    let mut final_branch: Option<String> = fix.branch_drift.clone();
    if fix.has_git_steps() {
        let git_steps: Vec<Step> = fix.steps.iter().filter(|s| s.is_git()).cloned().collect();
        let script = render_git_script(&spec.project_root, &git_steps);
        let out = match exec.run_script(&script).await {
            Ok(o) => o,
            Err(e) => {
                let e = host_offline(spec, e);
                record_event(
                    store,
                    spec.session_id,
                    EVENT_REPAIR_FAILED,
                    &format!("{}: {}", e.code, e.message),
                );
                return Err(e);
            }
        };
        if !out.ok {
            let e = IpcError::new(
                codes::E_REPAIR_FAILED,
                format!(
                    "workspace repair step failed on {}: {}",
                    spec.host_alias,
                    out.stderr.trim()
                ),
            );
            record_event(
                store,
                spec.session_id,
                EVENT_REPAIR_FAILED,
                &format!("{}: {}", e.code, e.message),
            );
            return Err(e);
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
        // Verify against the (possibly re-pathed) cwd.
        let mut vspec = spec.clone();
        if let Some(w) = vspec.worktree.as_mut() {
            w.path = fix.cwd.clone();
        }
        let after = run_probe(exec, &vspec).await?;
        if !healthy_after(spec, &fix.cwd, &after) {
            let e = IpcError::new(
                codes::E_REPAIR_FAILED,
                format!(
                    "workspace at {} on {} is still not a healthy worktree after repair \
                     (steps: {})",
                    fix.cwd,
                    spec.host_alias,
                    report.actions.join("; ")
                ),
            );
            record_event(
                store,
                spec.session_id,
                EVENT_REPAIR_FAILED,
                &format!("{}: {}", e.code, e.message),
            );
            return Err(e);
        }
        if final_branch.is_none() {
            final_branch = after
                .worktrees
                .iter()
                .find(|r| norm(&r.path) == norm(&fix.cwd))
                .and_then(|r| r.branch.clone());
        }
        report.tmux_alive = after.tmux_alive;
        dir_changed = true;
    }
    if fix
        .steps
        .iter()
        .any(|s| matches!(s, Step::AdoptPath { .. }))
    {
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
        match res {
            Ok(what) => {
                report.tmux = Some(what.to_string());
                report.tmux_alive = true;
                report.tmux_cwd_stale = false;
                report.actions.push(step.describe());
            }
            Err(e) => {
                record_event(
                    store,
                    spec.session_id,
                    EVENT_REPAIR_FAILED,
                    &format!("{}: {}", e.code, e.message),
                );
                return Err(e);
            }
        }
    }
    if dir_changed && report.tmux.is_none() {
        // The directory was recreated but tmux was left to the caller.
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
            // The `worktrees.path` column is a LOCAL path; a remote repair
            // may only refresh the (portable) branch of an existing row.
            let write: Option<(String, String)> = match (&existing, w.row_is_local) {
                (Some(row), true)
                    if row.path != fix.cwd || row.branch.as_deref() != Some(&branch) =>
                {
                    Some((fix.cwd.clone(), branch.clone()))
                }
                (None, true) => Some((fix.cwd.clone(), branch.clone())),
                (Some(row), false) if row.branch.as_deref() != Some(&branch) => {
                    Some((row.path.clone(), branch.clone()))
                }
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
                let detail = serde_json::json!({
                    "cwd": report.cwd,
                    "actions": report.actions,
                    "branch_source": report.branch_source,
                    "tmux": report.tmux,
                    "warnings": report.warnings,
                })
                .to_string();
                if let Err(e) = s.insert_session_event(sid, EVENT_REPAIRED, Some(&detail)) {
                    eprintln!("[repair] event insert failed for session {sid}: {e}");
                }
            }
        }
    }
    report.healthy = report.actions.is_empty();
    Ok(report)
}

// ---------------------------------------------------------------------------
// Building a spec from an existing session row
// ---------------------------------------------------------------------------

/// `(project_root, cwd)` of a project (and optional worktree) on a REMOTE
/// host, resolved with the exact function `new_session_inner` uses
/// (`sessions::remote_project_path`), so repair never disagrees with where a
/// session is created. This is the ONLY place `repair` derives a remote
/// path: when the projects-base setting changes that function's signature,
/// only this helper follows. The local `worktrees.path` column is a local
/// path and is never used for a remote host.
async fn resolve_remote_paths(
    ssh: &Arc<SshClient>,
    host: &str,
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
    Ok(crate::service::sessions::remote_project_path(
        &home, owner, repo, wt_name,
    ))
}

/// Snapshot taken under the store lock; finalized off-lock (remote `$HOME`).
struct SpecSeed {
    row: SessionRow,
    owner: String,
    repo: String,
    base_path: String,
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
        worktree,
        siblings,
    })
}

/// Build the [`WorkspaceSpec`] for an existing session row: paths from the
/// local `projects` / `worktrees` tables for `local`, from the
/// `~/projects/github.com/<owner>/<repo>` convention (plus the remote `$HOME`)
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
        let (root, cwd) =
            resolve_remote_paths(ssh, &row.host_alias, &seed.owner, &seed.repo, wt_name).await?;
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
    /// `~/projects/github.com/<owner>/<repo>[/.claude/worktrees/<name>]`
    /// convention remotely.
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
    Ok(WorkspaceSpec {
        host_alias: w.host_alias.to_string(),
        tmux_name: w.tmux_name.to_string(),
        project_root,
        worktree,
        base_branch: w.base_branch.map(str::to_string),
        pane_cmd: w.pane_cmd.to_string(),
        session_id: None,
        project_id: Some(w.project_id),
    })
}

/// `new_session`'s pre-tmux check for an existing worktree row / main
/// checkout: verify (and if needed rebuild) the directory, returning the
/// path tmux must start in. No row exists yet, so nothing is recorded on a
/// timeline here — the caller does that once the row appears.
pub async fn ensure_for_new_session(
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    w: NewSessionWorkspace<'_>,
) -> Result<RepairReport, IpcError> {
    let remote_root = if w.host_alias == "local" {
        None
    } else {
        let (owner, repo) = {
            let s = store.lock().map_err(|_| IpcError::lock())?;
            crate::service::sessions::fetch_owner_repo(&s, w.project_id)?
        };
        Some(
            resolve_remote_paths(ssh, w.host_alias, &owner, &repo, None)
                .await?
                .0,
        )
    };
    let spec = {
        let s = store.lock().map_err(|_| IpcError::lock())?;
        spec_for_new_session(&s, &w, remote_root)?
    };
    let exec = HostExec::new(w.host_alias, ssh);
    ensure_workspace(&spec, TmuxPolicy::Leave, Vec::new(), store, &exec).await
}

/// Explicit repair of one session (Tauri command `repair_session`, MCP tool
/// `repair_session`, and the pre-attach check): make its directory a healthy
/// worktree and its tmux session run there, creating tmux when it is gone.
/// Refuses on an unreachable host (`E_HOST_OFFLINE`) before touching anything.
pub async fn repair_session(
    session_id: i64,
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
    ensure_workspace(&spec, TmuxPolicy::Ensure, siblings, store, &exec).await
}

/// Same as [`repair_session`] but with `TmuxPolicy::Leave`, for lifecycle
/// callers that create / respawn tmux themselves. Returns the verified cwd
/// inside the report.
pub async fn ensure_session_workspace(
    session_id: i64,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<RepairReport, IpcError> {
    let (spec, siblings) = spec_for_session(store, ssh, session_id).await?;
    let exec = HostExec::new(&spec.host_alias, ssh);
    ensure_workspace(&spec, TmuxPolicy::Leave, siblings, store, &exec).await
}

#[cfg(test)]
mod tests {
    use super::plan as make_plan;
    use super::*;
    use std::collections::VecDeque;

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
        assert!(script.trim_end().ends_with("exit 0"));
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
        for policy in [TmuxPolicy::Leave, TmuxPolicy::Ensure] {
            let p = make_plan(&spec(true), &healthy(), policy).unwrap();
            assert!(p.is_noop(), "{policy:?}: {:?}", p.steps);
            assert_eq!(p.cwd, "/repo/.claude/worktrees/feat");
            assert!(p.warnings.is_empty());
        }
    }

    #[test]
    fn case_a_dir_deleted_tmux_alive_prunes_adds_and_respawns() {
        let mut p = healthy();
        p.wt_exists = false;
        p.wt_git = false;
        p.wt_gitdir_ok = false;
        p.tmux_cwd_exists = false;
        p.worktrees[1].prunable = true; // git already flags it
        let plan = make_plan(&spec(true), &p, TmuxPolicy::Ensure).unwrap();
        assert_eq!(
            plan.steps,
            vec![
                Step::Prune,
                add_local(),
                Step::TmuxRespawn {
                    cwd: "/repo/.claude/worktrees/feat".into()
                }
            ]
        );
    }

    #[test]
    fn case_b_dir_and_tmux_gone_prunes_adds_and_creates_tmux() {
        let mut p = healthy();
        p.wt_exists = false;
        p.wt_git = false;
        p.wt_gitdir_ok = false;
        p.tmux_alive = false;
        p.tmux_cwd = None;
        p.tmux_cwd_exists = false;
        p.worktrees.truncate(1); // already pruned
        let plan = make_plan(&spec(true), &p, TmuxPolicy::Ensure).unwrap();
        assert_eq!(
            plan.steps,
            vec![
                Step::Prune,
                add_local(),
                Step::TmuxCreate {
                    cwd: "/repo/.claude/worktrees/feat".into()
                }
            ]
        );
        // Leave: the caller creates tmux.
        let plan = make_plan(&spec(true), &p, TmuxPolicy::Leave).unwrap();
        assert_eq!(plan.steps, vec![Step::Prune, add_local()]);
    }

    #[test]
    fn case_c_dir_present_but_unregistered_or_stale_link_is_repaired() {
        // Unregistered: `.git` file present, git no longer lists it.
        let mut p = healthy();
        p.wt_gitdir_ok = false;
        p.worktrees.truncate(1);
        let plan = make_plan(&spec(true), &p, TmuxPolicy::Ensure).unwrap();
        assert_eq!(
            plan.steps,
            vec![Step::RepairLinks {
                path: "/repo/.claude/worktrees/feat".into()
            }]
        );
        // Registered but the checkout's link is stale.
        let mut p = healthy();
        p.wt_gitdir_ok = false;
        let plan = make_plan(&spec(true), &p, TmuxPolicy::Leave).unwrap();
        assert_eq!(
            plan.steps,
            vec![Step::RepairLinks {
                path: "/repo/.claude/worktrees/feat".into()
            }]
        );
    }

    #[test]
    fn case_d_prunable_registration_is_pruned_before_add() {
        let mut p = healthy();
        p.wt_exists = false;
        p.wt_git = false;
        p.wt_gitdir_ok = false;
        p.worktrees[1].prunable = true;
        let plan = make_plan(&spec(true), &p, TmuxPolicy::Leave).unwrap();
        assert_eq!(plan.steps, vec![Step::Prune, add_local()]);
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
        let plan = make_plan(&spec(true), &p, TmuxPolicy::Leave).unwrap();
        assert_eq!(
            plan.steps[1],
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
        let plan1 = make_plan(&spec(true), &p, TmuxPolicy::Leave).unwrap();
        assert_eq!(
            plan1.steps[1],
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
        let plan2 = make_plan(&s, &p, TmuxPolicy::Leave).unwrap();
        assert!(matches!(
            &plan2.steps[1],
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
        let plan = make_plan(&spec(true), &p, TmuxPolicy::Ensure).unwrap();
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
        let err = make_plan(&spec(true), &p, TmuxPolicy::Ensure).unwrap_err();
        assert_eq!(err.code, codes::E_BRANCH_CHECKED_OUT);
    }

    #[test]
    fn case_h_and_m_missing_repo_is_reported_never_faked() {
        for wt in [true, false] {
            let mut p = healthy();
            p.root_exists = false;
            p.root_git = false;
            p.root_gitdir_ok = false;
            let err = make_plan(&spec(wt), &p, TmuxPolicy::Ensure).unwrap_err();
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
            make_plan(&spec(false), &p, TmuxPolicy::Leave)
                .unwrap_err()
                .code,
            codes::E_REPO_MISSING
        );
    }

    #[test]
    fn case_j_tmux_cwd_gone_but_dir_healthy_respawns_only() {
        let mut p = healthy();
        p.tmux_cwd = Some("/repo/.claude/worktrees/feat".into());
        p.tmux_cwd_exists = false;
        let plan = make_plan(&spec(true), &p, TmuxPolicy::Ensure).unwrap();
        assert_eq!(
            plan.steps,
            vec![Step::TmuxRespawn {
                cwd: "/repo/.claude/worktrees/feat".into()
            }]
        );
        // A pane that merely `cd`ed elsewhere is left alone.
        let mut p = healthy();
        p.tmux_cwd = Some("/repo/.claude/worktrees/feat/src".into());
        assert!(make_plan(&spec(true), &p, TmuxPolicy::Ensure)
            .unwrap()
            .is_noop());
    }

    #[test]
    fn case_l_partial_add_empty_dir_is_reused_non_empty_is_refused() {
        // Empty leftover dir: git accepts it, so just prune + add.
        let mut p = healthy();
        p.wt_git = false;
        p.wt_gitdir_ok = false;
        p.wt_empty = true;
        p.worktrees.truncate(1);
        let plan1 = make_plan(&spec(true), &p, TmuxPolicy::Leave).unwrap();
        assert_eq!(plan1.steps, vec![Step::Prune, add_local()]);
        // Non-empty dir without `.git`: refuse, never delete user files.
        p.wt_empty = false;
        let err = make_plan(&spec(true), &p, TmuxPolicy::Leave).unwrap_err();
        assert_eq!(err.code, codes::E_REPAIR_FAILED);
        assert!(err.message.contains("never deleted"));
    }

    #[test]
    fn case_m_main_checkout_session_is_healthy_when_root_is_a_repo() {
        let plan = make_plan(&spec(false), &healthy(), TmuxPolicy::Ensure).unwrap();
        assert!(plan.is_noop());
        assert_eq!(plan.cwd, "/repo");
        // …and gets its tmux recreated when that is gone.
        let mut p = healthy();
        p.tmux_alive = false;
        let plan = make_plan(&spec(false), &p, TmuxPolicy::Ensure).unwrap();
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
        p.worktrees[1].prunable = true;
        p.worktrees[1].locked = true;
        let err = make_plan(&spec(true), &p, TmuxPolicy::Leave).unwrap_err();
        assert_eq!(err.code, codes::E_WORKSPACE_LOCKED);
        assert!(err.message.contains("git worktree unlock"));
        // A healthy worktree with an index.lock is reported, not touched.
        let mut p = healthy();
        p.index_lock = true;
        let plan = make_plan(&spec(true), &p, TmuxPolicy::Ensure).unwrap();
        assert!(plan.is_noop());
        assert!(plan.warnings.iter().any(|w| w.contains("index.lock")));
    }

    #[test]
    fn branch_drift_is_reported_not_fixed() {
        let mut p = healthy();
        p.worktrees[1].branch = Some("feat-v2".into());
        let plan = make_plan(&spec(true), &p, TmuxPolicy::Ensure).unwrap();
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
        let plan = make_plan(&s, &p, TmuxPolicy::Leave).unwrap();
        assert_eq!(plan.cwd, "/repo/.worktrees/feat");
        assert!(
            matches!(&plan.steps[1], Step::AddWorktree { path, .. } if path == "/repo/.worktrees/feat")
        );
    }

    // ── script rendering ─────────────────────────────────────────────────

    #[test]
    fn git_script_renders_steps_in_order_with_quoting() {
        let script = render_git_script(
            "/re po",
            &[
                Step::Prune,
                Step::AddWorktree {
                    path: "/re po/.claude/worktrees/it's".into(),
                    branch: "feat".into(),
                    from: BranchSource::Local,
                },
            ],
        );
        assert!(script.starts_with("set -e\n"));
        let prune = script.find("worktree prune").unwrap();
        let add = script.find("worktree add").unwrap();
        assert!(prune < add, "prune must precede add: {script}");
        assert!(
            script.contains(
                "git -C '/re po' worktree add '/re po/.claude/worktrees/it'\\''s' 'feat'"
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
        assert!(
            remote.contains("worktree add --track -b 'feat' '/repo/w' origin/'feat'"),
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
        assert!(fob.contains("worktree repair '/repo/w'"));
        assert!(fob.contains("fetch origin 'feat'"));
        assert!(fob.contains("refs/remotes/origin/'feat'"));
        assert!(fob.contains("basebr='dev'"));
        assert!(fob.contains("defbr='main'"));
        assert!(fob.contains("else start=HEAD"));
        assert!(fob.contains("worktree add '/repo/w' -b 'feat' \"$start\""));
        assert!(fob.contains("outcome=branch_from_base:$start"));
        // Failures inside the if-branches must abort (set -e does not cover `&&` lists).
        assert!(fob.contains("|| exit 1"));
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
            TmuxPolicy::Ensure,
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
            TmuxPolicy::Ensure,
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
        let prune = scripts[1].find("worktree prune").unwrap();
        let add = scripts[1].find("worktree add").unwrap();
        assert!(prune < add);
        assert!(scripts[1].contains("'/repo/.claude/worktrees/feat' 'feat'"));
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
        assert!(ev[0].detail.as_deref().unwrap().contains("worktree prune"));
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
        let out = HEALTHY_OUT.replace("tmux_alive=1", "tmux_alive=0");
        let exec = FakeExec::new(vec![ok(&out)]);
        let rep = ensure_workspace(
            &spec_with_ids(sid, pid),
            TmuxPolicy::Ensure,
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
        let rep = ensure_workspace(
            &spec_with_ids(sid, pid),
            TmuxPolicy::Leave,
            vec![],
            &store,
            &exec,
        )
        .await
        .unwrap();
        assert!(exec.tmux_calls().is_empty());
        assert!(rep.tmux.is_none());
        assert!(rep.tmux_alive);
        assert!(rep.tmux_cwd_stale, "caller must respawn with the new cwd");
    }

    #[tokio::test]
    async fn adopting_another_path_rewrites_the_local_worktree_row() {
        let (store, sid, pid) = seeded_store("/repo/.claude/worktrees/feat");
        let out = "root_exists=1\nroot_git=1\nroot_gitdir_ok=1\nwt_exists=0\nbranch_local=1\n\
                   tmux_alive=1\ntmux_cwd_exists=0\n@@worktrees\nworktree /repo\nbranch refs/heads/main\n\n\
                   worktree /repo/.worktrees/feat\nbranch refs/heads/feat\n";
        let exec = FakeExec::new(vec![ok(out)]);
        let rep = ensure_workspace(
            &spec_with_ids(sid, pid),
            TmuxPolicy::Ensure,
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
        let rep = ensure_workspace(&s, TmuxPolicy::Ensure, vec![], &store, &exec)
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
            TmuxPolicy::Ensure,
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
            TmuxPolicy::Ensure,
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
            TmuxPolicy::Ensure,
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
            TmuxPolicy::Ensure,
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
            TmuxPolicy::Ensure,
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
        let out = HEALTHY_OUT.replace("tmux_alive=1", "tmux_alive=0");
        let mut exec = FakeExec::new(vec![ok(&out)]);
        exec.tmux_fail = true;
        let err = ensure_workspace(
            &spec_with_ids(sid, pid),
            TmuxPolicy::Ensure,
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
        // `new_session` calls with session_id = None: the fix runs, nothing
        // is attached to a row (the row does not exist yet).
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let exec = FakeExec::new(vec![
            ok(DIR_GONE_OUT),
            ok("outcome=branch_local\n"),
            ok(HEALTHY_OUT),
        ]);
        let rep = ensure_workspace(&spec(true), TmuxPolicy::Leave, vec![], &store, &exec)
            .await
            .unwrap();
        assert_eq!(rep.actions.len(), 2);
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
        let err = repair_session(sid, &store, &ssh).await.unwrap_err();
        assert_eq!(err.code, codes::E_HOST_OFFLINE);
    }

    // ── against a real git repo (local) ───────────────────────────────────

    /// Real `git` + a real `bash`, no tmux (policy Leave): delete a
    /// worktree directory and watch the repair bring it back on its branch.
    #[tokio::test]
    async fn real_git_repairs_a_deleted_worktree_directory() {
        use std::process::Command;
        let base = tempfile::TempDir::new().unwrap();
        let root = base.path().join("repo");
        std::fs::create_dir_all(&root).unwrap();
        let r = root.to_str().unwrap();
        let git = |args: &[&str]| {
            let st = Command::new("git")
                .args(["-C", r])
                .args(args)
                .status()
                .unwrap();
            assert!(st.success(), "git {args:?}");
        };
        assert!(Command::new("git")
            .args(["init", "-q", "-b", "main", r])
            .status()
            .unwrap()
            .success());
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        std::fs::write(root.join("f"), "x").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "init"]);
        let wt = root.join(".claude/worktrees/feat");
        git(&["worktree", "add", "-q", wt.to_str().unwrap(), "-b", "feat"]);
        // Simulate the user (or a cleanup job) deleting the directory.
        std::fs::remove_dir_all(&wt).unwrap();

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
            async fn tmux_new_session(&self, _: &str, _: &str, _: &str) -> Result<(), IpcError> {
                unreachable!()
            }
            async fn tmux_respawn(&self, _: &str, _: &str, _: &str) -> Result<(), IpcError> {
                unreachable!()
            }
        }
        let mut s = spec(true);
        s.project_root = r.to_string();
        s.tmux_name = format!("cf-repair-test-{}", std::process::id());
        s.worktree.as_mut().unwrap().path = wt.to_str().unwrap().to_string();
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let rep = ensure_workspace(&s, TmuxPolicy::Leave, vec![], &store, &LocalExec)
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
        let rep2 = ensure_workspace(&s, TmuxPolicy::Leave, vec![], &store, &LocalExec)
            .await
            .unwrap();
        assert!(rep2.healthy);
    }
}
