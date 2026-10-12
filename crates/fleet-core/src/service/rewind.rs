//! Truncating a transcript into a new conversation id: the shared engine
//! behind Fork here, Rewind here and Retry.

use crate::cancel::CancellationRegistry;
use crate::ipc_error::{codes, lock, IpcError};
use crate::shell::quote;
use crate::ssh::SshClient;
use crate::store::{SessionRow, StartSource, Store};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

/// Sentinel the script prints (on stderr) when the anchor uuid is not in the
/// transcript — so the caller maps it to a code instead of parsing prose.
pub const NO_ANCHOR: &str = "__CF_NO_ANCHOR__";

/// Sentinel the script prints (on stderr) when the truncated copy holds no
/// conversation entry at all — rewinding to the FIRST turn, whose prefix is
/// nothing but the transcript's metadata header. Spec §4.3 refuses that: it is
/// `/clear` by a longer route and under a misleading label, and the engine
/// would otherwise succeed at it. The guard lives here, in the one place that
/// can actually know, rather than in a client (spec §5.1).
pub const NO_TURNS: &str = "__CF_NO_TURNS__";

/// The bash script that copies the head of a transcript into a new
/// conversation's `.jsonl`.
///
/// Locating the source file is [`crate::service::transcript::locate_script`]'s
/// job, unchanged — stored path, then the pane's cwd, then a glob over
/// `~/.claude/projects/*/<id>.jsonl` — and a missing transcript still exits 4
/// with its own sentinel.
///
/// What this adds is one `awk` pass:
///
/// - lines are copied from the start and stop **before** the line whose
///   TOP-LEVEL `uuid` is `anchor_uuid`; `None` copies the whole file, which
///   is what forking the newest turn means (Fork only — `rewind_conversation`
///   refuses a Rewind without an anchor);
/// - `sessionId` is rewritten to `new_id` on every copied line;
/// - `cwd` is rewritten when `cwd_rewrite` is set (a fork into a different
///   worktree, whose pane must start where the transcript says it did): the
///   exact old value and any subdirectory of it (`"cwd":"<old>/…"`). An
///   EMPTY old value means "the first `cwd` the transcript records" — the
///   spelling Claude Code actually saw, which a stored path (a symlinked
///   root) need not match. The new value must already be JSON-escaped;
/// - the result lands in `dest_dir` (else beside the source), named
///   `<new_id>.jsonl`, and its path is the only thing on stdout.
///
/// Finding the anchor is `top_uuid_is()`: only a line that contains the
/// anchor text at all (a cheap `index` prefilter, so the scan runs on a
/// handful of lines, not the whole file) is walked character by character,
/// tracking string / escape state and brace depth, and it matches only a
/// `"uuid"` KEY at depth 1 whose value is the anchor, whitespace allowed
/// around the `:`. A `"uuid"` nested inside `message` or a tool result, or
/// the anchor appearing as some other key's value (the next entry's
/// `parentUuid`), does not stop the copy. `LC_ALL=C` keeps `substr` a byte
/// operation, so that walk is linear. Limitation: it is a lexer, not a JSON
/// parser — it trusts the line to be one well-formed JSON object, which is
/// what Claude Code writes.
///
/// Both replacements go through the script's own `rep()`, which is
/// `index`-based and therefore LITERAL. `gsub` would treat the search text as
/// a regex, and a cwd is full of `.` and `+`. They match the COMPACT form
/// (`"sessionId":"…"`, no whitespace), which is how Claude Code serialises
/// every entry (`JSON.stringify`); a hand-edited, pretty-printed line would
/// keep its old id.
///
/// Copying a prefix rather than filtering by turn is deliberate: it keeps the
/// header entries that carry no uuid (`custom-title`, `mode`, `agent-name`,
/// `bridge-session`) and leaves the `parentUuid` chain intact, because a
/// prefix of a chain is still a chain.
///
/// The output is written to a temp file and moved into place only on success,
/// so a missing anchor leaves no half-written transcript for `--resume` to
/// find — and a copy that turned out to hold no conversation entry at all
/// (the header alone: rewinding to the very first turn) is removed and
/// reported with [`NO_TURNS`], the second sentinel beside [`NO_ANCHOR`].
#[allow(clippy::too_many_arguments)]
pub fn rewind_script(
    tmux_name: Option<&str>,
    stored_path: Option<&str>,
    fallback_dir: Option<&str>,
    claude_session_id: &str,
    new_id: &str,
    anchor_uuid: Option<&str>,
    dest_dir: Option<&str>,
    cwd_rewrite: Option<(&str, &str)>,
) -> String {
    let mut s = crate::service::transcript::locate_script(
        tmux_name,
        stored_path,
        fallback_dir,
        claude_session_id,
    );
    let new_q = quote(new_id);
    let anchor_q = quote(anchor_uuid.unwrap_or(""));
    let destdir_q = quote(dest_dir.unwrap_or(""));
    let (oldcwd, newcwd) = cwd_rewrite.unwrap_or(("", ""));
    let oldcwd_q = quote(oldcwd);
    let newcwd_q = quote(newcwd);
    s.push_str(&format!(
        r#"newid={new_q}
anchor={anchor_q}
destdir={destdir_q}
oldcwd={oldcwd_q}
newcwd={newcwd_q}
if [ -z "$destdir" ]; then destdir=$(dirname -- "$f"); fi
mkdir -p -- "$destdir" || exit 1
dest="$destdir/$newid.jsonl"
tmp="$dest.part.$$"
# Whole lines only: a Claude still appending (a fork mid-turn) leaves an
# unterminated last line, which copied would be broken JSON.
n=$(wc -l < "$f") || exit 1
# The values reach awk through the ENVIRONMENT, never `-v`: `-v` processes
# backslash escapes, so a JSON-escaped cwd (`\"`, `\\`) would come out
# unescaped and the rewritten line would be broken JSON.
CF_ANCHOR="$anchor" CF_OLDID={id_q} CF_NEWID="$newid" \
CF_OLDCWD="$oldcwd" CF_NEWCWD="$newcwd" LC_ALL=C awk -v n="$n" '
BEGIN {{
  anchor = ENVIRON["CF_ANCHOR"]; oldid = ENVIRON["CF_OLDID"]; newid = ENVIRON["CF_NEWID"]
  oldcwd = ENVIRON["CF_OLDCWD"]; newcwd = ENVIRON["CF_NEWCWD"]
}}
NR > n + 0 {{ exit }}
function rep(s, from, to,    out, p) {{
  if (from == "") return s
  out = ""
  while ((p = index(s, from)) > 0) {{
    out = out substr(s, 1, p - 1) to
    s = substr(s, p + length(from))
  }}
  return out s
}}
function top_uuid_is(s, want,    n, i, c, depth, instr, esc, j) {{
  n = length(s); depth = 0; instr = 0; esc = 0
  for (i = 1; i <= n; i++) {{
    c = substr(s, i, 1)
    if (instr) {{
      if (esc) esc = 0
      else if (c == "\\") esc = 1
      else if (c == "\"") instr = 0
      continue
    }}
    if (c == "{{" || c == "[") {{ depth++; continue }}
    if (c == "}}" || c == "]") {{ depth--; continue }}
    if (c != "\"") continue
    if (depth == 1 && substr(s, i, 6) == "\"uuid\"") {{
      j = i + 6
      while (substr(s, j, 1) == " " || substr(s, j, 1) == "\t") j++
      if (substr(s, j, 1) == ":") {{
        j++
        while (substr(s, j, 1) == " " || substr(s, j, 1) == "\t") j++
        if (substr(s, j, length(want) + 2) == "\"" want "\"") return 1
      }}
    }}
    instr = 1
  }}
  return 0
}}
# The body of the first `"cwd":"…"` on the line, still JSON-escaped, read up
# to the first UNESCAPED quote (a `[^"]*` match would stop at a `\"`). The
# compact key can only be structural: inside a string its quotes are escaped.
function cwd_of(s,    p, i, n, c, esc) {{
  p = index(s, "\"cwd\":\"")
  if (p == 0) return ""
  n = length(s); esc = 0
  for (i = p + 7; i <= n; i++) {{
    c = substr(s, i, 1)
    if (esc) esc = 0
    else if (c == "\\") esc = 1
    else if (c == "\"") return substr(s, p + 7, i - p - 7)
  }}
  return ""
}}
anchor != "" && index($0, anchor) > 0 && top_uuid_is($0, anchor) {{ found = 1; exit }}
{{
  line = rep($0, "\"sessionId\":\"" oldid "\"", "\"sessionId\":\"" newid "\"")
  if (newcwd != "" && oldcwd == "") oldcwd = cwd_of(line)
  if (newcwd != "" && oldcwd != "") {{
    line = rep(line, "\"cwd\":\"" oldcwd "\"", "\"cwd\":\"" newcwd "\"")
    line = rep(line, "\"cwd\":\"" oldcwd "/", "\"cwd\":\"" newcwd "/")
  }}
  print line
}}
END {{ if (anchor != "" && found != 1) exit 3 }}
' "$f" > "$tmp"
rc=$?
if [ "$rc" -ne 0 ]; then
  rm -f -- "$tmp"
  if [ "$rc" -eq 3 ]; then printf '{NO_ANCHOR} %s\n' "$anchor" >&2; fi
  exit "$rc"
fi
if ! grep -q -E -e '"uuid"[[:space:]]*:[[:space:]]*"' -- "$tmp"; then
  rm -f -- "$tmp"
  printf '{NO_TURNS}\n' >&2
  exit 5
fi
mv -- "$tmp" "$dest" || exit 1
printf '%s\n' "$dest"
"#,
        id_q = quote(claude_session_id),
    ));
    s
}

/// Run a bash script on `host_alias` (local or remote) and return the
/// captured output. Mirrors `service::safe_kill::run_shell` /
/// `service::transcript`'s own local wrapper: `crate::ssh::run_shell` takes
/// a `&dyn SshExec`, not the `Arc<SshClient>` service functions carry, so
/// each service module keeps a thin wrapper rather than repeating the
/// `ssh.as_ref()` + timeout at every call site.
async fn run_shell(
    ssh: &Arc<SshClient>,
    host_alias: &str,
    script: &str,
) -> Result<std::process::Output, IpcError> {
    crate::ssh::run_shell(
        ssh.as_ref(),
        host_alias,
        script,
        std::time::Duration::from_secs(30),
    )
    .await
}

/// Sentinel [`fork_worktree_script`] prints (on stderr) when the worktree's
/// directory or branch already exists: a fork only ever creates a fresh
/// pair, so the cleanup after a failure can never remove someone's own.
pub const WT_EXISTS: &str = "__CF_WT_EXISTS__";

/// Sentinel [`fork_worktree_script`] prints (exit 7) when the host's git
/// refuses the name as a branch (`git check-ref-format --branch`) — checked
/// before anything is created, so the caller gets `E_INVALID`, not raw git
/// stderr. [`crate::validate::branch_name`] refuses the same names up front;
/// this is the host's git having the last word.
pub const BAD_BRANCH: &str = "__CF_BAD_BRANCH__";

/// The bash script that creates a fork's NEW worktree, before anything else
/// happens (see `RewindArgs::new_worktree`).
///
/// The source checkout is the pane's live cwd, else `src_dir` (the stored
/// worktree / project path) — the same order `locate_script` uses. From it:
/// the commit to branch off (`HEAD`, so the new branch starts at the source
/// branch's tip, or at a detached source's commit) and the repo root (the
/// first entry of `git worktree list`, the main checkout, so the new tree is
/// never nested inside the source's). The directory convention is
/// `new_session`'s own ([`crate::service::sessions::WORKTREE_BASE_SNIPPET`],
/// in [`crate::projects::worktree_dir_name`]'s flattened directory).
/// A name the host's git refuses as a branch exits 7 with [`BAD_BRANCH`]
/// before anything else runs. Unlike `worktree_add_script` it never adopts
/// an existing directory or branch: either one exits 6 with [`WT_EXISTS`]. The only stdout is the
/// new tree's physical path (`pwd -P`) — the cwd Claude Code will see.
pub fn fork_worktree_script(src_tmux: Option<&str>, src_dir: Option<&str>, name: &str) -> String {
    let tmux_q = quote(src_tmux.unwrap_or(""));
    let target_q = quote(&crate::tmux::exact_pane(src_tmux.unwrap_or("")));
    format!(
        r#"set -e
name={name_q}
if ! git check-ref-format --branch "$name" >/dev/null 2>&1 || [ "$name" = @ ]; then
  printf '{BAD_BRANCH} %s\n' "$name" >&2
  exit 7
fi
src=''
if [ -n {tmux_q} ]; then
  src=$(tmux display-message -p -t {target_q} '#{{pane_current_path}}' 2>/dev/null) || src=''
fi
if [ -z "$src" ] || [ ! -d "$src" ]; then src={dir_q}; fi
if [ -z "$src" ]; then echo "no checkout to fork from" >&2; exit 1; fi
head=$(git -C "$src" rev-parse --verify HEAD)
root=$(git -C "$src" worktree list --porcelain | sed -n '1s/^worktree //p')
cd -- "$root"
{snippet}wt="$base/"{wtdir_q}
if [ -e "$wt" ] || git show-ref --verify --quiet "refs/heads/$name"; then
  printf '{WT_EXISTS} %s
' "$name" >&2
  exit 6
fi
git worktree add "$wt" -b "$name" "$head" 1>&2
( cd "$wt" && pwd -P )
"#,
        name_q = quote(name),
        dir_q = quote(src_dir.unwrap_or("")),
        wtdir_q = quote(&crate::projects::worktree_dir_name(name)),
        snippet = crate::service::sessions::WORKTREE_BASE_SNIPPET,
    )
}

