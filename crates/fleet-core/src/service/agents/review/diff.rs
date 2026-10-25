//! The diff the reviewer reads, ported from claude-code-reviewer's
//! `src/reviewer/diff-parser.ts` with its tests (`diff-parser.test.ts`).
//!
//! * Exclusions: the base branch's `.claudeignore` (gitignore syntax, parsed
//!   by [`parse_claude_ignore`]) first, then the rule's `exclude_paths`,
//!   last match wins ([`is_excluded`]), so an operator's rule always beats a
//!   repo's.
//! * [`filter_diff`] drops a whole `diff --git` block whose path is
//!   excluded, and it runs before every size measurement: an excluded file
//!   never counts toward `max_diff_lines` ([`count_diff_lines`]).
//! * [`commentable_lines`] are the right-side lines GitHub's review API
//!   takes an inline comment on; [`snap_line`] moves a finding's line to the
//!   nearest one within three lines, or gives up.

use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};

/// PURE: a glob as a regex. `*` and `?` stay within one path segment, `**`
/// crosses segments, and `**/` also matches no leading segment at all.
fn glob_regex(pattern: &str) -> Option<Regex> {
    let mut out = String::from("^");
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '*' if chars.get(i + 1) == Some(&'*') => {
                i += 1;
                if chars.get(i + 1) == Some(&'/') {
                    i += 1;
                    out.push_str("(?:.*/)?");
                } else {
                    out.push_str(".*");
                }
            }
            '*' => out.push_str("[^/]*"),
            '?' => out.push_str("[^/]"),
            c => out.push_str(&regex::escape(&c.to_string())),
        }
        i += 1;
    }
    out.push('$');
    Regex::new(&out).ok()
}

/// PURE: whether `path` matches the glob `pattern`.
pub fn glob_match(pattern: &str, path: &str) -> bool {
    glob_regex(pattern).is_some_and(|r| r.is_match(path))
}

/// PURE: whether `path` is excluded by `patterns`, with `!` negations and
/// gitignore's last-match-wins.
pub fn is_excluded(path: &str, patterns: &[String]) -> bool {
    let mut excluded = false;
    for p in patterns {
        if let Some(neg) = p.strip_prefix('!') {
            if excluded && glob_match(neg, path) {
                excluded = false;
            }
        } else if !excluded && glob_match(p, path) {
            excluded = true;
        }
    }
    excluded
}

/// PURE: a `.claudeignore` (gitignore syntax) as glob patterns for
/// [`is_excluded`]. Negations keep their `!`; dropping them would silently
/// over-exclude.
pub fn parse_claude_ignore(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    for raw in content.split('\n') {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let negated = line.starts_with('!');
        let body = if negated {
            &line[1..]
        } else if line.starts_with("\\!") || line.starts_with("\\#") {
            // A leading backslash escapes a literal `!` or `#`.
            &line[1..]
        } else {
            line
        };
        if body.is_empty() {
            continue;
        }
        let is_dir = body.ends_with('/');
        let core = if is_dir {
            &body[..body.len() - 1]
        } else {
            body
        };
        let explicit_anchor = core.starts_with('/');
        let base = if explicit_anchor { &core[1..] } else { core };
        if base.is_empty() {
            continue;
        }
        // Anchored to the root when it starts with `/` or has an inner `/`;
        // otherwise it matches at any depth.
        let anchored = explicit_anchor || base.contains('/');
        let bases = if anchored {
            vec![base.to_string()]
        } else {
            vec![base.to_string(), format!("**/{base}")]
        };
        let prefix = if negated { "!" } else { "" };
        for b in bases {
            // The entry itself and, unless dir-only, everything under it: a
            // bare pattern may name a directory too.
            if !is_dir {
                out.push(format!("{prefix}{b}"));
            }
            out.push(format!("{prefix}{b}/**"));
        }
    }
    out
}

/// PURE: the exclusion list for one review: the repo's `.claudeignore`
/// first, then the rule's own paths, which therefore win.
pub fn exclude_patterns(exclude_paths: &[String], claude_ignore: Option<&str>) -> Vec<String> {
    let mut out = claude_ignore.map(parse_claude_ignore).unwrap_or_default();
    out.extend(exclude_paths.iter().cloned());
    out
}

