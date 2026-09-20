//! The carry engine of `move_session`: script builders and output parsers
//! that take a worktree's state — unpushed commits, staged, modified and
//! untracked files, small git-ignored files — from the source host to the
//! target without origin. Pure: no I/O, no `async`; `mod.rs` runs the
//! scripts. See `docs/superpowers/specs/2026-09-19-move-carry-engine-design.md`.

use crate::ipc_error::{codes, IpcError};
use crate::service::safe_kill::DirtyFile;
use crate::shell::quote;
use serde::{Deserialize, Serialize};

/// `settings` key: largest git bundle (MiB) a move relays.
pub const SETTING_MAX_BUNDLE_MB: &str = "move.max_bundle_mb";
pub const DEFAULT_MAX_BUNDLE_MB: u64 = 500;
/// `settings` key: largest single git-ignored entry (KiB) a move carries.
pub const SETTING_IGNORED_ENTRY_KB: &str = "move.ignored_entry_kb";
pub const DEFAULT_IGNORED_ENTRY_KB: u64 = 1024;
/// `settings` key: total git-ignored payload (MiB) a move carries.
pub const SETTING_IGNORED_TOTAL_MB: &str = "move.ignored_total_mb";
pub const DEFAULT_IGNORED_TOTAL_MB: u64 = 20;

/// Final path components that are never carried: rebuildable, often huge,
/// frequently platform-specific. `worktrees` / `.worktrees`: nested git
/// worktrees never travel.
pub const DENYLIST: &[&str] = &[
    "node_modules",
    "target",
    ".venv",
    "venv",
    "dist",
    "build",
    "out",
    ".next",
    ".nuxt",
    ".svelte-kit",
    "__pycache__",
    ".gradle",
    ".cache",
    ".turbo",
    "coverage",
    "worktrees",
    ".worktrees",
];
/// Bound on carried ignored entries: they reach `tar` as argv.
pub const MAX_IGNORED_ENTRIES: usize = 500;
/// Bound on the target's haves. `run_shell` hands the whole script to the
/// host as ONE `bash -lc` argument, and Linux caps a single argument at
/// 128 KiB (`MAX_ARG_STRLEN`) — around 3,000 object names. A tag-heavy
/// target would make every snapshot fail with "Argument list too long", and
/// every retry with it. Haves are only an optimisation (a fatter bundle,
/// never a wrong one), so the list is cut to the ones most likely to help.
pub const MAX_HAVES: usize = 1000;

/// How the target's main clone came to exist.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetSeed {
    #[default]
    Existing,
    Cloned,
    /// `git init` + the bundle: origin was unreachable from the target.
    Initialized,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeftReason {
    Denylisted,
    OverCap,
    /// The name is one the carry will not handle: not valid UTF-8 (the
    /// git-ignored files), a character outside the half's safe charset or a
    /// `..` segment (the session directory), or a name that only differs
    /// from one already carried by ASCII case (the project memory, since a
    /// case-insensitive target volume would make the two one file).
    UnsupportedName,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IgnoredEntry {
    pub path: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeftBehind {
    pub path: String,
    /// `None` for a deny-listed entry: it is never walked, so never sized.
    pub bytes: Option<u64>,
    pub reason: LeftReason,
}

/// What travelled of the per-session directory (`<project dir>/<id>/`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionStateReport {
    /// Files merged into place on the target (path inside `<id>/`).
    pub carried: Vec<IgnoredEntry>,
    /// The target already had an equal or larger copy; it was kept.
    pub kept_target: Vec<String>,
    pub left_behind: Vec<LeftBehind>,
}

/// What travelled of the project's Claude memory (`<repo root>/memory/`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryReport {
    pub carried: Vec<IgnoredEntry>,
    /// Same name on both hosts, different contents: the target's file stays.
    pub kept_target: Vec<String>,
    pub identical: u32,
    /// Lines appended to the target's `MEMORY.md`.
    pub index_lines_added: u32,
    pub left_behind: Vec<LeftBehind>,
}

/// What a move carried besides the transcript.
///
/// Read back from a hub in remote mode, so every field is required on the
/// wire — see `service::repo_read` for the rule.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CarryReport {
    /// Commits in the bundle besides the two snapshot commits.
    pub commits: u32,
    pub bundle_bytes: u64,
    /// The porcelain rows restored on the target.
    pub dirty_entries: Vec<DirtyFile>,
    pub ignored_carried: Vec<IgnoredEntry>,
    pub ignored_left_behind: Vec<LeftBehind>,
    pub target_seeded: TargetSeed,
    pub session_state: SessionStateReport,
    pub memory: MemoryReport,
}

/// One record of the ignored-list script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedIgnored {
    pub path: String,
    /// `None`: the script recognised a deny-listed name and did not size it.
    pub kb: Option<u64>,
    /// `false` when the path was not valid UTF-8 (`path` is then lossy).
    pub valid_name: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IgnoredSelection {
    pub carry: Vec<IgnoredEntry>,
    pub left: Vec<LeftBehind>,
}

/// Parse `<kb>\t<path>\0` records after [`OUT_MARKER`]; a record without a
/// tab is dropped. No marker (the script failed before listing anything) —
/// an empty list, since a failed listing never fails the move.
pub fn parse_ignored_list(stdout: &[u8]) -> Vec<ListedIgnored> {
    let Some(body) = payload(stdout) else {
        return Vec::new();
    };
    body.split(|b| *b == 0)
        .filter(|rec| !rec.is_empty())
        .filter_map(|rec| {
            let tab = rec.iter().position(|b| *b == b'\t')?;
            let kb = std::str::from_utf8(&rec[..tab])
                .ok()?
                .trim()
                .parse::<i64>()
                .ok()?;
            let raw = &rec[tab + 1..];
            let (path, valid_name) = match std::str::from_utf8(raw) {
                Ok(p) => (p.to_string(), true),
                Err(_) => (String::from_utf8_lossy(raw).into_owned(), false),
            };
            Some(ListedIgnored {
                path,
                kb: u64::try_from(kb).ok(),
                valid_name,
            })
        })
        .collect()
}

fn denylisted(path: &str) -> bool {
    let base = path.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    DENYLIST.contains(&base)
}

/// The carry policy: deny-list, per-entry cap, then smallest-first up to the
/// total cap and [`MAX_IGNORED_ENTRIES`].
pub fn select_ignored(
    listed: Vec<ListedIgnored>,
    entry_kb: u64,
    total_kb: u64,
) -> IgnoredSelection {
    let mut sel = IgnoredSelection::default();
    let mut candidates: Vec<(u64, String)> = Vec::new();
    for l in listed {
        let bytes = l.kb.map(|k| k.saturating_mul(1024));
        if !l.valid_name {
            sel.left.push(LeftBehind {
                path: l.path,
                bytes,
                reason: LeftReason::UnsupportedName,
            });
        } else if l.kb.is_none() || denylisted(&l.path) {
            sel.left.push(LeftBehind {
                path: l.path,
                bytes: None,
                reason: LeftReason::Denylisted,
            });
        } else if l.kb.unwrap_or(0) > entry_kb {
            sel.left.push(LeftBehind {
                path: l.path,
                bytes,
                reason: LeftReason::OverCap,
            });
        } else {
            candidates.push((l.kb.unwrap_or(0), l.path));
        }
    }
    candidates.sort();
    let mut used = 0u64;
    for (kb, path) in candidates {
        let bytes = kb.saturating_mul(1024);
        if sel.carry.len() < MAX_IGNORED_ENTRIES && used.saturating_add(kb) <= total_kb {
            used += kb;
            sel.carry.push(IgnoredEntry { path, bytes });
        } else {
            sel.left.push(LeftBehind {
                path,
                bytes: Some(bytes),
                reason: LeftReason::OverCap,
            });
        }
    }
    sel
}

pub const FAILED: &str = "__CF_CARRY_FAILED__";
pub const BUNDLE_TOO_LARGE: &str = "__CF_BUNDLE_TOO_LARGE__";
pub const TARGET_DIRTY: &str = "__CF_TARGET_DIRTY__";
pub const HEAD_MISMATCH: &str = "__CF_HEAD_MISMATCH__";
/// The target worktree is dirty and what it holds is NOT the snapshot: the
/// stderr records that follow say which paths are the snapshot's (`ours`) and
/// which the target's own (`theirs`).
pub const LEFTOVERS_DIFFER: &str = "__CF_LEFTOVERS_DIFFER__";
/// Printed on its own line immediately before a script's real payload, with
/// nothing else reaching stdout after it. `ssh.rs` runs every carry script
/// as `bash -lc` — a LOGIN shell that sources `/etc/profile` and
/// `~/.bash_profile` — so a chatty profile (motd, nvm/conda/rvm init, a
/// "welcome" banner) can write to stdout BEFORE the script body ever runs.
/// Anchoring parsing on this marker, rather than trusting stdout to be
/// pristine, is what makes the parsers immune to that noise.
pub const OUT_MARKER: &str = "__CF_OUT__";
/// The one `git status` invocation the carry ever runs. Everything a host's
/// config could change about the text is pinned: `core.quotePath` (how a
/// non-ASCII name is spelled), `status.renames`, and
/// `status.showUntrackedFiles` — with which a target host can otherwise hide
/// its own untracked work from the dirty check. The source's porcelain and
/// the target's are compared for equality, so both must be produced by the
/// same rules; one const, so the call sites cannot drift apart.
pub const STATUS_PORCELAIN: &str =
    "-c core.quotePath=true -c status.renames=true status --porcelain=v1 --untracked-files=normal";
/// Bytes per relay chunk: the orchestrator's peak memory for a payload.
pub const CHUNK_BYTES: u64 = 8 * 1024 * 1024;

