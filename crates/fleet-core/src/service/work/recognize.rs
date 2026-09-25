//! The one work-reference recogniser (work graph M4.1): ticket keys, ticket
//! URLs and repo-relative `#n` in free text. Pure; no store, no I/O.
//!
//! Its TypeScript twin is `src/lib/work_keys.ts` (`extractTicketRefs`, and
//! `extractWorkKey` over it). Both run the cases in
//! `testdata/recognize_cases.json`, so the desktop's fallback recognition and
//! the hub's detection can never disagree about what a string names.
//!
//! Rules (review C11, plan M4.1):
//!
//! * A key is `PREFIX-123`: a letter, then 1–9 of `[A-Za-z0-9_]`, a dash,
//!   1–7 digits, with no ASCII letter or digit directly before or after it.
//! * An UPPER-case key is taken as written. A lower-case one (`abc-123-fix`)
//!   only when its prefix is letters only and not a word branch names use
//!   for other things ([`DENY`]).
//! * A `.` followed by a digit after the number is a version (`lodash-4.17`),
//!   not a key; a sentence's full stop (`see ABC-12.`) is not.
//! * When any tracker exists (`ctx.prefixes` non-empty), a key must carry one
//!   of the trackers' prefixes — the trackers settle what a key is.
//! * URLs: Jira `/browse/KEY` and `?selectedIssue=KEY`, Linear
//!   `/<ws>/issue/KEY`, Asana `/0/<p>/<task>` and
//!   `/1/<ws>/project/<p>/task/<t>`, GitHub `/<o>/<r>/issues/<n>`. The URL's
//!   host names the tracker when one is configured for it. Text inside any
//!   URL is not scanned for bare keys.
//! * GitHub Enterprise Server (M11.4): `https://<host>/<o>/<r>/issues/<n>`
//!   and a written-out `<host>/<o>/<r>#<n>` name `<host>/<o>/<r>#<n>` — but
//!   ONLY for a `<host>` a configured GitHub tracker has (`ctx.trackers`):
//!   the same path on any other host is not a ticket, so an arbitrary URL is
//!   never taken for a tracker's.
//! * A bare `#123` names an issue of the session's own `owner/repo`, and only
//!   when that repo is known.

use serde::{Deserialize, Serialize};

/// Lower-case prefixes that name something other than a ticket. Checked only
/// for keys that were not written in upper case. Keep in sync with `DENY` in
/// `src/lib/work_keys.ts` (the shared fixture exercises both).
pub const DENY: &[&str] = &[
    "utf",
    "sha",
    "md",
    "iso",
    "rfc",
    "cve",
    "ipv",
    "http",
    "python",
    "node",
    "java",
    "release",
    "hotfix",
    "fix",
    "bugfix",
    "bug",
    "feature",
    "feat",
    "chore",
    "patch",
    "wip",
    "tmp",
    "temp",
    "test",
    "tests",
    "build",
    "version",
    "rc",
    "beta",
    "alpha",
    "step",
    "phase",
    "part",
    "round",
    "try",
    "attempt",
    "day",
    "week",
    "sprint",
    "iter",
    "iteration",
    "draft",
    "backup",
    "copy",
    "old",
    "new",
    "dev",
    "main",
    "master",
    "revert",
    "dependabot",
    "renovate",
    "spike",
    "poc",
    "demo",
];

/// What a match is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchKind {
    /// A bare `ABC-123` in text.
    Key,
    /// A ticket URL.
    Url,
    /// A GitHub issue of a named repository: a bare `#123` resolved against
    /// the session's repo, or an enterprise `host/owner/repo#123` (M11.4).
    RepoIssue,
}

/// A configured tracker, as far as recognition needs it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackerHost {
    pub id: i64,
    /// Lower-case host name, e.g. `acme.atlassian.net`.
    pub host: String,
    /// jira | linear | asana | github.
    #[serde(default)]
    pub provider: String,
}

/// What recognition knows besides the text.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecognizeCtx {
    /// Every tracker's key prefixes, upper case. Empty: no tracker, so any
    /// key-shaped token that passes the case rules is a key.
    #[serde(default)]
    pub prefixes: Vec<String>,
    /// Configured trackers, for naming a URL's tracker by its host.
    #[serde(default)]
    pub trackers: Vec<TrackerHost>,
    /// The session's GitHub `owner/repo`, for a bare `#123`.
    #[serde(default)]
    pub repo: Option<String>,
}