/// PURE: the size every gate compares: the filtered diff's line count.
pub fn count_diff_lines(diff: &str) -> usize {
    if diff.is_empty() {
        0
    } else {
        diff.split('\n').count()
    }
}

/// PURE: the path a `diff --git` block at `start` is about: the new-file
/// header, else the rename target, else the old-file header (a deletion),
/// else the header line's `b/` path.
fn block_path(lines: &[&str], start: usize) -> Option<String> {
    let (mut new, mut old, mut renamed) = (None, None, None);
    for line in &lines[start + 1..] {
        if line.starts_with("@@") || line.starts_with("diff --git ") {
            break;
        }
        if let Some(p) = line.strip_prefix("+++ b/") {
            new = Some(p);
        } else if let Some(p) = line.strip_prefix("--- a/") {
            old = Some(p);
        } else if let Some(p) = line.strip_prefix("rename to ") {
            renamed = Some(p);
        }
    }
    if let Some(p) = new.or(renamed).or(old) {
        return Some(p.to_string());
    }
    let header = lines[start].strip_prefix("diff --git a/")?;
    header.split_once(" b/").map(|(_, b)| b.to_string())
}

/// PURE: `diff` without the blocks of excluded files, and how many went.
pub fn filter_diff(diff: &str, patterns: &[String]) -> (String, usize) {
    if patterns.is_empty() {
        return (diff.to_string(), 0);
    }
    let lines: Vec<&str> = diff.split('\n').collect();
    let mut out = Vec::with_capacity(lines.len());
    let (mut excluded, mut excluding) = (0, false);
    for (i, line) in lines.iter().enumerate() {
        if line.starts_with("diff --git ") {
            excluding = block_path(&lines, i).is_some_and(|p| is_excluded(&p, patterns));
            if excluding {
                excluded += 1;
            }
        }
        if !excluding {
            out.push(*line);
        }
    }
    (out.join("\n"), excluded)
}

/// Why a diff is not reviewed: nothing is left after the exclusions, or it
/// is longer than the limit (its filtered line count).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    Empty,
    TooLarge(usize),
}

impl Gate {
    /// The skip's code, as stored and shown: `empty_diff` / `diff_too_large`.
    pub fn code(self) -> &'static str {
        match self {
            Gate::Empty => "empty_diff",
            Gate::TooLarge(_) => "diff_too_large",
        }
    }

    /// The skip's reason, `max_lines` the limit it was held to.
    pub fn reason(self, max_lines: usize) -> String {
        match self {
            Gate::Empty => "empty_diff: nothing left after the exclusions".into(),
            Gate::TooLarge(lines) => format!("diff_too_large: {lines} lines (max {max_lines})"),
        }
    }
}

/// PURE: `raw` without the excluded files' blocks, or why it is not
/// reviewed: nothing left, or more than `max_lines` lines.
pub fn gate(raw: &str, patterns: &[String], max_lines: usize) -> Result<String, Gate> {
    let (filtered, _) = filter_diff(raw, patterns);
    if !filtered.split('\n').any(|l| l.starts_with("diff --git ")) {
        return Err(Gate::Empty);
    }
    let lines = count_diff_lines(&filtered);
    if lines > max_lines {
        return Err(Gate::TooLarge(lines));
    }
    Ok(filtered)
}

/// Per file, the right-side lines an inline comment may sit on.
pub type Commentable = BTreeMap<String, BTreeSet<u32>>;

/// PURE: the commentable lines of a unified diff: additions and context on
/// the new side.
pub fn commentable_lines(diff: &str) -> Commentable {
    let hunk = Regex::new(r"^@@ -\d+(?:,\d+)? \+(\d+)(?:,\d+)? @@").expect("static regex");
    let mut out = Commentable::new();
    let mut current: Option<String> = None;
    let mut right = 0u32;
    for line in diff.split('\n') {
        if let Some(p) = line.strip_prefix("+++ b/") {
            out.entry(p.to_string()).or_default();
            current = Some(p.to_string());
            continue;
        }
        if line.starts_with("--- ") {
            continue;
        }
        if line.starts_with("diff --git") {
            current = None;
            continue;
        }
        if let Some(c) = hunk.captures(line) {
            right = c[1].parse().unwrap_or(0);
            continue;
        }
        let Some(path) = &current else {
            continue;
        };
        if line.starts_with('+') || line.starts_with(' ') {
            out.entry(path.clone()).or_default().insert(right);
            right += 1;
        }
    }
    out
}