fn is_sha(s: &str) -> bool {
    (s.len() == 40 || s.len() == 64) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

pub(super) fn parse_err(what: &str, got: &str) -> IpcError {
    IpcError::new(codes::E_PARSE, format!("unexpected {what} output: {got:?}"))
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// The bytes after the FIRST line that is exactly [`OUT_MARKER`]: after the
/// first `__CF_OUT__\n` that either opens the output or is directly
/// preceded by `\n`. First, not last: a login-shell banner prints BEFORE
/// the script body runs, and nothing legitimate prints after the payload
/// begins — so the first marker line is always the real one. This also
/// matters for [`chunk_script`], whose payload is an arbitrary binary slice
/// of a git bundle: a later marker-shaped run of bytes inside that payload
/// must never be mistaken for a second boundary.
/// `None` when no such line exists — the script failed before reaching its
/// payload, or produced no output at all.
pub fn payload(stdout: &[u8]) -> Option<&[u8]> {
    let marker = OUT_MARKER.as_bytes();
    let mut start = 0usize;
    while start <= stdout.len() {
        let idx = find_subslice(&stdout[start..], marker)?;
        let pos = start + idx;
        let at_line_start = pos == 0 || stdout[pos - 1] == b'\n';
        let after = pos + marker.len();
        if at_line_start && stdout.get(after) == Some(&b'\n') {
            return Some(&stdout[after + 1..]);
        }
        start = pos + 1;
    }
    None
}

pub(super) fn payload_str(stdout: &str) -> Option<&str> {
    std::str::from_utf8(payload(stdout.as_bytes())?).ok()
}

/// Shell `case` guard rejecting an empty `$id`, or one containing `/` or
/// `..`, before it reaches a path (`$HOME/.cache/claude-fleet/transfer/$id`)
/// or a ref name (`refs/fleet/transfer/$id`). Interpolated right after
/// `id=...` in every script that builds one of those.
pub(super) fn id_guard() -> String {
    format!(r#"case "$id" in ''|*/*|*..*) printf '{FAILED} id\n' >&2; exit 5;; esac"#)
}

/// Guard for scripts that build `$HOME/.cache/claude-fleet/transfer/...`: an
/// empty `$HOME` would otherwise silently resolve to a repo-relative path.
pub(super) fn home_guard() -> String {
    format!(r#"[ -n "$HOME" ] || {{ printf '{FAILED} HOME\n' >&2; exit 5; }}"#)
}

/// Make sure the target's main clone exists. Prints [`OUT_MARKER`] then one
/// word: `existing`, `cloned` or `initialized`. Never prompts (a host
/// without credentials falls through to `git init`) and never removes a
/// directory it did not create. An `init`-seeded clone gets a neutral
/// unborn HEAD: a host whose `init.defaultBranch` happens to BE the session
/// branch would otherwise make the later `worktree add <branch>` fail as
/// "already checked out" in the main clone.
pub fn seed_script(project_root: &str, clone_url: &str) -> String {
    format!(
        r#"# cf-carry:seed
set +e
r={r}
url={url}
export GIT_TERMINAL_PROMPT=0 GIT_SSH_COMMAND='ssh -oBatchMode=yes -oConnectTimeout=20'
if [ -e "$r/.git" ]; then printf '\n{OUT_MARKER}\nexisting\n'; exit 0; fi
if [ -e "$r" ]; then printf '{FAILED} %s exists and is not a git repository\n' "$r" >&2; exit 5; fi
mkdir -p -- "$(dirname -- "$r")" || {{ printf '{FAILED} mkdir\n' >&2; exit 5; }}
if git clone -q -- "$url" "$r" >/dev/null 2>&1; then printf '\n{OUT_MARKER}\ncloned\n'; exit 0; fi
rm -rf -- "$r"
git init -q -- "$r" >/dev/null 2>&1 \
  && git -C "$r" symbolic-ref HEAD refs/heads/fleet-seed >/dev/null 2>&1 \
  && git -C "$r" remote add origin "$url" || {{ printf '{FAILED} init\n' >&2; exit 5; }}
printf '\n{OUT_MARKER}\ninitialized\n'
"#,
        r = quote(project_root),
        url = quote(clone_url),
    )
}

pub fn parse_seed(stdout: &str) -> Result<TargetSeed, IpcError> {
    let body = payload_str(stdout).ok_or_else(|| parse_err("seed", stdout))?;
    match body.trim() {
        "existing" => Ok(TargetSeed::Existing),
        "cloned" => Ok(TargetSeed::Cloned),
        "initialized" => Ok(TargetSeed::Initialized),
        other => Err(parse_err("seed", other)),
    }
}

/// Create the private transfer dir on the target and list the ref tips it
/// has, most useful first: the session branch and its origin counterpart,
/// then every other ref newest-commit-first, de-duplicated. Prints
/// [`OUT_MARKER`], then the absolute dir, then one object name per line.
pub fn haves_script(project_root: &str, claude_id: &str, branch: &str) -> String {
    format!(
        r#"# cf-carry:haves
set +e
r={r}
id={id}
br={br}
{id_guard}
{home_guard}
umask 077
dir="$HOME/.cache/claude-fleet/transfer/$id"
rm -rf -- "$dir"
mkdir -p -- "$dir" || {{ printf '{FAILED} mkdir\n' >&2; exit 5; }}
printf '\n{OUT_MARKER}\n'
printf '%s\n' "$dir"
{{
  git -C "$r" rev-parse --verify --quiet "refs/heads/$br"
  git -C "$r" rev-parse --verify --quiet "refs/remotes/origin/$br"
  git -C "$r" for-each-ref --sort=-committerdate --format='%(objectname)'
}} 2>/dev/null | awk -v m={max} '!seen[$0]++ {{ print; if (++n >= m) exit }}'
exit 0
"#,
        r = quote(project_root),
        id = quote(claude_id),
        br = quote(branch),
        max = MAX_HAVES,
        id_guard = id_guard(),
        home_guard = home_guard(),
    )
}

pub fn parse_haves(stdout: &str) -> Result<(String, Vec<String>), IpcError> {
    let body = payload_str(stdout).ok_or_else(|| parse_err("haves", stdout))?;
    let mut lines = body.lines();
    let dir = lines
        .next()
        .map(str::trim)
        .filter(|d| d.starts_with('/'))
        .ok_or_else(|| parse_err("haves", stdout))?;
    let haves = lines
        .map(str::trim)
        .filter(|l| is_sha(l))
        .map(str::to_string)
        .collect();
    Ok((dir.to_string(), haves))
}

/// What the snapshot script produced on the source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleInfo {
    pub bytes: u64,
    /// Commits the target lacked, not counting the two snapshot commits.
    pub commits: u32,
    pub submodules: bool,
    pub lfs: bool,
    /// Absolute path of the bundle on the source.
    pub path: String,
}

/// Snapshot the worktree (temporary index — the real index, the working tree
/// and every user ref stay untouched), park it under `refs/fleet/transfer/`,
/// and bundle what the target lacks. Both index reads go through COPIES:
/// even `git write-tree` rewrites the index it reads (the cache-tree
/// extension). The target's haves reach `git rev-list` on stdin (there can be
/// thousands); only the resulting boundary — a handful of hex shas, left
/// unquoted on purpose — reaches `git bundle create` as argv, because
/// `bundle create --stdin` regressed in some git releases. Prints
/// [`OUT_MARKER`] then `<bytes>\t<commits>\t<submodules 0|1>\t<lfs 0|1>\t<path>`.
pub fn snapshot_script(
    worktree: &str,
    claude_id: &str,
    haves: &[String],
    cap_bytes: u64,
) -> String {
    let haves: String = haves
        .iter()
        .filter(|h| is_sha(h))
        .take(MAX_HAVES)
        .map(|h| format!("{h}\n"))
        .collect();
    format!(
        r#"# cf-carry:snapshot
set +e
wt={wt}
id={id}
cap={cap}
fail() {{ printf '{FAILED} %s\n' "$1" >&2; exit 5; }}
{id_guard}
{home_guard}
old=$(umask)
umask 077
cd -- "$wt" 2>/dev/null || fail cd
dir="$HOME/.cache/claude-fleet/transfer/$id"
rm -rf -- "$dir"
mkdir -p -- "$dir" || fail mkdir
export GIT_AUTHOR_NAME=claude-fleet GIT_AUTHOR_EMAIL=fleet@localhost GIT_COMMITTER_NAME=claude-fleet GIT_COMMITTER_EMAIL=fleet@localhost
real=$(git rev-parse --git-path index)
cp -- "$real" "$dir/index.ix" 2>/dev/null && cp -- "$real" "$dir/index.wt" 2>/dev/null || fail index
# Everything that writes into the USER's repository — the blobs `add`
# hashes, the trees, the two commits, the refs — runs under the umask the
# host really has: a 0700 `objects/ab/` or ref file would lock every other
# writer out of a group-shared clone. Only what lands in the transfer dir
# stays 0077.
umask "$old"
itree=$(GIT_INDEX_FILE="$dir/index.ix" git write-tree 2>/dev/null) || fail write-tree-index
GIT_INDEX_FILE="$dir/index.wt" git add -A >/dev/null 2>&1 || fail add
wtree=$(GIT_INDEX_FILE="$dir/index.wt" git write-tree 2>/dev/null) || fail write-tree-worktree
rm -f -- "$dir/index.ix" "$dir/index.wt"
ix=$(git commit-tree "$itree" -p HEAD -m 'fleet transfer: index' 2>/dev/null) || fail commit-index
w=$(git commit-tree "$wtree" -p HEAD -m 'fleet transfer: worktree' 2>/dev/null) || fail commit-worktree
ref="refs/fleet/transfer/$id"
git update-ref "$ref/ix" "$ix" && git update-ref "$ref/wt" "$w" && git update-ref "$ref/head" HEAD || fail update-ref
umask 077
: > "$dir/nots"
while IFS= read -r h; do
  [ -n "$h" ] || continue
  if git cat-file -e "$h^{{commit}}" 2>/dev/null; then printf '^%s\n' "$h" >> "$dir/nots"; fi
done <<'CF_HAVES'
{haves}CF_HAVES
commits=$(git rev-list --count "$ref/head" --stdin < "$dir/nots" 2>/dev/null)
bnd=$(git rev-list --boundary "$ref/head" "$ref/ix" "$ref/wt" --stdin < "$dir/nots" 2>/dev/null | sed -n 's/^-/^/p' | tr '\n' ' ')
git bundle create "$dir/carry.bundle" "$ref/head" "$ref/ix" "$ref/wt" $bnd >/dev/null 2>&1 || fail bundle
n=$(wc -c < "$dir/carry.bundle" | tr -d ' ')
[ -n "$n" ] || fail size
if [ "$n" -gt "$cap" ]; then printf '{BUNDLE_TOO_LARGE} %s\n' "$n" >&2; exit 8; fi
sub=0; [ -f .gitmodules ] && sub=1
lfs=0; grep -qs 'filter=lfs' .gitattributes && lfs=1
printf '\n{OUT_MARKER}\n'
printf '%s\t%s\t%s\t%s\t%s\n' "$n" "${{commits:-0}}" "$sub" "$lfs" "$dir/carry.bundle"
"#,
        wt = quote(worktree),
        id = quote(claude_id),
        cap = cap_bytes,
        id_guard = id_guard(),
        home_guard = home_guard(),
    )
}

pub fn parse_snapshot(stdout: &str) -> Result<BundleInfo, IpcError> {
    let body = payload_str(stdout).ok_or_else(|| parse_err("snapshot", stdout))?;
    let line = body.trim_end_matches('\n');
    let p: Vec<&str> = line.splitn(5, '\t').collect();
    let bad = || parse_err("snapshot", line);
    if p.len() != 5 || !p[4].starts_with('/') {
        return Err(bad());
    }
    Ok(BundleInfo {
        bytes: p[0].trim().parse().map_err(|_| bad())?,
        commits: p[1].trim().parse().map_err(|_| bad())?,
        submodules: p[2].trim() == "1",
        lfs: p[3].trim() == "1",
        path: p[4].to_string(),
    })
}

/// `len` bytes of `path` starting at `offset` (binary-safe, GNU and BSD).
/// On success prints [`OUT_MARKER`] followed immediately by the raw bytes;
/// callers read the payload back with [`payload`] directly rather than a
/// dedicated parser, since it is an opaque binary slice, not text.
pub fn chunk_script(path: &str, offset: u64, len: u64) -> String {
    format!(
        r#"# cf-carry:chunk
set +e
f={f}
[ -f "$f" ] && [ -r "$f" ] || {{ printf '{FAILED} unreadable\n' >&2; exit 5; }}
printf '\n{OUT_MARKER}\n'
tail -c +{offset} -- "$f" | head -c {len}
"#,
        f = quote(path),
        offset = offset + 1,
        len = len,
    )
}

/// Verify and fetch the bundle into the target's main clone; create the
/// local branch at the source HEAD when the target has none, tracking
/// `origin/<branch>` when the clone has one — the workspace step used to
/// create the branch with `worktree add --track`, and without that upstream
/// `git pull` and a bare `git push` fail in the moved session. Only a
/// branch this script created is configured: an existing one's tracking is
/// the user's, and it is never moved here either. Setting the upstream is
/// best effort; it can never fail the fetch. Judged by exit status and the
/// stderr sentinel only — its stdout (`ok`) carries no payload and is not
/// marked.
pub fn fetch_script(
    project_root: &str,
    bundle_path: &str,
    claude_id: &str,
    branch: &str,
) -> String {
    format!(
        r#"# cf-carry:fetch
set +e
r={r}
b={b}
id={id}
br={br}
{id_guard}
fail() {{ printf '{FAILED} %s\n' "$1" >&2; exit 5; }}
git -C "$r" bundle verify "$b" >/dev/null 2>&1 || fail verify
git -C "$r" fetch -q "$b" "+refs/fleet/transfer/$id/*:refs/fleet/transfer/$id/*" >/dev/null 2>&1 || fail fetch
if ! git -C "$r" show-ref --verify --quiet "refs/heads/$br"; then
  git -C "$r" branch -- "$br" "refs/fleet/transfer/$id/head" >/dev/null 2>&1 || fail branch
  if git -C "$r" config --get remote.origin.url >/dev/null 2>&1 \
     && git -C "$r" show-ref --verify --quiet "refs/remotes/origin/$br"; then
    git -C "$r" branch --set-upstream-to="origin/$br" -- "$br" >/dev/null 2>&1
  fi
fi
printf 'ok\n'
"#,
        r = quote(project_root),
        b = quote(bundle_path),
        id = quote(claude_id),
        br = quote(branch),
        id_guard = id_guard(),
    )
}

/// Replay the snapshot in the target worktree: working tree := snapshot,
/// index := what was staged. Refuses a dirty worktree ([`TARGET_DIRTY`]) and
/// one not at `want_head` ([`HEAD_MISMATCH`]). Both the dirty check and the
/// printed result go through [`STATUS_PORCELAIN`], so a target host that
/// hides untracked files from `git status` cannot let the replay overwrite
/// them. On success prints [`OUT_MARKER`] then that porcelain; parse with
/// [`parse_apply`]. If either `read-tree` fails partway, the worktree is
/// restored to a clean `HEAD` before the [`FAILED`] sentinel is reported: a
/// partially replayed worktree must never be left behind. The rollback
/// removes EXACTLY what this script can have written — the paths the
/// snapshot tree adds relative to `HEAD` — never `git clean`, which would
/// also take the target's own untracked files and empty directories.
pub fn apply_script(cwd: &str, claude_id: &str, want_head: &str) -> String {
    format!(
        r#"# cf-carry:apply
set +e
cwd={cwd}
id={id}
want={want}
fail() {{ printf '{FAILED} %s\n' "$1" >&2; exit 5; }}
{id_guard}
cd -- "$cwd" 2>/dev/null || fail cd
if [ -n "$(git {status} 2>/dev/null)" ]; then printf '{TARGET_DIRTY}\n' >&2; exit 9; fi
h=$(git rev-parse HEAD 2>/dev/null)
if [ "$h" != "$want" ]; then printf '{HEAD_MISMATCH} %s\n' "$h" >&2; exit 10; fi
{recover}
git read-tree -u --reset "refs/fleet/transfer/$id/wt^{{tree}}" >/dev/null 2>&1 || {{ recover; fail read-tree-worktree; }}
git read-tree "refs/fleet/transfer/$id/ix^{{tree}}" >/dev/null 2>&1 || {{ recover; fail read-tree-index; }}
printf '\n{OUT_MARKER}\n'
git {status}
"#,
        cwd = quote(cwd),
        id = quote(claude_id),
        want = quote(want_head),
        id_guard = id_guard(),
        status = STATUS_PORCELAIN,
        recover = recover_body(),
    )
}

/// The `git status --porcelain=v1` text [`apply_script`] printed.
pub fn parse_apply(stdout: &str) -> Result<&str, IpcError> {
    payload_str(stdout).ok_or_else(|| parse_err("apply", stdout))
}

/// How a dirty target's contents differ from the snapshot about to be
/// replayed. `ours` are paths the snapshot writes (so a cleanup could replace
/// them); `theirs` are paths it does not hold at all (so nothing may touch
/// them). The lists are capped; `more_*` counts what was left out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Leftovers {
    pub ours: Vec<String>,
    pub theirs: Vec<String>,
    pub more_ours: u64,
    pub more_theirs: u64,
}

/// Longest path list either side of [`Leftovers`] carries.
const LEFTOVER_CAP: usize = 50;

/// The rollback shared by [`apply_script`] and [`recover_script`]: reset the
/// worktree and index to `HEAD` via `git read-tree -u --reset HEAD`, then
/// remove EXACTLY the paths the snapshot tree adds relative to `HEAD` — never
/// `git clean`, which would also take the target's own untracked files and
/// empty directories. Requires `$id` to be set and guarded.
///
/// `n` counts the paths it WENT THROUGH, not files it deleted: `rm -f` exits
/// 0 on a path the preceding `read-tree -u --reset HEAD` already removed (or
/// that was never written), so `n` ends up equal to the number of paths the
/// snapshot tree adds relative to `HEAD`. Callers must word it that way —
/// never as "N files deleted".
///
/// `read-tree -u --reset HEAD` is a hard reset of every TRACKED path: it
/// discards any uncommitted modification to a tracked file, whoever made it
/// — the snapshot's or the target's own. What it does NOT touch is anything
/// untracked: a git-ignored file, or an untracked path the snapshot does not
/// add. So this body is only safe to run once the caller already knows every
/// dirty tracked path in the worktree belongs to the snapshot — see
/// [`recover_script`]'s doc for the required gate.
fn recover_body() -> String {
    r#"n=0
recover() {
  git read-tree -u --reset HEAD >/dev/null 2>&1
  while IFS= read -r -d '' p; do
    rm -f -- "$p" && n=$((n+1))
    d=$(dirname -- "$p")
    [ "$d" = . ] || rmdir -p -- "$d" 2>/dev/null
  done < <(git diff-tree -r -z --name-only --diff-filter=A HEAD "refs/fleet/transfer/$id/wt" 2>/dev/null)
}"#
    .to_string()
}

/// Undo what an unfinished earlier attempt replayed into `cwd`: [`recover_body`],
/// then [`OUT_MARKER`] and its `n` — the number of paths the snapshot adds
/// relative to `HEAD`, which is what was reset and removed, not a count of
/// files that were actually there (see [`recover_body`]). Parse with
/// [`parse_recover`]. It never touches a git-ignored file, an untracked path
/// the snapshot does not add, or any path outside the snapshot's additions —
/// but [`recover_body`]'s `read-tree -u --reset HEAD` DOES discard any
/// uncommitted change to a TRACKED file in that worktree, including the
/// target's own, with no way to tell whose it was after the fact.
///
/// So this must only be called once the caller has already established that
/// every dirty tracked path in the worktree is one the snapshot itself
/// writes — never on a target that might hold its own genuine work. That is
/// exactly what [`verify_replayed_script`]'s `ours`/`theirs` split exists to
/// answer first: a non-empty `theirs` means real target work is present, and
/// `recover_script` must not run.
pub fn recover_script(cwd: &str, claude_id: &str) -> String {
    format!(
        r#"# cf-carry:recover
set +e
cwd={cwd}
id={id}
fail() {{ printf '{FAILED} %s\n' "$1" >&2; exit 5; }}
{id_guard}
cd -- "$cwd" 2>/dev/null || fail cd
git rev-parse --verify -q "refs/fleet/transfer/$id/wt" >/dev/null 2>&1 || fail no-snapshot
{recover}
recover
printf '\n{OUT_MARKER}\n'
printf '%s\n' "$n"
"#,
        cwd = quote(cwd),
        id = quote(claude_id),
        id_guard = id_guard(),
        recover = recover_body(),
    )
}

/// The count [`recover_script`] printed.
pub fn parse_recover(stdout: &str) -> Result<u64, IpcError> {
    payload_str(stdout)
        .map(str::trim)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| parse_err("recover", stdout))
}

/// Is this dirty target worktree already EXACTLY the snapshot we were about
/// to replay? Run only after [`apply_script`] refused with [`TARGET_DIRTY`].
///
/// Three questions, all answered by git itself over a throwaway
/// `GIT_INDEX_FILE` so the target's real index is never written:
/// the worktree's content against `wt` (paths the snapshot holds), the paths
/// it does NOT hold (`ls-files --others`, ignored files excluded — those are
/// the ignored-carry step's business), and the real index against `ix`.
/// `update-index --refresh` is what forces content hashing: a
/// `read-tree`-seeded index has no stat data, so every file is re-read rather
/// than trusted.
///
/// All three empty ⇒ prints [`OUT_MARKER`] then [`STATUS_PORCELAIN`], exactly
/// as [`apply_script`] does on success, so the move's own verification runs
/// over the adopted state unchanged. Otherwise exits 11 with
/// [`LEFTOVERS_DIFFER`] first, then `<tag>\t<path>\0` records — parse with
/// [`parse_leftovers`]. A `want_head` mismatch is [`HEAD_MISMATCH`] and never
/// an adopt.
pub fn verify_replayed_script(cwd: &str, claude_id: &str, want_head: &str) -> String {
    format!(
        r#"# cf-carry:verify
set +e
cwd={cwd}
id={id}
want={want}
fail() {{ printf '{FAILED} %s\n' "$1" >&2; exit 5; }}
{id_guard}
{home_guard}
cd -- "$cwd" 2>/dev/null || fail cd
ref="refs/fleet/transfer/$id"
git rev-parse --verify -q "$ref/wt" >/dev/null 2>&1 || fail no-snapshot
h=$(git rev-parse HEAD 2>/dev/null)
if [ "$h" != "$want" ]; then printf '{HEAD_MISMATCH} %s\n' "$h" >&2; exit 10; fi
dir="$HOME/.cache/claude-fleet/transfer/$id"
umask 077
mkdir -p -- "$dir" || fail mkdir
tmp="$dir/verify.ix"
ours="$dir/verify.ours"
theirs="$dir/verify.theirs"
rm -f -- "$tmp" "$ours" "$theirs"
clean() {{ rm -f -- "$tmp" "$ours" "$theirs"; }}
GIT_INDEX_FILE="$tmp" git read-tree "$ref/wt^{{tree}}" >/dev/null 2>&1 || {{ clean; fail read-tree; }}
GIT_INDEX_FILE="$tmp" git update-index -q --refresh >/dev/null 2>&1
GIT_INDEX_FILE="$tmp" git diff-index -z --name-only "$ref/wt^{{tree}}" -- > "$ours" 2>/dev/null || {{ clean; fail diff-worktree; }}
GIT_INDEX_FILE="$tmp" git ls-files -z --others --exclude-standard > "$theirs" 2>/dev/null || {{ clean; fail ls-others; }}
git diff-index -z --name-only --cached "$ref/ix^{{tree}}" -- >> "$ours" 2>/dev/null || {{ clean; fail diff-index; }}
rm -f -- "$tmp"
if [ -s "$ours" ] || [ -s "$theirs" ]; then
  printf '{LEFTOVERS_DIFFER}\n' >&2
  for t in ours theirs; do
    n=0
    while IFS= read -r -d '' p; do
      n=$((n+1))
      [ "$n" -le {cap} ] && printf '%s\t%s\0' "$t" "$p" >&2
    done < "$dir/verify.$t"
    if [ "$n" -gt {cap} ]; then printf 'more\t%s\t%s\0' "$t" "$((n-{cap}))" >&2; fi
  done
  clean
  exit 11
fi
clean
printf '\n{OUT_MARKER}\n'
git {status}
"#,
        cwd = quote(cwd),
        id = quote(claude_id),
        want = quote(want_head),
        id_guard = id_guard(),
        home_guard = home_guard(),
        status = STATUS_PORCELAIN,
        cap = LEFTOVER_CAP,
    )
}

/// The `<tag>\t<path>\0` records [`verify_replayed_script`] wrote to stderr.
/// Tolerant by design: unknown tags, a truncated stream and duplicate paths
/// (a path can differ in both the worktree and the index) are all fine — the
/// lists only ever drive a message and a cleanup confirmation, never a
/// decision about whether to overwrite. Both lists come out sorted and
/// deduplicated.
pub fn parse_leftovers(stderr: &str) -> Leftovers {
    let mut out = Leftovers::default();
    for rec in stderr.split('\0') {
        let mut it = rec.splitn(3, '\t');
        match (it.next(), it.next(), it.next()) {
            (Some(t), Some(p), None) if t.ends_with("ours") => out.ours.push(p.to_string()),
            (Some(t), Some(p), None) if t.ends_with("theirs") => out.theirs.push(p.to_string()),
            (Some(t), Some(side), Some(n)) if t.ends_with("more") => {
                let n = n.trim().parse().unwrap_or(0);
                if side == "ours" {
                    out.more_ours = n;
                } else if side == "theirs" {
                    out.more_theirs = n;
                }
            }
            _ => {}
        }
    }
    for v in [&mut out.ours, &mut out.theirs] {
        v.sort();
        v.dedup();
    }
    out
}

/// Best effort: drop the private refs and the transfer dir. Always exits 0
/// — except for a malicious/empty id or an empty `$HOME` (which would make
/// `rm -rf` reach a relative `.cache/...`), which it refuses outright and
/// does NOTHING for (no ref is touched, no directory is removed).
pub fn cleanup_script(repo_dir: &str, claude_id: &str) -> String {
    format!(
        r#"# cf-carry:cleanup
set +e
r={r}
id={id}
{id_guard}
{home_guard}
git -C "$r" for-each-ref --format='%(refname)' "refs/fleet/transfer/$id/" 2>/dev/null | while IFS= read -r ref; do
  git -C "$r" update-ref -d "$ref" >/dev/null 2>&1
done
rm -rf -- "$HOME/.cache/claude-fleet/transfer/$id"
exit 0
"#,
        r = quote(repo_dir),
        id = quote(claude_id),
        id_guard = id_guard(),
        home_guard = home_guard(),
    )
}

/// List the worktree's top-level git-ignored entries as [`OUT_MARKER`]
/// followed by `<kb>\t<path>\0` records. A wholly ignored directory is one
/// entry; a deny-listed name is printed with `-1` and never walked by `du`.
/// Refuses with the [`FAILED`] sentinel (non-zero exit) when `$wt` is not a
/// git work tree, or when `git ls-files` itself fails — a real failure must
/// never look identical to "no ignored files exist". `PIPESTATUS` is read
/// immediately after the pipeline, before any other command can reset it.
/// The one place [`OUT_MARKER`] may already have been printed when a
/// failure is later detected (`ls-files` fails mid-stream, after some
/// records were already emitted): callers always check the exit status
/// before parsing, so a marker followed by an incomplete or bogus listing
/// is harmless.
pub fn ignored_list_script(worktree: &str) -> String {
    format!(
        r#"# cf-carry:ignored-list
set +e
wt={wt}
deny=' {deny} '
cd -- "$wt" 2>/dev/null || {{ printf '{FAILED} cd\n' >&2; exit 5; }}
git rev-parse --is-inside-work-tree >/dev/null 2>&1 || {{ printf '{FAILED} not-a-repo\n' >&2; exit 5; }}
printf '\n{OUT_MARKER}\n'
git ls-files -o -i --exclude-standard --directory -z 2>/dev/null | while IFS= read -r -d '' p; do
  b=$(basename -- "${{p%/}}")
  case "$deny" in
    *" $b "*) k=-1 ;;
    *) k=$(du -sk -- "$p" 2>/dev/null | cut -f1) ;;
  esac
  printf '%s\t%s\0' "${{k:-0}}" "$p"
done
[ "${{PIPESTATUS[0]}}" -eq 0 ] || {{ printf '{FAILED} ls-files\n' >&2; exit 5; }}
exit 0
"#,
        wt = quote(worktree),
        deny = DENYLIST.join(" "),
    )
}