/// One recognised reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Match {
    pub kind: MatchKind,
    /// The normalised reference: `ABC-123`, `owner/repo#42`, `asana:<task>`.
    pub key: String,
    /// The tracker the URL's host names, when one is configured.
    #[serde(default)]
    pub tracker_id: Option<i64>,
    /// jira | linear | asana | github, for a URL; `None` for a bare key.
    #[serde(default)]
    pub provider: Option<String>,
    /// The matched text exactly as written.
    pub text: String,
    /// Byte span of `text` in the input (not part of the shared fixture:
    /// JavaScript counts UTF-16 units).
    #[serde(skip)]
    pub span: (usize, usize),
    /// The key was written in upper case (always true for a URL's key).
    #[serde(default)]
    pub upper_written: bool,
}

fn is_alnum(b: u8) -> bool {
    b.is_ascii_alphanumeric()
}

fn prefix_known(ctx: &RecognizeCtx, prefix: &str) -> bool {
    ctx.prefixes.is_empty() || ctx.prefixes.iter().any(|p| p.eq_ignore_ascii_case(prefix))
}

/// A key starting at byte `i` of `s` (which must be a letter with no ASCII
/// letter/digit before it): `(prefix, number, end)`.
fn key_at(s: &[u8], i: usize) -> Option<(usize, usize, usize)> {
    let mut j = i;
    while j < s.len() && (is_alnum(s[j]) || s[j] == b'_') {
        j += 1;
    }
    let plen = j - i;
    if !(2..=10).contains(&plen) || s.get(j) != Some(&b'-') {
        return None;
    }
    let dstart = j + 1;
    let mut k = dstart;
    while k < s.len() && s[k].is_ascii_digit() {
        k += 1;
    }
    let dlen = k - dstart;
    if !(1..=7).contains(&dlen) {
        return None;
    }
    if k < s.len() && is_alnum(s[k]) {
        return None;
    }
    // `lodash-4.17`: a dot and a digit after the number is a version.
    if s.get(k) == Some(&b'.') && s.get(k + 1).is_some_and(u8::is_ascii_digit) {
        return None;
    }
    Some((j, k, k))
}

/// Case rules for a key found in text: `Some(upper_written)` when it counts.
fn key_accepted(prefix: &str, ctx: &RecognizeCtx) -> Option<bool> {
    let upper = prefix.bytes().any(|b| b.is_ascii_uppercase())
        && !prefix.bytes().any(|b| b.is_ascii_lowercase());
    if !upper {
        if !prefix.bytes().all(|b| b.is_ascii_alphabetic()) {
            return None;
        }
        if DENY.contains(&prefix.to_ascii_lowercase().as_str()) {
            return None;
        }
    }
    prefix_known(ctx, prefix).then_some(upper)
}