/// PURE: every file a diff adds or changes.
pub fn diff_paths(diff: &str) -> Vec<String> {
    diff.split('\n')
        .filter_map(|l| l.strip_prefix("+++ b/"))
        .map(str::to_string)
        .collect()
}

/// How far [`snap_line`] looks either way.
pub const SNAP_DISTANCE: u32 = 3;

/// PURE: the commentable line nearest `line` in `path`: the line itself,
/// else `line ± d` for `d` in `1..=3`, below before above. `None` when
/// there is none that close.
pub fn snap_line(c: &Commentable, path: &str, line: u32) -> Option<u32> {
    let lines = c.get(path)?;
    if lines.contains(&line) {
        return Some(line);
    }
    (1..=SNAP_DISTANCE).find_map(|d| {
        if lines.contains(&(line + d)) {
            Some(line + d)
        } else if line > d && lines.contains(&(line - d)) {
            Some(line - d)
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn excluded(path: &str, ignore: &str) -> bool {
        is_excluded(path, &parse_claude_ignore(ignore))
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn claude_ignore_follows_gitignore() {
        // A directory and everything under it.
        assert!(excluded("graphify-out/cache/a.json", "graphify-out/cache/"));
        assert!(excluded(
            "graphify-out/cache/deep/nested/b.json",
            "graphify-out/cache/"
        ));
        assert!(!excluded("graphify-out/src/a.ts", "graphify-out/cache/"));
        // Unanchored at any depth; anchored by a leading or inner slash.
        assert!(excluded("dist/main.js", "dist/"));
        assert!(excluded("packages/api/dist/main.js", "dist/"));
        assert!(excluded("dist/main.js", "/dist/"));
        assert!(!excluded("packages/api/dist/main.js", "/dist/"));
        assert!(excluded("build/out.js", "build/out.js"));
        assert!(!excluded("sub/build/out.js", "build/out.js"));
        // A bare file name at root and at depth.
        assert!(excluded("package-lock.json", "package-lock.json"));
        assert!(excluded(
            "packages/api/package-lock.json",
            "package-lock.json"
        ));
        // Wildcards stay within a segment; the pattern may name a directory.
        assert!(excluded("yarn.lock", "*.lock"));
        assert!(excluded("vendor/yarn.lock", "*.lock"));
        assert!(excluded("a.lock/b.ts", "*.lock"));
        assert!(!excluded("src/main.ts", "*.lock"));
        assert!(excluded("a1.log", "a?.log"));
        assert!(!excluded("a.log", "a?.log"));
        assert!(!excluded("a/b.log", "a?b.log"));
        // `**/` matches at the root too.
        assert!(excluded("generated/x.ts", "**/generated/"));
        assert!(excluded("src/generated/x.ts", "**/generated/"));
        // Negations, last match wins.
        let ignore = "generated/\n!generated/schema.ts";
        assert!(excluded("generated/client.ts", ignore));
        assert!(!excluded("generated/schema.ts", ignore));
        // Comments, blanks, escapes.
        assert!(parse_claude_ignore("# comment\n\n   \n").is_empty());
        assert!(excluded("#notes.md", "\\#notes.md"));
        // Regex metacharacters are literal.
        assert!(excluded("a+b(1).txt", "a+b(1).txt"));
        assert!(!excluded("aab1.txt", "a+b(1).txt"));
    }

    #[test]
    fn the_rules_own_paths_beat_the_repos_negation() {
        assert_eq!(exclude_patterns(&s(&["dist/**"]), None), s(&["dist/**"]));
        let p = exclude_patterns(&s(&["secrets/**"]), Some("!secrets/prod.env"));
        assert!(is_excluded("secrets/prod.env", &p));
        let p = exclude_patterns(&s(&["dist/**"]), Some("logs/"));
        assert_eq!(p, s(&["logs/**", "**/logs/**", "dist/**"]));
    }

    const MODIFIED: &str = "diff --git a/src/app.ts b/src/app.ts\nindex 111..222 100644\n--- a/src/app.ts\n+++ b/src/app.ts\n@@ -1,2 +1,2 @@\n-old\n+new\n";
    const DELETED: &str = "diff --git a/cache/big.json b/cache/big.json\ndeleted file mode 100644\nindex 333..0000000\n--- a/cache/big.json\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-{\"a\":1}\n-{\"b\":2}\n";
    const RENAMED: &str = "diff --git a/cache/old.json b/cache/new.json\nsimilarity index 100%\nrename from cache/old.json\nrename to cache/new.json\n";

    fn big(path: &str, n: usize) -> String {
        let body: Vec<String> = (0..n).map(|i| format!("+{{\"node\":{i}}}")).collect();
        format!(
            "diff --git a/{path} b/{path}\nnew file mode 100644\nindex 0000000..444\n--- /dev/null\n+++ b/{path}\n@@ -0,0 +1,{n} @@\n{}\n",
            body.join("\n")
        )
    }

    #[test]
    fn filter_drops_whole_blocks() {
        assert_eq!(filter_diff(MODIFIED, &[]), (MODIFIED.to_string(), 0));
        let cache = s(&["cache/**"]);
        let (f, n) = filter_diff(&format!("{MODIFIED}{DELETED}"), &cache);
        assert_eq!(n, 1);
        assert!(f.contains("src/app.ts") && !f.contains("cache/big.json"));
        assert_eq!(
            filter_diff(DELETED, &cache).1,
            1,
            "a deletion has no +++ b/"
        );
        let (f, n) = filter_diff(RENAMED, &cache);
        assert_eq!(
            (f.trim(), n),
            ("", 1),
            "a pure rename has no ---/+++ at all"
        );
        let (f, _) = filter_diff(&format!("{DELETED}{MODIFIED}{RENAMED}"), &cache);
        assert!(f.contains("+new") && !f.contains("{\"a\":1}"));
    }

    #[test]
    fn excluded_files_never_count_toward_the_limit() {
        assert_eq!(count_diff_lines(""), 0);
        assert_eq!(count_diff_lines(MODIFIED), MODIFIED.split('\n').count());
        let diff = format!(
            "{MODIFIED}{}{}",
            big("build/out.js", 40_000),
            big("vendor/lib.js", 40_000)
        );
        let p = exclude_patterns(&s(&["vendor/**"]), Some("**/build/"));
        let (f, n) = filter_diff(&diff, &p);
        assert_eq!(n, 2);
        assert!(count_diff_lines(&f) <= count_diff_lines(MODIFIED));
        assert!(f.contains("+new"));
    }

    #[test]
    fn commentable_lines_are_the_new_side() {
        let diff = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -10,4 +10,5 @@ fn x\n ctx\n-gone\n+added\n+added2\n ctx2\n\\ No newline at end of file\n";
        let c = commentable_lines(diff);
        assert_eq!(c["a.rs"], [10, 11, 12, 13].into_iter().collect());
        assert_eq!(snap_line(&c, "a.rs", 11), Some(11));
        assert_eq!(snap_line(&c, "a.rs", 15), Some(13), "three below");
        assert_eq!(snap_line(&c, "a.rs", 8), Some(10), "two above");
        assert_eq!(snap_line(&c, "a.rs", 30), None);
        assert_eq!(snap_line(&c, "b.rs", 10), None);
        assert_eq!(diff_paths(diff), s(&["a.rs"]));
        // Below wins a tie: 7 is commentable, 5 is not; from 6 the +1 is 7.
        let mut t = Commentable::new();
        t.insert("f".into(), [3, 7].into_iter().collect());
        assert_eq!(snap_line(&t, "f", 5), Some(7));
        assert_eq!(snap_line(&t, "f", 1), Some(3));
    }
}