/// Shared body of [`ignored_pack_script`] and [`pack_script`]: tar the
/// chosen entries into the transfer dir. Each path is a quoted argv word
/// prefixed with `./` (so a leading `-` is never an option);
/// `COPYFILE_DISABLE` keeps macOS `._*` files out. Prints [`OUT_MARKER`]
/// then `<bytes>\t<path>`. `marker` is only ever `"ignored-pack"` or
/// `"pack"` — the two call sites below — and picks the `# cf-carry:` comment
/// a `FakeSsh` test rule matches on.
fn pack_script_as(
    marker: &str,
    dir: &str,
    claude_id: &str,
    archive_name: &str,
    paths: &[String],
) -> String {
    let argv: Vec<String> = paths.iter().map(|p| quote(&format!("./{p}"))).collect();
    let archive = quote(archive_name);
    format!(
        r#"# cf-carry:{marker}
set +e
wt={wt}
id={id}
{id_guard}
{home_guard}
umask 077
cd -- "$wt" 2>/dev/null || {{ printf '{FAILED} cd\n' >&2; exit 5; }}
dir="$HOME/.cache/claude-fleet/transfer/$id"
mkdir -p -- "$dir" || {{ printf '{FAILED} mkdir\n' >&2; exit 5; }}
COPYFILE_DISABLE=1 tar -czf "$dir/"{archive} {argv} >/dev/null 2>&1 || {{ printf '{FAILED} tar\n' >&2; exit 5; }}
n=$(wc -c < "$dir/"{archive} | tr -d ' ')
[ -n "$n" ] || {{ printf '{FAILED} size\n' >&2; exit 5; }}
printf '\n{OUT_MARKER}\n'
printf '%s\t%s\n' "$n" "$dir/"{archive}
"#,
        wt = quote(dir),
        id = quote(claude_id),
        argv = argv.join(" "),
        id_guard = id_guard(),
        home_guard = home_guard(),
        marker = marker,
        archive = archive,
    )
}

/// Tar the worktree's chosen git-ignored entries into `ignored.tgz`.
pub fn ignored_pack_script(worktree: &str, claude_id: &str, paths: &[String]) -> String {
    pack_script_as("ignored-pack", worktree, claude_id, "ignored.tgz", paths)
}

/// [`ignored_pack_script`] for any directory and archive name.
pub fn pack_script(dir: &str, claude_id: &str, archive_name: &str, paths: &[String]) -> String {
    pack_script_as("pack", dir, claude_id, archive_name, paths)
}

pub fn parse_pack(stdout: &str) -> Result<(u64, String), IpcError> {
    let body = payload_str(stdout).ok_or_else(|| parse_err("ignored-pack", stdout))?;
    let line = body.trim_end_matches('\n');
    let (n, path) = line
        .split_once('\t')
        .ok_or_else(|| parse_err("ignored-pack", line))?;
    let bytes = n
        .trim()
        .parse::<u64>()
        .map_err(|_| parse_err("ignored-pack", line))?;
    if !path.starts_with('/') {
        return Err(parse_err("ignored-pack", line));
    }
    Ok((bytes, path.to_string()))
}

/// The two lines that extract `"$a"` into the current directory while
/// keeping any file already there (`--skip-old-files` on GNU tar, `-k` on
/// BSD tar — GNU's `-k` reports existing files as errors). Factored out so
/// that `claude_state::memory_extract_script`, which validates the member
/// list before extracting, runs byte for byte the same extraction as
/// [`extract_keep_existing_script`] does. Assumes `$a` is set and a `cd`
/// into the destination has already happened.
pub(super) fn keep_existing_extract() -> String {
    format!(
        r#"if tar --version 2>/dev/null | grep -q 'GNU tar'; then k=--skip-old-files; else k=-k; fi
tar -xzf "$a" $k >/dev/null 2>&1 || {{ printf '{FAILED} extract\n' >&2; exit 5; }}"#
    )
}