/// Every key-shaped token in `text` (no URL handling), as `(start, end,
/// normalised key, upper_written)`.
fn scan_keys(text: &str, ctx: &RecognizeCtx) -> Vec<(usize, usize, String, bool)> {
    let s = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        let starts = s[i].is_ascii_alphabetic() && (i == 0 || !is_alnum(s[i - 1]));
        if starts {
            if let Some((dash, end, next)) = key_at(s, i) {
                let prefix = &text[i..dash];
                if let Some(upper) = key_accepted(prefix, ctx) {
                    let key = format!("{}-{}", prefix.to_ascii_uppercase(), &text[dash + 1..end]);
                    out.push((i, end, key, upper));
                }
                // A key-shaped token is consumed whether or not it counts,
                // exactly as the TypeScript twin's `matchAll` does.
                i = next;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// The single key a URL path segment or query value names (upper-cased), if
/// the whole value is key-shaped.
fn whole_key(v: &str) -> Option<String> {
    let s = v.as_bytes();
    if s.first().is_some_and(u8::is_ascii_alphabetic) {
        if let Some((dash, end, _)) = key_at(s, 0) {
            if end == s.len() {
                return Some(format!(
                    "{}-{}",
                    v[..dash].to_ascii_uppercase(),
                    &v[dash + 1..]
                ));
            }
        }
    }
    None
}

/// Where each URL in `text` is: `(start, end)`, trailing punctuation off.
fn url_spans(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let lower = text.to_ascii_lowercase();
    let mut from = 0;
    while from < text.len() {
        let next = ["https://", "http://"]
            .iter()
            .filter_map(|p| lower[from..].find(p).map(|i| from + i))
            .min();
        let Some(start) = next else { break };
        let mut end = start;
        for (off, c) in text[start..].char_indices() {
            if c.is_whitespace()
                || matches!(c, '<' | '>' | '"' | '\'' | '`' | '(' | ')' | '[' | ']')
            {
                break;
            }
            end = start + off + c.len_utf8();
        }
        while end > start
            && matches!(
                text.as_bytes()[end - 1],
                b'.' | b',' | b';' | b':' | b'!' | b'?'
            )
        {
            end -= 1;
        }
        out.push((start, end));
        from = end.max(start + 1);
    }
    out
}

/// Percent-decode all or nothing, like the twin's `decodeURIComponent`: one
/// malformed `%XX`, or bytes that are not UTF-8, and the input comes back
/// unchanged — so both recognisers read the same key (or none) from one URL.
fn percent_decode(s: &str) -> String {
    let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            match (
                b.get(i + 1).and_then(|&c| hex(c)),
                b.get(i + 2).and_then(|&c| hex(c)),
            ) {
                (Some(h), Some(l)) => {
                    out.push(h * 16 + l);
                    i += 3;
                    continue;
                }
                _ => return s.to_string(),
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

/// A GitHub owner or repository name as a reference carries it:
/// `[A-Za-z0-9_.-]`, not starting with a dot or a dash.
fn github_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && !s.starts_with(['.', '-'])
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

/// The configured GitHub Enterprise hosts (M11.4): every GitHub tracker's
/// host that is not github.com, lower case.
fn ghes_hosts(ctx: &RecognizeCtx) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in ctx.trackers.iter().filter(|t| t.provider == "github") {
        let h = t.host.to_ascii_lowercase();
        if !h.is_empty() && h != "github.com" && h != "www.github.com" && !out.contains(&h) {
            out.push(h);
        }
    }
    out
}

/// Recognise one URL: `(key, provider, host)`.
fn ticket_url(url: &str, ctx: &RecognizeCtx) -> Option<(String, &'static str, String)> {
    let rest = url.split_once("://")?.1;
    let (authority, tail) = match rest.find(['/', '?', '#']) {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let host = authority
        .rsplit('@')
        .next()?
        .split(':')
        .next()?
        .to_ascii_lowercase();
    let (path, query) = {
        let no_frag = tail.split('#').next().unwrap_or("");
        match no_frag.split_once('?') {
            Some((p, q)) => (p, q),
            None => (no_frag, ""),
        }
    };
    let path = percent_decode(path);
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

    if host == "github.com" || host == "www.github.com" {
        if let [o, r, "issues", n, ..] = segs.as_slice() {
            if !n.is_empty() && n.len() <= 9 && n.bytes().all(|b| b.is_ascii_digit()) {
                let key = format!(
                    "{}/{}#{}",
                    o.to_ascii_lowercase(),
                    r.to_ascii_lowercase(),
                    n
                );
                return Some((key, "github", host));
            }
        }
        return None;
    }
    if host == "app.asana.com" {
        let task = match segs.as_slice() {
            ["0", _p, t, ..] => Some(*t),
            ["1", _ws, "project", _p, "task", t, ..] => Some(*t),
            _ => None,
        };
        return task
            .filter(|t| !t.is_empty() && t.len() <= 24 && t.bytes().all(|b| b.is_ascii_digit()))
            .map(|t| (format!("asana:{t}"), "asana", host));
    }
    if host == "linear.app" {
        if let [_ws, "issue", k, ..] = segs.as_slice() {
            return whole_key(k).map(|k| (k, "linear", host));
        }
        return None;
    }
    if ghes_hosts(ctx).contains(&host) {
        if let [o, r, "issues", n, ..] = segs.as_slice() {
            if github_name(o)
                && github_name(r)
                && !n.is_empty()
                && n.len() <= 9
                && n.bytes().all(|b| b.is_ascii_digit())
            {
                let key = format!(
                    "{host}/{}/{}#{n}",
                    o.to_ascii_lowercase(),
                    r.to_ascii_lowercase()
                );
                return Some((key, "github", host));
            }
        }
        return None;
    }
    for pair in query.split('&') {
        if let Some(v) = pair.strip_prefix("selectedIssue=") {
            if let Some(k) = whole_key(&percent_decode(v)) {
                return Some((k, "jira", host));
            }
        }
    }
    if let Some(i) = segs.iter().position(|s| *s == "browse") {
        if let Some(k) = segs.get(i + 1).and_then(|k| whole_key(k)) {
            return Some((k, "jira", host));
        }
    }
    None
}

/// After an enterprise host at byte `i`: `/owner/repo#n` with no letter,
/// digit or `_` after the number → `(end, owner, repo, n)`.
fn ghes_ref_at(text: &str, i: usize) -> Option<(usize, &str, &str, &str)> {
    let s = text.as_bytes();
    let name_end = |from: usize| {
        let mut j = from;
        while j < s.len() && (is_alnum(s[j]) || matches!(s[j], b'_' | b'.' | b'-')) {
            j += 1;
        }
        j
    };
    if s.get(i) != Some(&b'/') {
        return None;
    }
    let o_end = name_end(i + 1);
    if s.get(o_end) != Some(&b'/') {
        return None;
    }
    let r_end = name_end(o_end + 1);
    if s.get(r_end) != Some(&b'#') {
        return None;
    }
    let mut k = r_end + 1;
    while k < s.len() && s[k].is_ascii_digit() {
        k += 1;
    }
    let (o, r, n) = (
        &text[i + 1..o_end],
        &text[o_end + 1..r_end],
        &text[r_end + 1..k],
    );
    if !(1..=9).contains(&n.len())
        || (k < s.len() && (is_alnum(s[k]) || s[k] == b'_'))
        || !github_name(o)
        || !github_name(r)
    {
        return None;
    }
    Some((k, o, r, n))
}

/// Every reference in `text`, in order of appearance.
pub fn recognize(text: &str, ctx: &RecognizeCtx) -> Vec<Match> {
    let urls = url_spans(text);
    let mut out: Vec<Match> = Vec::new();
    for &(a, b) in &urls {
        let raw = &text[a..b];
        if let Some((key, provider, host)) = ticket_url(raw, ctx) {
            let tracker_id = ctx
                .trackers
                .iter()
                .find(|t| t.host.eq_ignore_ascii_case(&host))
                .map(|t| t.id);
            out.push(Match {
                kind: MatchKind::Url,
                key,
                tracker_id,
                provider: Some(provider.to_string()),
                text: raw.to_string(),
                span: (a, b),
                upper_written: true,
            });
        }
    }
    let in_url = |i: usize| urls.iter().any(|&(a, b)| i >= a && i < b);
    for (a, b, key, upper) in scan_keys(text, ctx) {
        if in_url(a) {
            continue;
        }
        out.push(Match {
            kind: MatchKind::Key,
            key,
            tracker_id: None,
            provider: None,
            text: text[a..b].to_string(),
            span: (a, b),
            upper_written: upper,
        });
    }
    // `host/owner/repo#n` of a configured enterprise host (M11.4).
    let lower = text.to_ascii_lowercase();
    let b = text.as_bytes();
    for host in ghes_hosts(ctx) {
        for (a, _) in lower.match_indices(host.as_str()) {
            if in_url(a)
                || (a > 0
                    && (is_alnum(b[a - 1])
                        || matches!(b[a - 1], b'.' | b'_' | b'-' | b'/' | b':' | b'@')))
            {
                continue;
            }
            let Some((end, o, r, n)) = ghes_ref_at(text, a + host.len()) else {
                continue;
            };
            out.push(Match {
                kind: MatchKind::RepoIssue,
                key: format!(
                    "{host}/{}/{}#{n}",
                    o.to_ascii_lowercase(),
                    r.to_ascii_lowercase()
                ),
                tracker_id: ctx
                    .trackers
                    .iter()
                    .find(|t| t.provider == "github" && t.host.eq_ignore_ascii_case(&host))
                    .map(|t| t.id),
                provider: Some("github".into()),
                text: text[a..end].to_string(),
                span: (a, end),
                upper_written: false,
            });
        }
    }
    if let Some(repo) = ctx.repo.as_deref().filter(|r| r.contains('/')) {
        let s = text.as_bytes();
        for (i, _) in text.match_indices('#') {
            if in_url(i)
                || (i > 0 && (is_alnum(s[i - 1]) || matches!(s[i - 1], b'&' | b'#' | b'/')))
            {
                continue;
            }
            let mut k = i + 1;
            while k < s.len() && s[k].is_ascii_digit() {
                k += 1;
            }
            let n = k - i - 1;
            if !(1..=7).contains(&n) || (k < s.len() && (is_alnum(s[k]) || s[k] == b'_')) {
                continue;
            }
            out.push(Match {
                kind: MatchKind::RepoIssue,
                key: format!("{}#{}", repo.to_ascii_lowercase(), &text[i + 1..k]),
                tracker_id: None,
                provider: Some("github".into()),
                text: text[i..k].to_string(),
                span: (i, k),
                upper_written: false,
            });
        }
    }
    out.sort_by_key(|m| m.span.0);
    out
}

/// Is the WHOLE of `key` a work reference of a shape fleet recognises: a
/// ticket key (`ABC-123`, Jira and Linear), a GitHub `owner/repo#n` (or an
/// enterprise `host/owner/repo#n`), or an
/// Asana `asana:<gid>`? Every such shape is made of `[A-Za-z0-9:_#./-]`
/// only, so a key that passes is safe to quote in a prompt typed into a
/// pane; a free-text key that does not is never interpolated (M9.3).
pub fn is_work_key(key: &str) -> bool {
    if !key
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b':' | b'_' | b'#' | b'.' | b'/' | b'-'))
    {
        return false;
    }
    if whole_key(key).is_some() {
        return true;
    }
    if crate::store::github_ref(key).is_some() {
        return true;
    }
    key.strip_prefix("asana:")
        .is_some_and(|g| (1..=24).contains(&g.len()) && g.bytes().all(|b| b.is_ascii_digit()))
}

/// The first bare key in `text` (no tracker restriction), for branch names,
/// tags and worktree names: the rule `extractWorkKey` applies on the desktop.
pub fn first_key(text: &str, ctx: &RecognizeCtx) -> Option<String> {
    recognize(text, ctx)
        .into_iter()
        .find(|m| m.kind == MatchKind::Key)
        .map(|m| m.key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize)]
    struct Case {
        name: String,
        text: String,
        #[serde(default)]
        ctx: RecognizeCtx,
        expect: Vec<Match>,
    }

    /// The fixture both test suites read (see the module docs).
    #[test]
    fn the_shared_fixture_passes() {
        let cases: Vec<Case> =
            serde_json::from_str(include_str!("testdata/recognize_cases.json")).unwrap();
        assert!(cases.len() >= 30, "the fixture lost its cases");
        for c in cases {
            let got = recognize(&c.text, &c.ctx);
            let strip = |v: Vec<Match>| {
                v.into_iter()
                    .map(|m| Match { span: (0, 0), ..m })
                    .collect::<Vec<_>>()
            };
            assert_eq!(strip(got), c.expect, "case {:?}: {:?}", c.name, c.text);
        }
    }

    /// The deny list is written twice (Rust and TypeScript); they must agree.
    #[test]
    fn the_deny_list_matches_the_typescript_twin() {
        let ts = include_str!("../../../../../src/lib/work_keys.ts");
        let start = ts
            .find("const DENY = new Set([")
            .expect("DENY in work_keys.ts");
        let body = &ts[start..start + ts[start..].find("]);").unwrap()];
        let words: Vec<&str> = body.split('\'').skip(1).step_by(2).collect();
        assert_eq!(
            words, DENY,
            "DENY differs between recognize.rs and work_keys.ts"
        );
    }

    #[test]
    fn spans_point_at_the_matched_text() {
        let text = "é see abc-12 and https://x.atlassian.net/browse/ABC-9 #4";
        let ctx = RecognizeCtx {
            repo: Some("o/r".into()),
            ..Default::default()
        };
        for m in recognize(text, &ctx) {
            assert_eq!(&text[m.span.0..m.span.1], m.text);
        }
    }

    #[test]
    fn never_panics_on_odd_input() {
        for t in [
            "%",
            "https://",
            "http://%zz/%",
            "#",
            "a-",
            "-1",
            "é-1",
            "https://é/browse/É-1%",
        ] {
            let _ = recognize(
                t,
                &RecognizeCtx {
                    repo: Some("o/r".into()),
                    ..Default::default()
                },
            );
        }
    }

    #[test]
    fn a_work_key_is_one_of_the_recognised_shapes_and_nothing_else() {
        for k in ["PAY-7", "abc-123", "acme/api#42", "asana:1207000000000004"] {
            assert!(is_work_key(k), "{k}");
        }
        for k in [
            "",
            "X-1$(curl h|sh)",
            "PAY-7; rm -rf ~",
            "PAY-7\nexit",
            "acme/api#+4",
            "asana:12a",
            "asana:",
            "just words",
            "`PAY-7`",
        ] {
            assert!(!is_work_key(k), "{k:?}");
        }
    }
}