/// Undo [`fork_worktree_script`] after a later step failed: `git worktree
/// remove` WITHOUT `--force` (a tree with changes in it stays), and only
/// then the branch. Never touches the source: both values are the ones the
/// creation just produced. A tree a live tmux pane is working in stays too
/// (exit 3, `{TREE_IN_USE}`): `new_session` can fail after its pane started.
pub fn remove_fork_worktree_script(path: &str, name: &str) -> String {
    format!(
        r#"set -e
wt={path_q}
name={name_q}
phys=$(cd -- "$wt" 2>/dev/null && pwd -P || printf '%s' "$wt")
if tmux list-panes -a -F '#{{pane_current_path}}' 2>/dev/null \
  | CF_A="$wt" CF_B="$phys" awk 'BEGIN {{ a = ENVIRON["CF_A"]; b = ENVIRON["CF_B"] }} $0==a || $0==b || index($0, a "/")==1 || index($0, b "/")==1 {{ f=1 }} END {{ exit !f }}'; then
  printf '{TREE_IN_USE} %s\n' "$wt" >&2
  exit 3
fi
root=$(git -C "$wt" worktree list --porcelain | sed -n '1s/^worktree //p')
cd -- "$root"
git worktree remove "$wt"
git update-ref -d "refs/heads/$name"
"#,
        path_q = quote(path),
        name_q = quote(name),
        TREE_IN_USE = TREE_IN_USE,
    )
}

/// Sentinel [`remove_fork_worktree_script`] prints when a live pane is in
/// the tree it was asked to remove.
pub const TREE_IN_USE: &str = "FLEET_FORK_TREE_IN_USE";

/// What happens after the truncated transcript exists.
///
/// `snake_case` so the serialised form IS the wire vocabulary — `"rewind"` /
/// `"fork"` — in the MCP params, in the Tauri command's arguments and in the
/// hub route alike. One vocabulary beats a `String` in one layer and an enum
/// in another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RewindMode {
    /// Rebind THIS session to the new conversation and restart its pane.
    Rewind,
    /// Leave this session alone; spawn a new one on the new conversation.
    Fork,
}

/// Serde-derived because this is also the `#[tauri::command]` parameter type
/// (Task 5): the desktop deserialises it from the frontend and the hub route
/// serialises it back out.
#[derive(Debug, Serialize, Deserialize)]
pub struct RewindArgs {
    pub session_id: i64,
    /// Keep strictly before this entry. `None` keeps the whole transcript,
    /// which is what forking the newest turn means.
    pub anchor_uuid: Option<String>,
    pub mode: RewindMode,
    /// Fork only: `Some(name)` puts the new session in a NEW worktree, and
    /// branch, of that name, cut from the source's current commit
    /// ([`fork_worktree_script`]); `None` reuses the source's worktree.
    ///
    /// The ordering is the whole design: the transcript copy must land in the
    /// NEW cwd's encoded project dir, and that cwd's physical path exists
    /// only once `git worktree add` has run. So the fork creates the worktree
    /// FIRST, writes the copy under its `pwd -P`, records the worktree row,
    /// and only then asks `new_session` to start in that existing worktree.
    /// The source's uncommitted changes stay with the source: the new tree is
    /// the committed HEAD.
    pub new_worktree: Option<String>,
}

/// A fresh conversation id, minted the way `claude_id_and_pane_cmd` does.
fn mint_conversation_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Fork needs a concrete project to spawn into: `SessionRow.project_id` is
/// `Option<i64>` (a `bg`/`external` row, or one whose project id never got
/// set, has none) while `NewSessionArgs.project_id` is required. Pulled out
/// of the Fork arm so it is reachable by a plain unit test without a live
/// SSH round trip — `service::work::resume::resume_session_args` guards the
/// identical Option-to-required-field situation the same way.
fn project_id_for_fork(project_id: Option<i64>) -> Result<i64, IpcError> {
    project_id.ok_or_else(|| {
        IpcError::new(
            codes::E_INVALID_STATE,
            "this session has no project to fork",
        )
    })
}

/// What `rewind_conversation` does to the world after the transcript copy:
/// read the pane, restart it, spawn a session. A seam so the service's own
/// sequencing — each mode's follow-up called exactly once, and what is undone
/// when the restart fails — is testable without a live tmux or a host
/// (spec §7). Production is [`LiveOps`]; the script and the cleanup `rm`
/// still go through `run_shell` either way.
#[async_trait::async_trait]
trait ReplyOps: Send + Sync {
    /// The pane's live `claude_status` (a `session_activity` probe), or
    /// `None` when it cannot tell — an unreachable host, a blank pane.
    async fn pane_status(&self, session_id: i64) -> Option<String>;
    async fn restart(
        &self,
        args: crate::service::sessions::RestartSessionArgs,
    ) -> Result<SessionRow, IpcError>;
    async fn spawn(
        &self,
        args: crate::service::sessions::NewSessionArgs,
    ) -> Result<SessionRow, IpcError>;
    /// Create a fork's new worktree ([`fork_worktree_script`]); returns its
    /// physical path.
    async fn add_worktree(
        &self,
        host_alias: &str,
        src_tmux: Option<&str>,
        src_dir: Option<&str>,
        name: &str,
    ) -> Result<String, IpcError>;
    /// Undo [`ReplyOps::add_worktree`] ([`remove_fork_worktree_script`]).
    async fn remove_worktree(
        &self,
        host_alias: &str,
        path: &str,
        name: &str,
    ) -> Result<(), IpcError>;
    /// `~/.claude/projects` on the host, where a transcript for a new cwd goes.
    async fn claude_projects_dir(&self, host_alias: &str) -> Result<String, IpcError>;
}

/// The real follow-ups: `session_activity`, `restart_session`, `new_session`.
struct LiveOps<'a> {
    store: &'a Mutex<Store>,
    ssh: &'a Arc<SshClient>,
    reg: &'a Arc<CancellationRegistry>,
}

#[async_trait::async_trait]
impl ReplyOps for LiveOps<'_> {
    async fn pane_status(&self, session_id: i64) -> Option<String> {
        crate::service::sessions::session_activity(self.store, self.ssh, session_id)
            .await
            .ok()
            .and_then(|p| p.claude_status)
    }
    async fn restart(
        &self,
        args: crate::service::sessions::RestartSessionArgs,
    ) -> Result<SessionRow, IpcError> {
        crate::service::sessions::restart_session(args, self.store, self.ssh).await
    }
    async fn spawn(
        &self,
        args: crate::service::sessions::NewSessionArgs,
    ) -> Result<SessionRow, IpcError> {
        crate::service::sessions::new_session(args, self.store, self.ssh, self.reg).await
    }
    async fn add_worktree(
        &self,
        host_alias: &str,
        src_tmux: Option<&str>,
        src_dir: Option<&str>,
        name: &str,
    ) -> Result<String, IpcError> {
        let script = fork_worktree_script(src_tmux, src_dir, name);
        let out = crate::ssh::run_shell(
            self.ssh.as_ref(),
            host_alias,
            &script,
            std::time::Duration::from_secs(60),
        )
        .await?;
        let stderr = String::from_utf8_lossy(&out.stderr);
        if !out.status.success() {
            if stderr.contains(BAD_BRANCH) {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    format!("{name:?} is not a valid branch name"),
                ));
            }
            if stderr.contains(WT_EXISTS) {
                return Err(IpcError::new(
                    codes::E_CONFLICT,
                    format!(
                        "a worktree or branch named {name:?} already exists; pick another name"
                    ),
                ));
            }
            return Err(IpcError::new(codes::E_GIT_SETUP, stderr.trim().to_string()));
        }
        let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !path.starts_with('/') {
            return Err(IpcError::new(
                codes::E_GIT_SETUP,
                format!("git worktree add reported no path: {}", stderr.trim()),
            ));
        }
        Ok(path)
    }
    async fn remove_worktree(
        &self,
        host_alias: &str,
        path: &str,
        name: &str,
    ) -> Result<(), IpcError> {
        let out = run_shell(
            self.ssh,
            host_alias,
            &remove_fork_worktree_script(path, name),
        )
        .await?;
        if out.status.success() {
            Ok(())
        } else {
            Err(IpcError::new(
                codes::E_GIT_SETUP,
                String::from_utf8_lossy(&out.stderr).trim().to_string(),
            ))
        }
    }
    async fn claude_projects_dir(&self, host_alias: &str) -> Result<String, IpcError> {
        let home = if host_alias == crate::service::projects::LOCAL_HOST {
            crate::service::hosts::local_home_dir()
                .to_string_lossy()
                .into_owned()
        } else {
            self.ssh.remote_home(host_alias).await?
        };
        Ok(format!("{}/.claude/projects", home.trim_end_matches('/')))
    }
}

/// Whether a `claude_status` value means the session is between turns — the
/// same set as the frontend's `isQuietStatus`. `None` and anything this
/// build cannot parse are NOT quiet: an unknown state is not evidence that a
/// restart throws nothing away.
fn status_is_quiet(status: Option<&str>) -> bool {
    status
        .and_then(|s| s.parse::<crate::service::pane_intel::ClaudeStatus>().ok())
        .is_some_and(crate::service::pane_intel::ClaudeStatus::is_quiet)
}

/// Truncate a session's transcript into a new conversation and act on it.
///
/// One engine for three buttons: Fork here spawns on the copy, Rewind here
/// rebinds this session to it and restarts the pane, and Retry is Rewind
/// followed by a `send_prompt` the CALLER makes — so Retry inherits every
/// refusal below instead of duplicating it.
///
/// The source transcript is never mutated. That is what makes a rewind
/// undoable and leaves the pre-rewind conversation listed; `StartSource::Fork`
/// is what labels the new one, a value both clients already render.
///
/// `reg` is threaded through to `sessions::new_session` (the Fork arm) so a
/// forked spawn's remote git/clone step is cancellable through the same
/// `CancellationRegistry` every other spawn uses — `rewind_script`'s brief
/// omitted it, but `new_session` hard-requires one and minting a throwaway
/// registry per call would make that cancellation silently unreachable.
pub async fn rewind_conversation(
    args: RewindArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    reg: &Arc<CancellationRegistry>,
) -> Result<SessionRow, IpcError> {
    rewind_conversation_with(args, store, ssh, &LiveOps { store, ssh, reg }).await
}

