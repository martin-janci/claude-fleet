//! The Claude-side state of a moved session: its per-session directory
//! (subagent transcripts, tool results, title) and the project's memory.
//! Pure, like `carry.rs`: policies, script builders, parsers — `mod.rs` runs
//! the scripts. Both halves only ever ADD to the target.
//! See `docs/superpowers/specs/2026-09-20-move-carry-claude-state-design.md`.

use crate::service::move_session::carry::{
    home_guard, id_guard, keep_existing_extract, payload, payload_str, IgnoredEntry, LeftBehind,
    LeftReason, FAILED, OUT_MARKER,
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
/// How far an ANNOUNCED archive may exceed the size of the content that went
/// into it before the orchestrator refuses to relay it. A `.tgz` is normally
/// much smaller than its content, but tar adds a 512-byte header and up to
/// 511 bytes of padding per member plus a 10 KiB end-of-archive block, and
/// gzip compresses those hard but never to nothing.
pub const PACK_OVERHEAD_ALLOWANCE_BYTES: u64 = 1 << 20;
pub const INDEX_APPEND_MAX_BYTES: usize = 32 * 1024;
pub const INDEX_NAME: &str = "MEMORY.md";
/// The heredoc delimiter of the index-append script; never allowed as a line.
pub(super) const INDEX_HEREDOC: &str = "CF_INDEX";
/// Archive name for a memory carry (`carry::pack_script` /
/// [`memory_extract_script`]).
pub const MEMORY_ARCHIVE: &str = "memory.tgz";

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
/// the caller's making: every char outside the safe set becomes `[!/]` — a
/// bracket expression matching exactly one NON-slash character, on GNU and
/// BSD tar alike. A plain `?` wildcard is not safe here: both tars' fnmatch
/// let `?` match `/` too, so a pattern built for one odd file (say
/// `a[?]b.txt` for `a b.txt`) can also match an unrelated `a/b.txt` one
/// directory over — an odd name would silently reach across a directory
/// boundary and exclude a file the caller never named. The explicit `[!/]`
/// negation excludes `/` regardless of that.
pub fn exclude_pattern(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for c in path.chars() {
        if c.is_ascii_alphanumeric() || "._/-".contains(c) {
            out.push(c);
        } else {
            out.push_str("[!/]");
        }
    }
    out
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

/// Whether `target` already has a file named `name`, deterministic even when
/// `target` itself holds two entries that differ only by ASCII case (a
/// case-sensitive Linux target can have both `note.md` and `Note.md`):
/// an EXACT name match is authoritative and decides alone — `Some(true)` if
/// its hash agrees with `hash`, `Some(false)` otherwise — regardless of any
/// other case-variant entry's hash. Only when there is no exact match do ALL
/// case-insensitive matches count: `Some(true)` if any of them has `hash`,
/// else `Some(false)`. `None` when nothing in `target` matches at all.
fn target_has(target: &[ListedMemory], name: &str, hash: &str) -> Option<bool> {
    if let Some(t) = target.iter().find(|t| t.name == name) {
        return Some(t.hash == hash);
    }
    let mut any_case_insensitive = false;
    for t in target {
        if t.name.eq_ignore_ascii_case(name) {
            any_case_insensitive = true;
            if t.hash == hash {
                return Some(true);
            }
        }
    }
    any_case_insensitive.then_some(false)
}

/// Memory only ever adds: a name the target has is never carried, and the
/// index is merged line-wise elsewhere, never copied. Names are compared
/// ASCII-case-insensitively throughout — the index filter, the target match,
/// and source-vs-source collisions — because a case-insensitive target
/// volume (macOS default) treats `Note.md` and `note.md` as one file. The
/// target match is resolved deterministically by `target_has`: an exact name
/// match decides outright, ahead of any case-variant entry.
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
        } else if let Some(identical) = target_has(target, &f.name, &f.hash) {
            if identical {
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
    // The fresh-line decision (does the target need a "\n" before this text)
    // belongs to `memory_append_index_script`, not here: `target_index` is
    // only ever a possibly-truncated read (capped at `INDEX_READ_MAX_BYTES`,
    // or empty when the file could not be read despite existing), so ITS
    // trailing-newline state is not reliable evidence about the real file's
    // last byte. The script inspects the real file directly instead.
    let prefix = match target_index {
        None => "# Memory Index\n\n",
        Some(_) => "",
    };
    IndexMerge {
        append: format!("{prefix}{body}"),
        lines,
    }
}

/// Where a repo's Claude memory lives and what is in it. Claude Code keys
/// memory by the MAIN checkout (the parent of the git common dir), not by the
/// worktree; `fallback_dir` (the worktree itself) is tried when that has no
/// `memory/`. First record `dir\t<abs>\t<0|1>` — reported even when the dir
/// does not exist, because a target creates it there. `enc()`'s result is
/// checked before use: an empty encoding for `top` (which is always supposed
/// to resolve, having just been validated) fails the script outright, rather
/// than silently collapsing `m` to `$HOME/.claude/projects//memory` — a path
/// with no per-repo hash at all, which a coincidental directory there would
/// then be mistaken for. A nonexistent `fallback_dir` is not an anomaly
/// (unlike an empty `top`), so an empty fallback encoding is never fatal: the
/// fallback is simply skipped and the primary `m` is reported, exactly as
/// when no fallback applies at all.
pub fn memory_list_script(repo_dir: &str, fallback_dir: Option<&str>) -> String {
    format!(
        r#"# cf-carry:memory-list
set +e
r={r}
fb={fb}
{home_guard}
fail() {{ printf '{FAILED} %s\n' "$1" >&2; exit 5; }}
enc() {{ ( cd -- "$1" 2>/dev/null && pwd -P ) | sed 's/[^A-Za-z0-9]/-/g'; }}
cd -- "$r" 2>/dev/null || fail cd
g=$(git rev-parse --git-common-dir 2>/dev/null) || fail not-a-repo
top=$( cd -- "$g" 2>/dev/null && cd .. && pwd -P ) || fail common-dir
[ -n "$top" ] || fail common-dir
enc_top=$(enc "$top")
[ -n "$enc_top" ] || fail enc
m="$HOME/.claude/projects/$enc_top/memory"
if [ ! -d "$m" ] && [ -n "$fb" ]; then
  enc_fb=$(enc "$fb")
  if [ -n "$enc_fb" ]; then
    alt="$HOME/.claude/projects/$enc_fb/memory"
    [ -d "$alt" ] && m="$alt"
  fi
fi
printf '\n{OUT_MARKER}\n'
if [ -d "$m" ]; then printf 'dir\t%s\t1\0' "$m"; else printf 'dir\t%s\t0\0' "$m"; exit 0; fi
for f in "$m"/*.md; do
  [ -f "$f" ] && [ ! -L "$f" ] || continue
  h=$(git hash-object --no-filters -- "$f" 2>/dev/null) || continue
  n=$(wc -c < "$f" | tr -d ' ')
  printf '%s\t%s\t%s\0' "$h" "${{n:-0}}" "$(basename -- "$f")"
done
exit 0
"#,
        r = quote(repo_dir),
        fb = quote(fallback_dir.unwrap_or("")),
        home_guard = home_guard(),
    )
}

/// The target's or source's `MEMORY.md`: after the marker, `present` or
/// `absent` on the first line, then at most [`INDEX_READ_MAX_BYTES`] + 1
/// bytes of content (one over, so the caller can tell "too large").
pub fn memory_read_index_script(memory_dir: &str) -> String {
    format!(
        r#"# cf-carry:memory-index
set +e
m={m}
f="$m/{INDEX_NAME}"
printf '\n{OUT_MARKER}\n'
if [ -f "$f" ] && [ ! -L "$f" ]; then printf 'present\n'; head -c {max} -- "$f"; else printf 'absent\n'; fi
exit 0
"#,
        m = quote(memory_dir),
        max = INDEX_READ_MAX_BYTES + 1,
    )
}

/// `None`: no marker; `Some(None)`: no index file; `Some(Some(text))`: the
/// index's content, exactly as read (up to the read cap).
pub fn parse_index(stdout: &str) -> Option<Option<String>> {
    let body = payload_str(stdout)?;
    match body.split_once('\n') {
        Some(("present", text)) => Some(Some(text.to_string())),
        Some(("absent", _)) => Some(None),
        None if body == "absent" => Some(None),
        _ => None,
    }
}

/// Append `text` (typically from [`merge_index`], but this builder is `pub`
/// and takes any `&str` — it does not trust its caller) to the target's
/// index. The text reaches the file through a QUOTED heredoc, so nothing in
/// it is expanded; but bash still ends a heredoc on the FIRST line that is an
/// exact match for the delimiter, whoever wrote it. A `text` with such a line
/// would truncate the `cat` early and hand the remainder of THIS SCRIPT to
/// the shell as real commands — `merge_index`'s own `line == INDEX_HEREDOC`
/// filter only protects text it built itself, not an arbitrary caller, so
/// this builder refuses the text itself: any line exactly equal to
/// [`INDEX_HEREDOC`], text containing a NUL byte, or text longer than
/// [`INDEX_APPEND_MAX_BYTES`], yields a script that touches nothing and
/// reports `{FAILED} index-text`. A line that merely contains the delimiter
/// text, or has leading/trailing whitespace around it, is not an exact match
/// and is accepted unchanged.
///
/// The NUL matters because bash DISCARDS NUL bytes while reading a script,
/// so a line `CF_INDEX\0` is not equal to [`INDEX_HEREDOC`] in Rust yet
/// still ends the heredoc in bash — the remainder would run as real
/// commands. A NUL cannot survive an argv word (`execve` refuses it), so the
/// transport rejects such a script before any host reads it; refusing here
/// turns that spawn error into the same deterministic refusal as the
/// delimiter line, and keeps the guarantee true for any other way this
/// `pub` builder's output might be run.
///
/// Before appending, if the index already exists, is non-empty and its LAST
/// BYTE is not a newline, one is written first — the script decides this
/// from the real file, not from `merge_index`'s (possibly truncated, or
/// empty because an existing file could not be read) view of it. Empty text
/// → a no-op that creates nothing, but still prints `ok`: one success
/// predicate for both the no-op and the real append.
///
/// `cat >>` is not atomic: an interrupted append is reported (the sentinel,
/// non-zero) but can leave a partial line at the end of the file, and simply
/// retrying can duplicate lines already written. The index is append-only by
/// design — nothing here ever re-reads or rewrites it to repair that.
pub fn memory_append_index_script(memory_dir: &str, text: &str) -> String {
    if text.is_empty() {
        return "# cf-carry:memory-append\nprintf 'ok\\n'\n".to_string();
    }
    if text.len() > INDEX_APPEND_MAX_BYTES
        || text.contains('\0')
        || text.lines().any(|l| l == INDEX_HEREDOC)
    {
        return format!("# cf-carry:memory-append\nprintf '{FAILED} index-text\\n' >&2\nexit 5\n");
    }
    let body = text.strip_suffix('\n').unwrap_or(text);
    format!(
        r#"# cf-carry:memory-append
set +e
m={m}
umask 077
mkdir -p -- "$m" || {{ printf '{FAILED} mkdir\n' >&2; exit 5; }}
f="$m/{INDEX_NAME}"
if [ -L "$f" ]; then printf '{FAILED} symlink\n' >&2; exit 5; fi
[ -s "$f" ] && [ -n "$(tail -c 1 -- "$f")" ] && printf '\n' >> "$f"
cat >> "$f" <<'{INDEX_HEREDOC}' || {{ printf '{FAILED} append\n' >&2; exit 5; }}
{body}
{INDEX_HEREDOC}
printf 'ok\n'
"#,
        m = quote(memory_dir),
    )
}

/// Extract a memory archive into `memory_dir` — created `0700` if absent —
/// keeping every file already there, but only after VALIDATING the member
/// list, which `carry::extract_keep_existing_script` deliberately does not
/// do (see its note). This is the one extract that does not stage into a
/// scratch directory first: it writes straight into the user's own notes, so
/// containment cannot rest on tar's defaults. A crafted archive — or a file
/// swapped for a symlink between the listing and the pack — would otherwise
/// plant a symlink, a subdirectory, or a whole `MEMORY.md`.
///
/// Every member must be `<name>` or `./<name>` with `<name>` matching
/// `[A-Za-z0-9._-]+\.md` (so no `/`, no `..`, no odd byte), must not be
/// `MEMORY.md` in any ASCII case (the index is merged line-wise by
/// [`memory_append_index_script`], never copied), and must be shown as a
/// REGULAR file by `tar -tvzf` (first column `-`, so never a symlink, a
/// directory or a device). One bad member refuses the whole archive: the
/// sentinel, a non-zero exit and NOTHING extracted. The two `PIPESTATUS[1]`
/// reads take the `while`'s status, not `tar`'s, and come immediately after
/// their pipeline.
///
/// The one name shape neither pass rejects is a member whose name holds a
/// NEWLINE and whose every line happens to look like a valid `.md` name
/// (`a.md\n-x.md`): `tar -tzf` prints it as two lines that each pass, and
/// `tar -tvzf` prints a second line that starts with `-`. Such a member
/// lands as one oddly-named file inside the memory dir — it can still not
/// escape it, be a symlink, or be the index — so it is a cosmetic residue,
/// not a containment hole.
pub fn memory_extract_script(memory_dir: &str, archive: &str) -> String {
    format!(
        r#"# cf-carry:memory-extract
set +e
m={m}
a={a}
fail() {{ printf '{FAILED} %s\n' "$1" >&2; exit 5; }}
umask 077
mkdir -p -- "$m" || fail mkdir
cd -- "$m" 2>/dev/null || fail cd
tar -tzf "$a" >/dev/null 2>&1 || fail corrupt
tar -tzf "$a" 2>/dev/null | while IFS= read -r n; do
  n=${{n#./}}
  case "$n" in
    *[!A-Za-z0-9._-]*) exit 7 ;;
    .md|..|.|'') exit 7 ;;
    [Mm][Ee][Mm][Oo][Rr][Yy].[Mm][Dd]) exit 7 ;;
    *.md) ;;
    *) exit 7 ;;
  esac
done
[ "${{PIPESTATUS[1]}}" -eq 0 ] || fail member-name
tar -tvzf "$a" 2>/dev/null | while IFS= read -r line; do
  case "$line" in
    -*) ;;
    *) exit 7 ;;
  esac
done
[ "${{PIPESTATUS[1]}}" -eq 0 ] || fail member-type
{extract}
printf 'ok\n'
"#,
        m = quote(memory_dir),
        a = quote(archive),
        extract = keep_existing_extract(),
    )
}

/// Every regular file under `<project dir>/<id>/` as `<bytes>\t<path>\0`,
/// the path relative to `<id>/`. Symlinks and special files are not listed —
/// and the merge moves regular files only, so they never travel. A leftover
/// `*.cf-part` from an interrupted merge (see `session_merge_script`) is
/// never listed either. The directory is checked and entered BEFORE the
/// marker is printed, so a caller never sees a marker it cannot trust — "no
/// such directory" still prints the marker (with an empty list) since that
/// is a legitimate, successful outcome. Only a `find` failure remains an
/// after-the-marker exception, documented at the call site: streaming a
/// listing means its own failure can only be discovered once records are
/// already flowing.
pub fn session_list_script(project_dir: &str, claude_id: &str) -> String {
    format!(
        r#"# cf-carry:state-list
set +e
d={d}
id={id}
{id_guard}
cd -- "$d" 2>/dev/null || {{ printf '{FAILED} cd\n' >&2; exit 5; }}
if [ -d "./$id" ]; then
  cd -- "./$id" || {{ printf '{FAILED} cd-id\n' >&2; exit 5; }}
  printf '\n{OUT_MARKER}\n'
  find . -type f ! -name '*.cf-part' -exec sh -c 'for f; do n=$(wc -c < "$f" | tr -d " "); printf "%s\t%s\0" "${{n:-0}}" "${{f#./}}"; done' _ {{}} +
  [ "$?" -eq 0 ] || {{ printf '{FAILED} find\n' >&2; exit 5; }}
else
  printf '\n{OUT_MARKER}\n'
fi
exit 0
"#,
        d = quote(project_dir),
        id = quote(claude_id),
        id_guard = id_guard(),
    )
}

/// One tar of `./<id>` minus `excludes` (patterns relative to `<id>/`, each
/// given in both member-name spellings so GNU and BSD tar agree). A
/// `*.cf-part` is always excluded too, regardless of the caller's list: it
/// is the merge script's own same-directory temp name (see
/// `session_merge_script`), and a leftover one from an earlier interrupted
/// merge must never travel as if it were real session content.
pub fn session_pack_script(project_dir: &str, claude_id: &str, excludes: &[String]) -> String {
    let mut ex: Vec<String> = excludes
        .iter()
        .flat_map(|p| {
            [
                format!("--exclude=./{claude_id}/{p}"),
                format!("--exclude={claude_id}/{p}"),
            ]
        })
        .map(|a| quote(&a))
        .collect();
    ex.push(quote("--exclude=*.cf-part"));
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
///
/// The first thing the loop does is re-check the NAME, because the three
/// sets "what the listing selected", "what was packed" and "what is merged"
/// are not the same set: [`session_pack_script`] archives the whole `./<id>`
/// minus the excludes, so a path [`parse_file_list`] dropped (a TAB or a
/// newline in the name, a non-UTF-8 name) or one created after the listing
/// still arrives here. A `rel` holding a character outside `[A-Za-z0-9._/-]`,
/// or a `..` segment, is therefore never moved and is reported
/// `failed\t(unsupported name)` WITHOUT its raw name: echoing a name with a
/// newline in it would let the archive forge a whole extra report line (a
/// crafted `…\ncarried\t5\tsubagents/agent-zz.jsonl` member would otherwise
/// make [`parse_merge`] report a file that never existed). The bracket
/// expression is ASCII-exact in the C and in a UTF-8 locale alike, so no
/// `LC_ALL` is needed.
///
/// Every doubt falls toward
/// KEEP: a destination that is not a plain regular file, OR one whose size
/// cannot even be read (permissions, an ACL, an fs quirk), is treated as
/// larger and kept rather than risk replacing something the merge could not
/// verify; a STAGED file whose own size cannot be read is reported `failed`
/// and never moved. A replacement is staged through a same-directory
/// `<dst>.cf-part` name first, so the final step onto `<dst>` is a rename
/// (atomic) rather than a possibly cross-filesystem copy+unlink that ENOSPC
/// could interrupt mid-write; on any failure the `.cf-part` is removed and
/// the entry reported `failed`. `*.cf-part` itself is never treated as real
/// staged content (a leftover from an earlier interrupted merge). The
/// `find | while` pipeline's own exit status is read immediately after it
/// (`PIPESTATUS`, not `$?`, since the pipeline's last stage is the `while`)
/// so a `find` failure partway through (e.g. an unreadable staged
/// subdirectory) still surfaces as one `failed` line instead of a silently
/// truncated report — the files already moved by then stay moved and
/// reported. `fail()` always cleans up the staging dir first: it is only
/// ever called after `stage=` has run, so `$stage` is already set.
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
stage="$HOME/.cache/claude-fleet/transfer/$id/state-staging"
fail() {{ rm -rf -- "$stage" 2>/dev/null; printf '{FAILED} %s\n' "$1" >&2; exit 5; }}
rm -rf -- "$stage"
mkdir -p -- "$stage" || fail mkdir
tar -tzf "$a" >/dev/null 2>&1 || fail corrupt
tar -xzf "$a" -C "$stage" >/dev/null 2>&1 || fail extract
mkdir -p -- "$d/$id" || fail mkdir-target
printf '\n{OUT_MARKER}\n'
if cd -- "$stage/$id" 2>/dev/null; then
  find . -type f ! -name '*.cf-part' -print0 | while IFS= read -r -d '' f; do
    rel=${{f#./}}
    case "$rel" in
      *[!A-Za-z0-9._/-]*|..|../*|*/../*|*/..)
        printf 'failed\t(unsupported name)\n'
        continue
        ;;
    esac
    dst="$d/$id/$rel"
    s=$(wc -c < "$f" 2>/dev/null | tr -d ' ')
    if [ -z "$s" ]; then
      printf 'failed\t%s\n' "$rel"
      continue
    fi
    if [ -e "$dst" ] || [ -L "$dst" ]; then
      if [ -f "$dst" ] && [ ! -L "$dst" ]; then
        t=$(wc -c < "$dst" 2>/dev/null | tr -d ' ')
        [ -n "$t" ] || t=999999999999
      else
        t=999999999999
      fi
    else
      t=-1
    fi
    if [ "$t" -lt "$s" ]; then
      if mkdir -p -- "$(dirname -- "$dst")" && mv -f -- "$f" "$dst.cf-part" && mv -f -- "$dst.cf-part" "$dst"; then
        printf 'carried\t%s\t%s\n' "$s" "$rel"
      else
        rm -f -- "$dst.cf-part"
        printf 'failed\t%s\n' "$rel"
      fi
    else
      printf 'kept\t%s\n' "$rel"
    fi
  done
  [ "${{PIPESTATUS[0]}}" -eq 0 ] || printf 'failed\t(listing the staged files)\n'
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
/// `failed\t<path>`. `None` only when the marker itself is missing — a
/// malformed `carried` size does not sink the rest of an otherwise-good
/// report; it is kept with `bytes: 0`.
pub fn parse_merge(stdout: &str) -> Option<MergeResult> {
    let mut r = MergeResult::default();
    for line in payload_str(stdout)?.lines() {
        match line.split('\t').collect::<Vec<_>>().as_slice() {
            ["carried", n, path] => r.carried.push(IgnoredEntry {
                path: path.to_string(),
                bytes: n.trim().parse().unwrap_or(0),
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

    /// `true` when this test process is root — permission-denial tests are
    /// meaningless there (uid 0 reads/writes anything regardless of mode
    /// bits), so those assertions are skipped, loudly, rather than silently
    /// passing for the wrong reason.
    fn is_root() -> bool {
        std::process::Command::new("id")
            .arg("-u")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim() == "0")
            .unwrap_or(false)
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

    /// Every doubt must fall toward KEEP: `wc -c < "$dst"` has no failure
    /// handling of its own, so an existing destination that cannot be READ
    /// (mode 000, an ACL, an fs quirk) must never be treated as if it were
    /// absent — that would let a smaller staged file silently replace
    /// something the merge could not even measure.
    #[test]
    fn an_unreadable_destination_is_kept_not_replaced() {
        if !require(&["bash", "tar"]) {
            return;
        }
        if is_root() {
            eprintln!("skipping: uid 0 can read anything regardless of mode 000");
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
        // Larger than the source's 4000 B copy, then made unreadable.
        let bb_bigger = tgt.join(ID).join("subagents/agent-bb.jsonl");
        std::fs::create_dir_all(bb_bigger.parent().unwrap()).unwrap();
        std::fs::write(&bb_bigger, "b\n".repeat(2500)).unwrap();
        std::fs::set_permissions(&bb_bigger, std::fs::Permissions::from_mode(0o000)).unwrap();
        let before = {
            // Can't read the bytes without restoring the mode first — just
            // pin the mtime/size via metadata as a before/after sentinel.
            std::fs::metadata(&bb_bigger).unwrap().len()
        };

        let out = bash(
            &session_merge_script(tgt.to_str().unwrap(), ID, &archive),
            &home,
        );
        // restore before any assertion can fail and leave it unreadable
        std::fs::set_permissions(&bb_bigger, std::fs::Permissions::from_mode(0o644)).unwrap();

        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let merged = parse_merge(&String::from_utf8_lossy(&out.stdout)).unwrap();
        assert!(
            merged
                .kept
                .contains(&"subagents/agent-bb.jsonl".to_string()),
            "{merged:?}"
        );
        assert!(
            !merged
                .carried
                .iter()
                .any(|e| e.path == "subagents/agent-bb.jsonl"),
            "{merged:?}"
        );
        assert_eq!(
            std::fs::metadata(&bb_bigger).unwrap().len(),
            before,
            "byte-identical to before the merge"
        );
        assert_eq!(
            std::fs::read_to_string(&bb_bigger).unwrap(),
            "b\n".repeat(2500)
        );
    }

    /// `fail()` used to exit immediately, leaving the staging dir (a partial
    /// copy of the user's conversation history) behind on every failure
    /// path. It must clean up first.
    #[test]
    fn the_staging_dir_does_not_survive_a_corrupt_archive() {
        if !require(&["bash", "tar"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let corrupt = tmp.path().join("corrupt.tgz");
        std::fs::write(&corrupt, b"not a tarball").unwrap();
        let tgt = tmp.path().join("-Users-other--claude-worktrees-feat");

        let out = bash(
            &session_merge_script(tgt.to_str().unwrap(), ID, corrupt.to_str().unwrap()),
            &home,
        );
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains(FAILED));
        assert!(
            !home
                .join(".cache/claude-fleet/transfer")
                .join(ID)
                .join("state-staging")
                .exists(),
            "the staging dir must not survive a failed merge"
        );
    }

    /// Replacing a smaller target file is not atomic today: `mv -f` across
    /// filesystems is copy+unlink, so ENOSPC (or any interrupted copy) could
    /// leave a truncated destination where a good smaller file used to be.
    /// Moving through a same-directory `.cf-part` name first makes the final
    /// step a rename. A leftover `*.cf-part` (from an earlier crash, or one
    /// already sitting in the source) must never be listed, packed, or
    /// merged as if it were real content.
    #[test]
    fn a_replacement_goes_through_a_same_directory_temp_name_and_leaves_none_behind() {
        if !require(&["bash", "tar"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let src = source_project(tmp.path());
        // A leftover .cf-part sitting in the SOURCE session dir, as if this
        // host had once been a merge target itself.
        std::fs::write(src.join(ID).join("subagents/x.cf-part"), "stale").unwrap();

        // --- list must never report it ---
        let out = bash(&session_list_script(src.to_str().unwrap(), ID), &home);
        assert!(out.status.success());
        assert!(parse_file_list(&out.stdout)
            .iter()
            .all(|f| !f.path.ends_with(".cf-part")));

        // --- pack must never archive it, even with no caller-supplied excludes ---
        let out = bash(&session_pack_script(src.to_str().unwrap(), ID, &[]), &home);
        assert!(out.status.success());
        let (_, archive) =
            crate::service::move_session::carry::parse_pack(&String::from_utf8_lossy(&out.stdout))
                .unwrap();
        let listing = std::process::Command::new("tar")
            .args(["-tzf", &archive])
            .output()
            .unwrap();
        let names = String::from_utf8_lossy(&listing.stdout);
        assert!(!names.contains("x.cf-part"), "{names}");

        // --- merge: a normal run leaves no *.cf-part anywhere on the target ---
        let tgt = tmp.path().join("-Users-other--claude-worktrees-feat");
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
        assert!(
            !merged.carried.iter().any(|e| e.path.ends_with(".cf-part")),
            "{merged:?}"
        );
        fn has_cf_part(dir: &std::path::Path) -> bool {
            std::fs::read_dir(dir).unwrap().any(|e| {
                let e = e.unwrap();
                let p = e.path();
                if p.is_dir() {
                    has_cf_part(&p)
                } else {
                    p.extension().is_some_and(|x| x == "cf-part")
                }
            })
        }
        assert!(
            !has_cf_part(&tgt.join(ID)),
            "no .cf-part temp file survives a successful merge"
        );
    }

    /// The rule "a destination that is not a regular file is kept" had no
    /// coverage: a pre-existing DIRECTORY and a pre-existing SYMLINK on the
    /// target must both be reported `kept`, left exactly as they were, and
    /// nothing may be written inside/through them.
    #[test]
    fn a_non_regular_destination_is_kept_and_left_untouched() {
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
        // custom-title.json as a DIRECTORY.
        std::fs::create_dir_all(tgt.join(ID).join("custom-title.json")).unwrap();
        // tool-results/out1.txt as a SYMLINK to some other file.
        let referent = tmp.path().join("referent.txt");
        std::fs::write(&referent, "referent content\n").unwrap();
        std::fs::create_dir_all(tgt.join(ID).join("tool-results")).unwrap();
        std::os::unix::fs::symlink(&referent, tgt.join(ID).join("tool-results/out1.txt")).unwrap();

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
        assert!(
            merged.kept.contains(&"custom-title.json".to_string()),
            "{merged:?}"
        );
        assert!(
            merged.kept.contains(&"tool-results/out1.txt".to_string()),
            "{merged:?}"
        );
        assert!(!merged
            .carried
            .iter()
            .any(|e| e.path == "custom-title.json" || e.path == "tool-results/out1.txt"));

        let dir_meta = std::fs::symlink_metadata(tgt.join(ID).join("custom-title.json")).unwrap();
        assert!(dir_meta.file_type().is_dir(), "still a directory");
        assert_eq!(
            std::fs::read_dir(tgt.join(ID).join("custom-title.json"))
                .unwrap()
                .count(),
            0,
            "nothing was written inside it"
        );

        let link_meta =
            std::fs::symlink_metadata(tgt.join(ID).join("tool-results/out1.txt")).unwrap();
        assert!(link_meta.file_type().is_symlink(), "still a symlink");
        assert_eq!(
            std::fs::read_link(tgt.join(ID).join("tool-results/out1.txt")).unwrap(),
            referent
        );
        assert_eq!(
            std::fs::read_to_string(&referent).unwrap(),
            "referent content\n",
            "the referent is unchanged"
        );
    }

    /// Builds a `.tgz` containing `<id>/custom-title.json` and
    /// `<id>/subagents/locked/inner.jsonl`, with the `locked` directory's
    /// STORED mode forced to `0o000` — independent of the mode of the real
    /// files on disk while archiving (a plain `tar` can't do this: you can't
    /// archive the *contents* of a directory that is already unreadable).
    /// `tar` defers restoring a directory's final permissions until after
    /// its children are written, so extracting this archive lands the file
    /// inside `locked` and THEN locks the directory down — exactly the
    /// shape of a directory that becomes unreadable only once staged.
    fn build_archive_with_a_locked_directory(
        dir: &std::path::Path,
        id: &str,
    ) -> std::path::PathBuf {
        let src = dir.join("archive_src");
        std::fs::create_dir_all(src.join(id).join("subagents/locked")).unwrap();
        std::fs::write(src.join(id).join("custom-title.json"), "{\"title\":\"t\"}").unwrap();
        std::fs::write(
            src.join(id).join("subagents/locked/inner.jsonl"),
            "secret\n",
        )
        .unwrap();
        let builder = dir.join("build_archive.py");
        std::fs::write(
            &builder,
            r#"
import tarfile, os, sys
root, id_, out = sys.argv[1], sys.argv[2], sys.argv[3]
locked_rel = os.path.join(id_, "subagents", "locked")
with tarfile.open(out, "w:gz") as tar:
    for dirpath, dirnames, filenames in os.walk(os.path.join(root, id_)):
        rel_dir = os.path.relpath(dirpath, root)
        ti = tar.gettarinfo(dirpath, arcname="./" + rel_dir)
        if rel_dir == locked_rel:
            ti.mode = 0o000
        tar.addfile(ti)
        for fn in filenames:
            fp = os.path.join(dirpath, fn)
            rel_fp = os.path.relpath(fp, root)
            ti2 = tar.gettarinfo(fp, arcname="./" + rel_fp)
            with open(fp, "rb") as fh:
                tar.addfile(ti2, fh)
"#,
        )
        .unwrap();
        let archive = dir.join("locked.tgz");
        let out = std::process::Command::new("python3")
            .args([
                builder.to_str().unwrap(),
                src.to_str().unwrap(),
                id,
                archive.to_str().unwrap(),
            ])
            .output()
            .expect("python3");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        archive
    }

    /// A `find` failure partway through the merge loop (e.g. an unreadable
    /// staged subdirectory) must not silently truncate the report: the
    /// pipeline's exit status is checked immediately after it, and a single
    /// `failed\t(listing the staged files)` line warns the flow — without
    /// losing the report lines for files already moved.
    #[test]
    fn a_find_failure_mid_merge_is_reported_without_losing_files_already_moved() {
        if !require(&["bash", "tar", "python3"]) {
            return;
        }
        if is_root() {
            eprintln!("skipping: uid 0 can traverse a mode-000 directory anyway");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let archive = build_archive_with_a_locked_directory(tmp.path(), ID);

        let tgt = tmp.path().join("-Users-other--claude-worktrees-feat");
        let out = bash(
            &session_merge_script(tgt.to_str().unwrap(), ID, archive.to_str().unwrap()),
            &home,
        );
        // Whatever permissions tar left on the target's copy, restore them
        // before the tempdir tries to clean itself up.
        let target_locked = tgt.join(ID).join("subagents/locked");
        if target_locked.exists() {
            let _ =
                std::fs::set_permissions(&target_locked, std::fs::Permissions::from_mode(0o700));
        }
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let merged = parse_merge(&String::from_utf8_lossy(&out.stdout)).unwrap();
        // The file outside the locked directory must still have made it.
        assert!(
            merged.carried.iter().any(|e| e.path == "custom-title.json"),
            "{merged:?}"
        );
        assert!(
            merged
                .failed
                .iter()
                .any(|p| p.contains("listing the staged files")),
            "a find failure must be reported: {merged:?}"
        );
    }

    /// Builds a `.tgz` whose members carry names the LISTING would have
    /// dropped: a space, a TAB, and one with a NEWLINE whose second line is
    /// an exact `carried\t…` report line. Only python3's `tarfile` can put
    /// such names in an archive; `tar` from a real directory cannot be made
    /// to produce the newline one reliably.
    fn build_archive_with_hostile_names(dir: &std::path::Path, id: &str) -> std::path::PathBuf {
        let builder = dir.join("build_hostile.py");
        std::fs::write(
            &builder,
            r#"
import tarfile, io, sys
id_, out = sys.argv[1], sys.argv[2]
names = [
    "subagents/agent-ok.jsonl",
    "we ird.txt",
    "tab\there.txt",
    "nl\ncarried\t5\tsubagents/agent-zz.jsonl",
]
with tarfile.open(out, "w:gz") as tar:
    for n in names:
        body = ("body of " + n.replace("\n", "_").replace("\t", "_") + "\n").encode()
        ti = tarfile.TarInfo("./" + id_ + "/" + n)
        ti.size = len(body)
        ti.mode = 0o600
        tar.addfile(ti, io.BytesIO(body))
"#,
        )
        .unwrap();
        let archive = dir.join("hostile.tgz");
        let out = std::process::Command::new("python3")
            .args([builder.to_str().unwrap(), id, archive.to_str().unwrap()])
            .output()
            .expect("python3");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        archive
    }

    /// (A.1) The pack is "the whole `./<id>` minus the excludes", never "the
    /// files the listing selected", so a name the listing dropped — a TAB, a
    /// newline, a byte outside the safe charset — can still reach the staging
    /// dir. The merge is the point of effect and must enforce the charset
    /// itself: such a file is never moved, its raw name is never echoed (a
    /// newline in it would otherwise FORGE a `carried\t…` line that
    /// `parse_merge` believes), and it is reported `failed` anonymously.
    #[test]
    fn a_hostile_member_name_is_never_moved_and_cannot_forge_a_report_line() {
        if !require(&["bash", "tar", "python3"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let archive = build_archive_with_hostile_names(tmp.path(), ID);

        let tgt = tmp.path().join("-Users-other--claude-worktrees-feat");
        let out = bash(
            &session_merge_script(tgt.to_str().unwrap(), ID, archive.to_str().unwrap()),
            &home,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let merged = parse_merge(&stdout).unwrap();

        let carried: Vec<&str> = merged.carried.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(
            carried,
            vec!["subagents/agent-ok.jsonl"],
            "only the safe member may be carried: {stdout:?}"
        );
        assert!(
            !merged
                .carried
                .iter()
                .any(|e| e.path == "subagents/agent-zz.jsonl"),
            "the forged report line must never be believed: {merged:?}"
        );
        assert_eq!(
            merged.failed,
            vec![
                "(unsupported name)".to_string(),
                "(unsupported name)".to_string(),
                "(unsupported name)".to_string()
            ],
            "each odd name is reported anonymously: {merged:?}"
        );
        for raw in ["we ird.txt", "agent-zz.jsonl", "tab\there.txt"] {
            assert!(
                !stdout.contains(raw),
                "the raw name {raw:?} must never be echoed: {stdout:?}"
            );
        }

        // Only the safe file exists on the target; nothing of the odd names.
        fn walk(dir: &std::path::Path, base: &std::path::Path, out: &mut Vec<String>) {
            for e in std::fs::read_dir(dir).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    walk(&p, base, out);
                } else {
                    out.push(p.strip_prefix(base).unwrap().to_string_lossy().into_owned());
                }
            }
        }
        let mut landed = Vec::new();
        walk(&tgt.join(ID), &tgt.join(ID), &mut landed);
        landed.sort();
        assert_eq!(landed, vec!["subagents/agent-ok.jsonl".to_string()]);
        assert!(
            !home
                .join(".cache/claude-fleet/transfer")
                .join(ID)
                .join("state-staging")
                .exists(),
            "staging dir is cleaned up"
        );
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

    /// A single-char wildcard (`?`) in a tar `--exclude` pattern can match
    /// `/` on both GNU and BSD tar's fnmatch, so a pattern built for one odd
    /// file can silently reach into an unrelated file across a directory
    /// boundary. `exclude_pattern` must use a token that matches exactly one
    /// NON-slash character.
    #[test]
    fn exclude_pattern_never_reaches_across_a_directory_boundary() {
        if !require(&["bash", "tar"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let src = source_project(tmp.path());
        // "a b.txt" (one odd top-level file) and "a/b.txt" (an unrelated file
        // nested under a directory literally named "a") — a pattern for the
        // first must never also match the second.
        std::fs::write(src.join(ID).join("a b.txt"), "space").unwrap();
        std::fs::create_dir_all(src.join(ID).join("a")).unwrap();
        std::fs::write(src.join(ID).join("a/b.txt"), "nested").unwrap();

        let excludes = vec![exclude_pattern("a b.txt")];
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
        assert!(!names.contains("a b.txt"), "the odd file itself: {names}");
        assert!(
            names.contains("a/b.txt"),
            "an unrelated file one directory over must survive: {names}"
        );
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

    /// The list script must confirm the session directory is actually
    /// reachable BEFORE printing the marker — printing it first and only
    /// then discovering `cd` fails leaves a caller staring at a marker with
    /// no trustworthy payload. `chmod 000` on `<id>/` makes `cd` fail
    /// deterministically (no race required) while `[ -d ]` still reports it
    /// as a directory.
    #[test]
    fn session_list_script_confirms_the_directory_before_printing_the_marker() {
        if !require(&["bash"]) {
            return;
        }
        if is_root() {
            eprintln!("skipping: chmod 000 has no effect on root");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let src = tmp.path().join("-Users-me-r--claude-worktrees-locked");
        std::fs::create_dir_all(&src).unwrap();
        let idp = src.join(ID);
        std::fs::create_dir_all(&idp).unwrap();
        std::fs::set_permissions(&idp, std::fs::Permissions::from_mode(0o000)).unwrap();

        let out = bash(&session_list_script(src.to_str().unwrap(), ID), &home);
        // restore so the tempdir can be cleaned up regardless of the assertions below
        std::fs::set_permissions(&idp, std::fs::Permissions::from_mode(0o700)).unwrap();

        assert!(!out.status.success(), "cd into an unreadable dir must fail");
        assert!(String::from_utf8_lossy(&out.stderr).contains(FAILED));
        assert!(
            crate::service::move_session::carry::payload(&out.stdout).is_none(),
            "no marker until the directory is confirmed reachable"
        );
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
            sel.exclude
                .contains(&"we[!/]ird[!/][!/]x[!/].txt".to_string()),
            "{:?}",
            sel.exclude
        );
        assert!(sel.exclude.iter().all(|p| p
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._/-[]!".contains(c))));
        assert_eq!(exclude_pattern("a b"), "a[!/]b");
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
        // The fresh-line decision now belongs to `memory_append_index_script`
        // (it can see the real file's last byte; `merge_index` only ever sees
        // a possibly-truncated read) — a target WITH an index never gets a
        // prefix here, whatever its trailing-newline state.
        assert!(!merge_index(src, Some("- [Mine](mine.md)"), &carried)
            .append
            .starts_with('\n'));
        assert_eq!(
            merge_index(src, Some("- [Mine](mine.md)"), &carried).append,
            merge_index(src, Some("- [Mine](mine.md)\n"), &carried).append,
            "Some(_) is always \"\", regardless of the target's trailing newline"
        );
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

    // --- Fix round 2 ---

    #[test]
    fn decide_memory_prefers_the_exact_target_name_over_a_case_variant() {
        let order_a = [m("note.md", "h1", 10), m("Note.md", "h2", 10)];
        let order_b = [m("Note.md", "h2", 10), m("note.md", "h1", 10)];
        for target in [order_a.as_slice(), order_b.as_slice()] {
            // the exact name wins even though `note.md` may come first
            let d = decide_memory(&[m("Note.md", "h2", 10)], target);
            assert_eq!(d.identical, 1, "{:?}", d);
            assert!(d.kept_target.is_empty() && d.carry.is_empty(), "{:?}", d);

            // the exact `Note.md` entry has h2, not h1 -- the OTHER entry's
            // hash must not make this identical
            let d = decide_memory(&[m("Note.md", "h1", 10)], target);
            assert_eq!(d.kept_target, vec!["Note.md"], "{:?}", d);
            assert_eq!(d.identical, 0);
            assert!(d.carry.is_empty());

            // no exact match: ANY case-insensitive entry with the matching
            // hash makes it identical
            let d = decide_memory(&[m("NOTE.md", "h2", 10)], target);
            assert_eq!(d.identical, 1, "{:?}", d);
            assert!(d.kept_target.is_empty() && d.carry.is_empty());

            // no exact match, and no case-insensitive entry's hash matches
            let d = decide_memory(&[m("NOTE.md", "h9", 10)], target);
            assert_eq!(d.kept_target, vec!["NOTE.md"], "{:?}", d);
            assert_eq!(d.identical, 0);
            assert!(d.carry.is_empty());
        }
    }

    // --- Fix round 1 (session-directory scripts) ---

    #[test]
    fn parse_merge_keeps_the_report_when_one_size_is_malformed() {
        let stdout = format!(
            "banner\n\n{OUT_MARKER}\ncarried\t10\tgood.jsonl\ncarried\tNaN\tbad.jsonl\nkept\tsame.md\n"
        );
        let got = parse_merge(&stdout).unwrap();
        assert_eq!(got.carried.len(), 2, "{got:?}");
        assert_eq!(
            got.carried[0],
            IgnoredEntry {
                path: "good.jsonl".into(),
                bytes: 10
            }
        );
        assert_eq!(
            got.carried[1],
            IgnoredEntry {
                path: "bad.jsonl".into(),
                bytes: 0
            },
            "a malformed size becomes 0; the line is not dropped and the rest of the report survives"
        );
        assert_eq!(got.kept, vec!["same.md"]);
        assert!(
            parse_merge("carried\t10\tgood.jsonl\n").is_none(),
            "only a missing marker is None"
        );
    }

    // --- Task 4: memory scripts ---

    /// The physical path of `p` (symlinks resolved, e.g. macOS `/var` →
    /// `/private/var`), encoded exactly as `memory_list_script`'s own `enc()`
    /// does: every char outside `[A-Za-z0-9]` becomes `-`.
    fn enc(p: &std::path::Path) -> String {
        let real = std::fs::canonicalize(p).unwrap();
        real.to_string_lossy()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect()
    }

    fn memory_dir_for(home: &std::path::Path, enc_name: &str) -> std::path::PathBuf {
        home.join(".claude/projects").join(enc_name).join("memory")
    }

    #[test]
    fn memory_is_found_by_the_repo_root_not_the_worktree() {
        if !require(&["bash", "git"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let r = tmp.path().join("r");
        std::fs::create_dir_all(&r).unwrap();
        crate::service::move_session::carry::tests::git(&r, &["init", "-q", "-b", "main"]);
        std::fs::write(r.join("f.txt"), "x\n").unwrap();
        crate::service::move_session::carry::tests::git(&r, &["add", "-A"]);
        crate::service::move_session::carry::tests::git(&r, &["commit", "-q", "-m", "base"]);
        let wt = tmp.path().join("wt");
        crate::service::move_session::carry::tests::git(
            &r,
            &["worktree", "add", "-q", wt.to_str().unwrap(), "-b", "feat"],
        );

        let r_enc = enc(&r);
        let wt_enc = enc(&wt);
        assert_ne!(r_enc, wt_enc, "sanity: the two paths encode differently");

        // --- primary: memory keyed by the repo root exists ---
        let r_memory = memory_dir_for(&home, &r_enc);
        std::fs::create_dir_all(r_memory.join("sub")).unwrap();
        std::fs::write(r_memory.join("note.md"), "hello\n").unwrap();
        std::fs::write(r_memory.join("MEMORY.md"), "# Memory Index\n").unwrap();
        std::fs::write(r_memory.join("notes.txt"), "not memory\n").unwrap();
        std::os::unix::fs::symlink(r_memory.join("note.md"), r_memory.join("linked.md")).unwrap();

        let out = bash(
            &memory_list_script(wt.to_str().unwrap(), Some(wt.to_str().unwrap())),
            &home,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let listing = parse_memory_list(&out.stdout).expect("listing");
        assert!(listing.exists);
        assert_eq!(listing.dir, r_memory.to_str().unwrap());
        let mut names: Vec<&str> = listing.files.iter().map(|f| f.name.as_str()).collect();
        names.sort();
        assert_eq!(
            names,
            vec!["MEMORY.md", "note.md"],
            "sub/, the symlink and the non-.md file are never listed"
        );
        for f in &listing.files {
            let expect_hash = crate::service::move_session::carry::tests::git(
                &r_memory,
                &["hash-object", "--no-filters", "--", &f.name],
            )
            .trim()
            .to_string();
            assert_eq!(f.hash, expect_hash, "{}", f.name);
        }

        // --- the repo-root memory is gone; the worktree's own is used as a fallback ---
        std::fs::remove_dir_all(&r_memory).unwrap();
        let wt_memory = memory_dir_for(&home, &wt_enc);
        std::fs::create_dir_all(&wt_memory).unwrap();
        std::fs::write(wt_memory.join("fallback.md"), "fb\n").unwrap();

        let out = bash(
            &memory_list_script(wt.to_str().unwrap(), Some(wt.to_str().unwrap())),
            &home,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let listing = parse_memory_list(&out.stdout).expect("listing");
        assert!(listing.exists);
        assert_eq!(listing.dir, wt_memory.to_str().unwrap(), "the fallback dir");
        assert_eq!(
            listing
                .files
                .iter()
                .map(|f| f.name.as_str())
                .collect::<Vec<_>>(),
            vec!["fallback.md"]
        );

        // --- neither exists: exists=false, dir is still the repo-root one ---
        std::fs::remove_dir_all(&wt_memory).unwrap();
        let out = bash(
            &memory_list_script(wt.to_str().unwrap(), Some(wt.to_str().unwrap())),
            &home,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let listing = parse_memory_list(&out.stdout).expect("listing");
        assert!(!listing.exists);
        assert_eq!(
            listing.dir,
            r_memory.to_str().unwrap(),
            "a target would create memory at the repo-root location"
        );
    }

    #[test]
    fn memory_files_are_added_and_never_replaced() {
        if !require(&["bash", "tar"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();

        let src_mem = tmp.path().join("src-memory");
        std::fs::create_dir_all(&src_mem).unwrap();
        std::fs::write(src_mem.join("new.md"), "new content\n").unwrap();
        std::fs::write(src_mem.join("differs.md"), "source version\n").unwrap();

        // --- the target already has its own differs.md; only new.md is carried ---
        let tgt_mem = tmp.path().join("tgt-memory");
        std::fs::create_dir_all(&tgt_mem).unwrap();
        std::fs::write(tgt_mem.join("differs.md"), "target version\n").unwrap();

        let out = bash(
            &crate::service::move_session::carry::pack_script(
                src_mem.to_str().unwrap(),
                ID,
                MEMORY_ARCHIVE,
                &["new.md".to_string()],
            ),
            &home,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let (_bytes, archive) =
            crate::service::move_session::carry::parse_pack(&String::from_utf8_lossy(&out.stdout))
                .unwrap();

        let out = bash(
            &crate::service::move_session::carry::extract_keep_existing_script(
                tgt_mem.to_str().unwrap(),
                &archive,
                true,
            ),
            &home,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            std::fs::read_to_string(tgt_mem.join("new.md")).unwrap(),
            "new content\n"
        );
        assert_eq!(mode(&tgt_mem.join("new.md")), 0o600, "arrived private");
        assert_eq!(
            std::fs::read_to_string(tgt_mem.join("differs.md")).unwrap(),
            "target version\n",
            "not part of the archive: byte-identical to before"
        );

        // --- pack BOTH names: the target's differs.md still wins ---
        let out = bash(
            &crate::service::move_session::carry::pack_script(
                src_mem.to_str().unwrap(),
                ID,
                MEMORY_ARCHIVE,
                &["new.md".to_string(), "differs.md".to_string()],
            ),
            &home,
        );
        assert!(out.status.success());
        let (_bytes, archive2) =
            crate::service::move_session::carry::parse_pack(&String::from_utf8_lossy(&out.stdout))
                .unwrap();
        let out = bash(
            &crate::service::move_session::carry::extract_keep_existing_script(
                tgt_mem.to_str().unwrap(),
                &archive2,
                true,
            ),
            &home,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            std::fs::read_to_string(tgt_mem.join("differs.md")).unwrap(),
            "target version\n",
            "still wins even though the source's differs.md was in this archive too"
        );

        // --- a missing target dir is created private ---
        let fresh_tgt = tmp.path().join("fresh-tgt-memory");
        assert!(!fresh_tgt.exists());
        let out = bash(
            &crate::service::move_session::carry::extract_keep_existing_script(
                fresh_tgt.to_str().unwrap(),
                &archive,
                true,
            ),
            &home,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(mode(&fresh_tgt), 0o700, "the created dir is private");
        assert_eq!(
            std::fs::read_to_string(fresh_tgt.join("new.md")).unwrap(),
            "new content\n"
        );
    }

    /// Builds one `.tgz` per hostile shape python3's `tarfile` can express
    /// but `tar` from a real directory cannot be talked into producing
    /// safely: a symlink member, a subdirectory member, a `..` member, the
    /// index under two spellings, and a bare directory member.
    fn build_memory_archive(dir: &std::path::Path, shape: &str) -> std::path::PathBuf {
        let builder = dir.join("build_memory.py");
        std::fs::write(
            &builder,
            r##"
import tarfile, io, sys
shape, out = sys.argv[1], sys.argv[2]

def reg(tar, name, body=b"carried body\n"):
    ti = tarfile.TarInfo(name)
    ti.size = len(body)
    ti.mode = 0o600
    tar.addfile(ti, io.BytesIO(body))

with tarfile.open(out, "w:gz") as tar:
    if shape == "clean":
        reg(tar, "./new.md")
        reg(tar, "second.md")
    elif shape == "symlink":
        ti = tarfile.TarInfo("./evil.md")
        ti.type = tarfile.SYMTYPE
        ti.linkname = "/etc/hosts"
        tar.addfile(ti)
    elif shape == "subdir":
        reg(tar, "./sub/x.md")
    elif shape == "dotdot":
        reg(tar, "../x.md")
    elif shape == "index":
        reg(tar, "./MEMORY.md", b"# hijacked\n")
    elif shape == "index-case":
        reg(tar, "./memory.MD", b"# hijacked\n")
    elif shape == "dir":
        ti = tarfile.TarInfo("./sub")
        ti.type = tarfile.DIRTYPE
        ti.mode = 0o700
        tar.addfile(ti)
    elif shape == "mixed":
        reg(tar, "./good.md")
        reg(tar, "./bad.txt")
    else:
        raise SystemExit("unknown shape " + shape)
"##,
        )
        .unwrap();
        let archive = dir.join(format!("memory-{shape}.tgz"));
        let out = std::process::Command::new("python3")
            .args([builder.to_str().unwrap(), shape, archive.to_str().unwrap()])
            .output()
            .expect("python3");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        archive
    }

    /// (D) The memory half is the one extract that writes straight into the
    /// user's own notes, so it must not trust the archive: every member is
    /// checked before a single byte is extracted. One bad member refuses the
    /// whole archive and leaves the memory directory byte-identical.
    #[test]
    fn the_memory_extract_refuses_every_member_it_did_not_ask_for() {
        if !require(&["bash", "tar", "python3"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();

        for shape in [
            "symlink",
            "subdir",
            "dotdot",
            "index",
            "index-case",
            "dir",
            "mixed",
        ] {
            let archive = build_memory_archive(tmp.path(), shape);
            let mem = tmp.path().join(format!("memory-{shape}"));
            std::fs::create_dir_all(&mem).unwrap();
            std::fs::write(mem.join(INDEX_NAME), "# Memory Index\n\n- [Own](own.md)\n").unwrap();
            std::fs::write(mem.join("own.md"), "the target's own note\n").unwrap();
            let before: Vec<std::ffi::OsString> = {
                let mut v: Vec<_> = std::fs::read_dir(&mem)
                    .unwrap()
                    .map(|e| e.unwrap().file_name())
                    .collect();
                v.sort();
                v
            };

            let out = bash(
                &memory_extract_script(mem.to_str().unwrap(), archive.to_str().unwrap()),
                &home,
            );
            assert!(
                !out.status.success(),
                "{shape}: must be refused: {}",
                String::from_utf8_lossy(&out.stdout)
            );
            assert!(
                String::from_utf8_lossy(&out.stderr).contains(FAILED),
                "{shape}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            let after: Vec<std::ffi::OsString> = {
                let mut v: Vec<_> = std::fs::read_dir(&mem)
                    .unwrap()
                    .map(|e| e.unwrap().file_name())
                    .collect();
                v.sort();
                v
            };
            assert_eq!(after, before, "{shape}: nothing was extracted");
            assert_eq!(
                std::fs::read_to_string(mem.join(INDEX_NAME)).unwrap(),
                "# Memory Index\n\n- [Own](own.md)\n",
                "{shape}: the index is untouched"
            );
            assert_eq!(
                std::fs::read_to_string(mem.join("own.md")).unwrap(),
                "the target's own note\n",
                "{shape}"
            );
            // Nothing escaped the memory dir either.
            assert!(
                !tmp.path().join("x.md").exists(),
                "{shape}: a `..` member must never land beside the memory dir"
            );
        }
    }

    /// The other half of (D): a clean archive still extracts, still keeps
    /// what the target already has, and still creates a missing memory dir
    /// `0700` with `0600` files — the same semantics the generic
    /// keep-existing extract has, since it runs the very same two lines.
    #[test]
    fn the_memory_extract_still_adds_only_what_the_target_lacks() {
        if !require(&["bash", "tar", "python3"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let archive = build_memory_archive(tmp.path(), "clean");

        // An existing memory dir whose own `second.md` must win.
        let mem = tmp.path().join("memory-clean");
        std::fs::create_dir_all(&mem).unwrap();
        std::fs::write(mem.join("second.md"), "the target's version\n").unwrap();
        let out = bash(
            &memory_extract_script(mem.to_str().unwrap(), archive.to_str().unwrap()),
            &home,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "ok");
        assert_eq!(
            std::fs::read_to_string(mem.join("new.md")).unwrap(),
            "carried body\n"
        );
        assert_eq!(mode(&mem.join("new.md")), 0o600, "arrived private");
        assert_eq!(
            std::fs::read_to_string(mem.join("second.md")).unwrap(),
            "the target's version\n",
            "the target's own copy still wins"
        );

        // A memory dir that does not exist yet is created, private.
        let fresh = tmp.path().join("memory-fresh");
        assert!(!fresh.exists());
        let out = bash(
            &memory_extract_script(fresh.to_str().unwrap(), archive.to_str().unwrap()),
            &home,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(mode(&fresh), 0o700, "the created dir is private");
        assert_eq!(
            std::fs::read_to_string(fresh.join("new.md")).unwrap(),
            "carried body\n"
        );

        // A missing or corrupt archive is still a clean refusal.
        for bad in ["/nonexistent.tgz", "corrupt"] {
            let path = if bad == "corrupt" {
                let p = tmp.path().join("corrupt.tgz");
                std::fs::write(&p, b"not a tarball").unwrap();
                p.to_str().unwrap().to_string()
            } else {
                bad.to_string()
            };
            let out = bash(
                &memory_extract_script(tmp.path().join("memory-bad").to_str().unwrap(), &path),
                &home,
            );
            assert!(!out.status.success(), "{bad}");
            assert!(
                String::from_utf8_lossy(&out.stderr).contains(FAILED),
                "{bad}"
            );
        }

        // And it quotes everything, like every other builder here.
        let evil = "a b'$(touch /tmp/pwn)\n;x";
        let q = quote(evil);
        let script = memory_extract_script(evil, evil);
        assert!(script.contains(&q), "{script}");
        assert!(
            !script.replace(&q, "").contains("touch /tmp/pwn"),
            "raw value leaked: {script}"
        );
    }

    #[test]
    fn the_index_is_only_ever_appended_to() {
        if !require(&["bash"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();

        // --- append onto an existing index with NO trailing newline: the
        // SCRIPT (not `merge_index`) notices and inserts the missing "\n" ---
        let mem_a = tmp.path().join("memory-a");
        std::fs::create_dir_all(&mem_a).unwrap();
        let original = "# Memory Index\n\n- [Mine](mine.md) — t";
        std::fs::write(mem_a.join(INDEX_NAME), original).unwrap();
        std::fs::set_permissions(
            mem_a.join(INDEX_NAME),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();

        let merge = merge_index(
            "- [New](new.md) — hook\n",
            Some(original),
            &["new.md".to_string()],
        );
        assert_eq!(
            merge.append, "- [New](new.md) — hook\n",
            "merge_index never prefixes a newline for Some(_) any more"
        );
        let out = bash(
            &memory_append_index_script(mem_a.to_str().unwrap(), &merge.append),
            &home,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let got = std::fs::read_to_string(mem_a.join(INDEX_NAME)).unwrap();
        assert_eq!(
            got,
            format!("{original}\n{}", merge.append),
            "the script inserted the missing newline itself"
        );
        assert_eq!(mode(&mem_a.join(INDEX_NAME)), 0o644, "mode is untouched");

        // --- append onto an existing index that ALREADY ends with a
        // newline: no blank line is introduced ---
        let mem_a2 = tmp.path().join("memory-a2");
        std::fs::create_dir_all(&mem_a2).unwrap();
        let original2 = "# Memory Index\n\n- [Mine](mine.md) — t\n";
        std::fs::write(mem_a2.join(INDEX_NAME), original2).unwrap();
        let merge_a2 = merge_index(
            "- [New](new.md) — hook\n",
            Some(original2),
            &["new.md".to_string()],
        );
        let out = bash(
            &memory_append_index_script(mem_a2.to_str().unwrap(), &merge_a2.append),
            &home,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            std::fs::read_to_string(mem_a2.join(INDEX_NAME)).unwrap(),
            format!("{original2}{}", merge_a2.append),
            "no blank line is introduced when the target already ends with a newline"
        );

        // --- an existing but EMPTY index file: lines only, nothing prepended ---
        let mem_a3 = tmp.path().join("memory-a3");
        std::fs::create_dir_all(&mem_a3).unwrap();
        std::fs::write(mem_a3.join(INDEX_NAME), "").unwrap();
        let merge_a3 = merge_index(
            "- [New](new.md) — hook\n",
            Some(""),
            &["new.md".to_string()],
        );
        let out = bash(
            &memory_append_index_script(mem_a3.to_str().unwrap(), &merge_a3.append),
            &home,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            std::fs::read_to_string(mem_a3.join(INDEX_NAME)).unwrap(),
            merge_a3.append,
            "an empty existing file gets exactly the lines"
        );

        // --- no index on the target: created 0600 with exactly `append` ---
        let mem_b = tmp.path().join("memory-b");
        let merge2 = merge_index("- [New](new.md) — hook\n", None, &["new.md".to_string()]);
        assert!(merge2.append.starts_with("# Memory Index\n\n"));
        let out = bash(
            &memory_append_index_script(mem_b.to_str().unwrap(), &merge2.append),
            &home,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let got2 = std::fs::read_to_string(mem_b.join(INDEX_NAME)).unwrap();
        assert_eq!(got2, merge2.append);
        assert_eq!(mode(&mem_b.join(INDEX_NAME)), 0o600);
        assert_eq!(mode(&mem_b), 0o700, "the memory dir is created too");

        // --- empty text: a no-op that creates nothing, but still signals
        // success the same way the real append path does ---
        let mem_c = tmp.path().join("memory-c");
        let out = bash(
            &memory_append_index_script(mem_c.to_str().unwrap(), ""),
            &home,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(!mem_c.exists(), "no directory is created for a no-op");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            "ok",
            "one success predicate for both the no-op and the real path"
        );

        // --- round-trip content with quotes, $(...), backslashes, and a
        // __CF_OUT__-lookalike mid-line; a missing index reports Some(None) ---
        let tricky = "line with 'quotes' and \"double\"\nline with $(cmd) substitution\nline with \\backslash\\ chars\nsomething __CF_OUT__ mid-line here\n";
        let mem_d = tmp.path().join("memory-d");
        let out = bash(
            &memory_append_index_script(mem_d.to_str().unwrap(), tricky),
            &home,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let out = bash(&memory_read_index_script(mem_d.to_str().unwrap()), &home);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            parse_index(&String::from_utf8_lossy(&out.stdout)),
            Some(Some(tricky.to_string())),
            "nothing in the body was expanded"
        );

        let mem_e = tmp.path().join("memory-e");
        std::fs::create_dir_all(&mem_e).unwrap();
        let out = bash(&memory_read_index_script(mem_e.to_str().unwrap()), &home);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            parse_index(&String::from_utf8_lossy(&out.stdout)),
            Some(None),
            "a missing index file"
        );
    }

    // --- Fix round 1 ---

    /// (Important) `memory_append_index_script` embeds `text` raw between
    /// `<<'CF_INDEX'` and `CF_INDEX`; it is `pub` and takes any `&str`, so
    /// `merge_index`'s own `line == INDEX_HEREDOC` filter (which only ever
    /// sees text IT built) is not a defence for this function. A `text`
    /// containing a bare `CF_INDEX` line ends the heredoc early and the
    /// remainder is executed as shell — proven here with real bash: this
    /// test's `touch`, if it ran, would leave `pwned.txt` in the memory dir.
    /// The builder must refuse such text (and over-long text) itself, doing
    /// NOTHING to the file, before ever emitting the heredoc.
    #[test]
    fn memory_append_index_script_refuses_a_delimiter_line_or_over_long_text() {
        if !require(&["bash"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();

        let mem = tmp.path().join("memory");
        std::fs::create_dir_all(&mem).unwrap();
        std::fs::write(mem.join(INDEX_NAME), "# Memory Index\n\n- [Old](old.md)\n").unwrap();
        let before = std::fs::read(mem.join(INDEX_NAME)).unwrap();

        // A bare delimiter line followed by a command: if the heredoc ends
        // early, this "runs" as a real shell command.
        let hostile = format!(
            "- [New](new.md) — hook\n{INDEX_HEREDOC}\ntouch {}/pwned.txt\n",
            mem.to_str().unwrap()
        );
        let out = bash(
            &memory_append_index_script(mem.to_str().unwrap(), &hostile),
            &home,
        );
        assert!(
            !out.status.success(),
            "must refuse rather than half-execute: {}",
            String::from_utf8_lossy(&out.stdout)
        );
        assert!(String::from_utf8_lossy(&out.stderr).contains(FAILED));
        assert!(String::from_utf8_lossy(&out.stderr).contains("index-text"));
        assert!(
            !mem.join("pwned.txt").exists(),
            "the remainder was never executed as shell"
        );
        assert_eq!(
            std::fs::read(mem.join(INDEX_NAME)).unwrap(),
            before,
            "the index is untouched, never half-appended"
        );

        // Over-long text: the same refusal, nothing written.
        let huge = "x".repeat(INDEX_APPEND_MAX_BYTES + 1);
        let out = bash(
            &memory_append_index_script(mem.to_str().unwrap(), &huge),
            &home,
        );
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains(FAILED));
        assert_eq!(std::fs::read(mem.join(INDEX_NAME)).unwrap(), before);

        // A line that merely CONTAINS the delimiter text, or has
        // leading/trailing spaces around it, does not end the heredoc: it is
        // accepted and lands byte-exactly.
        let benign = format!("- [x](x.md) {INDEX_HEREDOC}\n  {INDEX_HEREDOC}  \n");
        let out = bash(
            &memory_append_index_script(mem.to_str().unwrap(), &benign),
            &home,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let got = std::fs::read_to_string(mem.join(INDEX_NAME)).unwrap();
        assert_eq!(
            got,
            format!("{}{benign}", String::from_utf8_lossy(&before)),
            "a line that merely contains the delimiter is not the delimiter"
        );
    }

    /// (J1) bash DISCARDS a NUL byte while READING a script, so a line
    /// `CF_INDEX\0` is not equal to [`INDEX_HEREDOC`] as far as Rust's
    /// `l == INDEX_HEREDOC` can see, yet still ends the heredoc as far as
    /// bash is concerned — and the remainder runs as real shell. A NUL can
    /// never travel inside an argv word (`execve` refuses it, so `bash -lc
    /// '<script>'` and every ssh hop reject it before the host sees it), so
    /// the route that proves this is bash reading the script from a FILE;
    /// that is what this test does. The builder must refuse a NUL exactly
    /// like a bare delimiter line.
    #[test]
    fn memory_append_index_script_refuses_a_nul_byte_like_a_delimiter_line() {
        if !require(&["bash"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let mem = tmp.path().join("memory");
        std::fs::create_dir_all(&mem).unwrap();
        std::fs::write(mem.join(INDEX_NAME), "# Memory Index\n\n- [Old](old.md)\n").unwrap();
        let before = std::fs::read(mem.join(INDEX_NAME)).unwrap();

        let hostile = format!(
            "{INDEX_HEREDOC}\0\ntouch {}/pwned.txt\n",
            mem.to_str().unwrap()
        );
        let script = memory_append_index_script(mem.to_str().unwrap(), &hostile);
        let path = tmp.path().join("append.sh");
        std::fs::write(&path, script.as_bytes()).unwrap();
        let out = std::process::Command::new("bash")
            .arg(&path)
            .env("HOME", &home)
            .output()
            .expect("bash");

        assert!(
            !out.status.success(),
            "must refuse rather than let the heredoc end early: {}",
            String::from_utf8_lossy(&out.stdout)
        );
        assert!(String::from_utf8_lossy(&out.stderr).contains(FAILED));
        assert!(String::from_utf8_lossy(&out.stderr).contains("index-text"));
        assert!(
            !mem.join("pwned.txt").exists(),
            "the remainder was never executed as shell"
        );
        assert_eq!(
            std::fs::read(mem.join(INDEX_NAME)).unwrap(),
            before,
            "the index is untouched"
        );
    }

    /// `enc()`'s result was never checked: if its `cd` fails the encoded name
    /// is empty and `m` collapses to `$HOME/.claude/projects//memory` — a
    /// path `parse_memory_list` accepts and a later `create_dir` extract
    /// would happily create, which a coincidental `.claude/projects/memory`
    /// directory (no per-repo hash at all) would then be mistaken for this
    /// repo's memory. A nonexistent fallback (a normal, expected condition —
    /// not an anomaly, unlike an empty `top`) must never be used as `m`; the
    /// primary is reported instead, exactly as when no fallback applies.
    #[test]
    fn a_nonexistent_fallback_never_yields_a_double_slash_memory_dir() {
        if !require(&["bash", "git"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let r = tmp.path().join("r2");
        std::fs::create_dir_all(&r).unwrap();
        crate::service::move_session::carry::tests::git(&r, &["init", "-q", "-b", "main"]);
        std::fs::write(r.join("f.txt"), "x\n").unwrap();
        crate::service::move_session::carry::tests::git(&r, &["add", "-A"]);
        crate::service::move_session::carry::tests::git(&r, &["commit", "-q", "-m", "base"]);

        let r_enc = enc(&r);
        let r_memory = memory_dir_for(&home, &r_enc);
        assert!(!r_memory.exists(), "the primary starts out absent");

        // The directory an empty-encoded fallback would silently collapse
        // onto: `$HOME/.claude/projects//memory` is the same path as this on
        // any POSIX filesystem.
        let trap = home.join(".claude/projects/memory");
        std::fs::create_dir_all(&trap).unwrap();
        std::fs::write(trap.join("someone-elses.md"), "not this repo's memory\n").unwrap();

        let missing_fb = tmp.path().join("does-not-exist-fb");
        assert!(!missing_fb.exists());

        let out = bash(
            &memory_list_script(r.to_str().unwrap(), Some(missing_fb.to_str().unwrap())),
            &home,
        );
        assert!(
            out.status.success(),
            "a nonexistent fallback is not itself a failure: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let listing = parse_memory_list(&out.stdout).expect("listing");
        assert!(
            !listing.exists,
            "the trap directory must never be mistaken for this repo's memory"
        );
        assert_eq!(
            listing.dir,
            r_memory.to_str().unwrap(),
            "the correctly-encoded primary is reported, never the trap directory"
        );
    }

    #[test]
    fn memory_scripts_fail_cleanly_and_quote_everything() {
        if !require(&["bash", "git"]) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();

        // memory_list_script refuses a path that is not a git repo, without
        // ever printing the marker.
        let plain = tmp.path().join("not-a-repo");
        std::fs::create_dir_all(&plain).unwrap();
        let out = bash(&memory_list_script(plain.to_str().unwrap(), None), &home);
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains(FAILED));
        assert!(
            crate::service::move_session::carry::payload(&out.stdout).is_none(),
            "no marker on a real failure"
        );
        let out = bash(
            &memory_list_script(plain.to_str().unwrap(), Some(plain.to_str().unwrap())),
            &home,
        );
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains(FAILED));

        // Every interpolated value is a single inert shell word.
        let evil = "a b'$(touch /tmp/pwn)\n;x";
        let q = quote(evil);
        for script in [
            memory_list_script(evil, Some(evil)),
            memory_read_index_script(evil),
            memory_append_index_script(evil, "safe text\n"),
        ] {
            assert!(script.contains(&q), "{script}");
            let without = script.replace(&q, "");
            assert!(
                !without.contains("touch /tmp/pwn"),
                "raw value leaked: {script}"
            );
        }

        // The generalised carry functions this task introduces quote too.
        for script in [
            crate::service::move_session::carry::pack_script(evil, evil, evil, &[evil.to_string()]),
            crate::service::move_session::carry::extract_keep_existing_script(evil, evil, true),
        ] {
            let alt = quote(&format!("./{evil}"));
            assert!(script.contains(&q) || script.contains(&alt), "{script}");
            let without = script.replace(&q, "").replace(&alt, "");
            assert!(
                !without.contains("touch /tmp/pwn"),
                "raw value leaked: {script}"
            );
        }

        // extract_keep_existing_script still refuses a missing/corrupt archive.
        let cwd = tmp.path().join("extract-target");
        let out = bash(
            &crate::service::move_session::carry::extract_keep_existing_script(
                cwd.to_str().unwrap(),
                "/nonexistent.tgz",
                true,
            ),
            &home,
        );
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains(FAILED));
    }
}
