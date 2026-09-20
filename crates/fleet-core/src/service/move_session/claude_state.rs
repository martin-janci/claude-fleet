//! The Claude-side state of a moved session: its per-session directory
//! (subagent transcripts, tool results, title) and the project's memory.
//! Pure, like `carry.rs`: policies, script builders, parsers — `mod.rs` runs
//! the scripts. Both halves only ever ADD to the target.
//! See `docs/superpowers/specs/2026-09-20-move-carry-claude-state-design.md`.

use crate::service::move_session::carry::{
    home_guard, id_guard, payload, payload_str, IgnoredEntry, LeftBehind, LeftReason, FAILED,
    OUT_MARKER,
};
use crate::shell::quote;
use std::collections::HashSet;

/// `settings` key: largest per-session directory (MiB) a move carries.
pub const SETTING_MAX_SESSION_STATE_MB: &str = "move.max_session_state_mb";
pub const DEFAULT_MAX_SESSION_STATE_MB: u64 = 200;
pub const MAX_SESSION_EXCLUDES: usize = 200;
pub const MEMORY_FILE_MAX_BYTES: u64 = 1 << 20;
pub const MEMORY_TOTAL_MAX_BYTES: u64 = 8 << 20;
pub const MEMORY_MAX_FILES: usize = 300;
pub const INDEX_READ_MAX_BYTES: u64 = 256 * 1024;
pub const INDEX_APPEND_MAX_BYTES: usize = 32 * 1024;
pub const INDEX_NAME: &str = "MEMORY.md";
/// The heredoc delimiter of the index-append script; never allowed as a line.
pub(super) const INDEX_HEREDOC: &str = "CF_INDEX";

fn records(stdout: &[u8]) -> impl Iterator<Item = Vec<&str>> {
    payload(stdout)
        .unwrap_or_default()
        .split(|b| *b == 0)
        .filter(|r| !r.is_empty())
        .filter_map(|r| std::str::from_utf8(r).ok())
        .map(|r| r.split('\t').collect())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedFile {
    pub path: String,
    pub bytes: u64,
}

/// `<bytes>\t<path>\0` records after the marker. No marker, a non-UTF-8 or a
/// malformed record → dropped: a listing can never fail a move.
pub fn parse_file_list(stdout: &[u8]) -> Vec<ListedFile> {
    records(stdout)
        .filter_map(|p| match p.as_slice() {
            [n, path] => Some(ListedFile {
                path: path.to_string(),
                bytes: n.trim().parse().ok()?,
            }),
            _ => None,
        })
        .collect()
}

fn safe_session_path(p: &str) -> bool {
    !p.is_empty()
        && p.chars()
            .all(|c| c.is_ascii_alphanumeric() || "._/-".contains(c))
        && !p.split('/').any(|seg| seg == ".." || seg.is_empty())
}

/// A tar exclude pattern for `path` that cannot carry shell or glob syntax of
/// the caller's making: every char outside the safe set becomes `?` (one-char
/// wildcard on GNU and BSD tar alike).
pub fn exclude_pattern(path: &str) -> String {
    path.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "._/-".contains(c) {
                c
            } else {
                '?'
            }
        })
        .collect()
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionSelection {
    pub carry: Vec<IgnoredEntry>,
    /// Exclude patterns, relative to `<id>/`.
    pub exclude: Vec<String>,
    pub left: Vec<LeftBehind>,
    /// Set when the half must be skipped; `carry`/`exclude` are then empty.
    pub skip: Option<String>,
}