async fn rewind_conversation_with(
    args: RewindArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    ops: &dyn ReplyOps,
) -> Result<SessionRow, IpcError> {
    if let Some(a) = args.anchor_uuid.as_deref() {
        crate::validate::claude_session_id(a)
            .map_err(|_| IpcError::new(codes::E_INVALID, "anchor_uuid must be a lowercase UUID"))?;
    }
    // A rewind with no anchor would copy the WHOLE transcript and restart
    // the pane on it: the same conversation under a new id, reported as a
    // success — a silent no-op. Spec §3 gives "no anchor ⇒ keep the whole
    // file" to Fork alone (forking the newest turn); Rewind and Retry always
    // name their own turn's prompt.
    if args.mode == RewindMode::Rewind && args.anchor_uuid.is_none() {
        return Err(IpcError::new(
            codes::E_INVALID,
            "rewind needs anchor_uuid: the prompt to rewind to before",
        ));
    }
    // A new worktree is a Fork-only shape, and its name becomes a branch:
    // checked here, before any I/O, by the rule `new_session` applies.
    if let Some(name) = args.new_worktree.as_deref() {
        if args.mode != RewindMode::Fork {
            return Err(IpcError::new(
                codes::E_INVALID,
                "new_worktree applies to a fork, not a rewind",
            ));
        }
        crate::service::sessions::validate_new_worktree_name(name)?;
    }
    // Snapshot under one short lock; every I/O below happens with it released.
    let (sess, claude_id, stored_transcript_path, fallback_cwd, launch) = {
        let s = lock(store)?;
        let sess = s
            .get_session_by_id(args.session_id)?
            .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, "session not found"))?;
        let claude_id = sess.claude_session_id.clone().ok_or_else(|| {
            IpcError::new(
                codes::E_INVALID,
                "this session has no Claude conversation to rewind",
            )
        })?;
        crate::validate::claude_session_id(&claude_id).map_err(|_| {
            IpcError::new(
                codes::E_INVALID,
                "this session's Claude conversation id is not a UUID",
            )
        })?;
        // A rewind delegates the pane restart to `restart_session`, which runs
        // `tmux_name_addressable` and `guard_not_controller` INSIDE itself —
        // i.e. AFTER the rebind below has already committed. Spec §4.3 lists
        // both as PRE-conditions, so check them here, before any I/O: a `bg:`
        // row (a background agent or an external interactive session) has no
        // pane to restart, and the controller refuses to restart itself. Fork
        // touches nothing live and is exempt, exactly as the mid-turn guard is.
        if args.mode == RewindMode::Rewind {
            crate::validate::tmux_name_addressable(&sess.tmux_name)?;
            crate::service::sessions::guard_not_controller(
                s.get_controller()?.as_ref(),
                &sess.host_alias,
                &sess.tmux_name,
                false,
            )?;
        }
        // `SessionRow` carries no `transcript_path` field of its own (only
        // conversation rows do); the session table's copy is a separate
        // getter, same as `transcript::resolve_args` uses.
        let stored_transcript_path = s.session_transcript_path(sess.id)?;
        // Locate the transcript the way EVERY other reader of it does
        // (`transcript::resolve_args`): the worktree's path, else the
        // project's, as the fallback cwd. Without it, a `tmux display-message`
        // that yields no cwd sends `locate_script` to its
        // `~/.claude/projects/*/<id>.jsonl` glob, and `dest_dir` becomes the
        // `dirname` of whatever that found — which need not be the encoded
        // dir for the session's actual cwd, so `cl --resume <newid>` would
        // then find nothing, silently.
        let wt = match sess.worktree_id {
            Some(wid) => s.worktree_path(wid).ok().flatten(),
            None => None,
        };
        let fallback_cwd = match wt {
            Some(p) => Some(p),
            None => match sess.project_id {
                Some(pid) => s.project_base_path(pid).ok().flatten(),
                None => None,
            },
        };
        // A fork opens on the source's model / effort, as recreate, repair
        // and move do; a rewind keeps them through `restart_session`.
        let launch = crate::service::sessions::stored_launch(&s, sess.id)?;
        (
            sess,
            claude_id,
            stored_transcript_path,
            fallback_cwd,
            launch,
        )
    };

    // A shell row has no Claude: its restart respawns a bare shell, which
    // would kill whatever it runs and bind a conversation nothing resumes.
    if sess.kind == "shell" {
        return Err(IpcError::new(
            codes::E_INVALID,
            "a shell session has no Claude conversation to rewind or fork",
        ));
    }
    // A fork starts a session in the source's project, so one with no
    // project is refused here, before the copy (or a new worktree) is made:
    // refused after it, the copy would stay behind as an orphan transcript.
    let fork_project_id = match args.mode {
        RewindMode::Fork => Some(project_id_for_fork(sess.project_id)?),
        RewindMode::Rewind => None,
    };
    // Rewind respawns the pane, so doing it mid-turn throws the turn away —
    // and "mid-turn" is not only `working`: a `blocked` session is waiting
    // on a permission prompt or a question INSIDE its turn. So the rule is
    // the positive one, quiet or refused (`ClaudeStatus::is_quiet`, the
    // frontend's `isQuietStatus`). The stored status can lag the pane by a
    // whole reconcile tick, so the pane is asked first, live; only when it
    // cannot say (unreachable, blank) does the stored value decide. Fork is
    // exempt: it touches nothing live.
    if args.mode == RewindMode::Rewind {
        let live = ops.pane_status(sess.id).await;
        let status = live.as_deref().or(sess.claude_status.as_deref());
        if !status_is_quiet(status) {
            return Err(IpcError::new(
                codes::E_INVALID,
                match status {
                    Some(s) => format!(
                        "this session is mid-turn ({s}); interrupt it or answer its prompt first, then rewind"
                    ),
                    None => "can't tell whether this session is mid-turn (no status yet); interrupt it first, then rewind".to_string(),
                },
            ));
        }
    }

    let new_id = mint_conversation_id();
    // Same rule as `resolve_args`: a row with no pane (`bg` / `external`) has
    // no `tmux display-message` to ask, so do not pretend it has one.
    let no_pane = crate::store::has_no_pane(&sess.kind) || sess.tmux_name.starts_with("bg:");
    let src_tmux = (!no_pane).then_some(sess.tmux_name.as_str());

    // A fork into a new worktree creates that worktree FIRST: its physical
    // path is what the copy's project dir and `cwd` must name (spec §5.2),
    // and nothing else can say what it is (`RewindArgs::new_worktree`).
    let new_wt: Option<NewWorktree> = match args.new_worktree.as_deref() {
        Some(name) => {
            let project_id = project_id_for_fork(fork_project_id)?;
            let path = ops
                .add_worktree(&sess.host_alias, src_tmux, fallback_cwd.as_deref(), name)
                .await?;
            let mut wt = NewWorktree {
                name: name.to_string(),
                path,
                project_id,
                projects_dir: String::new(),
            };
            match ops.claude_projects_dir(&sess.host_alias).await {
                Ok(dir) => wt.projects_dir = dir,
                Err(e) => {
                    undo_new_worktree(ops, store, &sess.host_alias, &wt, None).await;
                    return Err(e);
                }
            }
            Some(wt)
        }
        None => None,
    };
    let dest_dir = new_wt.as_ref().map(|wt| {
        format!(
            "{}/{}",
            wt.projects_dir,
            crate::service::transcript::encode_project_dir(&wt.path)
        )
    });
    // The value goes into JSON text as-is, so it is escaped the way the
    // transcript's own strings are; the old value is the transcript's first
    // `cwd` (the `""`), whatever spelling of the source that was.
    let json_cwd = new_wt.as_ref().map(|wt| json_string_body(&wt.path));
    let script = rewind_script(
        src_tmux,
        stored_transcript_path.as_deref(),
        fallback_cwd.as_deref(),
        &claude_id,
        &new_id,
        args.anchor_uuid.as_deref(),
        dest_dir.as_deref(),
        json_cwd.as_deref().map(|c| ("", c)),
    );
    let new_path = match copy_transcript(ssh, &sess.host_alias, &script).await {
        Ok(p) => p,
        Err(e) => {
            if let Some(wt) = &new_wt {
                undo_new_worktree(ops, store, &sess.host_alias, wt, None).await;
            }
            return Err(e);
        }
    };

    match args.mode {
        RewindMode::Rewind => {
            {
                let s = lock(store)?;
                s.rebind_conversation(
                    sess.id,
                    &new_id,
                    StartSource::Fork,
                    Some(&new_path),
                    sess.context.model.as_deref(),
                )?;
            }
            let restarted = ops
                .restart(crate::service::sessions::RestartSessionArgs {
                    host_alias: sess.host_alias.clone(),
                    name: sess.tmux_name.clone(),
                    force: false,
                    profile: None,
                    model: None,
                    effort: None,
                })
                .await;
            // The rebind is committed before the restart can be attempted at
            // all, and the restart can still fail for something only it can
            // know (`E_REPAIR_REQUIRED` from `ensure_session_workspace`, an
            // unreachable host). Undo both halves of what this call did, so
            // the failure leaves nothing behind: the row goes back on the
            // conversation it was on (it must never read a frozen copy while
            // the live Claude keeps writing the original), the new
            // conversation's row is dropped rather than left listed as a
            // conversation that never ran, and the new `.jsonl` is removed
            // from the host rather than left as an orphan. `revert_rebind`
            // is not a second rebind: a `Resume` rebind would write a
            // conversation row of its own and mark the context stale.
            // Best-effort: the restart's error is what the caller must see.
            if let Err(e) = &restarted {
                match lock(store).and_then(|s| {
                    s.revert_rebind(
                        sess.id,
                        &new_id,
                        &claude_id,
                        stored_transcript_path.as_deref(),
                    )
                }) {
                    Ok(_) => {}
                    Err(re) => tracing::warn!(
                        session_id = sess.id,
                        error = %re.message,
                        "[rewind] restoring the previous conversation failed"
                    ),
                }
                remove_copy(ssh, &sess.host_alias, &new_path, &new_id, sess.id).await;
                tracing::warn!(
                    session_id = sess.id,
                    error = %e.message,
                    "[rewind] restart failed; rebound to the previous conversation"
                );
            }
            restarted
        }
        RewindMode::Fork => {
            // Same worktree: the pane starts exactly where the copy was
            // written, beside the source's transcript. New worktree: its row
            // is recorded now, so `new_session` starts in an EXISTING
            // worktree — the path above, which the copy already names.
            let project_id = project_id_for_fork(fork_project_id)?;
            let worktree_id = match &new_wt {
                None => sess.worktree_id,
                Some(wt) => match lock(store).and_then(|s| {
                    s.upsert_worktree_on(
                        &sess.host_alias,
                        wt.project_id,
                        &crate::projects::worktree_dir_name(&wt.name),
                        &wt.path,
                        Some(&wt.name),
                    )
                    .map_err(IpcError::from)
                }) {
                    Ok(id) => Some(id),
                    Err(e) => {
                        remove_copy(ssh, &sess.host_alias, &new_path, &new_id, sess.id).await;
                        undo_new_worktree(ops, store, &sess.host_alias, wt, None).await;
                        return Err(e);
                    }
                },
            };
            let spawned = ops
                .spawn(crate::service::sessions::NewSessionArgs {
                    host_alias: sess.host_alias.clone(),
                    project_id,
                    worktree_id,
                    name: String::new(),
                    call_id: None,
                    new_worktree: None,
                    base_branch: None,
                    kind: None,
                    start_command: None,
                    friendly_name: None,
                    resume_claude_session_id: Some(new_id.clone()),
                    model: launch.model.clone(),
                    effort: launch.effort.clone(),
                    profile: launch.profile.clone(),
                    agent: None,
                    origin: None,
                    // Multi-user M1 (T5, spec §4.3 invariant 6): a fork
                    // inherits the SOURCE's owner, never the forker's. The
                    // fork is a permanent verbatim copy of the source's
                    // transcript, so an owner whose session was forked by
                    // somebody else would have no way to revoke it — and
                    // `RewindArgs` carries no forker identity anyway, so
                    // source-inheritance is both the safe answer and the only
                    // expressible one. Forking a session you do not own is
                    // refused at the gate: it is an `own` operation.
                    over_limit_ok: false,
                    owner_person_id: sess.owner_person_id,
                    start_token: None,
                })
                .await;
            // A failed start leaves nothing a new-worktree fork made: the
            // copy, the tree and its branch, and the row (only if no session
            // row came to point at it). A same-worktree fork made only the
            // copy, which stays as it always has — beside the source, where
            // its conversation id alone names it.
            let row = match spawned {
                Ok(row) => row,
                Err(e) => {
                    // The tree goes first: a start can fail after its pane
                    // came up, and a pane still working in the tree keeps
                    // the tree AND the copy it resumed.
                    if let Some(wt) = &new_wt {
                        if undo_new_worktree(ops, store, &sess.host_alias, wt, worktree_id).await {
                            remove_copy(ssh, &sess.host_alias, &new_path, &new_id, sess.id).await;
                        }
                    }
                    return Err(e);
                }
            };
            // `new_session` records the resumed id through
            // `set_claude_session_id`, which is `rebind_conversation(...,
            // StartSource::Fleet, ...)`: the forked conversation would be
            // labelled `fleet`, not `fork`, and `Fleet` also
            // `resets_context()` — so the new row would report 0 context for a
            // transcript it inherited whole. Relabel it, with the path the
            // script actually wrote. Best-effort: the spawn has succeeded and
            // its row is what the caller asked for; a label is not worth
            // failing it.
            // A fork does the same work, as `move_session { keep_source }`'s
            // does: the source's confirmed links come along (source
            // `forked`). Best-effort, like the label below.
            if let Err(e) = lock(store).and_then(|s| s.copy_work_links(sess.id, row.id)) {
                tracing::warn!(
                    session_id = row.id,
                    error = %e.message,
                    "[rewind] copying the work links to the fork failed"
                );
            }
            match lock(store).and_then(|s| {
                s.relabel_conversation(row.id, &new_id, StartSource::Fork, Some(&new_path))
            }) {
                Ok(Some(relabelled)) => Ok(relabelled),
                Ok(None) => Ok(row),
                Err(e) => {
                    tracing::warn!(
                        session_id = row.id,
                        error = %e.message,
                        "[rewind] labelling the forked conversation failed"
                    );
                    Ok(row)
                }
            }
        }
    }
}

/// A fork's freshly created worktree, carried through the steps after it so
/// any failure can undo it ([`undo_new_worktree`]).
struct NewWorktree {
    /// The branch; the row and directory are `projects::worktree_dir_name` of it.
    name: String,
    /// Physical path on the host (`pwd -P`).
    path: String,
    project_id: i64,
    /// `~/.claude/projects` on the host.
    projects_dir: String,
}

/// A string's JSON-escaped body, without the surrounding quotes: how a value
/// is spelled inside a transcript line.
fn json_string_body(v: &str) -> String {
    let q = serde_json::to_string(v).unwrap_or_default();
    q.get(1..q.len().saturating_sub(1))
        .unwrap_or("")
        .to_string()
}

/// Run [`rewind_script`] and return the copy's path, mapping the script's
/// sentinels to codes rather than letting prose leak out, exactly as
/// `read_tail` maps NO_TRANSCRIPT. `run_shell` handles the SSH quoting
/// round-trip.
async fn copy_transcript(
    ssh: &Arc<SshClient>,
    host_alias: &str,
    script: &str,
) -> Result<String, IpcError> {
    let out = run_shell(ssh, host_alias, script).await?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        if stderr.contains(NO_ANCHOR) {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                "that turn is no longer in this conversation's transcript — the conversation was rewound (or compacted) past it since it was loaded; reload the conversation and try again",
            ));
        }
        if stderr.contains(NO_TURNS) {
            return Err(IpcError::new(
                codes::E_INVALID,
                "there is nothing before this turn to rewind to; use /clear to start fresh",
            ));
        }
        if stderr.contains(crate::service::transcript::NO_TRANSCRIPT) {
            return Err(IpcError::new(
                codes::E_NO_TRANSCRIPT,
                format!("no transcript for this session on {host_alias}"),
            ));
        }
        return Err(IpcError::new(
            codes::E_SHELL,
            format!("rewind failed: {}", stderr.trim()),
        ));
    }
    copy_path_from_stdout(&out.stdout)
}