/// Shared body of [`ignored_extract_script`] and [`extract_keep_existing_script`]:
/// extract into `dir`; a file already there wins (see [`keep_existing_extract`]).
/// `create_dir`: `mkdir -p -- "$cwd"` (private, `umask 077`) before the `cd`,
/// for a target whose memory directory may not exist yet — the ignored-file
/// extract never needs this, since the worktree it extracts into already
/// exists. `marker` is only ever `"ignored-extract"` or `"extract"`.
fn extract_script_as(marker: &str, dir: &str, archive: &str, create_dir: bool) -> String {
    let mkdir = if create_dir {
        format!(
            r#"umask 077; mkdir -p -- "$cwd" || {{ printf '{FAILED} mkdir\n' >&2; exit 5; }}
"#
        )
    } else {
        String::new()
    };
    format!(
        r#"# cf-carry:{marker}
set +e
cwd={cwd}
a={a}
{mkdir}cd -- "$cwd" 2>/dev/null || {{ printf '{FAILED} cd\n' >&2; exit 5; }}
tar -tzf "$a" >/dev/null 2>&1 || {{ printf '{FAILED} corrupt archive\n' >&2; exit 5; }}
{extract}
printf 'ok\n'
"#,
        cwd = quote(dir),
        a = quote(archive),
        mkdir = mkdir,
        marker = marker,
        extract = keep_existing_extract(),
    )
}

/// Extract `archive` in the target worktree.
pub fn ignored_extract_script(cwd: &str, archive: &str) -> String {
    extract_script_as("ignored-extract", cwd, archive, false)
}

/// [`ignored_extract_script`] for any directory and archive, optionally
/// creating `dir` first (`create_dir`) for a target that may not have it yet.
///
/// This builder TRUSTS its archive: beyond "keep what is already there" it
/// relies on `tar`'s own defaults for containment, so a crafted (or
/// tampered-with) archive can still plant a symlink, a subdirectory or a
/// name the caller never chose. That is acceptable for the git-ignored
/// files, whose archive is built from the same worktree the extraction
/// target is. It is NOT acceptable for the project's Claude memory, which
/// extracts straight into a directory of the user's own notes: the memory
/// half therefore uses `claude_state::memory_extract_script`, which
/// validates every member before extracting and only then runs the same
/// [`keep_existing_extract`] lines.
pub fn extract_keep_existing_script(dir: &str, archive: &str, create_dir: bool) -> String {
    extract_script_as("extract", dir, archive, create_dir)
}

