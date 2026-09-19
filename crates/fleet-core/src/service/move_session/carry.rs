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
    /// The path is not valid UTF-8.
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

/// What a move carried besides the transcript.
#[derive(Debug, Clone, Default, Serialize)]
pub struct CarryReport {
    /// Commits in the bundle besides the two snapshot commits.
    pub commits: u32,
    pub bundle_bytes: u64,
    /// The porcelain rows restored on the target.
    pub dirty_entries: Vec<DirtyFile>,
    pub ignored_carried: Vec<IgnoredEntry>,
    pub ignored_left_behind: Vec<LeftBehind>,
    pub target_seeded: TargetSeed,
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

/// Parse `<kb>\t<path>\0` records; a record without a tab is dropped.
pub fn parse_ignored_list(stdout: &[u8]) -> Vec<ListedIgnored> {
    stdout
        .split(|b| *b == 0)
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
/// Bytes per relay chunk: the orchestrator's peak memory for a payload.
pub const CHUNK_BYTES: u64 = 8 * 1024 * 1024;

fn is_sha(s: &str) -> bool {
    (s.len() == 40 || s.len() == 64) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn parse_err(what: &str, got: &str) -> IpcError {
    IpcError::new(codes::E_PARSE, format!("unexpected {what} output: {got:?}"))
}

/// Make sure the target's main clone exists. Prints `existing`, `cloned` or
/// `initialized`. Never prompts (a host without credentials falls through to
/// `git init`) and never removes a directory it did not create.
pub fn seed_script(project_root: &str, clone_url: &str) -> String {
    format!(
        r#"# cf-carry:seed
set +e
r={r}
url={url}
export GIT_TERMINAL_PROMPT=0 GIT_SSH_COMMAND='ssh -oBatchMode=yes'
if [ -e "$r/.git" ]; then printf 'existing\n'; exit 0; fi
if [ -e "$r" ]; then printf '{FAILED} %s exists and is not a git repository\n' "$r" >&2; exit 5; fi
mkdir -p -- "$(dirname -- "$r")" || {{ printf '{FAILED} mkdir\n' >&2; exit 5; }}
if git clone -q -- "$url" "$r" >/dev/null 2>&1; then printf 'cloned\n'; exit 0; fi
rm -rf -- "$r"
git init -q -- "$r" >/dev/null 2>&1 && git -C "$r" remote add origin "$url" || {{ printf '{FAILED} init\n' >&2; exit 5; }}
printf 'initialized\n'
"#,
        r = quote(project_root),
        url = quote(clone_url),
    )
}

pub fn parse_seed(stdout: &str) -> Result<TargetSeed, IpcError> {
    match stdout.trim() {
        "existing" => Ok(TargetSeed::Existing),
        "cloned" => Ok(TargetSeed::Cloned),
        "initialized" => Ok(TargetSeed::Initialized),
        other => Err(parse_err("seed", other)),
    }
}

/// Create the private transfer dir on the target and list every ref tip it
/// has. Line 1: the absolute dir; then one object name per line.
pub fn haves_script(project_root: &str, claude_id: &str) -> String {
    format!(
        r#"# cf-carry:haves
set +e
r={r}
id={id}
dir="$HOME/.cache/claude-fleet/transfer/$id"
rm -rf -- "$dir"
( umask 077; mkdir -p -- "$dir" ) || {{ printf '{FAILED} mkdir\n' >&2; exit 5; }}
printf '%s\n' "$dir"
git -C "$r" for-each-ref --format='%(objectname)' 2>/dev/null | sort -u
exit 0
"#,
        r = quote(project_root),
        id = quote(claude_id),
    )
}

pub fn parse_haves(stdout: &str) -> Result<(String, Vec<String>), IpcError> {
    let mut lines = stdout.lines();
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
/// `bundle create --stdin` regressed in some git releases. Prints `<bytes>\t<commits>\t<submodules 0|1>\t<lfs 0|1>\t<path>`.
pub fn snapshot_script(
    worktree: &str,
    claude_id: &str,
    haves: &[String],
    cap_bytes: u64,
) -> String {
    let haves: String = haves
        .iter()
        .filter(|h| is_sha(h))
        .map(|h| format!("{h}\n"))
        .collect();
    format!(
        r#"# cf-carry:snapshot
set +e
wt={wt}
id={id}
cap={cap}
fail() {{ printf '{FAILED} %s\n' "$1" >&2; exit 5; }}
cd -- "$wt" 2>/dev/null || fail cd
dir="$HOME/.cache/claude-fleet/transfer/$id"
rm -rf -- "$dir"
( umask 077; mkdir -p -- "$dir" ) || fail mkdir
export GIT_AUTHOR_NAME=claude-fleet GIT_AUTHOR_EMAIL=fleet@localhost GIT_COMMITTER_NAME=claude-fleet GIT_COMMITTER_EMAIL=fleet@localhost
real=$(git rev-parse --git-path index)
cp -- "$real" "$dir/index.ix" 2>/dev/null && cp -- "$real" "$dir/index.wt" 2>/dev/null || fail index
itree=$(GIT_INDEX_FILE="$dir/index.ix" git write-tree 2>/dev/null) || fail write-tree-index
GIT_INDEX_FILE="$dir/index.wt" git add -A >/dev/null 2>&1 || fail add
wtree=$(GIT_INDEX_FILE="$dir/index.wt" git write-tree 2>/dev/null) || fail write-tree-worktree
rm -f -- "$dir/index.ix" "$dir/index.wt"
ix=$(git commit-tree "$itree" -p HEAD -m 'fleet transfer: index' 2>/dev/null) || fail commit-index
w=$(git commit-tree "$wtree" -p HEAD -m 'fleet transfer: worktree' 2>/dev/null) || fail commit-worktree
ref="refs/fleet/transfer/$id"
git update-ref "$ref/ix" "$ix" && git update-ref "$ref/wt" "$w" && git update-ref "$ref/head" HEAD || fail update-ref
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
if [ "$n" -gt "$cap" ]; then printf '{BUNDLE_TOO_LARGE} %s\n' "$n" >&2; exit 8; fi
sub=0; [ -f .gitmodules ] && sub=1
lfs=0; grep -qs 'filter=lfs' .gitattributes && lfs=1
printf '%s\t%s\t%s\t%s\t%s\n' "$n" "${{commits:-0}}" "$sub" "$lfs" "$dir/carry.bundle"
"#,
        wt = quote(worktree),
        id = quote(claude_id),
        cap = cap_bytes,
    )
}

pub fn parse_snapshot(stdout: &str) -> Result<BundleInfo, IpcError> {
    let line = stdout.trim_end_matches('\n');
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
pub fn chunk_script(path: &str, offset: u64, len: u64) -> String {
    format!(
        "# cf-carry:chunk\ntail -c +{} -- {} | head -c {len}\n",
        offset + 1,
        quote(path)
    )
}

/// Verify and fetch the bundle into the target's main clone; create the
/// local branch at the source HEAD when the target has none. An existing
/// local branch is never moved here.
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
fail() {{ printf '{FAILED} %s\n' "$1" >&2; exit 5; }}
git -C "$r" bundle verify "$b" >/dev/null 2>&1 || fail verify
git -C "$r" fetch -q "$b" '+refs/fleet/transfer/*:refs/fleet/transfer/*' >/dev/null 2>&1 || fail fetch
if ! git -C "$r" show-ref --verify --quiet "refs/heads/$br"; then
  git -C "$r" branch -- "$br" "refs/fleet/transfer/$id/head" >/dev/null 2>&1 || fail branch
fi
printf 'ok\n'
"#,
        r = quote(project_root),
        b = quote(bundle_path),
        id = quote(claude_id),
        br = quote(branch),
    )
}

/// Replay the snapshot in the target worktree: working tree := snapshot,
/// index := what was staged. Refuses a dirty worktree ([`TARGET_DIRTY`]) and
/// one not at `want_head` ([`HEAD_MISMATCH`]). Prints the resulting
/// `git status --porcelain=v1`.
pub fn apply_script(cwd: &str, claude_id: &str, want_head: &str) -> String {
    format!(
        r#"# cf-carry:apply
set +e
cwd={cwd}
id={id}
want={want}
fail() {{ printf '{FAILED} %s\n' "$1" >&2; exit 5; }}
cd -- "$cwd" 2>/dev/null || fail cd
if [ -n "$(git status --porcelain 2>/dev/null)" ]; then printf '{TARGET_DIRTY}\n' >&2; exit 9; fi
h=$(git rev-parse HEAD 2>/dev/null)
if [ "$h" != "$want" ]; then printf '{HEAD_MISMATCH} %s\n' "$h" >&2; exit 10; fi
git read-tree -u --reset "refs/fleet/transfer/$id/wt^{{tree}}" >/dev/null 2>&1 || fail read-tree-worktree
git read-tree "refs/fleet/transfer/$id/ix^{{tree}}" >/dev/null 2>&1 || fail read-tree-index
git status --porcelain=v1
"#,
        cwd = quote(cwd),
        id = quote(claude_id),
        want = quote(want_head),
    )
}

/// Best effort: drop the private refs and the transfer dir. Always exits 0.
pub fn cleanup_script(repo_dir: &str, claude_id: &str) -> String {
    format!(
        r#"# cf-carry:cleanup
set +e
r={r}
id={id}
git -C "$r" for-each-ref --format='%(refname)' "refs/fleet/transfer/$id/" 2>/dev/null | while IFS= read -r ref; do
  git -C "$r" update-ref -d "$ref" >/dev/null 2>&1
done
rm -rf -- "$HOME/.cache/claude-fleet/transfer/$id"
exit 0
"#,
        r = quote(repo_dir),
        id = quote(claude_id),
    )
}

/// List the worktree's top-level git-ignored entries as `<kb>\t<path>\0`.
/// A wholly ignored directory is one entry; a deny-listed name is printed
/// with `-1` and never walked by `du`.
pub fn ignored_list_script(worktree: &str) -> String {
    format!(
        r#"# cf-carry:ignored-list
set +e
wt={wt}
deny=' {deny} '
cd -- "$wt" 2>/dev/null || {{ printf '{FAILED} cd\n' >&2; exit 5; }}
git ls-files -o -i --exclude-standard --directory -z 2>/dev/null | while IFS= read -r -d '' p; do
  b=$(basename -- "${{p%/}}")
  case "$deny" in
    *" $b "*) k=-1 ;;
    *) k=$(du -sk -- "$p" 2>/dev/null | cut -f1) ;;
  esac
  printf '%s\t%s\0' "${{k:-0}}" "$p"
done
exit 0
"#,
        wt = quote(worktree),
        deny = DENYLIST.join(" "),
    )
}

/// Tar the chosen entries into the transfer dir. Each path is a quoted argv
/// word prefixed with `./` (so a leading `-` is never an option);
/// `COPYFILE_DISABLE` keeps macOS `._*` files out. Prints `<bytes>\t<path>`.
pub fn ignored_pack_script(worktree: &str, claude_id: &str, paths: &[String]) -> String {
    let argv: Vec<String> = paths.iter().map(|p| quote(&format!("./{p}"))).collect();
    format!(
        r#"# cf-carry:ignored-pack
set +e
wt={wt}
id={id}
cd -- "$wt" 2>/dev/null || {{ printf '{FAILED} cd\n' >&2; exit 5; }}
dir="$HOME/.cache/claude-fleet/transfer/$id"
( umask 077; mkdir -p -- "$dir" ) || {{ printf '{FAILED} mkdir\n' >&2; exit 5; }}
COPYFILE_DISABLE=1 tar -czf "$dir/ignored.tgz" {argv} >/dev/null 2>&1 || {{ printf '{FAILED} tar\n' >&2; exit 5; }}
n=$(wc -c < "$dir/ignored.tgz" | tr -d ' ')
printf '%s\t%s\n' "$n" "$dir/ignored.tgz"
"#,
        wt = quote(worktree),
        id = quote(claude_id),
        argv = argv.join(" "),
    )
}

pub fn parse_pack(stdout: &str) -> Result<(u64, String), IpcError> {
    let line = stdout.trim_end_matches('\n');
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

/// Extract in the target worktree; a file already there wins
/// (`--skip-old-files` on GNU tar, `-k` on BSD tar — GNU's `-k` reports
/// existing files as errors).
pub fn ignored_extract_script(cwd: &str, archive: &str) -> String {
    format!(
        r#"# cf-carry:ignored-extract
set +e
cwd={cwd}
a={a}
cd -- "$cwd" 2>/dev/null || {{ printf '{FAILED} cd\n' >&2; exit 5; }}
tar -tzf "$a" >/dev/null 2>&1 || {{ printf '{FAILED} corrupt archive\n' >&2; exit 5; }}
if tar --version 2>/dev/null | grep -q 'GNU tar'; then k=--skip-old-files; else k=-k; fi
tar -xzf "$a" $k >/dev/null 2>&1 || {{ printf '{FAILED} extract\n' >&2; exit 5; }}
printf 'ok\n'
"#,
        cwd = quote(cwd),
        a = quote(archive),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(path: &str, kb: Option<u64>) -> ListedIgnored {
        ListedIgnored {
            path: path.into(),
            kb,
            valid_name: true,
        }
    }

    #[test]
    fn ignored_list_parses_nul_records_and_flags_bad_names() {
        let mut out = b"4\t.env\0-1\tnode_modules/\0".to_vec();
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

    fn have(bin: &str) -> bool {
        Command::new(bin)
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }

    /// Run a generated script the way a host would, with an isolated `$HOME`
    /// (so `~/.cache/claude-fleet/transfer` lands in the temp dir) and no
    /// user/system git config.
    fn bash(script: &str, home: &Path) -> Output {
        Command::new("bash")
            .args(["-c", script])
            .env("HOME", home)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("bash")
    }

    fn git(dir: &Path, args: &[&str]) -> String {
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

    /// A source repo with every kind of state the carry must reproduce.
    /// Returns (repo dir, sha of the "pushed" base commit).
    fn dirty_source(root: &Path) -> (std::path::PathBuf, String) {
        let src = root.join("src");
        std::fs::create_dir_all(&src).unwrap();
        git(&src, &["init", "-q", "-b", "feat"]);
        for (f, body) in [
            ("keep.txt", "keep\n"),
            ("mod.txt", "v1\n"),
            ("del.txt", "bye\n"),
            ("both.txt", "v1\n"),
            ("mode.sh", "#!/bin/sh\n"),
        ] {
            std::fs::write(src.join(f), body).unwrap();
        }
        std::fs::write(src.join(".gitignore"), ".env\nnode_modules/\n").unwrap();
        git(&src, &["add", "-A"]);
        git(&src, &["commit", "-q", "-m", "base"]);
        let base = git(&src, &["rev-parse", "HEAD"]).trim().to_string();
        git(&src, &["branch", "pushed"]); // stands in for origin/feat: a sha is only fetchable as a ref tip
        for n in ["one", "two"] {
            std::fs::write(src.join(format!("{n}.txt")), n).unwrap();
            git(&src, &["add", "-A"]);
            git(&src, &["commit", "-q", "-m", n]); // two "unpushed" commits
        }
        std::fs::write(src.join("mod.txt"), "v2\n").unwrap(); // modified, unstaged
        std::fs::write(src.join("staged new.txt"), "new\n").unwrap(); // staged, space in name
        git(&src, &["add", "staged new.txt"]);
        std::fs::write(src.join("both.txt"), "staged\n").unwrap(); // staged...
        git(&src, &["add", "both.txt"]);
        std::fs::write(src.join("both.txt"), "then modified\n").unwrap(); // ...then modified
        std::fs::write(src.join("it's untracked.txt"), "u\n").unwrap(); // untracked, quote in name
        std::fs::remove_file(src.join("del.txt")).unwrap(); // deleted, unstaged
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(src.join("mode.sh"), std::fs::Permissions::from_mode(0o755))
                .unwrap();
            std::os::unix::fs::symlink("keep.txt", src.join("link")).unwrap();
        }
        std::fs::write(src.join(".env"), "SECRET=1\n").unwrap(); // ignored: must NOT be in the snapshot
        (src, base)
    }

    /// Everything that must be byte-identical on the source before and after.
    fn source_fingerprint(src: &Path) -> (String, Vec<u8>, String) {
        (
            // --no-optional-locks: a plain `git status` may refresh and rewrite the index.
            git(src, &["--no-optional-locks", "status", "--porcelain=v1"]),
            std::fs::read(src.join(".git/index")).unwrap(),
            git(
                src,
                &["for-each-ref", "refs/heads", "refs/remotes", "refs/tags"],
            ),
        )
    }

    /// snapshot → bundle → fetch → worktree add → apply, all through the real
    /// generated scripts. `seed_target` prepares the target's main clone.
    fn round_trip(seed_target: impl Fn(&Path, &Path, &str)) {
        if !have("git") || !have("bash") {
            eprintln!("skipping: git or bash is not available");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let (home_a, home_b) = (tmp.path().join("home-a"), tmp.path().join("home-b"));
        std::fs::create_dir_all(&home_a).unwrap();
        std::fs::create_dir_all(&home_b).unwrap();
        let (src, base) = dirty_source(tmp.path());
        let before = source_fingerprint(&src);
        let src_head = git(&src, &["rev-parse", "HEAD"]).trim().to_string();

        // Target main clone.
        let root = tmp.path().join("tgt");
        seed_target(&src, &root, &base);
        let out = bash(&haves_script(root.to_str().unwrap(), ID), &home_b);
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
            assert!(!out.stdout.is_empty(), "empty chunk at {}", got.len());
            got.extend_from_slice(&out.stdout);
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
        assert_eq!(
            sorted_lines(&String::from_utf8_lossy(&out.stdout)),
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
        round_trip(|src, root, base| {
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
        round_trip(|_src, root, _base| {
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
    fn a_thin_bundle_is_smaller_than_a_full_one_and_a_clean_source_still_bundles() {
        if !have("git") || !have("bash") {
            eprintln!("skipping: git or bash is not available");
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
        if !have("git") || !have("bash") {
            eprintln!("skipping: git or bash is not available");
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
        if !have("git") || !have("bash") {
            eprintln!("skipping: git or bash is not available");
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
        assert!(parse_seed("weird\n").is_err());
        assert_eq!(parse_seed("cloned\n").unwrap(), TargetSeed::Cloned);
        assert!(parse_haves("relative/dir\n").is_err());
        let (dir, haves) = parse_haves(&format!(
            "/h/.cache/x\n{}\nnot-a-sha\n{}\n",
            "a".repeat(40),
            "b".repeat(64)
        ))
        .unwrap();
        assert_eq!(dir, "/h/.cache/x");
        assert_eq!(haves.len(), 2, "non-hex lines are dropped");
        assert!(parse_snapshot("12\t3\t0\t1\t/abs/carry.bundle\n").is_ok());
        assert!(parse_snapshot("12\t3\t0\t1\trelative\n").is_err());
        assert!(parse_snapshot("x\t3\t0\t1\t/abs\n").is_err());
    }

    #[test]
    fn carry_scripts_quote_every_interpolated_value() {
        let evil = "a b'$(touch /tmp/pwn)\n;x";
        let q = quote(evil);
        for script in [
            seed_script(evil, evil),
            haves_script(evil, evil),
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
        if !have("git") || !have("bash") || !have("tar") {
            eprintln!("skipping: git, bash or tar is not available");
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
        assert!(parse_pack("12\t/abs/ignored.tgz\n").is_ok());
        assert!(parse_pack("12\trelative\n").is_err());
    }
}