/// The copy's path from [`rewind_script`]'s stdout: its LAST non-empty line.
/// The script runs under `bash -lc`, so a login banner prints ahead of it;
/// taken whole, the banner became part of the transcript path the session
/// was rebound to, and the cleanup's `rm` named a file that does not exist.
fn copy_path_from_stdout(stdout: &[u8]) -> Result<String, IpcError> {
    let text = String::from_utf8_lossy(stdout);
    let path = text
        .lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty())
        .unwrap_or_default();
    if !path.ends_with(".jsonl") {
        return Err(IpcError::new(
            codes::E_SHELL,
            format!("rewind did not report the copy's path: {path:?}"),
        ));
    }
    Ok(path.to_string())
}

/// Remove the transcript copy this call wrote, after a later step failed.
/// Only ever that file: the script prints `<dest dir>/<new id>.jsonl`, and
/// anything else on stdout is not a path this code is willing to delete.
/// Best-effort: the step's own error is what the caller must see.
async fn remove_copy(
    ssh: &Arc<SshClient>,
    host_alias: &str,
    new_path: &str,
    new_id: &str,
    session_id: i64,
) {
    if new_path.contains('\n') || !new_path.ends_with(&format!("/{new_id}.jsonl")) {
        return;
    }
    let rm = format!("rm -f -- {}", quote(new_path));
    match run_shell(ssh, host_alias, &rm).await {
        Ok(o) if o.status.success() => {}
        Ok(o) => tracing::warn!(
            session_id,
            stderr = %String::from_utf8_lossy(&o.stderr).trim(),
            "[rewind] removing the unused transcript copy failed"
        ),
        Err(re) => tracing::warn!(
            session_id,
            error = %re.message,
            "[rewind] removing the unused transcript copy failed"
        ),
    }
}