/// The real-script tests of both this module and `mod.rs` share the
/// [`tests::require`] guard, so the module is crate-visible; nothing else in
/// it is.
#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// `true` when every binary in `bins` can be run. A missing one lets a
    /// real-git test skip on a bare workstation — but never on CI, where a
    /// silent skip would hide the very regression the test exists for.
    pub(crate) fn require(bins: &[&str]) -> bool {
        let missing: Vec<&str> = bins
            .iter()
            .copied()
            .filter(|b| {
                !Command::new(b)
                    .arg("--version")
                    .output()
                    .is_ok_and(|o| o.status.success())
            })
            .collect();
        if missing.is_empty() {
            return true;
        }
        assert!(
            std::env::var_os("CI").is_none(),
            "{} must be installed on CI; this test may not skip there",
            missing.join(", ")
        );
        eprintln!("skipping: {} is not available", missing.join(", "));
        false
    }

    fn listed(path: &str, kb: Option<u64>) -> ListedIgnored {
        ListedIgnored {
            path: path.into(),
            kb,
            valid_name: true,
        }
    }

    #[test]
    fn ignored_list_parses_nul_records_and_flags_bad_names() {
        let mut out = format!("{OUT_MARKER}\n").into_bytes();
        out.extend_from_slice(b"4\t.env\0-1\tnode_modules/\0");
        out.extend_from_slice(b"1\tbad\xff.txt\0");
        out.extend_from_slice(b"garbage-without-tab\0");
        let got = parse_ignored_list(&out);
        assert_eq!(got.len(), 3, "the record without a tab is dropped");
        assert_eq!(got[0].path, ".env");
        assert_eq!(got[0].kb, Some(4));
        assert_eq!(got[1].path, "node_modules/");
        assert_eq!(got[1].kb, None, "-1 means the script never walked it");
        assert!(!got[2].valid_name, "non-UTF-8 path");
    }

    #[test]
    fn selection_applies_denylist_caps_and_smallest_first() {
        let sel = select_ignored(
            vec![
                listed("big.bin", Some(2048)),        // over the 1024 entry cap
                listed("web/node_modules/", Some(1)), // deny-listed by basename
                listed("target/", None),              // deny-listed by the script
                listed("c.cfg", Some(600)),
                listed(".env", Some(4)),
                listed("b.cfg", Some(500)),
                ListedIgnored {
                    path: "bad\u{fffd}".into(),
                    kb: Some(1),
                    valid_name: false,
                },
            ],
            1024,
            1000, // total cap: .env(4) + b.cfg(500) fit, c.cfg(600) does not
        );
        let carried: Vec<&str> = sel.carry.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(carried, vec![".env", "b.cfg"], "smallest first");
        assert_eq!(sel.carry[0].bytes, 4 * 1024);
        let left = |p: &str| {
            sel.left
                .iter()
                .find(|l| l.path == p)
                .unwrap_or_else(|| panic!("{p}"))
        };
        assert_eq!(left("big.bin").reason, LeftReason::OverCap);
        assert_eq!(left("big.bin").bytes, Some(2048 * 1024));
        assert_eq!(left("c.cfg").reason, LeftReason::OverCap);
        assert_eq!(left("web/node_modules/").reason, LeftReason::Denylisted);
        assert_eq!(left("target/").reason, LeftReason::Denylisted);
        assert_eq!(left("target/").bytes, None, "never walked, size unknown");
        assert_eq!(left("bad\u{fffd}").reason, LeftReason::UnsupportedName);
    }

    #[test]
    fn selection_stops_at_the_entry_count_bound() {
        let many: Vec<_> = (0..MAX_IGNORED_ENTRIES + 3)
            .map(|i| listed(&format!("f{i:04}"), Some(1)))
            .collect();
        let sel = select_ignored(many, 1024, u64::MAX);
        assert_eq!(sel.carry.len(), MAX_IGNORED_ENTRIES);
        assert_eq!(sel.left.len(), 3);
        assert!(sel.left.iter().all(|l| l.reason == LeftReason::OverCap));
    }

    /// A hub answers `move_session` with this report and the desktop reads it
    /// back (remote mode), so it must round-trip — and a dropped field must
    /// fail loudly, not default (the `service::repo_read` wire rule).
    #[test]
    fn carry_report_round_trips_and_a_missing_field_fails_loudly() {
        let report = CarryReport {
            commits: 2,
            bundle_bytes: 1234,
            dirty_entries: vec![DirtyFile {
                status: " M".into(),
                path: "src/lib.rs".into(),
            }],
            ignored_carried: vec![IgnoredEntry {
                path: ".env".into(),
                bytes: 4096,
            }],
            ignored_left_behind: vec![LeftBehind {
                path: "node_modules/".into(),
                bytes: None,
                reason: LeftReason::Denylisted,
            }],
            target_seeded: TargetSeed::Initialized,
            session_state: SessionStateReport {
                carried: vec![IgnoredEntry {
                    path: "subagents/agent-ab12.jsonl".into(),
                    bytes: 2048,
                }],
                kept_target: vec!["custom-title.json".into()],
                left_behind: vec![LeftBehind {
                    path: "subagents/agent-ff00.jsonl".into(),
                    bytes: Some(900_000_000),
                    reason: LeftReason::OverCap,
                }],
            },
            memory: MemoryReport {
                carried: vec![IgnoredEntry {
                    path: "build-notes.md".into(),
                    bytes: 512,
                }],
                kept_target: vec!["deploy.md".into()],
                identical: 3,
                index_lines_added: 1,
                left_behind: Vec::new(),
            },
        };
        let json = serde_json::to_value(&report).unwrap();
        let back: CarryReport = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(serde_json::to_value(&back).unwrap(), json);
        assert_eq!(
            back.dirty_entries[0].status, " M",
            "the leading space survives"
        );

        let mut missing = json.clone();
        missing.as_object_mut().unwrap().remove("target_seeded");
        assert!(serde_json::from_value::<CarryReport>(missing).is_err());

        for field in ["session_state", "memory"] {
            let mut missing = serde_json::to_value(&report).unwrap();
            missing.as_object_mut().unwrap().remove(field);
            assert!(
                serde_json::from_value::<CarryReport>(missing).is_err(),
                "{field}"
            );
        }
    }

    #[test]
    fn carry_report_serializes_snake_case() {
        let v = serde_json::to_value(CarryReport {
            target_seeded: TargetSeed::Initialized,
            ignored_left_behind: vec![LeftBehind {
                path: "target/".into(),
                bytes: None,
                reason: LeftReason::Denylisted,
            }],
            ..Default::default()
        })
        .unwrap();
        assert_eq!(v["target_seeded"], "initialized");
        assert_eq!(v["ignored_left_behind"][0]["reason"], "denylisted");
        assert!(v["ignored_left_behind"][0]["bytes"].is_null());
    }

    use std::path::Path;
    use std::process::{Command, Output};

    /// Run a generated script the way a host would, with an isolated `$HOME`
    /// (so `~/.cache/claude-fleet/transfer` lands in the temp dir) and no
    /// user/system git config.
    pub(crate) fn bash(script: &str, home: &Path) -> Output {
        Command::new("bash")
            .args(["-c", script])
            .env("HOME", home)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("bash")
    }

    pub(crate) fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .expect("git");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn sorted_lines(s: &str) -> Vec<String> {
        let mut v: Vec<String> = s
            .lines()
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect();
        v.sort();
        v
    }

    const ID: &str = "550e8400-e29b-41d4-a716-446655440000";

    /// Commit history shared by every fixture: a `base` commit, a `pushed`
    /// branch standing in for `origin/feat` (a sha is only fetchable as a
    /// ref tip), and two "unpushed" commits on `feat`. Returns the base sha.
    fn commit_history(dir: &Path) -> String {
        git(dir, &["init", "-q", "-b", "feat"]);
        for (f, body) in [
            ("keep.txt", "keep\n"),
            ("mod.txt", "v1\n"),
            ("del.txt", "bye\n"),
            ("both.txt", "v1\n"),
            ("mode.sh", "#!/bin/sh\n"),
        ] {
            std::fs::write(dir.join(f), body).unwrap();
        }
        std::fs::write(dir.join(".gitignore"), ".env\nnode_modules/\n").unwrap();
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", "base"]);
        let base = git(dir, &["rev-parse", "HEAD"]).trim().to_string();
        git(dir, &["branch", "pushed"]);
        for n in ["one", "two"] {
            std::fs::write(dir.join(format!("{n}.txt")), n).unwrap();
            git(dir, &["add", "-A"]);
            git(dir, &["commit", "-q", "-m", n]); // two "unpushed" commits
        }
        base
    }

    /// The dirty mutations every fixture applies on top of `commit_history`:
    /// modified/staged/untracked/deleted files, a mode bit, a symlink and an
    /// ignored file — every kind of state the carry must reproduce.
    fn make_dirty(dir: &Path) {
        std::fs::write(dir.join("mod.txt"), "v2\n").unwrap(); // modified, unstaged
        std::fs::write(dir.join("staged new.txt"), "new\n").unwrap(); // staged, space in name
        git(dir, &["add", "staged new.txt"]);
        std::fs::write(dir.join("both.txt"), "staged\n").unwrap(); // staged...
        git(dir, &["add", "both.txt"]);
        std::fs::write(dir.join("both.txt"), "then modified\n").unwrap(); // ...then modified
        std::fs::write(dir.join("it's untracked.txt"), "u\n").unwrap(); // untracked, quote in name
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub/nested.txt"), "n\n").unwrap(); // untracked, in a new dir
        std::fs::remove_file(dir.join("del.txt")).unwrap(); // deleted, unstaged
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.join("mode.sh"), std::fs::Permissions::from_mode(0o755))
                .unwrap();
            std::os::unix::fs::symlink("keep.txt", dir.join("link")).unwrap();
        }
        std::fs::write(dir.join(".env"), "SECRET=1\n").unwrap(); // ignored: must NOT be in the snapshot
    }

    /// A source repo with every kind of state the carry must reproduce.
    /// Returns (repo dir, sha of the "pushed" base commit).
    fn dirty_source(root: &Path) -> (std::path::PathBuf, String) {
        let src = root.join("src");
        std::fs::create_dir_all(&src).unwrap();
        let base = commit_history(&src);
        make_dirty(&src);
        (src, base)
    }

    /// Same fixture, but the "source" the carry operates on is a LINKED
    /// WORKTREE (`git worktree add`) of a separate main repo — the shape
    /// claude-fleet actually moves. Returns (worktree dir, main repo dir,
    /// base sha).
    fn dirty_source_via_linked_worktree(
        root: &Path,
    ) -> (std::path::PathBuf, std::path::PathBuf, String) {
        let main = root.join("main-repo");
        std::fs::create_dir_all(&main).unwrap();
        let base = commit_history(&main);
        git(&main, &["checkout", "-q", "--detach"]); // free "feat" for the worktree
        let wt = root.join("src-wt");
        git(
            &main,
            &["worktree", "add", "-q", wt.to_str().unwrap(), "feat"],
        );
        make_dirty(&wt);
        (wt, main, base)
    }

    /// Everything that must be byte-identical on the source before and after.
    /// The index path goes through `git rev-parse --git-path index` rather
    /// than a hardcoded `.git/index`: for a LINKED WORKTREE, `.git` is a
    /// file (a pointer to the main repo), not a directory, and the
    /// worktree's own index lives under the main repo's
    /// `.git/worktrees/<name>/index`.
    fn source_fingerprint(src: &Path) -> (String, Vec<u8>, String) {
        let index_path = git(
            src,
            &["rev-parse", "--path-format=absolute", "--git-path", "index"],
        )
        .trim()
        .to_string();
        (
            // --no-optional-locks: a plain `git status` may refresh and rewrite the index.
            git(src, &["--no-optional-locks", "status", "--porcelain=v1"]),
            std::fs::read(&index_path).unwrap(),
            git(
                src,
                &["for-each-ref", "refs/heads", "refs/remotes", "refs/tags"],
            ),
        )
    }

    /// snapshot → bundle → fetch → worktree add → apply, all through the real
    /// generated scripts. `make_source` builds the fixture (a plain repo, or
    /// a linked worktree of one); `seed_target` prepares the target's main
    /// clone.
    fn round_trip(
        make_source: impl Fn(&Path) -> (std::path::PathBuf, String),
        seed_target: impl Fn(&Path, &Path, &str),
    ) {
        if !require(&["git", "bash"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let (home_a, home_b) = (tmp.path().join("home-a"), tmp.path().join("home-b"));
        std::fs::create_dir_all(&home_a).unwrap();
        std::fs::create_dir_all(&home_b).unwrap();
        let (src, base) = make_source(tmp.path());
        let before = source_fingerprint(&src);
        let src_head = git(&src, &["rev-parse", "HEAD"]).trim().to_string();

        // Target main clone.
        let root = tmp.path().join("tgt");
        seed_target(&src, &root, &base);
        let out = bash(&haves_script(root.to_str().unwrap(), ID, "feat"), &home_b);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let (tgt_dir, haves) = parse_haves(&String::from_utf8_lossy(&out.stdout)).unwrap();

        // Source: snapshot + bundle.
        let out = bash(
            &snapshot_script(src.to_str().unwrap(), ID, &haves, u64::MAX),
            &home_a,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let info = parse_snapshot(&String::from_utf8_lossy(&out.stdout)).unwrap();
        assert_eq!(info.bytes, std::fs::metadata(&info.path).unwrap().len());
        assert_eq!(
            source_fingerprint(&src),
            before,
            "the source tree, index and refs are untouched"
        );

        // Relay: chunked read (tiny chunk to exercise the loop), plain copy in.
        let mut got = Vec::new();
        while (got.len() as u64) < info.bytes {
            let out = bash(&chunk_script(&info.path, got.len() as u64, 1000), &home_a);
            let chunk = payload(&out.stdout).expect("chunk marker");
            assert!(!chunk.is_empty(), "empty chunk at {}", got.len());
            got.extend_from_slice(chunk);
        }
        assert_eq!(
            got,
            std::fs::read(&info.path).unwrap(),
            "chunks reassemble the bundle"
        );
        let tgt_bundle = format!("{tgt_dir}/carry.bundle");
        std::fs::write(&tgt_bundle, &got).unwrap();

        // Target: fetch, worktree add from the LOCAL branch (no origin), apply.
        let out = bash(
            &fetch_script(root.to_str().unwrap(), &tgt_bundle, ID, "feat"),
            &home_b,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let wt = tmp.path().join("tgt-wt");
        git(
            &root,
            &["worktree", "add", "-q", wt.to_str().unwrap(), "feat"],
        );
        assert_eq!(
            git(&wt, &["rev-parse", "HEAD"]).trim(),
            src_head,
            "unpushed commits arrived"
        );
        let out = bash(&apply_script(wt.to_str().unwrap(), ID, &src_head), &home_b);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );

        // The claim: identical porcelain, contents, modes.
        let apply_out = String::from_utf8_lossy(&out.stdout);
        let porcelain = parse_apply(&apply_out).unwrap();
        assert_eq!(
            sorted_lines(porcelain),
            sorted_lines(&before.0),
            "target porcelain equals the source's"
        );
        for f in [
            "mod.txt",
            "staged new.txt",
            "both.txt",
            "it's untracked.txt",
            "one.txt",
        ] {
            assert_eq!(
                std::fs::read(wt.join(f)).unwrap(),
                std::fs::read(src.join(f)).unwrap(),
                "{f}"
            );
        }
        assert!(!wt.join("del.txt").exists(), "the deletion travelled");
        assert!(
            !wt.join(".env").exists(),
            "ignored files are not in the snapshot"
        );
        assert_eq!(
            git(&wt, &["diff", "--cached", "--name-only"]),
            git(&src, &["diff", "--cached", "--name-only"]),
            "the same files are staged"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(wt.join("mode.sh"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o111,
                0o111
            );
            assert_eq!(
                std::fs::read_link(wt.join("link")).unwrap().to_str(),
                Some("keep.txt")
            );
        }

        // Cleanup leaves no private refs and no temp dirs on either side.
        assert!(bash(&cleanup_script(src.to_str().unwrap(), ID), &home_a)
            .status
            .success());
        assert!(bash(&cleanup_script(root.to_str().unwrap(), ID), &home_b)
            .status
            .success());
        assert_eq!(git(&src, &["for-each-ref", "refs/fleet"]), "");
        assert_eq!(git(&root, &["for-each-ref", "refs/fleet"]), "");
        assert!(!home_a
            .join(".cache/claude-fleet/transfer")
            .join(ID)
            .exists());
        assert!(!home_b
            .join(".cache/claude-fleet/transfer")
            .join(ID)
            .exists());
        assert_eq!(
            source_fingerprint(&src),
            before,
            "still untouched after cleanup"
        );
    }

    #[test]
    fn round_trip_into_a_target_that_has_the_base_commit() {
        round_trip(dirty_source, |src, root, base| {
            // A clone cut back to the "pushed" base: the bundle must be thin.
            std::fs::create_dir_all(root).unwrap();
            git(root, &["init", "-q", "-b", "main"]);
            git(
                root,
                &[
                    "fetch",
                    "-q",
                    src.to_str().unwrap(),
                    "pushed:refs/remotes/origin/feat",
                ],
            );
            assert_eq!(
                git(root, &["rev-parse", "refs/remotes/origin/feat"]).trim(),
                base
            );
        });
    }

    #[test]
    fn round_trip_into_an_initialized_empty_target() {
        round_trip(dirty_source, |_src, root, _base| {
            let tmp_home = root.parent().unwrap().join("home-seed");
            std::fs::create_dir_all(&tmp_home).unwrap();
            // An unreachable origin: the seed falls through to `git init`.
            let out = bash(
                &seed_script(root.to_str().unwrap(), "/nonexistent/origin.git"),
                &tmp_home,
            );
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert_eq!(
                parse_seed(&String::from_utf8_lossy(&out.stdout)).unwrap(),
                TargetSeed::Initialized
            );
        });
    }

    #[test]
    fn round_trip_from_a_linked_worktree_source() {
        // The shape claude-fleet actually moves: the "source" is a linked
        // worktree, not the main repo clone. `source_fingerprint` must read
        // the worktree's OWN index (via `git rev-parse --git-path index`),
        // and cleanup (run with the worktree path) must still reach the
        // shared refs.
        round_trip(
            |root| {
                let (wt, _main, base) = dirty_source_via_linked_worktree(root);
                (wt, base)
            },
            |src, root, base| {
                std::fs::create_dir_all(root).unwrap();
                git(root, &["init", "-q", "-b", "main"]);
                git(
                    root,
                    &[
                        "fetch",
                        "-q",
                        src.to_str().unwrap(),
                        "pushed:refs/remotes/origin/feat",
                    ],
                );
                assert_eq!(
                    git(root, &["rev-parse", "refs/remotes/origin/feat"]).trim(),
                    base
                );
            },
        );
    }

    #[test]
    fn a_thin_bundle_is_smaller_than_a_full_one_and_a_clean_source_still_bundles() {
        if !require(&["git", "bash"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let (src, base) = dirty_source(tmp.path());
        let size = |haves: &[String]| {
            let out = bash(
                &snapshot_script(src.to_str().unwrap(), ID, haves, u64::MAX),
                &home,
            );
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            parse_snapshot(&String::from_utf8_lossy(&out.stdout)).unwrap()
        };
        let full = size(&[]);
        let thin = size(std::slice::from_ref(&base));
        assert!(
            thin.bytes < full.bytes,
            "thin {} < full {}",
            thin.bytes,
            full.bytes
        );
        assert_eq!(thin.commits, 2, "the two unpushed commits");
        assert_eq!(full.commits, 3);
        // Even when the target has HEAD, the snapshot commits make a bundle.
        let head = git(&src, &["rev-parse", "HEAD"]).trim().to_string();
        let none = size(&[head]);
        assert_eq!(none.commits, 0);
        assert!(none.bytes > 0);
    }

    #[test]
    fn a_bundle_over_the_cap_and_a_dirty_or_moved_target_are_recognisable() {
        if !require(&["git", "bash"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let (src, _) = dirty_source(tmp.path());
        let out = bash(&snapshot_script(src.to_str().unwrap(), ID, &[], 10), &home);
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains(BUNDLE_TOO_LARGE));

        let head = git(&src, &["rev-parse", "HEAD"]).trim().to_string();
        // The source itself is dirty: applying onto it must refuse, not overwrite.
        let out = bash(&apply_script(src.to_str().unwrap(), ID, &head), &home);
        assert!(String::from_utf8_lossy(&out.stderr).contains(TARGET_DIRTY));
        // A clean checkout at another commit: refused as a head mismatch.
        let clean = tmp.path().join("clean");
        git(
            tmp.path(),
            &[
                "clone",
                "-q",
                src.to_str().unwrap(),
                clean.to_str().unwrap(),
            ],
        );
        let out = bash(
            &apply_script(clean.to_str().unwrap(), ID, &"0".repeat(40)),
            &home,
        );
        assert!(String::from_utf8_lossy(&out.stderr).contains(HEAD_MISMATCH));
    }

    #[test]
    fn seed_never_deletes_an_existing_non_git_directory() {
        if !require(&["git", "bash"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("precious");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("data.txt"), "mine").unwrap();
        let out = bash(
            &seed_script(root.to_str().unwrap(), "/nonexistent/origin.git"),
            tmp.path(),
        );
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains(FAILED));
        assert_eq!(
            std::fs::read_to_string(root.join("data.txt")).unwrap(),
            "mine"
        );
    }

    #[test]
    fn parsers_reject_malformed_output() {
        // Every real parser now requires the OUT_MARKER line: without it,
        // stdout is treated as banner noise the script never got past.
        let marked = |body: &str| format!("{OUT_MARKER}\n{body}");

        assert!(
            parse_seed("cloned\n").is_err(),
            "no marker: not a valid payload"
        );
        assert!(parse_seed(&marked("weird\n")).is_err());
        assert_eq!(parse_seed(&marked("cloned\n")).unwrap(), TargetSeed::Cloned);

        assert!(parse_haves(&marked("relative/dir\n")).is_err());
        let (dir, haves) = parse_haves(&marked(&format!(
            "/h/.cache/x\n{}\nnot-a-sha\n{}\n",
            "a".repeat(40),
            "b".repeat(64)
        )))
        .unwrap();
        assert_eq!(dir, "/h/.cache/x");
        assert_eq!(haves.len(), 2, "non-hex lines are dropped");

        assert!(parse_snapshot(&marked("12\t3\t0\t1\t/abs/carry.bundle\n")).is_ok());
        assert!(parse_snapshot(&marked("12\t3\t0\t1\trelative\n")).is_err());
        assert!(parse_snapshot(&marked("x\t3\t0\t1\t/abs\n")).is_err());

        assert!(parse_apply("M file.txt\n").is_err(), "no marker");
        assert_eq!(
            parse_apply(&marked(" M file.txt\n")).unwrap(),
            " M file.txt\n"
        );
    }

    #[test]
    fn carry_scripts_quote_every_interpolated_value() {
        let evil = "a b'$(touch /tmp/pwn)\n;x";
        let q = quote(evil);
        for script in [
            seed_script(evil, evil),
            haves_script(evil, evil, evil),
            snapshot_script(evil, evil, &[], 1),
            chunk_script(evil, 0, 1),
            fetch_script(evil, evil, evil, evil),
            apply_script(evil, evil, evil),
            cleanup_script(evil, evil),
        ] {
            assert!(script.contains(&q), "quoted value present: {script}");
            let without = script.replace(&q, "");
            assert!(
                !without.contains("touch /tmp/pwn"),
                "raw value leaked: {script}"
            );
        }
        // Haves are hex-only: anything else never reaches the heredoc.
        let s = snapshot_script("/w", ID, &["zz; rm -rf /".into(), "a".repeat(40)], 1);
        assert!(!s.contains("rm -rf /"), "{s}");
        assert!(s.contains(&"a".repeat(40)));
    }

    #[test]
    fn ignored_files_are_listed_selected_packed_and_extracted_without_overwriting() {
        if !require(&["git", "bash", "tar"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(src.join("node_modules/pkg")).unwrap();
        std::fs::create_dir_all(src.join("conf.d")).unwrap();
        git(&src, &["init", "-q", "-b", "feat"]);
        std::fs::write(
            src.join(".gitignore"),
            ".env\nnode_modules/\nbig.bin\nconf.d/\nit's.cfg\n",
        )
        .unwrap();
        std::fs::write(src.join(".env"), "SECRET=1\n").unwrap();
        std::fs::write(src.join("it's.cfg"), "q\n").unwrap();
        std::fs::write(src.join("conf.d/a.conf"), "a\n").unwrap();
        std::fs::write(src.join("node_modules/pkg/index.js"), "x").unwrap();
        std::fs::write(src.join("big.bin"), vec![0u8; 3 * 1024 * 1024]).unwrap();
        std::fs::write(src.join("tracked.txt"), "t\n").unwrap();
        git(&src, &["add", ".gitignore", "tracked.txt"]);
        git(&src, &["commit", "-q", "-m", "base"]);

        let out = bash(&ignored_list_script(src.to_str().unwrap()), &home);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let sel = select_ignored(parse_ignored_list(&out.stdout), 1024, 20 * 1024);
        let mut carried: Vec<&str> = sel.carry.iter().map(|e| e.path.as_str()).collect();
        carried.sort();
        assert_eq!(carried, vec![".env", "conf.d/", "it's.cfg"]);
        let left = |p: &str| sel.left.iter().find(|l| l.path == p).map(|l| l.reason);
        assert_eq!(left("node_modules/"), Some(LeftReason::Denylisted));
        assert_eq!(left("big.bin"), Some(LeftReason::OverCap));

        let paths: Vec<String> = sel.carry.iter().map(|e| e.path.clone()).collect();
        let out = bash(
            &ignored_pack_script(src.to_str().unwrap(), ID, &paths),
            &home,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let (bytes, archive) = parse_pack(&String::from_utf8_lossy(&out.stdout)).unwrap();
        assert_eq!(bytes, std::fs::metadata(&archive).unwrap().len());

        // The target already has its own .env: it must win.
        let tgt = tmp.path().join("tgt");
        std::fs::create_dir_all(&tgt).unwrap();
        std::fs::write(tgt.join(".env"), "SECRET=target\n").unwrap();
        let out = bash(
            &ignored_extract_script(tgt.to_str().unwrap(), &archive),
            &home,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            std::fs::read_to_string(tgt.join(".env")).unwrap(),
            "SECRET=target\n",
            "never overwritten"
        );
        assert_eq!(
            std::fs::read_to_string(tgt.join("it's.cfg")).unwrap(),
            "q\n"
        );
        assert_eq!(
            std::fs::read_to_string(tgt.join("conf.d/a.conf")).unwrap(),
            "a\n"
        );
        assert!(!tgt.join("node_modules").exists());

        // A corrupt archive is recognised, nothing is extracted.
        std::fs::write(&archive, b"not a tarball").unwrap();
        let out = bash(
            &ignored_extract_script(tgt.to_str().unwrap(), &archive),
            &home,
        );
        assert!(String::from_utf8_lossy(&out.stderr).contains(FAILED));
    }

    #[test]
    fn ignored_scripts_quote_every_interpolated_value() {
        let evil = "a b'$(touch /tmp/pwn)\n;x";
        let q = quote(evil);
        for script in [
            ignored_list_script(evil),
            ignored_pack_script(evil, evil, &[evil.to_string()]),
            ignored_extract_script(evil, evil),
        ] {
            assert!(
                script.contains(&q) || script.contains(&quote(&format!("./{evil}"))),
                "{script}"
            );
            let without = script
                .replace(&q, "")
                .replace(&quote(&format!("./{evil}")), "");
            assert!(
                !without.contains("touch /tmp/pwn"),
                "raw value leaked: {script}"
            );
        }
        assert!(parse_pack(&format!("{OUT_MARKER}\n12\t/abs/ignored.tgz\n")).is_ok());
        assert!(parse_pack(&format!("{OUT_MARKER}\n12\trelative\n")).is_err());
        assert!(parse_pack("12\t/abs/ignored.tgz\n").is_err(), "no marker");
    }

    #[test]
    fn payload_finds_the_first_marker_line_only() {
        assert_eq!(payload(b"no marker here"), None, "no marker");
        assert_eq!(
            payload(format!("{OUT_MARKER}\nhello").as_bytes()),
            Some(b"hello".as_slice()),
            "marker at offset 0"
        );
        assert_eq!(
            payload(format!("some banner\nnoise\n{OUT_MARKER}\nhello").as_bytes()),
            Some(b"hello".as_slice()),
            "banner before the marker is skipped"
        );
        assert_eq!(
            payload(format!("xx{OUT_MARKER}\nhello").as_bytes()),
            None,
            "marker text not at a line start is not a marker"
        );
        // A payload that itself contains the marker text later on: the FIRST one wins.
        let body = format!("{OUT_MARKER}\nfirst\n{OUT_MARKER}\nsecond");
        assert_eq!(
            payload(body.as_bytes()),
            Some(format!("first\n{OUT_MARKER}\nsecond").as_bytes()),
            "the first marker line determines where the payload starts"
        );
    }

    /// End-to-end proof that a login-shell banner (`bash -lc` sources
    /// `/etc/profile` / `~/.bash_profile` on a real host) never corrupts a
    /// script's parsed output: haves, snapshot, chunk and apply are each run
    /// with a banner injected before the script body, exactly as a chatty
    /// profile would inject one, and compared against a bannerless run.
    #[test]
    fn carry_scripts_survive_a_login_shell_banner() {
        if !require(&["git", "bash"]) {
            return;
        }
        let banner = "printf '/usr/local/bin added to PATH\\nWelcome!\\n'; ";
        let with_banner = |script: &str| format!("{banner}{script}");

        let tmp = tempfile::tempdir().unwrap();
        let (home_a, home_b) = (tmp.path().join("home-a"), tmp.path().join("home-b"));
        std::fs::create_dir_all(&home_a).unwrap();
        std::fs::create_dir_all(&home_b).unwrap();
        let (src, _base) = dirty_source(tmp.path());
        let src_head = git(&src, &["rev-parse", "HEAD"]).trim().to_string();

        // haves_script: the banner line must never be mistaken for the transfer dir.
        let clean = bash(&haves_script(src.to_str().unwrap(), ID, "feat"), &home_b);
        let banner_out = bash(
            &with_banner(&haves_script(src.to_str().unwrap(), ID, "feat")),
            &home_b,
        );
        assert!(
            banner_out.status.success(),
            "{}",
            String::from_utf8_lossy(&banner_out.stderr)
        );
        assert!(
            String::from_utf8_lossy(&banner_out.stdout).contains("added to PATH"),
            "sanity: the banner really landed in stdout"
        );
        let (dir_clean, haves_clean) =
            parse_haves(&String::from_utf8_lossy(&clean.stdout)).unwrap();
        let (dir_banner, haves_banner) =
            parse_haves(&String::from_utf8_lossy(&banner_out.stdout)).unwrap();
        assert_eq!(
            dir_banner, dir_clean,
            "banner line must not be mistaken for the transfer dir"
        );
        assert_eq!(haves_banner, haves_clean);

        // snapshot_script: must still parse to a sane BundleInfo.
        let out = bash(
            &with_banner(&snapshot_script(src.to_str().unwrap(), ID, &[], u64::MAX)),
            &home_a,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(String::from_utf8_lossy(&out.stdout).contains("added to PATH"));
        let info = parse_snapshot(&String::from_utf8_lossy(&out.stdout)).unwrap();
        assert_eq!(info.bytes, std::fs::metadata(&info.path).unwrap().len());
        assert_eq!(
            info.commits, 3,
            "no haves passed: base + the two unpushed commits"
        );

        // chunk_script: the reassembled bytes must equal the file, banner excluded.
        let out = bash(
            &with_banner(&chunk_script(&info.path, 0, info.bytes)),
            &home_a,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(String::from_utf8_lossy(&out.stdout).contains("added to PATH"));
        let chunk = payload(&out.stdout).expect("marker present");
        assert_eq!(chunk, std::fs::read(&info.path).unwrap().as_slice());

        // apply_script: fetch the (real, bannerless-written) bundle into a target,
        // then apply it TWICE — once under a banner, once clean — into two
        // separate clean worktrees at the same HEAD, and compare the results.
        let root = tmp.path().join("tgt");
        std::fs::create_dir_all(&root).unwrap();
        // "main", not "feat": `feat` must stay free for the two linked
        // worktrees added below (a branch can only be checked out once).
        git(&root, &["init", "-q", "-b", "main"]);
        let out = bash(
            &fetch_script(root.to_str().unwrap(), &info.path, ID, "feat"),
            &home_b,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let wt1 = tmp.path().join("wt1");
        let wt2 = tmp.path().join("wt2");
        git(
            &root,
            &["worktree", "add", "-q", wt1.to_str().unwrap(), "feat"],
        );
        git(
            &root,
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                wt2.to_str().unwrap(),
                "feat",
            ],
        );

        let out_banner = bash(
            &with_banner(&apply_script(wt1.to_str().unwrap(), ID, &src_head)),
            &home_b,
        );
        assert!(
            out_banner.status.success(),
            "{}",
            String::from_utf8_lossy(&out_banner.stderr)
        );
        assert!(String::from_utf8_lossy(&out_banner.stdout).contains("added to PATH"));
        let banner_apply_out = String::from_utf8_lossy(&out_banner.stdout);
        let porcelain_banner = parse_apply(&banner_apply_out).unwrap();

        let out_clean = bash(&apply_script(wt2.to_str().unwrap(), ID, &src_head), &home_b);
        assert!(
            out_clean.status.success(),
            "{}",
            String::from_utf8_lossy(&out_clean.stderr)
        );
        let clean_apply_out = String::from_utf8_lossy(&out_clean.stdout);
        let porcelain_clean = parse_apply(&clean_apply_out).unwrap();

        assert_eq!(
            sorted_lines(porcelain_banner),
            sorted_lines(porcelain_clean),
            "banner text must not appear as phantom dirty entries"
        );
    }

    #[test]
    fn scripts_refuse_a_malicious_or_empty_id_and_leave_a_sibling_transfer_dir_untouched() {
        if !require(&["git", "bash"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let other_dir = home.join(".cache/claude-fleet/transfer/other");
        std::fs::create_dir_all(&other_dir).unwrap();
        std::fs::write(other_dir.join("sentinel.txt"), "keep").unwrap();

        let (src, _base) = dirty_source(tmp.path());
        let root = tmp.path().join("tgt");
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-q", "-b", "feat"]);

        for bad in ["", "a/b", ".."] {
            for (name, script) in [
                ("haves", haves_script(root.to_str().unwrap(), bad, "feat")),
                (
                    "snapshot",
                    snapshot_script(src.to_str().unwrap(), bad, &[], u64::MAX),
                ),
                (
                    "fetch",
                    fetch_script(root.to_str().unwrap(), "/nonexistent.bundle", bad, "feat"),
                ),
                (
                    "apply",
                    apply_script(src.to_str().unwrap(), bad, "deadbeef"),
                ),
                ("cleanup", cleanup_script(root.to_str().unwrap(), bad)),
                (
                    "ignored-pack",
                    ignored_pack_script(src.to_str().unwrap(), bad, &[]),
                ),
            ] {
                let out = bash(&script, &home);
                assert!(!out.status.success(), "{name} must refuse id {bad:?}");
                assert!(
                    String::from_utf8_lossy(&out.stderr).contains(FAILED),
                    "{name} id {bad:?}: {}",
                    String::from_utf8_lossy(&out.stderr)
                );
            }
            assert_eq!(
                std::fs::read_to_string(other_dir.join("sentinel.txt")).unwrap(),
                "keep",
                "a sibling transfer dir must survive a bad id {bad:?}"
            );
        }
    }

    #[test]
    fn scripts_that_build_the_transfer_dir_refuse_an_empty_home() {
        if !require(&["git", "bash"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let (src, _base) = dirty_source(tmp.path());
        let root = tmp.path().join("tgt");
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-q", "-b", "feat"]);

        let run_with_empty_home = |script: &str| {
            Command::new("bash")
                .args(["-c", script])
                .env("HOME", "")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()
                .expect("bash")
        };
        // The cleanup included: with an empty `$HOME` its `rm -rf` would
        // reach a repo-relative `.cache/claude-fleet/...`, so it must do
        // nothing at all — not even delete the private refs.
        git(
            &src,
            &[
                "update-ref",
                &format!("refs/fleet/transfer/{ID}/wt"),
                "HEAD",
            ],
        );
        for script in [
            haves_script(root.to_str().unwrap(), ID, "feat"),
            snapshot_script(src.to_str().unwrap(), ID, &[], u64::MAX),
            ignored_pack_script(src.to_str().unwrap(), ID, &[]),
            cleanup_script(src.to_str().unwrap(), ID),
        ] {
            let out = run_with_empty_home(&script);
            assert!(!out.status.success());
            assert!(String::from_utf8_lossy(&out.stderr).contains(FAILED));
        }
        assert_eq!(
            git(
                &src,
                &["rev-parse", &format!("refs/fleet/transfer/{ID}/wt")]
            )
            .trim(),
            git(&src, &["rev-parse", "HEAD"]).trim(),
            "the cleanup touched nothing with an empty HOME"
        );
    }

    #[test]
    fn chunk_script_refuses_a_missing_or_unreadable_file() {
        if !require(&["bash"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();

        let missing = tmp.path().join("nope.bundle");
        let out = bash(&chunk_script(missing.to_str().unwrap(), 0, 10), &home);
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains(FAILED));
        assert!(payload(&out.stdout).is_none(), "no payload on failure");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let unreadable = tmp.path().join("secret.bundle");
            std::fs::write(&unreadable, b"data").unwrap();
            std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o000)).unwrap();
            let out = bash(&chunk_script(unreadable.to_str().unwrap(), 0, 4), &home);
            assert!(!out.status.success());
            assert!(String::from_utf8_lossy(&out.stderr).contains(FAILED));
            std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
    }

    /// Like [`git`], but a failure is an answer rather than a panic.
    fn git_try(dir: &Path, args: &[&str]) -> Output {
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .expect("git")
    }

    /// Carry `src` into a fresh target main clone under `tmp` and add a
    /// linked worktree on `feat` at the source HEAD — everything an
    /// [`apply_script`] test needs. `configure` runs on the target clone
    /// right after it is created. Returns (source HEAD, target main clone,
    /// target worktree, target `$HOME`).
    fn carry_into_target(
        tmp: &Path,
        src: &Path,
        configure: impl Fn(&Path),
    ) -> (
        String,
        std::path::PathBuf,
        std::path::PathBuf,
        std::path::PathBuf,
    ) {
        let (home_a, home_b) = (tmp.join("home-a"), tmp.join("home-b"));
        std::fs::create_dir_all(&home_a).unwrap();
        std::fs::create_dir_all(&home_b).unwrap();
        let src_head = git(src, &["rev-parse", "HEAD"]).trim().to_string();

        let root = tmp.join("tgt");
        std::fs::create_dir_all(&root).unwrap();
        // "main", not "feat": `feat` must stay free for the linked worktree
        // added below (a branch can only be checked out in one place).
        git(&root, &["init", "-q", "-b", "main"]);
        configure(&root);
        let out = bash(&haves_script(root.to_str().unwrap(), ID, "feat"), &home_b);
        assert!(
            out.status.success(),
            "haves: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let (_tgt_dir, haves) = parse_haves(&String::from_utf8_lossy(&out.stdout)).unwrap();

        let out = bash(
            &snapshot_script(src.to_str().unwrap(), ID, &haves, u64::MAX),
            &home_a,
        );
        assert!(
            out.status.success(),
            "snapshot: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let info = parse_snapshot(&String::from_utf8_lossy(&out.stdout)).unwrap();

        let out = bash(
            &fetch_script(root.to_str().unwrap(), &info.path, ID, "feat"),
            &home_b,
        );
        assert!(
            out.status.success(),
            "fetch: {}",
            String::from_utf8_lossy(&out.stderr)
        );

        let wt = tmp.join("tgt-wt");
        git(
            &root,
            &["worktree", "add", "-q", wt.to_str().unwrap(), "feat"],
        );
        (src_head, root, wt, home_b)
    }

    #[test]
    fn apply_restores_a_clean_worktree_when_a_read_tree_fails_partway() {
        if !require(&["git", "bash"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let (src, _base) = dirty_source(tmp.path());
        let (src_head, root, wt, home_b) = carry_into_target(tmp.path(), &src, |_| {});

        // A pre-existing ignored file: the recovery must never touch it.
        std::fs::write(wt.join(".env"), "TARGET_SECRET=1\n").unwrap();
        // A pre-existing EMPTY untracked directory: `git status` never
        // reports one, so the apply proceeds — and a recovery that reaches
        // for `git clean -fd` would take the user's directory with it.
        std::fs::create_dir_all(wt.join("scratch/deep")).unwrap();

        // Break it: delete the `ix` ref so the SECOND read-tree fails after
        // the first (worktree) one already succeeded.
        git(
            &root,
            &["update-ref", "-d", &format!("refs/fleet/transfer/{ID}/ix")],
        );

        let out = bash(&apply_script(wt.to_str().unwrap(), ID, &src_head), &home_b);
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains(FAILED));

        assert_eq!(
            git(&wt, &["status", "--porcelain"]),
            "",
            "no half-applied leftovers"
        );
        assert_eq!(
            std::fs::read_to_string(wt.join(".env")).unwrap(),
            "TARGET_SECRET=1\n",
            "a pre-existing ignored file is untouched by the recovery"
        );
        assert!(
            wt.join("scratch/deep").is_dir(),
            "a pre-existing empty untracked directory survives the recovery"
        );
        assert!(
            !wt.join("sub").exists(),
            "a directory the snapshot itself created is pruned again"
        );
    }

    /// (I3a) A target host configured with `status.showUntrackedFiles=no`
    /// must still be seen as dirty: otherwise `read-tree -u --reset`
    /// overwrites a colliding untracked file the user has work in, and the
    /// rollback deletes the rest.
    #[test]
    fn apply_refuses_a_target_whose_config_hides_untracked_files() {
        if !require(&["git", "bash"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let (src, _base) = dirty_source(tmp.path());
        let (src_head, _root, wt, home_b) = carry_into_target(tmp.path(), &src, |root| {
            git(root, &["config", "status.showUntrackedFiles", "no"]);
        });
        // The target's own work, under a name the snapshot also carries.
        std::fs::write(wt.join("it's untracked.txt"), "TARGET WORK\n").unwrap();

        let out = bash(&apply_script(wt.to_str().unwrap(), ID, &src_head), &home_b);
        assert!(
            !out.status.success(),
            "a dirty target must be refused whatever its status config says"
        );
        assert!(
            String::from_utf8_lossy(&out.stderr).contains(TARGET_DIRTY),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            std::fs::read_to_string(wt.join("it's untracked.txt")).unwrap(),
            "TARGET WORK\n",
            "the target's own file is never overwritten"
        );
    }

    /// (I3b) The verification compares two porcelain texts produced on two
    /// different hosts. Hosts disagree about `core.quotePath`, so both sides
    /// must come from the same pinned invocation — or a file with a
    /// diacritic in its name fails a perfectly good move.
    #[test]
    fn hosts_that_disagree_about_quote_path_still_produce_a_matching_porcelain() {
        if !require(&["git", "bash"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let home_a = tmp.path().join("home-a");
        std::fs::create_dir_all(&home_a).unwrap();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        commit_history(&src);
        // The source host prints non-ASCII names verbatim; the target host
        // keeps git's default of C-quoting them.
        git(&src, &["config", "core.quotePath", "false"]);
        std::fs::write(src.join("ünïcode.txt"), "u\n").unwrap();
        std::fs::write(src.join("mod.txt"), "v2\n").unwrap();

        // The source porcelain exactly as the move takes it.
        let out = bash(
            &super::super::inspect_script("", Some(src.to_str().unwrap()), "feat"),
            &home_a,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let inspected = String::from_utf8_lossy(&out.stdout).into_owned();
        let source_porcelain = inspected.split('\x1e').nth(1).expect("porcelain field");

        let (src_head, _root, wt, home_b) = carry_into_target(tmp.path(), &src, |root| {
            git(root, &["config", "core.quotePath", "true"]);
        });
        let out = bash(&apply_script(wt.to_str().unwrap(), ID, &src_head), &home_b);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let applied = String::from_utf8_lossy(&out.stdout).into_owned();
        assert_eq!(
            sorted_lines(parse_apply(&applied).unwrap()),
            sorted_lines(source_porcelain),
            "the two hosts' porcelain must agree despite their configs"
        );
    }

    /// (I2) A branch the fetch creates must keep the upstream the old
    /// `worktree add --track -b <br> origin/<br>` gave it, or `git pull` and
    /// a bare `git push` fail on the moved session — and only a branch the
    /// fetch created: an existing one's config is the user's.
    #[test]
    fn fetch_sets_the_upstream_only_for_a_branch_it_creates() {
        if !require(&["git", "bash"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let (home_a, home_b) = (tmp.path().join("home-a"), tmp.path().join("home-b"));
        std::fs::create_dir_all(&home_a).unwrap();
        std::fs::create_dir_all(&home_b).unwrap();
        let (src, base) = dirty_source(tmp.path());
        let src_head = git(&src, &["rev-parse", "HEAD"]).trim().to_string();

        // A real origin: `main` is its HEAD, so a clone of it has
        // `origin/feat` but no local `feat` — exactly the shape a move into
        // a host that already has the repo finds.
        let origin = tmp.path().join("origin.git");
        git(
            tmp.path(),
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                origin.to_str().unwrap(),
            ],
        );
        git(
            &origin,
            &[
                "fetch",
                "-q",
                src.to_str().unwrap(),
                "pushed:refs/heads/feat",
                "pushed:refs/heads/main",
            ],
        );

        let bundle = |root: &Path| -> String {
            let out = bash(&haves_script(root.to_str().unwrap(), ID, "feat"), &home_b);
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            let (_dir, haves) = parse_haves(&String::from_utf8_lossy(&out.stdout)).unwrap();
            let out = bash(
                &snapshot_script(src.to_str().unwrap(), ID, &haves, u64::MAX),
                &home_a,
            );
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            parse_snapshot(&String::from_utf8_lossy(&out.stdout))
                .unwrap()
                .path
        };
        let upstream = |root: &Path| -> Option<String> {
            let out = git_try(root, &["rev-parse", "--abbrev-ref", "feat@{upstream}"]);
            out.status
                .success()
                .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        };

        // 1. A clone of origin: the fetch creates `feat` and tracks it.
        let cloned = tmp.path().join("cloned");
        git(
            tmp.path(),
            &[
                "clone",
                "-q",
                origin.to_str().unwrap(),
                cloned.to_str().unwrap(),
            ],
        );
        assert!(
            upstream(&cloned).is_none(),
            "no local feat before the fetch"
        );
        let out = bash(
            &fetch_script(cloned.to_str().unwrap(), &bundle(&cloned), ID, "feat"),
            &home_b,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let wt = tmp.path().join("cloned-wt");
        git(
            &cloned,
            &["worktree", "add", "-q", wt.to_str().unwrap(), "feat"],
        );
        assert_eq!(git(&wt, &["rev-parse", "HEAD"]).trim(), src_head);
        assert_eq!(
            upstream(&cloned).as_deref(),
            Some("origin/feat"),
            "the moved branch keeps an upstream to pull and push against"
        );

        // 2. An init-seeded target with no origin branch: still no upstream,
        //    and the fetch must not fail over it.
        let seeded = tmp.path().join("seeded");
        std::fs::create_dir_all(&seeded).unwrap();
        git(&seeded, &["init", "-q", "-b", "main"]);
        let out = bash(
            &fetch_script(seeded.to_str().unwrap(), &bundle(&seeded), ID, "feat"),
            &home_b,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            git(&seeded, &["rev-parse", "refs/heads/feat"]).trim(),
            src_head
        );
        assert_eq!(upstream(&seeded), None, "nothing on origin to track");

        // 3. A branch the fetch did NOT create: its config is left alone.
        let existing = tmp.path().join("existing");
        git(
            tmp.path(),
            &[
                "clone",
                "-q",
                origin.to_str().unwrap(),
                existing.to_str().unwrap(),
            ],
        );
        git(
            &existing,
            &["branch", "--no-track", "feat", "refs/remotes/origin/feat"],
        );
        let out = bash(
            &fetch_script(existing.to_str().unwrap(), &bundle(&existing), ID, "feat"),
            &home_b,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            git(&existing, &["rev-parse", "refs/heads/feat"]).trim(),
            base
        );
        assert_eq!(
            upstream(&existing),
            None,
            "an existing branch's tracking config is the user's, not the move's"
        );
    }

    /// (I4) `run_shell` sends the whole script as ONE `bash -lc` argument,
    /// which Linux caps at 128 KiB: a tag-heavy target would make every
    /// snapshot fail with "Argument list too long". Haves are only an
    /// optimisation, so they are capped — with the session branch offered
    /// first, since that is the one that actually thins the bundle.
    #[test]
    fn the_haves_list_is_capped_and_still_offers_the_session_branch() {
        if !require(&["git", "bash"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let (src, _base) = dirty_source(tmp.path());

        let root = tmp.path().join("tgt");
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        git(
            &root,
            &[
                "fetch",
                "-q",
                src.to_str().unwrap(),
                "pushed:refs/heads/feat",
            ],
        );
        let tip = git(&root, &["rev-parse", "refs/heads/feat"])
            .trim()
            .to_string();

        // Far more refs than one argv word could ever carry, each on its own
        // object so de-duplication cannot hide the problem.
        let blobs = tmp.path().join("blobs");
        std::fs::create_dir_all(&blobs).unwrap();
        let paths: Vec<String> = (0..MAX_HAVES + 100)
            .map(|i| {
                let p = blobs.join(format!("b{i}"));
                std::fs::write(&p, format!("blob {i}\n")).unwrap();
                p.to_string_lossy().into_owned()
            })
            .collect();
        let mut args: Vec<&str> = vec!["hash-object", "-w", "--"];
        args.extend(paths.iter().map(String::as_str));
        let shas = git(&root, &args);
        let batch: String = shas
            .lines()
            .enumerate()
            .map(|(i, s)| format!("create refs/tags/t{i} {s}\n"))
            .collect();
        let batch_file = tmp.path().join("refs.txt");
        std::fs::write(&batch_file, batch).unwrap();
        let out = bash(
            &format!(
                "git -C {} update-ref --stdin < {}",
                quote(root.to_str().unwrap()),
                quote(batch_file.to_str().unwrap())
            ),
            &home,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );

        let out = bash(&haves_script(root.to_str().unwrap(), ID, "feat"), &home);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let (_dir, haves) = parse_haves(&String::from_utf8_lossy(&out.stdout)).unwrap();
        assert!(
            haves.len() <= MAX_HAVES,
            "{} haves reached the orchestrator",
            haves.len()
        );
        assert!(
            haves.contains(&tip),
            "the session branch tip is always offered"
        );

        // The Rust side enforces the same bound, and the script it builds
        // still fits comfortably in one argument.
        let over: Vec<String> = (0..MAX_HAVES + 50).map(|i| format!("{i:040x}")).collect();
        let script = snapshot_script(src.to_str().unwrap(), ID, &over, u64::MAX);
        assert_eq!(
            script.lines().filter(|l| is_sha(l)).count(),
            MAX_HAVES,
            "the heredoc holds at most MAX_HAVES object names"
        );
        assert!(
            script.len() < 64 * 1024,
            "the snapshot script is {} bytes",
            script.len()
        );
    }

    /// (F3) A login profile whose last write has no trailing newline would
    /// otherwise glue itself onto the marker and hide the payload.
    #[test]
    fn a_banner_without_a_trailing_newline_still_leaves_the_marker_on_its_own_line() {
        if !require(&["git", "bash"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let (home_a, home_b) = (tmp.path().join("home-a"), tmp.path().join("home-b"));
        std::fs::create_dir_all(&home_a).unwrap();
        std::fs::create_dir_all(&home_b).unwrap();
        let glued = |script: &str| format!("printf 'Welcome'\n{script}");
        let (src, _base) = dirty_source(tmp.path());
        let src_head = git(&src, &["rev-parse", "HEAD"]).trim().to_string();

        let root = tmp.path().join("tgt");
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-q", "-b", "main"]);

        let out = bash(
            &glued(&haves_script(root.to_str().unwrap(), ID, "feat")),
            &home_b,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            String::from_utf8_lossy(&out.stdout).starts_with("Welcome"),
            "sanity: the banner really landed, unterminated"
        );
        let (_dir, haves) = parse_haves(&String::from_utf8_lossy(&out.stdout))
            .expect("haves parse through an unterminated banner");

        let out = bash(
            &glued(&snapshot_script(
                src.to_str().unwrap(),
                ID,
                &haves,
                u64::MAX,
            )),
            &home_a,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let info = parse_snapshot(&String::from_utf8_lossy(&out.stdout))
            .expect("snapshot parse through an unterminated banner");

        let out = bash(&glued(&chunk_script(&info.path, 0, info.bytes)), &home_a);
        let chunk = payload(&out.stdout).expect("chunk marker after an unterminated banner");
        assert_eq!(
            chunk,
            std::fs::read(&info.path).unwrap().as_slice(),
            "the chunk bytes are exact"
        );

        let out = bash(
            &fetch_script(root.to_str().unwrap(), &info.path, ID, "feat"),
            &home_b,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let wt = tmp.path().join("tgt-wt");
        git(
            &root,
            &["worktree", "add", "-q", wt.to_str().unwrap(), "feat"],
        );
        let out = bash(
            &glued(&apply_script(wt.to_str().unwrap(), ID, &src_head)),
            &home_b,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let applied = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(
            !parse_apply(&applied)
                .expect("apply parse through an unterminated banner")
                .contains("Welcome"),
            "the banner is not part of the payload"
        );
    }

    /// (F4) `umask 077` keeps the transfer directory private, but it must
    /// not govern what git writes into the USER's repository: a `0700`
    /// `objects/ab/` locks every other writer out of a group-shared clone.
    #[test]
    fn the_snapshot_writes_into_the_users_repo_under_the_original_umask() {
        if !require(&["git", "bash"]) {
            return;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let tmp = tempfile::tempdir().unwrap();
            let home = tmp.path().join("home");
            std::fs::create_dir_all(&home).unwrap();
            let (src, _) = dirty_source(tmp.path());

            let script = snapshot_script(src.to_str().unwrap(), ID, &[], u64::MAX);
            let out = bash(&format!("umask 022\n{script}"), &home);
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            let info = parse_snapshot(&String::from_utf8_lossy(&out.stdout)).unwrap();
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(
                mode(Path::new(&info.path)),
                0o600,
                "the bundle stays private"
            );
            assert_eq!(
                mode(Path::new(&info.path).parent().unwrap()),
                0o700,
                "the transfer dir stays private"
            );

            let wt_ref = src.join(format!(".git/refs/fleet/transfer/{ID}/wt"));
            assert_eq!(
                mode(&wt_ref) & 0o044,
                0o044,
                "the new ref file keeps the repo's own sharing: {:o}",
                mode(&wt_ref)
            );
            let sha = git(
                &src,
                &["rev-parse", &format!("refs/fleet/transfer/{ID}/wt")],
            )
            .trim()
            .to_string();
            let obj = src.join(format!(".git/objects/{}/{}", &sha[..2], &sha[2..]));
            assert_eq!(
                mode(&obj) & 0o044,
                0o044,
                "a new loose object keeps the repo's own sharing: {:o}",
                mode(&obj)
            );
        }
    }

    /// A target seeded by `git init` whose default branch NAME happens to be
    /// the session branch would make `worktree add <branch>` fail as
    /// "already checked out", so the seed parks HEAD on a neutral branch.
    #[test]
    fn an_init_seeded_target_whose_default_branch_is_the_session_branch_still_works() {
        if !require(&["git", "bash"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let (home_a, home_b) = (tmp.path().join("home-a"), tmp.path().join("home-b"));
        std::fs::create_dir_all(&home_a).unwrap();
        std::fs::create_dir_all(&home_b).unwrap();
        let (src, _base) = dirty_source(tmp.path());
        let src_head = git(&src, &["rev-parse", "HEAD"]).trim().to_string();

        // The host's own git config names the session branch as the default.
        let cfg = tmp.path().join("gitconfig");
        std::fs::write(&cfg, "[init]\n\tdefaultBranch = feat\n").unwrap();
        let with_default_branch = |script: &str, home: &Path| -> Output {
            Command::new("bash")
                .args(["-c", script])
                .env("HOME", home)
                .env("GIT_CONFIG_GLOBAL", &cfg)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()
                .expect("bash")
        };

        let root = tmp.path().join("tgt");
        let out = with_default_branch(
            &seed_script(root.to_str().unwrap(), "/nonexistent/origin.git"),
            &home_b,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            parse_seed(&String::from_utf8_lossy(&out.stdout)).unwrap(),
            TargetSeed::Initialized
        );

        let out = bash(&haves_script(root.to_str().unwrap(), ID, "feat"), &home_b);
        let (_dir, haves) = parse_haves(&String::from_utf8_lossy(&out.stdout)).unwrap();
        let out = bash(
            &snapshot_script(src.to_str().unwrap(), ID, &haves, u64::MAX),
            &home_a,
        );
        let info = parse_snapshot(&String::from_utf8_lossy(&out.stdout)).unwrap();
        let out = bash(
            &fetch_script(root.to_str().unwrap(), &info.path, ID, "feat"),
            &home_b,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );

        let wt = tmp.path().join("tgt-wt");
        let added = git_try(
            &root,
            &["worktree", "add", "-q", wt.to_str().unwrap(), "feat"],
        );
        assert!(
            added.status.success(),
            "worktree add: {}",
            String::from_utf8_lossy(&added.stderr)
        );
        assert_eq!(git(&wt, &["rev-parse", "HEAD"]).trim(), src_head);
    }

    #[test]
    fn snapshot_script_fails_cleanly_if_the_bundle_size_cannot_be_determined() {
        if !require(&["git", "bash"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let (src, _) = dirty_source(tmp.path());

        // A `wc` that prints nothing for `-c`, simulating a `wc -c` failure.
        let fakebin = tmp.path().join("fakebin");
        std::fs::create_dir_all(&fakebin).unwrap();
        std::fs::write(
            fakebin.join("wc"),
            "#!/bin/sh\ncase \"$1\" in -c) exit 0;; esac\nexec /usr/bin/wc \"$@\"\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(fakebin.join("wc"), std::fs::Permissions::from_mode(0o755))
                .unwrap();
        }
        let path = format!(
            "{}:{}",
            fakebin.display(),
            std::env::var("PATH").unwrap_or_default()
        );

        let out = Command::new("bash")
            .args([
                "-c",
                &snapshot_script(src.to_str().unwrap(), ID, &[], u64::MAX),
            ])
            .env("HOME", &home)
            .env("PATH", path)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("bash");
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains(FAILED));
    }

    #[test]
    fn snapshot_and_pack_write_their_transfer_dir_private() {
        if !require(&["git", "bash", "tar"]) {
            return;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let tmp = tempfile::tempdir().unwrap();
            let home = tmp.path().join("home");
            std::fs::create_dir_all(&home).unwrap();
            let (src, _) = dirty_source(tmp.path());

            let out = bash(
                &snapshot_script(src.to_str().unwrap(), ID, &[], u64::MAX),
                &home,
            );
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            let info = parse_snapshot(&String::from_utf8_lossy(&out.stdout)).unwrap();
            let bundle_mode = std::fs::metadata(&info.path).unwrap().permissions().mode() & 0o777;
            assert_eq!(bundle_mode, 0o600, "umask 077 keeps the bundle private");
            let dir_mode = std::fs::metadata(Path::new(&info.path).parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(dir_mode, 0o700, "umask 077 keeps the transfer dir private");

            let out = bash(
                &ignored_pack_script(src.to_str().unwrap(), ID, &["keep.txt".to_string()]),
                &home,
            );
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            let (_bytes, archive) = parse_pack(&String::from_utf8_lossy(&out.stdout)).unwrap();
            let archive_mode = std::fs::metadata(&archive).unwrap().permissions().mode() & 0o777;
            assert_eq!(archive_mode, 0o600, "umask 077 keeps the archive private");
        }
    }

    #[test]
    fn fetch_never_moves_an_existing_local_branch() {
        if !require(&["git", "bash"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let (home_a, home_b) = (tmp.path().join("home-a"), tmp.path().join("home-b"));
        std::fs::create_dir_all(&home_a).unwrap();
        std::fs::create_dir_all(&home_b).unwrap();
        let (src, base) = dirty_source(tmp.path());

        let root = tmp.path().join("tgt");
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        git(
            &root,
            &[
                "fetch",
                "-q",
                src.to_str().unwrap(),
                "pushed:refs/heads/feat",
            ],
        );
        assert_eq!(git(&root, &["rev-parse", "refs/heads/feat"]).trim(), base);

        let out = bash(&haves_script(root.to_str().unwrap(), ID, "feat"), &home_b);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let (_dir, haves) = parse_haves(&String::from_utf8_lossy(&out.stdout)).unwrap();

        let out = bash(
            &snapshot_script(src.to_str().unwrap(), ID, &haves, u64::MAX),
            &home_a,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let info = parse_snapshot(&String::from_utf8_lossy(&out.stdout)).unwrap();

        let out = bash(
            &fetch_script(root.to_str().unwrap(), &info.path, ID, "feat"),
            &home_b,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );

        assert_eq!(
            git(&root, &["rev-parse", "refs/heads/feat"]).trim(),
            base,
            "an existing local branch is never moved by fetch_script"
        );
    }

    #[test]
    fn ignored_list_script_fails_on_a_non_git_dir_but_succeeds_empty_on_a_clean_repo() {
        if !require(&["git", "bash"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();

        // Not a git repo at all: a real failure, never "nothing to carry".
        let plain = tmp.path().join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        let out = bash(&ignored_list_script(plain.to_str().unwrap()), &home);
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains(FAILED));
        assert!(
            payload(&out.stdout).is_none(),
            "no marker on a real failure"
        );

        // A real repo with nothing ignored: a genuinely empty listing.
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("a.txt"), "a\n").unwrap();
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "base"]);
        let out = bash(&ignored_list_script(repo.to_str().unwrap()), &home);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(parse_ignored_list(&out.stdout), Vec::new());
    }

    /// A target clone at the source HEAD, with the transfer refs fetched, and
    /// the snapshot already replayed into it — i.e. exactly the state a move
    /// leaves behind when it fails after the replay.
    fn replayed_target(root: &Path, src: &Path, home: &Path) -> std::path::PathBuf {
        let head = git(src, &["rev-parse", "HEAD"]).trim().to_string();
        let out = bash(
            &snapshot_script(src.to_str().unwrap(), ID, &[], 10_000_000),
            home,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let tgt = root.join("target");
        git(
            root,
            &["clone", "-q", src.to_str().unwrap(), tgt.to_str().unwrap()],
        );
        git(&tgt, &["checkout", "-q", &head]);
        git(
            &tgt,
            &[
                "fetch",
                "-q",
                src.to_str().unwrap(),
                "+refs/fleet/transfer/*:refs/fleet/transfer/*",
            ],
        );
        let out = bash(&apply_script(tgt.to_str().unwrap(), ID, &head), home);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        tgt
    }

    #[test]
    fn verify_adopts_a_target_that_already_holds_exactly_the_snapshot() {
        if !require(&["git", "bash"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let (src, _) = dirty_source(tmp.path());
        let head = git(&src, &["rev-parse", "HEAD"]).trim().to_string();
        let tgt = replayed_target(tmp.path(), &src, &home);

        let out = bash(
            &verify_replayed_script(tgt.to_str().unwrap(), ID, &head),
            &home,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        // The payload is the same porcelain `apply_script` prints, so the
        // move's own verification can run over it unchanged.
        let stdout = String::from_utf8_lossy(&out.stdout);
        let porcelain = parse_apply(&stdout).unwrap();
        assert!(porcelain.contains("staged new.txt"), "{porcelain}");
        assert!(
            !porcelain.contains(".env"),
            "ignored files never appear: {porcelain}"
        );
    }

    #[test]
    fn verify_calls_a_changed_snapshot_path_ours_and_a_new_one_theirs() {
        if !require(&["git", "bash"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let (src, _) = dirty_source(tmp.path());
        let head = git(&src, &["rev-parse", "HEAD"]).trim().to_string();
        let tgt = replayed_target(tmp.path(), &src, &home);

        // A path the snapshot writes, with different content: OURS.
        std::fs::write(tgt.join("mod.txt"), "v3-stale\n").unwrap();
        let out = bash(
            &verify_replayed_script(tgt.to_str().unwrap(), ID, &head),
            &home,
        );
        assert!(!out.status.success());
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains(LEFTOVERS_DIFFER), "{err}");
        let l = parse_leftovers(&err);
        assert_eq!(l.ours, vec!["mod.txt".to_string()], "{l:?}");
        assert!(l.theirs.is_empty(), "{l:?}");

        // A path the snapshot does not hold at all: THEIRS.
        std::fs::write(tgt.join("mod.txt"), "v2\n").unwrap();
        std::fs::write(tgt.join("their-own.txt"), "mine\n").unwrap();
        let out = bash(
            &verify_replayed_script(tgt.to_str().unwrap(), ID, &head),
            &home,
        );
        assert!(!out.status.success());
        let l = parse_leftovers(&String::from_utf8_lossy(&out.stderr));
        assert_eq!(l.theirs, vec!["their-own.txt".to_string()], "{l:?}");
        assert!(l.ours.is_empty(), "{l:?}");
    }

    #[test]
    fn verify_ignores_ignored_files_and_refuses_another_head() {
        if !require(&["git", "bash"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let (src, _) = dirty_source(tmp.path());
        let head = git(&src, &["rev-parse", "HEAD"]).trim().to_string();
        let tgt = replayed_target(tmp.path(), &src, &home);

        // An ignored file the snapshot never carried is not the target's work.
        std::fs::write(tgt.join(".env"), "SECRET=different\n").unwrap();
        let out = bash(
            &verify_replayed_script(tgt.to_str().unwrap(), ID, &head),
            &home,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );

        // A head mismatch is never an adopt.
        let out = bash(
            &verify_replayed_script(tgt.to_str().unwrap(), ID, &"0".repeat(40)),
            &home,
        );
        assert!(!out.status.success());
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains(HEAD_MISMATCH), "{err}");
        assert!(!err.contains(LEFTOVERS_DIFFER), "{err}");
    }

    #[test]
    fn recover_removes_only_what_the_snapshot_added() {
        if !require(&["git", "bash"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let (src, _) = dirty_source(tmp.path());
        let tgt = replayed_target(tmp.path(), &src, &home);
        // The target's own ignored file and its own untracked file survive.
        std::fs::write(tgt.join(".env"), "SECRET=theirs\n").unwrap();
        std::fs::write(tgt.join("their-own.txt"), "mine\n").unwrap();

        let out = bash(&recover_script(tgt.to_str().unwrap(), ID), &home);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let removed = parse_recover(&String::from_utf8_lossy(&out.stdout)).unwrap();
        assert!(
            removed >= 2,
            "the snapshot's added files were removed: {removed}"
        );
        assert!(
            !tgt.join("staged new.txt").exists(),
            "a snapshot addition is gone"
        );
        assert!(!tgt.join("sub").exists(), "its new directory is gone too");
        assert!(
            tgt.join(".env").exists(),
            "an ignored file is never touched"
        );
        assert!(
            tgt.join("their-own.txt").exists(),
            "nor is the target's own file"
        );
        assert_eq!(
            std::fs::read_to_string(tgt.join("mod.txt")).unwrap(),
            "v1\n"
        );
        assert!(tgt.join("del.txt").exists(), "a deletion was rolled back");
        // And the worktree is clean again but for what was never ours.
        let porcelain = git(
            &tgt,
            &["-c", "core.quotePath=true", "status", "--porcelain"],
        );
        assert!(porcelain.contains("their-own.txt"), "{porcelain}");
        assert!(!porcelain.contains("mod.txt"), "{porcelain}");
    }
}