/// The whole directory travels unless it is over `cap_bytes`; then the
/// largest files stay behind, one by one, until the rest fits.
pub fn select_session_files(listed: Vec<ListedFile>, cap_bytes: u64) -> SessionSelection {
    let mut sel = SessionSelection::default();
    let mut ok: Vec<ListedFile> = Vec::new();
    for f in listed {
        if safe_session_path(&f.path) {
            ok.push(f);
        } else {
            sel.exclude.push(exclude_pattern(&f.path));
            sel.left.push(LeftBehind {
                path: f.path,
                bytes: Some(f.bytes),
                reason: LeftReason::UnsupportedName,
            });
        }
    }
    ok.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.path.cmp(&b.path)));
    // A u128 running total: summing as u64 can saturate on a pathological
    // listing, and decrementing that already-saturated value with a plain
    // `-=` panics on underflow; even `saturating_sub` on a saturated u64
    // total would silently under-report what is left to remove (the excess
    // above u64::MAX is gone once saturation happens). u128 has enough
    // headroom that the sum of any listing that fits in memory never
    // saturates, so decrementing it stays exact.
    let mut total: u128 = ok.iter().map(|f| u128::from(f.bytes)).sum();
    let cap = u128::from(cap_bytes);
    let mut keep_from = 0;
    while total > cap && keep_from < ok.len() {
        let f = &ok[keep_from];
        total = total.saturating_sub(u128::from(f.bytes));
        sel.exclude.push(f.path.clone());
        sel.left.push(LeftBehind {
            path: f.path.clone(),
            bytes: Some(f.bytes),
            reason: LeftReason::OverCap,
        });
        keep_from += 1;
    }
    if sel.exclude.len() > MAX_SESSION_EXCLUDES {
        // Every file not yet walked would still have to be excluded to fit —
        // report it too, so the report never silently drops files.
        for f in &ok[keep_from..] {
            sel.left.push(LeftBehind {
                path: f.path.clone(),
                bytes: Some(f.bytes),
                reason: LeftReason::OverCap,
            });
        }
        return SessionSelection {
            left: sel.left,
            skip: Some(format!(
                "more than {MAX_SESSION_EXCLUDES} files would have to stay behind; raise {SETTING_MAX_SESSION_STATE_MB}"
            )),
            ..Default::default()
        };
    }
    sel.carry = ok[keep_from..]
        .iter()
        .map(|f| IgnoredEntry {
            path: f.path.clone(),
            bytes: f.bytes,
        })
        .collect();
    sel.carry.sort_by(|a, b| a.path.cmp(&b.path));
    sel
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedMemory {
    pub hash: String,
    pub bytes: u64,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryListing {
    /// Absolute path of the memory dir (it may not exist yet).
    pub dir: String,
    pub exists: bool,
    pub files: Vec<ListedMemory>,
}

/// First record `dir\t<abs path>\t<0|1>`, then `<hash>\t<bytes>\t<name>`.
pub fn parse_memory_list(stdout: &[u8]) -> Option<MemoryListing> {
    let mut recs = records(stdout);
    let (dir, exists) = match recs.next()?.as_slice() {
        ["dir", d, e] if d.starts_with('/') => (d.to_string(), *e == "1"),
        _ => return None,
    };
    let files = recs
        .filter_map(|p| match p.as_slice() {
            [h, n, name] => Some(ListedMemory {
                hash: h.to_string(),
                bytes: n.trim().parse().ok()?,
                name: name.to_string(),
            }),
            _ => None,
        })
        .collect();
    Some(MemoryListing { dir, exists, files })
}

/// Deliberately stricter than the spec's regex: a leading `.` is rejected even
/// though the regex would allow it — hidden files are not memory.
fn safe_memory_name(n: &str) -> bool {
    n.len() > 3
        && n.ends_with(".md")
        && !n.starts_with('.')
        && n.chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemoryDecision {
    pub carry: Vec<IgnoredEntry>,
    pub kept_target: Vec<String>,
    pub identical: u32,
    pub left: Vec<LeftBehind>,
}

/// Memory only ever adds: a name the target has is never carried, and the
/// index is merged line-wise elsewhere, never copied. Names are compared
/// ASCII-case-insensitively throughout — the index filter, the target match,
/// and source-vs-source collisions — because a case-insensitive target
/// volume (macOS default) treats `Note.md` and `note.md` as one file.
pub fn decide_memory(source: &[ListedMemory], target: &[ListedMemory]) -> MemoryDecision {
    let mut d = MemoryDecision::default();
    let mut src: Vec<&ListedMemory> = source
        .iter()
        .filter(|f| !f.name.eq_ignore_ascii_case(INDEX_NAME))
        .collect();
    src.sort_by(|a, b| a.name.cmp(&b.name));
    let mut total = 0u64;
    let mut seen_ci: HashSet<String> = HashSet::new();
    for f in src {
        let left = |reason| LeftBehind {
            path: f.name.clone(),
            bytes: Some(f.bytes),
            reason,
        };
        if !safe_memory_name(&f.name) {
            d.left.push(left(LeftReason::UnsupportedName));
        } else if !seen_ci.insert(f.name.to_ascii_lowercase()) {
            // A case-insensitive duplicate of an earlier source file (two
            // names that only exist as distinct files on a case-sensitive
            // source): only the first in name order is a candidate to carry.
            d.left.push(left(LeftReason::UnsupportedName));
        } else if let Some(t) = target.iter().find(|t| t.name.eq_ignore_ascii_case(&f.name)) {
            if t.hash == f.hash {
                d.identical += 1
            } else {
                d.kept_target.push(f.name.clone())
            }
        } else if f.bytes > MEMORY_FILE_MAX_BYTES
            || d.carry.len() >= MEMORY_MAX_FILES
            || total.saturating_add(f.bytes) > MEMORY_TOTAL_MAX_BYTES
        {
            d.left.push(left(LeftReason::OverCap));
        } else {
            total += f.bytes;
            d.carry.push(IgnoredEntry {
                path: f.name.clone(),
                bytes: f.bytes,
            });
        }
    }
    d
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IndexMerge {
    /// Exactly the text to append to the target's index ("" → do nothing).
    pub append: String,
    pub lines: u32,
}

/// The file a line's FIRST markdown link points at (`](name.md)`), without
/// a leading `./`.
fn link_target(line: &str) -> Option<&str> {
    let rest = &line[line.find("](")? + 2..];
    let t = &rest[..rest.find(')')?];
    Some(t.strip_prefix("./").unwrap_or(t))
}

/// An index line travels only with its carried file. The target's own lines
/// are never touched — this only produces text to append.
pub fn merge_index(
    source_index: &str,
    target_index: Option<&str>,
    carried: &[String],
) -> IndexMerge {
    let mut body = String::new();
    let mut lines = 0u32;
    for line in source_index.lines() {
        // Case-insensitive: the carried name and the index's link spelling
        // can differ only in case (same reasoning as `decide_memory`).
        let travels =
            link_target(line).is_some_and(|t| carried.iter().any(|c| c.eq_ignore_ascii_case(t)));
        if !travels || line == INDEX_HEREDOC {
            continue;
        }
        if body.len() + line.len() + 1 > INDEX_APPEND_MAX_BYTES - 64 {
            break;
        }
        body.push_str(line);
        body.push('\n');
        lines += 1;
    }
    if lines == 0 {
        return IndexMerge::default();
    }
    let prefix = match target_index {
        None => "# Memory Index\n\n",
        Some(t) if !t.is_empty() && !t.ends_with('\n') => "\n",
        Some(_) => "",
    };
    IndexMerge {
        append: format!("{prefix}{body}"),
        lines,
    }
}

/// Every regular file under `<project dir>/<id>/` as `<bytes>\t<path>\0`,
/// the path relative to `<id>/`. Symlinks and special files are not listed —
/// and the merge moves regular files only, so they never travel. No such
/// directory is not an error. Streaming: a `find` failure is detected after
/// the marker; callers check the exit status first.
pub fn session_list_script(project_dir: &str, claude_id: &str) -> String {
    format!(
        r#"# cf-carry:state-list
set +e
d={d}
id={id}
{id_guard}
cd -- "$d" 2>/dev/null || {{ printf '{FAILED} cd\n' >&2; exit 5; }}
printf '\n{OUT_MARKER}\n'
[ -d "./$id" ] || exit 0
cd -- "./$id" || {{ printf '{FAILED} cd-id\n' >&2; exit 5; }}
find . -type f -exec sh -c 'for f; do n=$(wc -c < "$f" | tr -d " "); printf "%s\t%s\0" "${{n:-0}}" "${{f#./}}"; done' _ {{}} +
[ "$?" -eq 0 ] || {{ printf '{FAILED} find\n' >&2; exit 5; }}
"#,
        d = quote(project_dir),
        id = quote(claude_id),
        id_guard = id_guard(),
    )
}

/// One tar of `./<id>` minus `excludes` (patterns relative to `<id>/`, each
/// given in both member-name spellings so GNU and BSD tar agree).
pub fn session_pack_script(project_dir: &str, claude_id: &str, excludes: &[String]) -> String {
    let ex: Vec<String> = excludes
        .iter()
        .flat_map(|p| {
            [
                format!("--exclude=./{claude_id}/{p}"),
                format!("--exclude={claude_id}/{p}"),
            ]
        })
        .map(|a| quote(&a))
        .collect();
    format!(
        r#"# cf-carry:state-pack
set +e
d={d}
id={id}
{id_guard}
{home_guard}
umask 077
cd -- "$d" 2>/dev/null || {{ printf '{FAILED} cd\n' >&2; exit 5; }}
dir="$HOME/.cache/claude-fleet/transfer/$id"
mkdir -p -- "$dir" || {{ printf '{FAILED} mkdir\n' >&2; exit 5; }}
COPYFILE_DISABLE=1 tar -czf "$dir/state.tgz" {ex} "./$id" >/dev/null 2>&1 || {{ printf '{FAILED} tar\n' >&2; exit 5; }}
n=$(wc -c < "$dir/state.tgz" | tr -d ' ')
[ -n "$n" ] || {{ printf '{FAILED} size\n' >&2; exit 5; }}
printf '\n{OUT_MARKER}\n'
printf '%s\t%s\n' "$n" "$dir/state.tgz"
"#,
        d = quote(project_dir),
        id = quote(claude_id),
        ex = ex.join(" "),
        id_guard = id_guard(),
        home_guard = home_guard(),
    )
}

/// Extract into a staging dir inside the transfer dir — never in place —
/// then move each staged REGULAR file into `<target project dir>/<id>/` iff
/// the target has no such file or a strictly smaller one (these files are
/// append-only: the larger copy is the newer one).
pub fn session_merge_script(target_project_dir: &str, claude_id: &str, archive: &str) -> String {
    format!(
        r#"# cf-carry:state-merge
set +e
d={d}
id={id}
a={a}
{id_guard}
{home_guard}
umask 077
fail() {{ printf '{FAILED} %s\n' "$1" >&2; exit 5; }}
stage="$HOME/.cache/claude-fleet/transfer/$id/state-staging"
rm -rf -- "$stage"
mkdir -p -- "$stage" || fail mkdir
tar -tzf "$a" >/dev/null 2>&1 || fail corrupt
tar -xzf "$a" -C "$stage" >/dev/null 2>&1 || fail extract
mkdir -p -- "$d/$id" || fail mkdir-target
printf '\n{OUT_MARKER}\n'
if cd -- "$stage/$id" 2>/dev/null; then
  find . -type f -print0 | while IFS= read -r -d '' f; do
    rel=${{f#./}}
    dst="$d/$id/$rel"
    s=$(wc -c < "$f" | tr -d ' ')
    t=-1
    if [ -f "$dst" ] && [ ! -L "$dst" ]; then t=$(wc -c < "$dst" | tr -d ' '); elif [ -e "$dst" ] || [ -L "$dst" ]; then t=999999999999; fi
    if [ "${{t:--1}}" -lt "${{s:-0}}" ]; then
      if mkdir -p -- "$(dirname -- "$dst")" && mv -f -- "$f" "$dst"; then printf 'carried\t%s\t%s\n' "$s" "$rel"; else printf 'failed\t%s\n' "$rel"; fi
    else
      printf 'kept\t%s\n' "$rel"
    fi
  done
fi
cd / 2>/dev/null
rm -rf -- "$stage"
exit 0
"#,
        d = quote(target_project_dir),
        id = quote(claude_id),
        a = quote(archive),
        id_guard = id_guard(),
        home_guard = home_guard(),
    )
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MergeResult {
    pub carried: Vec<IgnoredEntry>,
    pub kept: Vec<String>,
    pub failed: Vec<String>,
}

/// Lines after the marker: `carried\t<bytes>\t<path>`, `kept\t<path>`,
/// `failed\t<path>`. `None` without the marker.
pub fn parse_merge(stdout: &str) -> Option<MergeResult> {
    let mut r = MergeResult::default();
    for line in payload_str(stdout)?.lines() {
        match line.split('\t').collect::<Vec<_>>().as_slice() {
            ["carried", n, path] => r.carried.push(IgnoredEntry {
                path: path.to_string(),
                bytes: n.trim().parse().ok()?,
            }),
            ["kept", path] => r.kept.push(path.to_string()),
            ["failed", path] => r.failed.push(path.to_string()),
            _ => {}
        }
    }
    Some(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::move_session::carry::tests::{bash, require};
    use crate::service::move_session::carry::{LeftReason, OUT_MARKER};
    use std::os::unix::fs::PermissionsExt;

    const ID: &str = "550e8400-e29b-41d4-a716-446655440000";

    /// A source project dir whose NAME begins with `-`, like every real one.
    fn source_project(root: &std::path::Path) -> std::path::PathBuf {
        let p = root.join("-Users-me-r--claude-worktrees-feat");
        for (rel, body) in [
            ("subagents/agent-aa.jsonl", "aaaaaaaaaa\n".repeat(50)), // 550 B
            ("subagents/agent-aa.meta.json", "{}".to_string()),
            ("subagents/agent-bb.jsonl", "b\n".repeat(2000)), // 4000 B — the largest
            ("tool-results/out1.txt", "tool output\n".to_string()),
            ("workflows/w1/step.json", "{}".to_string()),
            ("custom-title.json", "{\"title\":\"t\"}".to_string()),
        ] {
            let f = p.join(ID).join(rel);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(&f, body).unwrap();
            std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        std::os::unix::fs::symlink("/etc/hosts", p.join(ID).join("link-out")).unwrap();
        std::fs::write(p.join(format!("{ID}.jsonl")), "main transcript\n").unwrap(); // must NOT be listed or packed
        p
    }

    const SESSION_RELS: [&str; 6] = [
        "subagents/agent-aa.jsonl",
        "subagents/agent-aa.meta.json",
        "subagents/agent-bb.jsonl",
        "tool-results/out1.txt",
        "workflows/w1/step.json",
        "custom-title.json",
    ];

    /// `(rel path, real size on disk)` for every fixture file under `<root>/<id>/`.
    fn real_sizes(root: &std::path::Path, id: &str, rels: &[&str]) -> Vec<(String, u64)> {
        let mut v: Vec<(String, u64)> = rels
            .iter()
            .map(|r| {
                let bytes = std::fs::metadata(root.join(id).join(r)).unwrap().len();
                (r.to_string(), bytes)
            })
            .collect();
        v.sort();
        v
    }

    fn mode(p: &std::path::Path) -> u32 {
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn session_state_is_listed_packed_staged_and_merged() {
        if !require(&["bash", "tar"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let src = source_project(tmp.path());

        // --- list ---
        let out = bash(&session_list_script(src.to_str().unwrap(), ID), &home);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let listed = parse_file_list(&out.stdout);
        let mut got: Vec<(String, u64)> =
            listed.iter().map(|f| (f.path.clone(), f.bytes)).collect();
        got.sort();
        assert_eq!(got, real_sizes(&src, ID, &SESSION_RELS));

        // --- select (whole thing fits) ---
        let sel = select_session_files(listed, u64::MAX);
        assert!(sel.exclude.is_empty() && sel.left.is_empty() && sel.skip.is_none());
        assert_eq!(sel.carry.len(), 6);

        // --- pack ---
        let out = bash(
            &session_pack_script(src.to_str().unwrap(), ID, &sel.exclude),
            &home,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let (bytes, archive) =
            crate::service::move_session::carry::parse_pack(&String::from_utf8_lossy(&out.stdout))
                .unwrap();
        let archive = std::path::PathBuf::from(archive);
        assert_eq!(bytes, std::fs::metadata(&archive).unwrap().len());
        assert_eq!(mode(&archive), 0o600, "archive itself is private");
        let transfer_dir = home.join(".cache/claude-fleet/transfer").join(ID);
        assert_eq!(mode(&transfer_dir), 0o700, "transfer dir is private");

        // --- merge into a FRESH target project dir ---
        let tgt = tmp.path().join("-Users-other--claude-worktrees-feat");
        assert!(!tgt.exists(), "must start out absent");
        let out = bash(
            &session_merge_script(tgt.to_str().unwrap(), ID, archive.to_str().unwrap()),
            &home,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let merged = parse_merge(&String::from_utf8_lossy(&out.stdout)).unwrap();
        assert_eq!(merged.carried.len(), 6, "{merged:?}");
        assert!(merged.kept.is_empty() && merged.failed.is_empty());

        for rel in SESSION_RELS {
            let s = src.join(ID).join(rel);
            let t = tgt.join(ID).join(rel);
            assert_eq!(
                std::fs::read(&s).unwrap(),
                std::fs::read(&t).unwrap(),
                "{rel}"
            );
            assert_eq!(mode(&t), 0o600, "{rel}");
        }
        for d in ["subagents", "tool-results", "workflows", "workflows/w1"] {
            assert_eq!(mode(&tgt.join(ID).join(d)), 0o700, "{d}");
        }
        assert!(
            std::fs::symlink_metadata(tgt.join(ID).join("link-out")).is_err(),
            "the symlink never travels"
        );
        assert!(
            !tgt.join(format!("{ID}.jsonl")).exists(),
            "the sibling transcript is untouched by the merge"
        );
        assert!(
            !home
                .join(".cache/claude-fleet/transfer")
                .join(ID)
                .join("state-staging")
                .exists(),
            "staging dir is cleaned up"
        );
        let top: Vec<_> = std::fs::read_dir(&tgt)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(top, vec![std::ffi::OsString::from(ID)], "{top:?}");
    }

    #[test]
    fn merge_keeps_an_equal_or_larger_target_copy_and_replaces_a_smaller_one() {
        if !require(&["bash", "tar"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let src = source_project(tmp.path());
        let out = bash(&session_pack_script(src.to_str().unwrap(), ID, &[]), &home);
        assert!(out.status.success());
        let (_, archive) =
            crate::service::move_session::carry::parse_pack(&String::from_utf8_lossy(&out.stdout))
                .unwrap();

        let tgt = tmp.path().join("-Users-other--claude-worktrees-feat");
        let aa_smaller = "x\n".repeat(10); // smaller than the source's 550 B
        let bb_bigger = "b\n".repeat(2500); // bigger than the source's 4000 B
        let title_identical = "{\"title\":\"t\"}";
        for (rel, body) in [
            ("subagents/agent-aa.jsonl", aa_smaller.as_str()),
            ("subagents/agent-bb.jsonl", bb_bigger.as_str()),
            ("custom-title.json", title_identical),
            ("subagents/agent-own.jsonl", "only-on-target\n"),
        ] {
            let f = tgt.join(ID).join(rel);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(&f, body).unwrap();
        }

        let out = bash(
            &session_merge_script(tgt.to_str().unwrap(), ID, &archive),
            &home,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let merged = parse_merge(&String::from_utf8_lossy(&out.stdout)).unwrap();

        // aa: smaller on target -> replaced by the source's copy.
        assert_eq!(
            std::fs::read(tgt.join(ID).join("subagents/agent-aa.jsonl")).unwrap(),
            std::fs::read(src.join(ID).join("subagents/agent-aa.jsonl")).unwrap()
        );
        // bb and the identical title: never touched.
        assert_eq!(
            std::fs::read_to_string(tgt.join(ID).join("subagents/agent-bb.jsonl")).unwrap(),
            bb_bigger
        );
        assert_eq!(
            std::fs::read_to_string(tgt.join(ID).join("custom-title.json")).unwrap(),
            title_identical
        );
        // a target-only file is untouched and never mentioned.
        assert_eq!(
            std::fs::read_to_string(tgt.join(ID).join("subagents/agent-own.jsonl")).unwrap(),
            "only-on-target\n"
        );

        let mut carried: Vec<&str> = merged.carried.iter().map(|e| e.path.as_str()).collect();
        carried.sort();
        assert_eq!(
            carried,
            vec![
                "subagents/agent-aa.jsonl",
                "subagents/agent-aa.meta.json",
                "tool-results/out1.txt",
                "workflows/w1/step.json",
            ]
        );
        let mut kept = merged.kept.clone();
        kept.sort();
        assert_eq!(kept, vec!["custom-title.json", "subagents/agent-bb.jsonl"]);
        assert!(merged.failed.is_empty());
        assert!(!merged
            .carried
            .iter()
            .any(|e| e.path == "subagents/agent-own.jsonl"));
    }

    #[test]
    fn excluded_files_do_not_travel_on_either_tar() {
        if !require(&["bash", "tar"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let src = source_project(tmp.path());
        std::fs::write(src.join(ID).join("we ird.txt"), "x").unwrap();

        let excludes = vec![
            "subagents/agent-bb.jsonl".to_string(),
            exclude_pattern("we ird.txt"),
        ];
        let out = bash(
            &session_pack_script(src.to_str().unwrap(), ID, &excludes),
            &home,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let (_, archive) =
            crate::service::move_session::carry::parse_pack(&String::from_utf8_lossy(&out.stdout))
                .unwrap();

        let listing = std::process::Command::new("tar")
            .args(["-tzf", &archive])
            .output()
            .unwrap();
        assert!(listing.status.success());
        let names = String::from_utf8_lossy(&listing.stdout);
        assert!(
            !names.contains("agent-bb.jsonl"),
            "excluded (plain name): {names}"
        );
        assert!(
            !names.contains("we ird.txt"),
            "excluded (odd name): {names}"
        );
        for present in [
            "agent-aa.jsonl",
            "agent-aa.meta.json",
            "out1.txt",
            "step.json",
            "custom-title.json",
        ] {
            assert!(names.contains(present), "missing {present}: {names}");
        }
    }

    #[test]
    fn a_source_without_a_session_directory_lists_nothing_and_succeeds() {
        if !require(&["bash"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let src = tmp.path().join("-Users-me-r--claude-worktrees-empty");
        std::fs::create_dir_all(&src).unwrap();
        // no `<id>/` under it at all

        let out = bash(&session_list_script(src.to_str().unwrap(), ID), &home);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            crate::service::move_session::carry::payload(&out.stdout).is_some(),
            "marker must still be printed"
        );
        assert!(parse_file_list(&out.stdout).is_empty());
    }

    #[test]
    fn session_scripts_refuse_a_bad_id_fail_cleanly_and_quote_everything() {
        if !require(&["bash", "tar"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let src = source_project(tmp.path());
        let tgt = tmp.path().join("-Users-other--claude-worktrees-feat");
        std::fs::create_dir_all(&tgt).unwrap();

        let other_dir = home.join(".cache/claude-fleet/transfer/other");
        std::fs::create_dir_all(&other_dir).unwrap();
        std::fs::write(other_dir.join("keep.txt"), "keep").unwrap();

        for bad in ["", "a/b", ".."] {
            for (name, script) in [
                ("list", session_list_script(src.to_str().unwrap(), bad)),
                ("pack", session_pack_script(src.to_str().unwrap(), bad, &[])),
                (
                    "merge",
                    session_merge_script(tgt.to_str().unwrap(), bad, "/nonexistent.tgz"),
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
                std::fs::read_to_string(other_dir.join("keep.txt")).unwrap(),
                "keep",
                "a sibling transfer dir must survive a bad id {bad:?}"
            );
        }

        // A corrupt archive: merge must fail, without the marker, target untouched.
        let corrupt = tmp.path().join("corrupt.tgz");
        std::fs::write(&corrupt, b"not a tarball").unwrap();
        let fresh_tgt = tmp.path().join("-Users-other--claude-worktrees-fresh");
        let out = bash(
            &session_merge_script(fresh_tgt.to_str().unwrap(), ID, corrupt.to_str().unwrap()),
            &home,
        );
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains(FAILED));
        assert!(
            crate::service::move_session::carry::payload(&out.stdout).is_none(),
            "no marker on failure"
        );
        assert!(!fresh_tgt.join(ID).exists(), "target untouched");

        // Every interpolated value goes through `quote`; the raw payload never leaks.
        let evil = "a b'$(touch /tmp/pwn)\n;x";
        let q = quote(evil);
        // `session_pack_script`'s excludes are re-quoted in two extra
        // spellings (`--exclude=./<id>/<p>` and `--exclude=<id>/<p>`), each
        // its own quoted argv word — account for those too.
        let pack_ex1 = quote(&format!("--exclude=./{evil}/{evil}"));
        let pack_ex2 = quote(&format!("--exclude={evil}/{evil}"));
        for script in [
            session_list_script(evil, evil),
            session_pack_script(evil, evil, &[evil.to_string()]),
            session_merge_script(evil, evil, evil),
        ] {
            assert!(script.contains(&q), "{script}");
            let without = script
                .replace(&q, "")
                .replace(&pack_ex1, "")
                .replace(&pack_ex2, "");
            assert!(
                !without.contains("touch /tmp/pwn"),
                "raw value leaked: {script}"
            );
        }

        // A login-shell banner (no trailing newline of its own) must not
        // confuse marker detection in the list or merge scripts.
        let banner = "printf 'Welcome'; ";
        let out = bash(
            &format!("{banner}{}", session_list_script(src.to_str().unwrap(), ID)),
            &home,
        );
        assert!(out.status.success());
        assert_eq!(parse_file_list(&out.stdout).len(), 6);

        let out = bash(&session_pack_script(src.to_str().unwrap(), ID, &[]), &home);
        assert!(out.status.success());
        let (_, archive) =
            crate::service::move_session::carry::parse_pack(&String::from_utf8_lossy(&out.stdout))
                .unwrap();
        let banner_tgt = tmp.path().join("-Users-other--claude-worktrees-banner");
        let out = bash(
            &format!(
                "{banner}{}",
                session_merge_script(banner_tgt.to_str().unwrap(), ID, &archive)
            ),
            &home,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let merged = parse_merge(&String::from_utf8_lossy(&out.stdout)).unwrap();
        assert_eq!(merged.carried.len(), 6);
    }

    fn marked(body: &[u8]) -> Vec<u8> {
        let mut v = format!("banner\n\n{OUT_MARKER}\n").into_bytes();
        v.extend_from_slice(body);
        v
    }
    fn f(path: &str, bytes: u64) -> ListedFile {
        ListedFile {
            path: path.into(),
            bytes,
        }
    }
    fn m(name: &str, hash: &str, bytes: u64) -> ListedMemory {
        ListedMemory {
            hash: hash.into(),
            bytes,
            name: name.into(),
        }
    }

    #[test]
    fn file_list_parses_after_the_marker_and_is_empty_without_one() {
        let got = parse_file_list(&marked(
            b"2048\tsubagents/agent-ab.jsonl\x0012\tcustom-title.json\x00garbage\x00",
        ));
        assert_eq!(got.len(), 2, "the record without a tab is dropped");
        assert_eq!(
            (got[0].path.as_str(), got[0].bytes),
            ("subagents/agent-ab.jsonl", 2048)
        );
        assert!(parse_file_list(b"12\tno-marker\0").is_empty());
    }

    #[test]
    fn under_the_cap_the_whole_session_directory_travels() {
        let sel = select_session_files(
            vec![
                f("subagents/a.jsonl", 600),
                f("tool-results/x.txt", 300),
                f("custom-title.json", 20),
            ],
            1000,
        );
        assert!(sel.exclude.is_empty() && sel.left.is_empty() && sel.skip.is_none());
        assert_eq!(sel.carry.len(), 3);
    }

    #[test]
    fn over_the_cap_the_largest_files_stay_behind_first() {
        let sel = select_session_files(
            vec![
                f("subagents/big.jsonl", 900),
                f("subagents/mid.jsonl", 400),
                f("subagents/small.jsonl", 100),
                f("custom-title.json", 20),
            ],
            600,
        );
        assert_eq!(
            sel.exclude,
            vec!["subagents/big.jsonl"],
            "dropping the largest is enough: 520 <= 600"
        );
        assert_eq!(sel.left[0].reason, LeftReason::OverCap);
        assert_eq!(sel.left[0].bytes, Some(900));
        let carried: Vec<&str> = sel.carry.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(carried.len(), 3);
        assert!(!carried.contains(&"subagents/big.jsonl"));
    }

    #[test]
    fn odd_names_are_excluded_by_a_wildcard_pattern_never_interpolated_raw() {
        let sel = select_session_files(
            vec![
                f("ok.json", 1),
                f("we ird*[x].txt", 1),
                f("../escape", 1),
                f("a/../b", 1),
            ],
            u64::MAX,
        );
        assert_eq!(sel.carry.len(), 1);
        assert_eq!(
            sel.left
                .iter()
                .filter(|l| l.reason == LeftReason::UnsupportedName)
                .count(),
            3
        );
        assert!(
            sel.exclude.contains(&"we?ird??x?.txt".to_string()),
            "{:?}",
            sel.exclude
        );
        assert!(sel.exclude.iter().all(|p| p
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._/-?".contains(c))));
        assert_eq!(exclude_pattern("a b"), "a?b");
    }

    #[test]
    fn too_many_exclusions_skip_the_half_instead_of_building_a_huge_command() {
        let many: Vec<ListedFile> = (0..MAX_SESSION_EXCLUDES + 5)
            .map(|i| f(&format!("subagents/a{i:04}.jsonl"), 1000))
            .collect();
        let sel = select_session_files(many, 1); // nothing fits → every file would be excluded
        assert!(
            sel.skip.as_deref().is_some_and(|w| w.contains("200")),
            "{:?}",
            sel.skip
        );
        assert!(sel.carry.is_empty() && sel.exclude.is_empty());
    }

    #[test]
    fn memory_list_needs_its_directory_record() {
        let l = parse_memory_list(&marked(
            b"dir\t/h/.claude/projects/-r/memory\t1\0aaa\t10\tnote.md\0bbb\tx\tbad.md\0",
        ))
        .unwrap();
        assert_eq!(
            (l.dir.as_str(), l.exists, l.files.len()),
            ("/h/.claude/projects/-r/memory", true, 1)
        );
        assert!(
            parse_memory_list(&marked(b"aaa\t10\tnote.md\0")).is_none(),
            "no dir record"
        );
        assert!(
            parse_memory_list(&marked(b"dir\trelative\t1\0")).is_none(),
            "the dir must be absolute"
        );
        assert!(parse_memory_list(b"dir\t/x\t1\0").is_none(), "no marker");
    }

    #[test]
    fn memory_only_ever_adds_and_the_index_never_travels_as_a_file() {
        let d = decide_memory(
            &[
                m("new.md", "h1", 100),
                m("same.md", "h2", 50),
                m("differs.md", "h3", 70),
                m("MEMORY.md", "h4", 30),
                m("we ird.md", "h5", 5),
                m("note.txt", "h6", 5),
                m("huge.md", "h7", MEMORY_FILE_MAX_BYTES + 1),
            ],
            &[
                m("same.md", "h2", 50),
                m("differs.md", "OTHER", 99),
                m("MEMORY.md", "hX", 10),
            ],
        );
        assert_eq!(
            d.carry.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(),
            vec!["new.md"]
        );
        assert_eq!(d.kept_target, vec!["differs.md"]);
        assert_eq!(d.identical, 1);
        let reason = |n: &str| d.left.iter().find(|l| l.path == n).map(|l| l.reason);
        assert_eq!(reason("we ird.md"), Some(LeftReason::UnsupportedName));
        assert_eq!(
            reason("note.txt"),
            Some(LeftReason::UnsupportedName),
            "only *.md is memory"
        );
        assert_eq!(reason("huge.md"), Some(LeftReason::OverCap));
        assert!(d.left.iter().all(|l| l.path != "MEMORY.md"));
    }

    #[test]
    fn memory_stops_at_the_total_and_count_bounds() {
        let many: Vec<ListedMemory> = (0..MEMORY_MAX_FILES + 2)
            .map(|i| m(&format!("n{i:04}.md"), "h", 10))
            .collect();
        let d = decide_memory(&many, &[]);
        assert_eq!(d.carry.len(), MEMORY_MAX_FILES);
        assert_eq!(d.left.len(), 2);
        let big: Vec<ListedMemory> = (0..10)
            .map(|i| m(&format!("b{i}.md"), "h", MEMORY_FILE_MAX_BYTES))
            .collect();
        assert_eq!(decide_memory(&big, &[]).carry.len(), 8, "8 MiB in total");
    }

    #[test]
    fn an_index_line_travels_only_with_its_carried_file() {
        let src = "# Memory Index\n\n- [New](new.md) — hook\n- [Kept](differs.md) — src view\n- plain line without a link\n- [Two](new.md) and [other](x.md)\n- [Dot](./dotted.md) — dot-slash link\n";
        let carried = vec!["new.md".to_string(), "dotted.md".to_string()];
        let got = merge_index(
            src,
            Some("# Memory Index\n\n- [Mine](mine.md) — target\n"),
            &carried,
        );
        assert_eq!(
            got.append,
            "- [New](new.md) — hook\n- [Two](new.md) and [other](x.md)\n- [Dot](./dotted.md) — dot-slash link\n"
        );
        assert_eq!(got.lines, 3);
        // no trailing newline on the target: start on a fresh line
        assert!(merge_index(src, Some("- [Mine](mine.md)"), &carried)
            .append
            .starts_with("\n- [New]"));
        // no index on the target: create one with a header
        assert!(merge_index(src, None, &carried)
            .append
            .starts_with("# Memory Index\n\n- [New]"));
        // nothing carried → nothing appended, not even a header
        let none = merge_index(src, None, &[]);
        assert_eq!((none.append.as_str(), none.lines), ("", 0));
    }

    #[test]
    fn the_index_append_is_bounded_and_cannot_close_the_heredoc() {
        let line = format!("- [N](n.md) {}\n", "x".repeat(1000));
        let src = format!("{}CF_INDEX\n", line.repeat(100)); // ~100 KiB, plus a hostile bare delimiter line
        let got = merge_index(&src, Some(""), &["n.md".to_string()]);
        assert!(
            got.append.len() <= INDEX_APPEND_MAX_BYTES,
            "{}",
            got.append.len()
        );
        assert!(got.lines > 0 && got.append.ends_with('\n'));
        assert!(!got.append.lines().any(|l| l == "CF_INDEX"));
    }

    // --- Fix round 1 ---

    #[test]
    fn decide_memory_matches_a_case_differing_target_name_as_the_same_file() {
        // different hash, case-differing name -> kept_target, reported under the SOURCE's spelling
        let d = decide_memory(&[m("Note.md", "h1", 10)], &[m("note.md", "OTHER", 10)]);
        assert_eq!(d.kept_target, vec!["Note.md"]);
        assert!(d.carry.is_empty());

        // same hash, case-differing name -> identical, not kept_target/carry
        let d2 = decide_memory(&[m("Note.md", "h1", 10)], &[m("note.md", "h1", 10)]);
        assert_eq!(d2.identical, 1);
        assert!(d2.kept_target.is_empty() && d2.carry.is_empty());
    }

    #[test]
    fn decide_memory_treats_a_lowercase_memory_md_as_the_index_too() {
        let d = decide_memory(&[m("memory.md", "h1", 10)], &[]);
        assert!(d.carry.is_empty(), "{:?}", d.carry);
        assert!(d.left.is_empty(), "{:?}", d.left);
    }

    #[test]
    fn decide_memory_carries_only_the_first_of_case_colliding_source_names() {
        let d = decide_memory(&[m("A.md", "h1", 10), m("a.md", "h2", 10)], &[]);
        assert_eq!(
            d.carry.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(),
            vec!["A.md"]
        );
        let reason = |n: &str| d.left.iter().find(|l| l.path == n).map(|l| l.reason);
        assert_eq!(reason("a.md"), Some(LeftReason::UnsupportedName));
    }

    #[test]
    fn merge_index_matches_a_carried_name_case_insensitively() {
        let src = "# Memory Index\n\n- [Note](note.md) — hook\n";
        let got = merge_index(src, None, &["Note.md".to_string()]);
        assert_eq!(got.lines, 1, "{:?}", got);
        assert!(got.append.contains("(note.md)"), "{:?}", got.append);
    }

    #[test]
    fn a_skipped_session_directory_reports_every_listed_file() {
        let many: Vec<ListedFile> = (0..MAX_SESSION_EXCLUDES + 1)
            .map(|i| f(&format!("subagents/a{i:04}.jsonl"), 1000))
            .collect();
        let smalls: Vec<ListedFile> = (0..3).map(|i| f(&format!("small{i}.json"), 1)).collect();
        let total_listed = many.len() + smalls.len();
        let mut listed = many;
        listed.extend(smalls.iter().cloned());
        // only the 3 small (1 byte each) files would fit under this cap
        let sel = select_session_files(listed, 3);
        assert!(sel.skip.is_some(), "{:?}", sel.skip);
        assert!(sel.carry.is_empty() && sel.exclude.is_empty());
        assert_eq!(
            sel.left.len(),
            total_listed,
            "every listed file must be reported, not just the ones already walked"
        );
        for s in &smalls {
            let reason = sel.left.iter().find(|l| l.path == s.path).map(|l| l.reason);
            assert_eq!(
                reason,
                Some(LeftReason::OverCap),
                "{} would have fit but the whole half was skipped",
                s.path
            );
        }
    }

    #[test]
    fn a_saturated_total_does_not_panic_when_decremented() {
        // three files whose sum overflows u64: exercises the fold's saturating_add
        // and must not panic when the running total is later decremented.
        let big = u64::MAX / 2 + 1;
        let sel = select_session_files(vec![f("a", big), f("b", big), f("c", big)], 1);
        assert!(sel.carry.is_empty(), "{:?}", sel.carry);
        assert_eq!(sel.exclude.len(), 3);
    }
}