/// Undo a fork's new worktree after a later step failed: the tree and its
/// branch (safe removal only, [`remove_fork_worktree_script`]), then its row
/// when one was recorded and no session row points at it. The row goes only
/// once the tree has; a tree that refused to go keeps the row that names it.
/// Best-effort, like [`remove_copy`]; true when the tree went.
async fn undo_new_worktree(
    ops: &dyn ReplyOps,
    store: &Mutex<Store>,
    host_alias: &str,
    wt: &NewWorktree,
    worktree_id: Option<i64>,
) -> bool {
    if let Err(e) = ops.remove_worktree(host_alias, &wt.path, &wt.name).await {
        tracing::warn!(
            path = %wt.path,
            error = %e.message,
            "[rewind] removing the fork's new worktree failed"
        );
        return false;
    }
    if let Some(id) = worktree_id {
        if let Err(e) =
            lock(store).and_then(|s| s.delete_worktree_if_unused(id).map_err(IpcError::from))
        {
            tracing::warn!(
                worktree_id = id,
                error = %e.message,
                "[rewind] dropping the fork's worktree row failed"
            );
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_login_banner_never_becomes_the_copy_s_path() {
        assert_eq!(
            copy_path_from_stdout(b"Welcome\n/h/.claude/projects/p/abc.jsonl\n").unwrap(),
            "/h/.claude/projects/p/abc.jsonl"
        );
        assert_eq!(
            copy_path_from_stdout(b"Welcome\n").unwrap_err().code,
            codes::E_SHELL
        );
        assert!(copy_path_from_stdout(b"").is_err());
    }

    /// Run a generated script against a real file in a temp dir. `bash` in a
    /// test is established here (`crate::shell::tests`, `crate::tmux::tests`).
    #[cfg(unix)]
    fn run(script: &str) -> std::process::Output {
        std::process::Command::new("bash")
            .arg("-c")
            .arg(script)
            .output()
            .expect("bash must be available")
    }

    const OLD: &str = "11111111-1111-1111-1111-111111111111";
    const NEW: &str = "22222222-2222-2222-2222-222222222222";
    const A1: &str = "aaaaaaaa-0000-0000-0000-000000000001";
    const A2: &str = "aaaaaaaa-0000-0000-0000-000000000002";

    #[cfg(unix)]
    fn fixture(dir: &std::path::Path) -> std::path::PathBuf {
        // Leading metadata lines carry no uuid and MUST survive: they are the
        // transcript's header (`custom-title`, `mode`, …).
        let body = format!(
            concat!(
                r#"{{"type":"mode","sessionId":"{old}"}}"#,
                "\n",
                r#"{{"type":"user","uuid":"{a1}","sessionId":"{old}","cwd":"/src/app","message":{{"role":"user","content":"one"}}}}"#,
                "\n",
                r#"{{"type":"assistant","uuid":"bbbb","sessionId":"{old}","cwd":"/src/app","message":{{"role":"assistant","content":[]}}}}"#,
                "\n",
                r#"{{"type":"user","uuid":"{a2}","sessionId":"{old}","cwd":"/src/app","message":{{"role":"user","content":"two"}}}}"#,
                "\n",
                r#"{{"type":"assistant","uuid":"cccc","sessionId":"{old}","cwd":"/src/app","message":{{"role":"assistant","content":[]}}}}"#,
                "\n",
            ),
            old = OLD,
            a1 = A1,
            a2 = A2
        );
        let p = dir.join(format!("{OLD}.jsonl"));
        std::fs::write(&p, body).unwrap();
        p
    }

    #[cfg(unix)]
    fn tmp() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("cf-rewind-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A fixture built to trap a `gsub`-based (rather than `rep()`'s
    /// `index`-based literal) rewrite: `/src/app.old` read as a REGEX also
    /// matches `/src/appXold`, because `.` means "any character". A buggy
    /// `gsub` implementation would clobber both lines; `rep()` must touch
    /// only the one that is an exact literal match.
    #[cfg(unix)]
    fn fixture_with_gsub_trap(dir: &std::path::Path) -> std::path::PathBuf {
        let body = format!(
            concat!(
                r#"{{"type":"mode","sessionId":"{old}"}}"#,
                "\n",
                r#"{{"type":"user","uuid":"{a1}","sessionId":"{old}","cwd":"/src/app.old","message":{{"role":"user","content":"one"}}}}"#,
                "\n",
                r#"{{"type":"assistant","uuid":"bbbb","sessionId":"{old}","cwd":"/src/appXold","message":{{"role":"assistant","content":[]}}}}"#,
                "\n",
                r#"{{"type":"user","uuid":"{a2}","sessionId":"{old}","cwd":"/src/app.old","message":{{"role":"user","content":"two"}}}}"#,
                "\n",
                r#"{{"type":"assistant","uuid":"cccc","sessionId":"{old}","cwd":"/src/app.old","message":{{"role":"assistant","content":[]}}}}"#,
                "\n",
            ),
            old = OLD,
            a1 = A1,
            a2 = A2
        );
        let p = dir.join(format!("{OLD}.jsonl"));
        std::fs::write(&p, body).unwrap();
        p
    }

    #[cfg(unix)]
    #[test]
    fn keeps_everything_strictly_before_the_anchor() {
        let d = tmp();
        let src = fixture(&d);
        let out = run(&rewind_script(
            None,
            Some(src.to_str().unwrap()),
            None,
            OLD,
            NEW,
            Some(A2),
            None,
            None,
        ));
        assert!(
            out.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let written = std::fs::read_to_string(d.join(format!("{NEW}.jsonl"))).unwrap();
        assert_eq!(
            written.lines().count(),
            3,
            "header + turn 1's two entries, and NOT the anchor line"
        );
        assert!(written.contains(A1), "the turn before the anchor is kept");
        assert!(
            !written.contains(A2),
            "the anchor line itself is dropped — 'strictly before'"
        );
        assert!(
            !written.contains("cccc"),
            "nothing after the anchor survives"
        );
        std::fs::remove_dir_all(&d).ok();
    }

    #[cfg(unix)]
    #[test]
    fn rewrites_the_session_id_on_every_copied_line() {
        let d = tmp();
        let src = fixture(&d);
        let out = run(&rewind_script(
            None,
            Some(src.to_str().unwrap()),
            None,
            OLD,
            NEW,
            Some(A2),
            None,
            None,
        ));
        assert!(out.status.success());
        let written = std::fs::read_to_string(d.join(format!("{NEW}.jsonl"))).unwrap();
        assert!(
            !written.contains(OLD),
            "no copied line may still name the old conversation"
        );
        assert_eq!(written.lines().filter(|l| l.contains(NEW)).count(), 3);
        std::fs::remove_dir_all(&d).ok();
    }

    /// A fork mid-turn: Claude is still appending, so the last line has no
    /// newline yet. It is left out rather than copied as broken JSON.
    #[cfg(unix)]
    #[test]
    fn an_unterminated_last_line_is_not_copied() {
        let d = tmp();
        let src = fixture(&d);
        let mut f = std::fs::OpenOptions::new().append(true).open(&src).unwrap();
        std::io::Write::write_all(&mut f, br#"{"type":"assistant","uuid":"dddd","mess"#).unwrap();
        let out = run(&rewind_script(
            None,
            Some(src.to_str().unwrap()),
            None,
            OLD,
            NEW,
            None,
            None,
            None,
        ));
        assert!(
            out.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let written = std::fs::read_to_string(d.join(format!("{NEW}.jsonl"))).unwrap();
        assert!(!written.contains("dddd"), "the partial line stays behind");
        assert!(written.contains("cccc"), "every whole line is copied");
        assert!(written.ends_with('\n'));
        std::fs::remove_dir_all(&d).ok();
    }

    #[cfg(unix)]
    #[test]
    fn no_anchor_copies_the_whole_file() {
        // Fork on the LAST turn: there is no later prompt, so "keep
        // everything" is the correct meaning, not an error.
        let d = tmp();
        let src = fixture(&d);
        let out = run(&rewind_script(
            None,
            Some(src.to_str().unwrap()),
            None,
            OLD,
            NEW,
            None,
            None,
            None,
        ));
        assert!(out.status.success());
        let written = std::fs::read_to_string(d.join(format!("{NEW}.jsonl"))).unwrap();
        assert_eq!(written.lines().count(), 5);
        std::fs::remove_dir_all(&d).ok();
    }

    #[cfg(unix)]
    #[test]
    fn an_anchor_that_is_not_in_the_file_fails_with_the_sentinel() {
        let d = tmp();
        let src = fixture(&d);
        let out = run(&rewind_script(
            None,
            Some(src.to_str().unwrap()),
            None,
            OLD,
            NEW,
            Some("dddddddd-0000-0000-0000-000000000009"),
            None,
            None,
        ));
        assert_eq!(out.status.code(), Some(3));
        assert!(String::from_utf8_lossy(&out.stderr).contains(NO_ANCHOR));
        assert!(
            !d.join(format!("{NEW}.jsonl")).exists(),
            "a failed truncation leaves no half-written transcript"
        );
        std::fs::remove_dir_all(&d).ok();
    }

    #[cfg(unix)]
    #[test]
    fn a_cross_worktree_fork_rewrites_cwd_literally() {
        // `/src/app.old` read as a REGEX also matches `/src/appXold` (`.`
        // means "any character"), so a `gsub`-based rewrite would clobber
        // both lines. `rep()` (index-based, literal) must touch only the
        // exact match — this fixture is what actually discriminates the
        // two implementations; a plain `/src/app` fixture would pass under
        // either.
        let d = tmp();
        let src = fixture_with_gsub_trap(&d);
        let dest = d.join("elsewhere");
        let out = run(&rewind_script(
            None,
            Some(src.to_str().unwrap()),
            None,
            OLD,
            NEW,
            Some(A2),
            Some(dest.to_str().unwrap()),
            Some(("/src/app.old", "/src/app.new-fork")),
        ));
        assert!(
            out.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let written = std::fs::read_to_string(dest.join(format!("{NEW}.jsonl"))).unwrap();
        assert!(
            written.contains(r#""cwd":"/src/app.new-fork""#),
            "the exact literal match is rewritten: {written}"
        );
        assert!(
            !written.contains(r#""cwd":"/src/app.old""#),
            "no copied line may still carry the un-rewritten old cwd: {written}"
        );
        assert!(
            written.contains(r#""cwd":"/src/appXold""#),
            "a `gsub` would treat '.' as 'any char' and clobber this line too; \
             rep() must leave it untouched: {written}"
        );
        std::fs::remove_dir_all(&d).ok();
    }

    /// I1 / spec §4.3: rewinding to the FIRST turn leaves nothing but the
    /// transcript's metadata header — a conversation with no entries at all,
    /// which is `/clear` under a misleading label. The engine is the only
    /// layer that can know, so the refusal is a script sentinel, not a
    /// client-side gate.
    #[cfg(unix)]
    #[test]
    fn a_prefix_with_no_conversation_entry_fails_with_its_own_sentinel() {
        let d = tmp();
        let src = fixture(&d);
        let out = run(&rewind_script(
            None,
            Some(src.to_str().unwrap()),
            None,
            OLD,
            NEW,
            Some(A1), // the first turn: only the header survives
            None,
            None,
        ));
        assert_eq!(out.status.code(), Some(5));
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains(NO_TURNS), "stderr: {stderr}");
        assert!(
            !stderr.contains(NO_ANCHOR),
            "the anchor WAS found; this is a different refusal: {stderr}"
        );
        assert!(
            !d.join(format!("{NEW}.jsonl")).exists(),
            "a refused truncation leaves no half-written transcript"
        );
        std::fs::remove_dir_all(&d).ok();
    }

    /// The counterpart: a prefix that DOES hold a turn is written, so the
    /// sentinel above cannot be firing on the ordinary case.
    #[cfg(unix)]
    #[test]
    fn a_prefix_with_one_turn_is_written() {
        let d = tmp();
        let src = fixture(&d);
        let out = run(&rewind_script(
            None,
            Some(src.to_str().unwrap()),
            None,
            OLD,
            NEW,
            Some(A2),
            None,
            None,
        ));
        assert!(out.status.success());
        assert!(!String::from_utf8_lossy(&out.stderr).contains(NO_TURNS));
        assert!(d.join(format!("{NEW}.jsonl")).exists());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn every_interpolated_value_is_shell_quoted() {
        let s = rewind_script(
            None,
            Some("/tmp/a b/t.jsonl"),
            None,
            OLD,
            NEW,
            Some(A1),
            None,
            None,
        );
        assert!(
            s.contains("'/tmp/a b/t.jsonl'"),
            "paths with spaces must arrive quoted: {s}"
        );
    }

    #[test]
    fn hostile_values_in_every_argument_this_function_owns_are_shell_quoted() {
        // `every_interpolated_value_is_shell_quoted` above only exercises
        // `stored_path`, which `locate_script` already owned and quoted
        // before this task existed. This test covers the six values
        // `rewind_script` itself interpolates: `new_id`, `anchor_uuid`,
        // `dest_dir`, both halves of `cwd_rewrite`, and `claude_session_id`
        // (embedded a second time here, into `-v oldid=...`, separately
        // from locate_script's own use of it). None of these is validated
        // at this layer — no caller exists yet (that's Task 3) — so a
        // hostile value (a space, an embedded single quote) is a legitimate
        // input to test here.
        let claude_session_id = "sess d'e";
        let new_id = "a b'c";
        let anchor = "anchor f'g";
        let dest_dir = "/tmp/dest h'i";
        let oldcwd = "/old j'k";
        let newcwd = "/new l'm";
        let s = rewind_script(
            None,
            Some("/tmp/t.jsonl"),
            None,
            claude_session_id,
            new_id,
            Some(anchor),
            Some(dest_dir),
            Some((oldcwd, newcwd)),
        );
        for raw in [claude_session_id, new_id, anchor, dest_dir, oldcwd, newcwd] {
            let quoted = crate::shell::quote(raw);
            assert!(
                s.contains(&quoted),
                "expected the shell-quoted form of {raw:?} (i.e. {quoted}) \
                 somewhere in the generated script, found none:\n{s}"
            );
        }
    }

    // ── `rewind_conversation` ──────────────────────────────────────────
    //
    // Helper/plumbing note: the plan this task followed guessed
    // `SshClient::for_tests()` and `Store::set_claude_status`; neither
    // exists. `SshClient::new()` is the real no-agent-routing constructor
    // (see the many `Arc::new(SshClient::new())` call sites in
    // `transcript.rs`'s own async tests), and the real per-id status setter
    // is the `#[cfg(test)]` helper `Store::set_session_claude_status_for_test`
    // in `store/orgs.rs`. `Store` and `StartSource` are already in scope via
    // `use super::*` (this module's own top-level `use`), so no extra import
    // is needed here.

    /// The bare session row, with NO Claude conversation bound yet — the
    /// "not reconciled" case `rewind_conversation`'s own claude-id guard
    /// must refuse before any SSH happens. `store_with_session` below adds
    /// the conversation on top rather than duplicating this.
    fn new_test_session(status: &str) -> (Store, i64) {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h1").unwrap();
        let id = s
            .upsert_session("sess", "h1", None, None, 0, 0, status, None)
            .unwrap();
        (s, id)
    }

    fn store_with_session(status: &str, claude_status: Option<&str>) -> (Store, i64) {
        let (s, id) = new_test_session(status);
        s.set_claude_session_id(id, OLD).unwrap();
        if let Some(cs) = claude_status {
            s.set_session_claude_status_for_test(id, cs);
        }
        (s, id)
    }

    #[tokio::test]
    async fn rewinding_a_working_session_is_refused_before_anything_runs() {
        let (s, id) = store_with_session("running", Some("working"));
        let store = std::sync::Mutex::new(s);
        let ops = FakeOps::new(&store, None);
        let err = rewind_conversation_with(
            RewindArgs {
                session_id: id,
                anchor_uuid: Some(A2.into()),
                mode: RewindMode::Rewind,
                new_worktree: None,
            },
            &store,
            &std::sync::Arc::new(SshClient::new()),
            &ops,
        )
        .await
        .expect_err("a mid-turn restart throws the turn away");
        assert_eq!(ops.restarts(), 0);
        assert_eq!(err.code, codes::E_INVALID);
        assert!(
            err.message.contains("interrupt"),
            "the refusal must say what to do about it: {}",
            err.message
        );
    }

    #[tokio::test]
    async fn forking_a_working_session_is_allowed() {
        // Fork touches nothing live, so the working-session guard must NOT
        // apply to it. Asserted on the refusal, not on success: the spawn
        // itself needs a host.
        let (s, id) = store_with_session("running", Some("working"));
        let err = rewind_conversation(
            RewindArgs {
                session_id: id,
                anchor_uuid: Some(A2.into()),
                mode: RewindMode::Fork,
                new_worktree: None,
            },
            &std::sync::Mutex::new(s),
            &std::sync::Arc::new(SshClient::new()),
            &CancellationRegistry::new(),
        )
        .await
        .expect_err("unreachable host");
        assert_ne!(
            err.code,
            codes::E_INVALID,
            "fork must not be refused for being busy; it failed for another reason: {err:?}"
        );
    }

    #[tokio::test]
    async fn an_unknown_session_is_not_found() {
        let s = Store::open_in_memory().unwrap();
        let err = rewind_conversation(
            RewindArgs {
                session_id: 9999,
                anchor_uuid: Some(A2.into()),
                mode: RewindMode::Rewind,
                new_worktree: None,
            },
            &std::sync::Mutex::new(s),
            &std::sync::Arc::new(SshClient::new()),
            &CancellationRegistry::new(),
        )
        .await
        .expect_err("no such session");
        assert_eq!(err.code, codes::E_NOTFOUND);
    }

    #[test]
    fn the_new_conversation_id_is_a_fresh_uuid_each_time() {
        assert_ne!(mint_conversation_id(), mint_conversation_id());
        assert!(crate::validate::claude_session_id(&mint_conversation_id()).is_ok());
    }

    #[tokio::test]
    async fn a_session_with_no_claude_conversation_is_refused() {
        // Nothing has bound a Claude conversation to this row yet (not
        // reconciled, or not a Claude session at all) — refused before any
        // SSH happens, same shape as the mid-turn guard above.
        let (s, id) = new_test_session("running");
        let err = rewind_conversation(
            RewindArgs {
                session_id: id,
                anchor_uuid: Some(A2.into()),
                mode: RewindMode::Rewind,
                new_worktree: None,
            },
            &std::sync::Mutex::new(s),
            &std::sync::Arc::new(SshClient::new()),
            &CancellationRegistry::new(),
        )
        .await
        .expect_err("no claude_session_id bound yet");
        assert_eq!(err.code, codes::E_INVALID);
        assert!(
            err.message.contains("no Claude conversation"),
            "{}",
            err.message
        );
    }

    /// C2: `restart_session` runs `tmux_name_addressable` and
    /// `guard_not_controller` inside itself — i.e. AFTER a rewind has already
    /// rebound the row. A `bg:` row has no pane to restart, so the refusal
    /// must land BEFORE the rebind, or the row permanently reads a frozen copy
    /// while the live Claude keeps writing the original transcript.
    #[tokio::test]
    async fn rewinding_a_pane_less_session_is_refused_before_the_rebind() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h1").unwrap();
        let id = s
            .upsert_session("bg:deadbeef", "h1", None, None, 0, 0, "running", None)
            .unwrap();
        s.set_claude_session_id(id, OLD).unwrap();
        let store = std::sync::Mutex::new(s);
        let err = rewind_conversation(
            RewindArgs {
                session_id: id,
                anchor_uuid: Some(A2.into()),
                mode: RewindMode::Rewind,
                new_worktree: None,
            },
            &store,
            &std::sync::Arc::new(SshClient::new()),
            &CancellationRegistry::new(),
        )
        .await
        .expect_err("a bg row has no pane to restart");
        assert_eq!(err.code, codes::E_BG_SESSION);
        assert_eq!(
            store
                .lock()
                .unwrap()
                .get_session_by_id(id)
                .unwrap()
                .unwrap()
                .claude_session_id
                .as_deref(),
            Some(OLD),
            "the row must still name the original conversation"
        );
    }

    /// C2, the controller half: the same refusal `restart_session` would make
    /// one commit too late.
    #[tokio::test]
    async fn rewinding_the_controller_is_refused_before_the_rebind() {
        let (s, id) = store_with_session("running", Some("idle"));
        s.set_controller("h1", "sess").unwrap();
        let store = std::sync::Mutex::new(s);
        let err = rewind_conversation(
            RewindArgs {
                session_id: id,
                anchor_uuid: Some(A2.into()),
                mode: RewindMode::Rewind,
                new_worktree: None,
            },
            &store,
            &std::sync::Arc::new(SshClient::new()),
            &CancellationRegistry::new(),
        )
        .await
        .expect_err("the controller must not restart itself");
        assert_eq!(err.code, codes::E_SELF_TARGET);
        assert_eq!(
            store
                .lock()
                .unwrap()
                .get_session_by_id(id)
                .unwrap()
                .unwrap()
                .claude_session_id
                .as_deref(),
            Some(OLD),
        );
        // Fork leaves the controller's pane alone, so it must NOT be refused
        // for being the controller.
        let err = rewind_conversation(
            RewindArgs {
                session_id: id,
                anchor_uuid: Some(A2.into()),
                mode: RewindMode::Fork,
                new_worktree: None,
            },
            &store,
            &std::sync::Arc::new(SshClient::new()),
            &CancellationRegistry::new(),
        )
        .await
        .expect_err("unreachable host");
        assert_ne!(err.code, codes::E_SELF_TARGET);
    }

    /// M4: the id reaches `awk -v oldid=...`, where awk performs escape
    /// processing on the value. Every other interpolation of this field
    /// validates it first (`recreate_pane_command`, `resolve_args_for`).
    #[tokio::test]
    async fn a_non_uuid_claude_id_is_refused_before_it_reaches_awk() {
        let (s, id) = new_test_session("running");
        s.set_claude_session_id(id, "not a uuid\\x41").unwrap();
        let err = rewind_conversation(
            RewindArgs {
                session_id: id,
                anchor_uuid: None,
                mode: RewindMode::Fork,
                new_worktree: None,
            },
            &std::sync::Mutex::new(s),
            &std::sync::Arc::new(SshClient::new()),
            &CancellationRegistry::new(),
        )
        .await
        .expect_err("the id is interpolated into an awk -v assignment");
        assert_eq!(err.code, codes::E_INVALID);
        assert!(err.message.contains("not a UUID"), "{}", err.message);
    }

    #[test]
    fn forking_without_a_project_is_refused() {
        assert_eq!(
            project_id_for_fork(None).unwrap_err().code,
            codes::E_INVALID_STATE
        );
        assert_eq!(project_id_for_fork(Some(7)).unwrap(), 7);
    }

    #[test]
    fn a_new_worktree_fork_rewrites_cwd_and_targets_the_new_project_dir() {
        // The pane must start where the transcript says it did, or
        // `cl --resume` will not find the conversation (see the constraint on
        // NewSessionArgs::resume_claude_session_id). So a cross-worktree fork
        // writes into the NEW cwd's encoded project dir with `cwd` rewritten.
        let enc = crate::service::transcript::encode_project_dir("/src/app-fork");
        assert_eq!(enc, "-src-app-fork");
        let s = rewind_script(
            None,
            Some("/t/x.jsonl"),
            None,
            OLD,
            NEW,
            Some(A2),
            Some(&format!("/home/u/.claude/projects/{enc}")),
            Some(("/src/app", "/src/app-fork")),
        );
        assert!(
            s.contains(&format!("projects/{enc}")),
            "dest dir must be the new cwd's: {s}"
        );
        assert!(s.contains("'/src/app-fork'"));
    }

    // ── forking into a NEW worktree: the checks before any I/O ─────────

    #[tokio::test]
    async fn a_new_worktree_is_a_fork_only_shape_with_a_valid_name() {
        for (mode, name) in [
            (RewindMode::Rewind, "fork-of-canopus"),
            (RewindMode::Fork, "main"),
            (RewindMode::Fork, "-x"),
            (RewindMode::Fork, ""),
        ] {
            let (s, id) = store_with_session("running", Some("idle"));
            let err = rewind_conversation(
                RewindArgs {
                    session_id: id,
                    anchor_uuid: Some(A2.into()),
                    mode,
                    new_worktree: Some(name.into()),
                },
                &std::sync::Mutex::new(s),
                &std::sync::Arc::new(SshClient::new()),
                &CancellationRegistry::new(),
            )
            .await
            .expect_err("refused before any I/O");
            assert_eq!(
                err.code,
                codes::E_INVALID,
                "{mode:?} {name:?}: {}",
                err.message
            );
        }
    }

    /// Before this build a new-worktree fork answered `E_UNSUPPORTED`; now
    /// it reaches the engine, whose first check (a project to create the
    /// worktree in) refuses this row before any I/O.
    #[tokio::test]
    async fn forking_into_a_new_worktree_is_no_longer_unsupported() {
        let (s, id) = store_with_session("running", Some("idle"));
        let err = rewind_conversation(
            RewindArgs {
                session_id: id,
                anchor_uuid: Some(A2.into()),
                mode: RewindMode::Fork,
                new_worktree: Some("fork-of-canopus".into()),
            },
            &std::sync::Mutex::new(s),
            &std::sync::Arc::new(SshClient::new()),
            &CancellationRegistry::new(),
        )
        .await
        .expect_err("this row has no project");
        assert_eq!(err.code, codes::E_INVALID_STATE, "{}", err.message);
    }

    // ── the anchor is the TOP-LEVEL uuid ───────────────────────────────

    /// A tool result (or any nested object) can carry a `"uuid"` of its own,
    /// and the anchor also appears as the NEXT entry's `parentUuid`. Neither
    /// may stop the copy: only the entry whose own `uuid` is the anchor does.
    /// And the key is found with whitespace around its `:`.
    #[cfg(unix)]
    #[test]
    fn only_a_top_level_uuid_is_the_anchor_and_whitespace_is_tolerated() {
        let d = tmp();
        let body = format!(
            concat!(
                r#"{{"type":"mode","sessionId":"{old}"}}"#,
                "\n",
                r#"{{"type":"user","uuid":"{a1}","sessionId":"{old}","message":{{"role":"user","content":"one"}}}}"#,
                "\n",
                // A nested object whose "uuid" IS the anchor: not the entry.
                r#"{{"type":"user","uuid":"bbbb","sessionId":"{old}","toolUseResult":{{"uuid":"{a2}","s":"a \"quoted\" {{ brace"}},"message":{{"role":"user","content":[{{"type":"tool_result"}}]}}}}"#,
                "\n",
                // A string VALUE equal to "uuid" followed by the anchor text.
                r#"{{"type":"assistant","uuid":"cccc","note":"uuid","parentUuid":"{a2}","sessionId":"{old}","message":{{"role":"assistant","content":[]}}}}"#,
                "\n",
                // The real anchor, pretty-spaced.
                r#"{{"type": "user", "uuid" : "{a2}", "sessionId":"{old}","message":{{"role":"user","content":"two"}}}}"#,
                "\n",
                r#"{{"type":"assistant","uuid":"dddd","sessionId":"{old}","message":{{"role":"assistant","content":[]}}}}"#,
                "\n",
            ),
            old = OLD,
            a1 = A1,
            a2 = A2
        );
        let src = d.join(format!("{OLD}.jsonl"));
        std::fs::write(&src, body).unwrap();
        let out = run(&rewind_script(
            None,
            Some(src.to_str().unwrap()),
            None,
            OLD,
            NEW,
            Some(A2),
            None,
            None,
        ));
        assert!(
            out.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let written = std::fs::read_to_string(d.join(format!("{NEW}.jsonl"))).unwrap();
        assert_eq!(
            written.lines().count(),
            4,
            "header, turn one, the nested-uuid line and the parentUuid line are \
             all BEFORE the real anchor: {written}"
        );
        assert!(written.contains("toolUseResult"));
        assert!(written.contains("cccc"));
        assert!(
            !written.contains("dddd"),
            "nothing after the anchor survives"
        );
        std::fs::remove_dir_all(&d).ok();
    }

    /// A nested `"uuid"` alone is not the anchor: the script must report it
    /// missing rather than cut there.
    #[cfg(unix)]
    #[test]
    fn a_nested_uuid_alone_is_not_found() {
        let d = tmp();
        let body = format!(
            concat!(
                r#"{{"type":"user","uuid":"{a1}","sessionId":"{old}","message":{{"role":"user","content":"one"}}}}"#,
                "\n",
                r#"{{"type":"user","uuid":"bbbb","sessionId":"{old}","toolUseResult":{{"uuid":"{a2}"}}}}"#,
                "\n",
            ),
            old = OLD,
            a1 = A1,
            a2 = A2
        );
        let src = d.join(format!("{OLD}.jsonl"));
        std::fs::write(&src, body).unwrap();
        let out = run(&rewind_script(
            None,
            Some(src.to_str().unwrap()),
            None,
            OLD,
            NEW,
            Some(A2),
            None,
            None,
        ));
        assert_eq!(out.status.code(), Some(3));
        assert!(String::from_utf8_lossy(&out.stderr).contains(NO_ANCHOR));
        std::fs::remove_dir_all(&d).ok();
    }

    // ── the service, over a fake for its follow-ups (spec §7) ──────────

    /// Records every follow-up `rewind_conversation` makes; answers the pane
    /// probe with `status`, the restart with `restart_err` (else the row),
    /// and a spawn by creating a real row the way `new_session` would
    /// (`set_claude_session_id` = a `Fleet` rebind), so the relabel after it
    /// has something to act on.
    struct FakeOps<'a> {
        store: &'a std::sync::Mutex<Store>,
        status: Option<String>,
        restart_err: Option<IpcError>,
        restarts: std::sync::Mutex<Vec<(String, String)>>,
        spawns: std::sync::Mutex<Vec<Option<String>>>,
        /// Each spawn's `(model, effort)`.
        launches: std::sync::Mutex<Vec<(Option<String>, Option<String>)>>,
        /// Where `add_worktree` makes its tree (`<root>/wt/<name>`) and
        /// `claude_projects_dir` points (`<root>/projects`).
        root: Option<std::path::PathBuf>,
        add_err: Option<IpcError>,
        spawn_err: Option<IpcError>,
        /// `remove_worktree` refuses (as the script does for a tree a live
        /// pane is in) and leaves the tree.
        remove_err: Option<IpcError>,
        /// Every worktree step and spawn, in order: `add:<name>`,
        /// `spawn:<worktree_id>`, `remove:<path>`.
        log: std::sync::Mutex<Vec<String>>,
    }

    impl<'a> FakeOps<'a> {
        fn new(store: &'a std::sync::Mutex<Store>, status: Option<&str>) -> Self {
            Self {
                store,
                status: status.map(String::from),
                restart_err: None,
                restarts: Default::default(),
                spawns: Default::default(),
                launches: Default::default(),
                root: None,
                add_err: None,
                remove_err: None,
                spawn_err: None,
                log: Default::default(),
            }
        }
        #[cfg(unix)]
        fn log(&self) -> Vec<String> {
            self.log.lock().unwrap().clone()
        }
        fn restarts(&self) -> usize {
            self.restarts.lock().unwrap().len()
        }
        #[cfg(unix)]
        fn spawns(&self) -> usize {
            self.spawns.lock().unwrap().len()
        }
    }

    #[async_trait::async_trait]
    impl ReplyOps for FakeOps<'_> {
        async fn pane_status(&self, _session_id: i64) -> Option<String> {
            self.status.clone()
        }
        async fn restart(
            &self,
            args: crate::service::sessions::RestartSessionArgs,
        ) -> Result<SessionRow, IpcError> {
            self.restarts
                .lock()
                .unwrap()
                .push((args.host_alias.clone(), args.name.clone()));
            if let Some(e) = &self.restart_err {
                return Err(IpcError::new(&e.code, e.message.clone()));
            }
            let s = self.store.lock().unwrap();
            Ok(s.get_session(&args.name, &args.host_alias)
                .unwrap()
                .unwrap())
        }
        async fn spawn(
            &self,
            args: crate::service::sessions::NewSessionArgs,
        ) -> Result<SessionRow, IpcError> {
            self.spawns
                .lock()
                .unwrap()
                .push(args.resume_claude_session_id.clone());
            self.launches
                .lock()
                .unwrap()
                .push((args.model.clone(), args.effort.clone()));
            self.log
                .lock()
                .unwrap()
                .push(format!("spawn:{:?}", args.worktree_id));
            if let Some(e) = &self.spawn_err {
                return Err(IpcError::new(&e.code, e.message.clone()));
            }
            let s = self.store.lock().unwrap();
            let id = s
                .upsert_session(
                    "forked",
                    &args.host_alias,
                    Some(args.project_id),
                    args.worktree_id,
                    0,
                    0,
                    "running",
                    None,
                )
                .unwrap();
            s.set_claude_session_id(id, args.resume_claude_session_id.as_deref().unwrap())
                .unwrap();
            Ok(s.get_session_by_id(id).unwrap().unwrap())
        }
        async fn add_worktree(
            &self,
            _host_alias: &str,
            _src_tmux: Option<&str>,
            _src_dir: Option<&str>,
            name: &str,
        ) -> Result<String, IpcError> {
            self.log.lock().unwrap().push(format!("add:{name}"));
            if let Some(e) = &self.add_err {
                return Err(IpcError::new(&e.code, e.message.clone()));
            }
            let wt = self.root.as_ref().unwrap().join("wt").join(name);
            std::fs::create_dir_all(&wt).unwrap();
            Ok(wt.canonicalize().unwrap().to_string_lossy().into_owned())
        }
        async fn remove_worktree(
            &self,
            _host_alias: &str,
            path: &str,
            _name: &str,
        ) -> Result<(), IpcError> {
            self.log.lock().unwrap().push(format!("remove:{path}"));
            if let Some(e) = &self.remove_err {
                return Err(IpcError::new(&e.code, e.message.clone()));
            }
            std::fs::remove_dir_all(path).ok();
            Ok(())
        }
        async fn claude_projects_dir(&self, _host_alias: &str) -> Result<String, IpcError> {
            let p = self.root.as_ref().unwrap().join("projects");
            Ok(p.to_string_lossy().into_owned())
        }
    }

    /// A `local` session (so the script and the cleanup `rm` run here, for
    /// real) bound to the fixture transcript, with a project to fork into.
    #[cfg(unix)]
    fn local_session(claude_status: &str) -> (std::path::PathBuf, std::path::PathBuf, Store, i64) {
        let d = tmp();
        let src = fixture(&d);
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let pid = s.upsert_project("o", "r", d.to_str().unwrap()).unwrap();
        let id = s
            .upsert_session("sess", "local", Some(pid), None, 0, 0, "running", None)
            .unwrap();
        s.rebind_conversation(
            id,
            OLD,
            StartSource::Fleet,
            Some(src.to_str().unwrap()),
            None,
        )
        .unwrap();
        s.set_session_claude_status_for_test(id, claude_status);
        (d, src, s, id)
    }

    #[cfg(unix)]
    fn args(id: i64, mode: RewindMode, anchor: Option<&str>) -> RewindArgs {
        RewindArgs {
            session_id: id,
            anchor_uuid: anchor.map(String::from),
            mode,
            new_worktree: None,
        }
    }

    #[cfg(unix)]
    fn jsonl_files(d: &std::path::Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(d)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".jsonl"))
            .collect();
        v.sort();
        v
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rewind_rebinds_to_the_copy_and_restarts_exactly_once() {
        let (d, _src, s, id) = local_session("idle");
        let store = std::sync::Mutex::new(s);
        let ops = FakeOps::new(&store, Some("idle"));
        let ssh = std::sync::Arc::new(SshClient::new());
        let row =
            rewind_conversation_with(args(id, RewindMode::Rewind, Some(A2)), &store, &ssh, &ops)
                .await
                .expect("an idle session rewinds");
        assert_eq!(ops.restarts(), 1, "the pane is restarted once");
        assert_eq!(ops.spawns(), 0, "a rewind spawns nothing");
        assert_eq!(
            ops.restarts.lock().unwrap()[0],
            ("local".to_string(), "sess".to_string())
        );
        let new_id = row.claude_session_id.clone().unwrap();
        assert_ne!(new_id, OLD, "the session is on a NEW conversation");
        assert!(d.join(format!("{new_id}.jsonl")).exists());
        let convs = store.lock().unwrap().list_conversations(id, 10).unwrap();
        let fresh = convs
            .iter()
            .find(|c| c.claude_session_id == new_id)
            .unwrap();
        assert_eq!(fresh.start_source, "fork");
        assert!(
            convs.iter().any(|c| c.claude_session_id == OLD),
            "the pre-rewind conversation stays listed"
        );
        std::fs::remove_dir_all(&d).ok();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fork_spawns_once_on_the_copy_and_labels_it_fork() {
        let (d, _src, s, id) = local_session("working");
        let store = std::sync::Mutex::new(s);
        let ops = FakeOps::new(&store, Some("working"));
        let ssh = std::sync::Arc::new(SshClient::new());
        let row =
            rewind_conversation_with(args(id, RewindMode::Fork, Some(A2)), &store, &ssh, &ops)
                .await
                .expect("fork is allowed mid-turn");
        assert_eq!(ops.spawns(), 1, "one new session");
        assert_eq!(ops.restarts(), 0, "the source pane is left alone");
        assert_ne!(row.id, id);
        let new_id = ops.spawns.lock().unwrap()[0].clone().unwrap();
        assert_eq!(row.claude_session_id.as_deref(), Some(new_id.as_str()));
        let convs = store
            .lock()
            .unwrap()
            .list_conversations(row.id, 10)
            .unwrap();
        assert_eq!(convs.len(), 1);
        assert_eq!(
            convs[0].start_source, "fork",
            "relabelled from new_session's `fleet`"
        );
        let expected = d.join(format!("{new_id}.jsonl"));
        assert_eq!(
            convs[0].transcript_path.as_deref(),
            Some(expected.to_str().unwrap())
        );
        // The source session is untouched.
        assert_eq!(
            store
                .lock()
                .unwrap()
                .get_session_by_id(id)
                .unwrap()
                .unwrap()
                .claude_session_id
                .as_deref(),
            Some(OLD)
        );
        std::fs::remove_dir_all(&d).ok();
    }

    /// A fork opens on the source's model and effort, as recreate, repair
    /// and move do, not on the host's default.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_fork_carries_the_sources_model_and_effort() {
        let (d, _src, s, id) = local_session("idle");
        s.set_session_launch_model(id, Some("opus")).unwrap();
        s.set_session_effort(id, Some("high")).unwrap();
        let store = std::sync::Mutex::new(s);
        let ops = FakeOps::new(&store, Some("idle"));
        let ssh = std::sync::Arc::new(SshClient::new());
        rewind_conversation_with(args(id, RewindMode::Fork, Some(A2)), &store, &ssh, &ops)
            .await
            .expect("the fork starts");
        assert_eq!(
            ops.launches.lock().unwrap().as_slice(),
            &[(Some("opus".to_string()), Some("high".to_string()))]
        );
        std::fs::remove_dir_all(&d).ok();
    }

    /// A same-worktree fork of a session with no project is refused BEFORE
    /// the copy: refused after it, the `.jsonl` would stay as an orphan that
    /// `discover_lost_sessions` later reports.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_same_worktree_fork_without_a_project_is_refused_before_the_copy() {
        let d = tmp();
        let src = fixture(&d);
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let id = s
            .upsert_session("sess", "local", None, None, 0, 0, "running", None)
            .unwrap();
        s.rebind_conversation(
            id,
            OLD,
            StartSource::Fleet,
            Some(src.to_str().unwrap()),
            None,
        )
        .unwrap();
        s.set_session_claude_status_for_test(id, "idle");
        let store = std::sync::Mutex::new(s);
        let ops = FakeOps::new(&store, Some("idle"));
        let ssh = std::sync::Arc::new(SshClient::new());
        let err =
            rewind_conversation_with(args(id, RewindMode::Fork, Some(A2)), &store, &ssh, &ops)
                .await
                .expect_err("no project to fork into");
        assert_eq!(err.code, codes::E_INVALID_STATE);
        assert_eq!(ops.spawns(), 0);
        assert_eq!(
            jsonl_files(&d),
            vec![format!("{OLD}.jsonl")],
            "no copy was written"
        );
        std::fs::remove_dir_all(&d).ok();
    }

    /// Item 4: a restart that fails leaves NOTHING behind — the row is back
    /// on the old conversation with its old path, no conversation row names
    /// the copy, and the copy itself is gone from the host.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_failed_restart_restores_the_old_binding_and_removes_the_copy() {
        let (d, src, s, id) = local_session("idle");
        let before = s.list_conversations(id, 10).unwrap();
        let store = std::sync::Mutex::new(s);
        let mut ops = FakeOps::new(&store, Some("idle"));
        ops.restart_err = Some(IpcError::new(codes::E_REPAIR_REQUIRED, "worktree gone"));
        let ssh = std::sync::Arc::new(SshClient::new());
        let err =
            rewind_conversation_with(args(id, RewindMode::Rewind, Some(A2)), &store, &ssh, &ops)
                .await
                .expect_err("the restart failed");
        assert_eq!(
            err.code,
            codes::E_REPAIR_REQUIRED,
            "the restart's own error"
        );
        assert_eq!(ops.restarts(), 1);
        let s = store.lock().unwrap();
        let row = s.get_session_by_id(id).unwrap().unwrap();
        assert_eq!(row.claude_session_id.as_deref(), Some(OLD));
        assert_eq!(
            s.session_transcript_path(id).unwrap().as_deref(),
            Some(src.to_str().unwrap()),
            "the old transcript path is back"
        );
        let after = s.list_conversations(id, 10).unwrap();
        assert_eq!(
            after.len(),
            before.len(),
            "no stray conversation row: {after:?}"
        );
        assert!(after.iter().all(|c| c.claude_session_id == OLD));
        assert!(
            after[0].ended_at.is_none(),
            "the old conversation is current again"
        );
        assert_eq!(
            jsonl_files(&d),
            vec![format!("{OLD}.jsonl")],
            "the unused copy is removed from the host"
        );
        std::fs::remove_dir_all(&d).ok();
    }

    /// Item 2: `blocked` is mid-turn too (a permission prompt waits INSIDE
    /// the turn), and the live pane beats a stale stored status.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_blocked_or_live_busy_session_is_refused_and_live_quiet_wins() {
        for (stored, live, ok) in [
            ("blocked", None, false),
            ("idle", Some("working"), false),
            ("idle", Some("blocked"), false),
            ("working", Some("idle"), true),
            ("idle", None, true),
        ] {
            let (d, _src, s, id) = local_session(stored);
            let store = std::sync::Mutex::new(s);
            let ops = FakeOps::new(&store, live);
            let ssh = std::sync::Arc::new(SshClient::new());
            let r = rewind_conversation_with(
                args(id, RewindMode::Rewind, Some(A2)),
                &store,
                &ssh,
                &ops,
            )
            .await;
            if ok {
                assert!(r.is_ok(), "stored {stored}, live {live:?}: {r:?}");
            } else {
                let e = r.expect_err("mid-turn");
                assert_eq!(e.code, codes::E_INVALID, "stored {stored}, live {live:?}");
                assert_eq!(ops.restarts(), 0);
                assert_eq!(
                    jsonl_files(&d),
                    vec![format!("{OLD}.jsonl")],
                    "refused before the copy is written"
                );
            }
            std::fs::remove_dir_all(&d).ok();
        }
    }

    /// Item 3: a rewind with no anchor would copy the whole file and restart
    /// on it — a silent no-op. Fork with none keeps the whole file (spec §3).
    #[cfg(unix)]
    #[tokio::test]
    async fn a_rewind_without_an_anchor_is_refused_but_a_fork_is_not() {
        let (d, _src, s, id) = local_session("idle");
        let store = std::sync::Mutex::new(s);
        let ops = FakeOps::new(&store, Some("idle"));
        let ssh = std::sync::Arc::new(SshClient::new());
        let err = rewind_conversation_with(args(id, RewindMode::Rewind, None), &store, &ssh, &ops)
            .await
            .expect_err("no anchor");
        assert_eq!(err.code, codes::E_INVALID);
        assert_eq!(ops.restarts(), 0);
        rewind_conversation_with(args(id, RewindMode::Fork, None), &store, &ssh, &ops)
            .await
            .expect("forking the newest turn keeps the whole file");
        assert_eq!(ops.spawns(), 1);
        std::fs::remove_dir_all(&d).ok();
    }

    /// Item 6: acting on a turn past a rewind point (a stale window) names
    /// what happened, not just "not found".
    #[cfg(unix)]
    #[tokio::test]
    async fn a_turn_past_the_rewind_point_says_the_conversation_was_rewound() {
        let (d, _src, s, id) = local_session("idle");
        let store = std::sync::Mutex::new(s);
        let ops = FakeOps::new(&store, Some("idle"));
        let ssh = std::sync::Arc::new(SshClient::new());
        rewind_conversation_with(args(id, RewindMode::Rewind, Some(A2)), &store, &ssh, &ops)
            .await
            .unwrap();
        // The old window still shows turn two; its anchor is not in the copy.
        let err =
            rewind_conversation_with(args(id, RewindMode::Rewind, Some(A2)), &store, &ssh, &ops)
                .await
                .expect_err("the turn is gone from the current conversation");
        assert_eq!(err.code, codes::E_NOTFOUND);
        assert!(err.message.contains("rewound"), "{}", err.message);
        std::fs::remove_dir_all(&d).ok();
    }

    // ── forking into a NEW worktree (spec §5.2) ────────────────────────

    #[cfg(unix)]
    fn fork_new(id: i64, anchor: Option<&str>, name: &str) -> RewindArgs {
        RewindArgs {
            new_worktree: Some(name.into()),
            ..args(id, RewindMode::Fork, anchor)
        }
    }

    /// The ordering the feature exists for: the worktree first, then the
    /// copy under ITS physical path's project dir with `cwd` rewritten, then
    /// the session started in that existing worktree (its row recorded).
    #[cfg(unix)]
    #[tokio::test]
    async fn a_new_worktree_fork_creates_the_tree_first_and_starts_there() {
        let (d, _src, s, id) = local_session("working");
        let store = std::sync::Mutex::new(s);
        let mut ops = FakeOps::new(&store, Some("working"));
        ops.root = Some(d.clone());
        let ssh = std::sync::Arc::new(SshClient::new());
        let row = rewind_conversation_with(fork_new(id, Some(A2), "fork-x"), &store, &ssh, &ops)
            .await
            .expect("forks into a new worktree");
        let log = ops.log();
        assert_eq!(log.len(), 2, "{log:?}");
        assert_eq!(log[0], "add:fork-x", "the tree comes first");
        assert!(log[1].starts_with("spawn:Some("), "{log:?}");

        let wt_path = d.join("wt/fork-x").canonicalize().unwrap();
        let wt_path = wt_path.to_str().unwrap();
        let s = store.lock().unwrap();
        let wid = row.worktree_id.expect("the fork's row names its worktree");
        let wt = s.get_worktree_row(wid).unwrap().unwrap();
        assert_eq!(wt.path, wt_path);
        assert_eq!(wt.name, "fork-x");
        assert_eq!(wt.branch.as_deref(), Some("fork-x"));
        assert_eq!(log[1], format!("spawn:Some({wid})"));

        let new_id = row.claude_session_id.clone().unwrap();
        let copy = d
            .join("projects")
            .join(crate::service::transcript::encode_project_dir(wt_path))
            .join(format!("{new_id}.jsonl"));
        let written = std::fs::read_to_string(&copy).expect("the copy is under the new cwd");
        assert!(
            written.contains(&format!(r#""cwd":"{wt_path}""#)),
            "cwd rewritten to the new tree: {written}"
        );
        assert!(!written.contains(r#""cwd":"/src/app""#), "{written}");
        assert_eq!(
            jsonl_files(&d),
            vec![format!("{OLD}.jsonl")],
            "nothing is written beside the source"
        );
        let convs = s.list_conversations(row.id, 10).unwrap();
        assert_eq!(convs[0].start_source, "fork");
        assert_eq!(convs[0].transcript_path.as_deref(), copy.to_str());
        drop(s);
        std::fs::remove_dir_all(&d).ok();
    }

    /// The copy fails (a stale anchor): the tree just made is removed, and
    /// nothing was started or recorded.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_failed_copy_removes_the_new_worktree() {
        let (d, _src, s, id) = local_session("idle");
        let store = std::sync::Mutex::new(s);
        let mut ops = FakeOps::new(&store, Some("idle"));
        ops.root = Some(d.clone());
        let ssh = std::sync::Arc::new(SshClient::new());
        let stale = "aaaaaaaa-0000-0000-0000-00000000dead";
        let err = rewind_conversation_with(fork_new(id, Some(stale), "fork-x"), &store, &ssh, &ops)
            .await
            .expect_err("the anchor is not in the transcript");
        assert_eq!(err.code, codes::E_NOTFOUND);
        let log = ops.log();
        assert_eq!(log.len(), 2, "{log:?}");
        assert_eq!(log[0], "add:fork-x");
        assert!(log[1].starts_with("remove:"), "{log:?}");
        assert!(!d.join("wt/fork-x").exists());
        assert_eq!(ops.spawns(), 0);
        assert!(
            !d.join("projects").exists()
                || std::fs::read_dir(d.join("projects"))
                    .unwrap()
                    .all(|e| jsonl_files(&e.unwrap().path()).is_empty()),
            "no copy is left behind"
        );
        std::fs::remove_dir_all(&d).ok();
    }

    /// The start fails: the copy, the tree (and branch) and the row all go.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_failed_start_removes_the_copy_the_tree_and_the_row() {
        let (d, _src, s, id) = local_session("idle");
        let store = std::sync::Mutex::new(s);
        let mut ops = FakeOps::new(&store, Some("idle"));
        ops.root = Some(d.clone());
        ops.spawn_err = Some(IpcError::new(codes::E_SHELL, "tmux refused"));
        let ssh = std::sync::Arc::new(SshClient::new());
        let err = rewind_conversation_with(fork_new(id, Some(A2), "fork-x"), &store, &ssh, &ops)
            .await
            .expect_err("the start failed");
        assert_eq!(err.code, codes::E_SHELL, "the start's own error");
        let log = ops.log();
        assert_eq!(log.len(), 3, "{log:?}");
        assert!(log[1].starts_with("spawn:Some("));
        assert!(log[2].starts_with("remove:"));
        assert!(!d.join("wt/fork-x").exists());
        let wt_path = d.canonicalize().unwrap().join("wt/fork-x");
        let enc = crate::service::transcript::encode_project_dir(wt_path.to_str().unwrap());
        assert!(
            jsonl_files(&d.join("projects").join(enc)).is_empty(),
            "the copy is removed"
        );
        let s = store.lock().unwrap();
        assert!(
            s.list_worktrees_on_host("local")
                .unwrap()
                .iter()
                .all(|w| w.name != "fork-x"),
            "the row is dropped"
        );
        drop(s);
        std::fs::remove_dir_all(&d).ok();
    }

    /// The start fails AFTER its pane came up: the removal refuses a tree
    /// a live pane is in, and the copy that pane resumed stays with it.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_failed_start_whose_pane_is_live_keeps_the_tree_and_the_copy() {
        let (d, _src, s, id) = local_session("idle");
        let store = std::sync::Mutex::new(s);
        let mut ops = FakeOps::new(&store, Some("idle"));
        ops.root = Some(d.clone());
        ops.spawn_err = Some(IpcError::new(codes::E_INTERNAL, "reconcile failed"));
        ops.remove_err = Some(IpcError::new(codes::E_GIT_SETUP, TREE_IN_USE));
        let ssh = std::sync::Arc::new(SshClient::new());
        let err = rewind_conversation_with(fork_new(id, Some(A2), "fork-x"), &store, &ssh, &ops)
            .await
            .expect_err("the start failed");
        assert_eq!(err.code, codes::E_INTERNAL, "the start's own error");
        assert!(d.join("wt/fork-x").exists(), "the tree stays");
        let wt_path = d.canonicalize().unwrap().join("wt/fork-x");
        let enc = crate::service::transcript::encode_project_dir(wt_path.to_str().unwrap());
        assert_eq!(
            jsonl_files(&d.join("projects").join(enc)).len(),
            1,
            "the copy the pane resumed stays"
        );
        std::fs::remove_dir_all(&d).ok();
    }

    /// Creating the tree fails (a name already taken): nothing else runs.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_worktree_that_cannot_be_made_stops_the_fork_before_the_copy() {
        let (d, _src, s, id) = local_session("idle");
        let store = std::sync::Mutex::new(s);
        let mut ops = FakeOps::new(&store, Some("idle"));
        ops.root = Some(d.clone());
        ops.add_err = Some(IpcError::new(codes::E_CONFLICT, "taken"));
        let ssh = std::sync::Arc::new(SshClient::new());
        let err = rewind_conversation_with(fork_new(id, Some(A2), "fork-x"), &store, &ssh, &ops)
            .await
            .expect_err("the name is taken");
        assert_eq!(err.code, codes::E_CONFLICT);
        assert_eq!(
            ops.log(),
            vec!["add:fork-x".to_string()],
            "no undo of what was never made"
        );
        assert_eq!(jsonl_files(&d), vec![format!("{OLD}.jsonl")]);
        assert!(!d.join("projects").exists());
        std::fs::remove_dir_all(&d).ok();
    }

    /// The copy's `cwd` is the transcript's own first `cwd` when no old
    /// value is given, and a subdirectory of it follows along.
    #[cfg(unix)]
    #[test]
    fn an_empty_old_cwd_rewrites_the_transcripts_own_and_its_subdirectories() {
        let d = tmp();
        let body = format!(
            concat!(
                r#"{{"type":"mode","sessionId":"{old}"}}"#,
                "\n",
                r#"{{"type":"user","uuid":"{a1}","cwd":"/src/app","sessionId":"{old}"}}"#,
                "\n",
                r#"{{"type":"user","uuid":"bbbb","cwd":"/src/app/sub","sessionId":"{old}"}}"#,
                "\n",
                r#"{{"type":"user","uuid":"cccc","cwd":"/src/apple","sessionId":"{old}"}}"#,
                "\n",
            ),
            old = OLD,
            a1 = A1
        );
        let src = d.join(format!("{OLD}.jsonl"));
        std::fs::write(&src, body).unwrap();
        let dest = d.join("dest");
        let out = run(&rewind_script(
            None,
            Some(src.to_str().unwrap()),
            None,
            OLD,
            NEW,
            None,
            Some(dest.to_str().unwrap()),
            Some(("", "/w/fork")),
        ));
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let written = std::fs::read_to_string(dest.join(format!("{NEW}.jsonl"))).unwrap();
        assert!(written.contains(r#""cwd":"/w/fork","#), "{written}");
        assert!(written.contains(r#""cwd":"/w/fork/sub""#), "{written}");
        assert!(
            written.contains(r#""cwd":"/src/apple""#),
            "a sibling that merely shares the prefix is not the tree: {written}"
        );
        std::fs::remove_dir_all(&d).ok();
    }

    /// A cwd with a quote and a backslash, old AND new, through the REAL
    /// awk: the new value reaches awk verbatim (the environment, not `-v`,
    /// which would unescape `\"` and `\\`), and the old value guessed from
    /// the line is read past its escaped quote. Every copied line must stay
    /// JSON and name exactly the new tree.
    #[cfg(unix)]
    #[test]
    fn a_cwd_with_a_quote_and_a_backslash_stays_valid_json() {
        let d = tmp();
        let old_cwd = r#"/src/a"b\c"#;
        let new_cwd = r#"/r/.worktrees/x"y\z"#;
        let line = |uuid: &str, cwd: &str| {
            serde_json::json!({"type": "user", "uuid": uuid, "sessionId": OLD, "cwd": cwd})
                .to_string()
        };
        let body = format!(
            "{}\n{}\n{}\n",
            line(A1, old_cwd),
            line("bbbb", &format!("{old_cwd}/sub")),
            line("cccc", old_cwd)
        );
        let src = d.join(format!("{OLD}.jsonl"));
        std::fs::write(&src, body).unwrap();
        let dest = d.join("dest");
        let out = run(&rewind_script(
            None,
            Some(src.to_str().unwrap()),
            None,
            OLD,
            NEW,
            None,
            Some(dest.to_str().unwrap()),
            Some(("", &json_string_body(new_cwd))),
        ));
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let written = std::fs::read_to_string(dest.join(format!("{NEW}.jsonl"))).unwrap();
        let cwds: Vec<String> = written
            .lines()
            .map(|l| {
                let v: serde_json::Value =
                    serde_json::from_str(l).unwrap_or_else(|e| panic!("not JSON ({e}): {l}"));
                assert_eq!(v["sessionId"], NEW);
                v["cwd"].as_str().unwrap().to_string()
            })
            .collect();
        assert_eq!(
            cwds,
            [
                new_cwd.to_string(),
                format!("{new_cwd}/sub"),
                new_cwd.to_string()
            ]
        );
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_path_is_json_escaped_for_the_transcript() {
        assert_eq!(json_string_body("/a/b"), "/a/b");
        assert_eq!(json_string_body(r#"/a/"q"\b"#), r#"/a/\"q\"\\b"#);
    }

    #[test]
    fn the_worktree_scripts_quote_every_value() {
        let hostile = "x'; rm -rf / #";
        let add = fork_worktree_script(Some(hostile), Some(hostile), hostile);
        let rm = remove_fork_worktree_script(hostile, hostile);
        for s in [&add, &rm] {
            assert!(s.contains(&quote(hostile)), "{s}");
            assert!(!s.contains(&format!("={hostile}")), "{s}");
        }
    }

    /// A real repo: the fork's tree lands where `new_session`'s would, on a
    /// new branch at the source's HEAD (not the default branch), never
    /// adopting an existing name; the undo removes both tree and branch.
    #[cfg(unix)]
    #[test]
    fn the_fork_worktree_scripts_create_and_remove_a_fresh_tree_and_branch() {
        let d = tmp();
        let repo = d.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let git = |dir: &std::path::Path, args: &[&str]| {
            let o = std::process::Command::new("git")
                .arg("-C")
                .arg(dir)
                .args([
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .output()
                .unwrap();
            assert!(
                o.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&o.stderr)
            );
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        };
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "one"]);
        git(
            &repo,
            &["worktree", "add", "-q", ".worktrees/feat", "-b", "feat"],
        );
        let src = repo.join(".worktrees/feat");
        git(&src, &["commit", "-q", "--allow-empty", "-m", "two"]);
        let head = git(&src, &["rev-parse", "HEAD"]);

        let out = run(&fork_worktree_script(
            None,
            Some(src.to_str().unwrap()),
            "fork-x",
        ));
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let expected = repo.canonicalize().unwrap().join(".worktrees/fork-x");
        assert_eq!(
            path,
            expected.to_str().unwrap(),
            "beside the others, not nested"
        );
        assert_eq!(
            git(&expected, &["rev-parse", "HEAD"]),
            head,
            "off the source's HEAD"
        );
        assert_eq!(
            git(&expected, &["rev-parse", "--abbrev-ref", "HEAD"]),
            "fork-x"
        );

        let again = run(&fork_worktree_script(
            None,
            Some(src.to_str().unwrap()),
            "fork-x",
        ));
        assert_eq!(again.status.code(), Some(6));
        assert!(String::from_utf8_lossy(&again.stderr).contains(WT_EXISTS));
        let branch_only = run(&fork_worktree_script(
            None,
            Some(src.to_str().unwrap()),
            "feat",
        ));
        assert_eq!(
            branch_only.status.code(),
            Some(6),
            "an existing branch is never adopted"
        );

        let rm = run(&remove_fork_worktree_script(&path, "fork-x"));
        assert!(
            rm.status.success(),
            "{}",
            String::from_utf8_lossy(&rm.stderr)
        );
        assert!(!expected.exists());
        assert!(git(&repo, &["branch", "--list", "fork-x"]).is_empty());
        assert!(src.exists(), "the source is untouched");
        assert_eq!(git(&src, &["rev-parse", "HEAD"]), head);

        // A branch-shaped name: the tree is one flat directory (the name its
        // row gets), the branch keeps its `/`.
        let out = run(&fork_worktree_script(
            None,
            Some(src.to_str().unwrap()),
            "fork/y",
        ));
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let flat = repo.canonicalize().unwrap().join(".worktrees/fork-y");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            flat.to_str().unwrap()
        );
        assert_eq!(git(&flat, &["rev-parse", "--abbrev-ref", "HEAD"]), "fork/y");
        std::fs::remove_dir_all(&d).ok();
    }

    /// A name git refuses as a branch is refused before anything is
    /// created, with the sentinel the caller maps to `E_INVALID` — not raw
    /// git stderr, and never `WT_EXISTS` (`.` is the repo root's own name).
    #[cfg(unix)]
    #[test]
    fn the_fork_worktree_script_refuses_a_name_git_would() {
        let d = tmp();
        let repo = d.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let init = std::process::Command::new("bash")
            .arg("-c")
            .arg(format!(
                "set -e; cd {}; git init -q -b main; git -c user.email=a@b -c user.name=a \
                 -c commit.gpgsign=false commit -q --allow-empty -m one",
                quote(repo.to_str().unwrap())
            ))
            .output()
            .unwrap();
        assert!(init.status.success());
        for bad in ["x.lock", "a:b", "@", "a/", "a.", "."] {
            let out = run(&fork_worktree_script(
                None,
                Some(repo.to_str().unwrap()),
                bad,
            ));
            assert_eq!(out.status.code(), Some(7), "{bad:?}");
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert!(stderr.contains(BAD_BRANCH), "{bad:?}: {stderr}");
            assert!(
                !repo.join(".worktrees").exists(),
                "{bad:?}: nothing created"
            );
            assert!(crate::validate::branch_name(bad).is_err(), "{bad:?}");
        }
        std::fs::remove_dir_all(&d).ok();
    }

    /// The removal script refuses a tree a live tmux pane is working in
    /// (below its root too) and leaves tree and branch; with no pane there
    /// it removes both. Skipped where tmux is not installed.
    #[cfg(unix)]
    #[test]
    fn the_removal_script_leaves_a_tree_a_live_pane_is_in() {
        if std::process::Command::new("tmux")
            .arg("-V")
            .output()
            .is_err()
        {
            return;
        }
        let d = tmp();
        // The server's socket is `$TMUX_TMPDIR/tmux-<uid>/default`, and a
        // unix socket path is capped at 104 bytes on macOS (108 on Linux).
        // Under a long TMPDIR (macOS's /var/folders/…, a deep worktree) the
        // test dir alone is past that and tmux fails to start, so the socket
        // gets a short dir of its own under /tmp.
        let sock = std::path::PathBuf::from(format!(
            "/tmp/cf-rw-{}",
            &uuid::Uuid::new_v4().simple().to_string()[..12]
        ));
        std::fs::create_dir_all(&sock).unwrap();
        let repo = d.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let sh = |script: &str| {
            std::process::Command::new("bash")
                .arg("-c")
                .arg(script)
                .env("TMUX_TMPDIR", &sock)
                .env_remove("TMUX")
                .env("GIT_CONFIG_COUNT", "1")
                .env("GIT_CONFIG_KEY_0", "commit.gpgsign")
                .env("GIT_CONFIG_VALUE_0", "false")
                .output()
                .unwrap()
        };
        let r = quote(repo.to_str().unwrap());
        let init = sh(&format!(
            "set -e; cd {r}; git init -q; git -c user.email=a@b -c user.name=a \
             commit -q --allow-empty -m init"
        ));
        assert!(init.status.success());
        // A backslash in the path too: awk's `-v` would read `\l` as an
        // escape and the live pane would go unseen.
        for (dir, branch) in [("wt-live", "fork-live"), (r"wt-li\ve", "fork-live2")] {
            let wt = repo.join(dir);
            let w = quote(wt.to_str().unwrap());
            let setup = sh(&format!(
                "set -e; cd {r}; git worktree add -q {w} -b {branch}; \
                 mkdir -p {w}/sub; tmux new-session -d -s live -c {w}/sub 'sleep 60'"
            ));
            assert!(
                setup.status.success(),
                "{}",
                String::from_utf8_lossy(&setup.stderr)
            );

            let rm = sh(&remove_fork_worktree_script(wt.to_str().unwrap(), branch));
            assert_eq!(
                rm.status.code(),
                Some(3),
                "{dir}: {}",
                String::from_utf8_lossy(&rm.stderr)
            );
            assert!(String::from_utf8_lossy(&rm.stderr).contains(TREE_IN_USE));
            assert!(wt.exists(), "{dir}: the tree stays");

            sh("tmux kill-server");
            let rm = sh(&remove_fork_worktree_script(wt.to_str().unwrap(), branch));
            assert!(
                rm.status.success(),
                "{}",
                String::from_utf8_lossy(&rm.stderr)
            );
            assert!(!wt.exists(), "{dir}: no pane: the tree goes");
        }
        std::fs::remove_dir_all(&d).ok();
        std::fs::remove_dir_all(&sock).ok();
    }
}
