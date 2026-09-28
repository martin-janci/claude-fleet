//! What Jira Cloud (M3) and Jira Data Center (M6.5) share: the error table
//! (CAPTCHA included), status category and resolution, the sprint field,
//! ADF → text, and key recognition. Everything else — the API version,
//! paging, bulk fetch, the epic — is each adapter's own.

use super::{CallKind as Call, TrackerError, DESCRIPTION_MAX_CHARS, MAX_RETRY_AFTER_SECS};
use crate::net::https::Response;
use serde_json::Value;

/// The remote link's global id for a PR (work graph M13.4e): Jira upserts
/// a remote link by it, so writing the same PR twice changes nothing.
pub fn pr_global_id(url: &str) -> String {
    format!("fleet:pr:{url}")
}

/// The body of `POST …/issue/{key}/remotelink` for a PR. Only fleet's own
/// words and the PR's URL: nothing from a transcript or a tracker.
pub fn pr_remote_link_body(url: &str, title: &str) -> Value {
    serde_json::json!({
        "globalId": pr_global_id(url),
        "application": { "type": "claude-fleet", "name": "claude-fleet" },
        "relationship": "pull request",
        "object": { "url": url, "title": title },
    })
}

/// The sprint field's `schema.custom`.
pub const SPRINT_FIELD_SCHEMA: &str = "com.pyxis.greenhopper.jira:gh-sprint";
/// The Epic Link field's `schema.custom` (Data Center; Cloud uses `parent`).
pub const EPIC_LINK_SCHEMA: &str = "com.pyxis.greenhopper.jira:gh-epic-link";

/// Map a non-2xx answer (C25–C28 and the plan's error list).
pub(crate) fn check(resp: &Response, call: Call) -> Result<(), TrackerError> {
    if resp.is_success() {
        return Ok(());
    }
    let captcha = resp
        .header("X-Seraph-LoginReason")
        .is_some_and(|r| r.contains("AUTHENTICATION_DENIED"));
    let retry_after = resp
        .header("Retry-After")
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(|s| s.min(MAX_RETRY_AFTER_SECS));
    Err(match resp.status {
        401 | 403 if captcha => TrackerError::Captcha,
        401 => TrackerError::Auth("401 Unauthorized".into()),
        403 if call == Call::Identity => TrackerError::Auth("403 on /myself".into()),
        403 if call == Call::View => TrackerError::Forbidden("403 on this view's query".into()),
        403 => TrackerError::Forbidden("403".into()),
        404 => TrackerError::NotFound,
        429 => TrackerError::RateLimited {
            retry_after_secs: retry_after,
        },
        503 if retry_after.is_some() => TrackerError::RateLimited {
            retry_after_secs: retry_after,
        },
        300..=399 => TrackerError::Invalid(format!(
            "{} redirect (not followed){}",
            resp.status,
            resp.header("Location")
                .map(|l| format!(" to {}", crate::logging::redact(l)))
                .unwrap_or_default()
        )),
        s => TrackerError::Invalid(format!("HTTP {s}")),
    })
}

/// `statusCategory.key` → fleet's category. `undefined` (C26) and anything
/// unknown count as todo: never claim work is under way or done on a guess.
pub fn map_status_category(key: Option<&str>) -> &'static str {
    match key {
        Some("indeterminate") => "in_progress",
        Some("done") => "done",
        _ => "todo",
    }
}

/// A resolution name → completed | not_planned | duplicate. Conservative:
/// only names that plainly say "not done" or "duplicate" are told apart;
/// everything else (Done, Fixed, a custom name) is `completed`.
pub fn normalize_resolution(name: &str) -> String {
    let n = name.trim().to_lowercase().replace('\u{2019}', "'");
    if n.contains("duplicate") {
        return "duplicate".into();
    }
    const NOT_PLANNED: &[&str] = &[
        "won't do",
        "wont do",
        "won't fix",
        "wont fix",
        "declined",
        "cancelled",
        "canceled",
        "rejected",
        "obsolete",
    ];
    if NOT_PLANNED.contains(&n.as_str()) {
        "not_planned".into()
    } else {
        "completed".into()
    }
}

/// The sprint field's value → (the current sprint's name, it is active). An
/// active sprint wins; else the newest future one; closed ones are history.
pub(crate) fn current_sprint(v: &Value) -> (Option<String>, bool) {
    let Some(list) = v.as_array() else {
        return (None, false);
    };
    // Cloud answers objects; Data Center's v2 often the legacy string
    // `com.atlassian.greenhopper.service.sprint.Sprint@1a2b[id=5,…,
    // state=ACTIVE,name=PLAT Sprint 3,…]`. Both become (state, name).
    let sprints: Vec<(String, Option<String>)> = list
        .iter()
        .map(|s| match s {
            Value::String(t) => (
                legacy_field(t, "state").unwrap_or_default().to_lowercase(),
                legacy_field(t, "name"),
            ),
            _ => (
                s["state"].as_str().unwrap_or_default().to_lowercase(),
                s["name"].as_str().map(str::to_string),
            ),
        })
        .collect();
    if let Some((_, n)) = sprints.iter().find(|(st, _)| st == "active") {
        return (n.clone(), true);
    }
    (
        sprints
            .iter()
            .rev()
            .find(|(st, _)| st == "future")
            .and_then(|(_, n)| n.clone()),
        false,
    )
}

/// `key=value` out of a legacy `Sprint@…[a=1,name=X,…]` string. A name may
/// hold a comma, so it runs to the next `,<word>=`.
fn legacy_field(t: &str, key: &str) -> Option<String> {
    let body = t.split_once('[')?.1.trim_end_matches(']');
    let start = body
        .match_indices(&format!("{key}="))
        .find(|(i, _)| *i == 0 || body.as_bytes()[i - 1] == b',')?
        .0
        + key.len()
        + 1;
    let rest = &body[start..];
    let end = rest
        .match_indices(',')
        .find(|(i, _)| {
            let after = &rest[i + 1..];
            after
                .split_once('=')
                .is_some_and(|(k, _)| !k.is_empty() && k.bytes().all(|b| b.is_ascii_alphanumeric()))
        })
        .map(|(i, _)| i)
        .unwrap_or(rest.len());
    let v = rest[..end].trim();
    (!v.is_empty() && v != "<null>").then(|| v.to_string())
}

/// Plain text out of an Atlassian Document Format value, at most
/// [`DESCRIPTION_MAX_CHARS`] characters, with its true length before that
/// cap. A plain string (API v2, or a renderer) is taken as is.
///
/// `adf_walk` stops *appending* to the excerpt once it reaches the cap (so
/// this stays cheap on a huge document), but keeps *counting* every node's
/// text regardless: the response is already downloaded and deserialised, so
/// walking the rest of an in-memory [`Value`] is not the expensive part, and
/// refusing to count would make the true length wrong for the common case of
/// a description spread across several blocks (a heading, paragraphs, a
/// list) that individually stay small but add up past the cap.
pub fn adf_excerpt(v: &Value) -> (Option<String>, Option<i64>) {
    let mut out = String::new();
    let mut total: i64 = 0;
    match v {
        Value::String(s) => {
            out.push_str(s);
            total = s.chars().count() as i64;
        }
        Value::Object(_) => adf_walk(v, &mut out, &mut total, DESCRIPTION_MAX_CHARS),
        _ => return (None, None),
    }
    let text = out
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();
    if text.is_empty() {
        return (None, None);
    }
    (
        Some(text.chars().take(DESCRIPTION_MAX_CHARS).collect()),
        Some(total),
    )
}

/// `total` accumulates every node's contributed text unconditionally (a few
/// characters of drift from `out`'s whitespace normalisation is fine — the
/// number exists to say "there is more", not to be byte-exact); `out` is
/// only ever appended to while it is still at or under `cap`, so its final
/// content is exactly what it was before `total` existed.
///
/// `cap` is [`DESCRIPTION_MAX_CHARS`] for [`adf_excerpt`]'s 2k excerpt, and
/// [`super::DESCRIBE_MAX_CHARS`] for [`adf_text`]'s whole-description answer
/// — the same walk, stopped at a different ceiling.
fn adf_walk(node: &Value, out: &mut String, total: &mut i64, cap: usize) {
    let attrs = &node["attrs"];
    let contribution: &str = match node["type"].as_str().unwrap_or_default() {
        "text" => node["text"].as_str().unwrap_or_default(),
        "hardBreak" => "\n",
        "mention" => attrs["text"].as_str().unwrap_or("@someone"),
        "emoji" => attrs["shortName"].as_str().unwrap_or_default(),
        "inlineCard" | "blockCard" => attrs["url"].as_str().unwrap_or_default(),
        "status" => attrs["text"].as_str().unwrap_or_default(),
        "listItem" => "- ",
        _ => "",
    };
    *total += contribution.chars().count() as i64;
    if out.chars().count() <= cap {
        out.push_str(contribution);
    }
    if let Some(children) = node["content"].as_array() {
        for c in children {
            adf_walk(c, out, total, cap);
        }
    }
    if matches!(
        node["type"].as_str(),
        Some("paragraph" | "heading" | "codeBlock" | "blockquote" | "rule" | "listItem")
    ) {
        *total += 1;
        if out.chars().count() <= cap && !out.ends_with('\n') {
            out.push('\n');
        }
    }
}

/// The WHOLE plain text of an ADF value, at most `cap` characters — unlike
/// [`adf_excerpt`], this is not the 2k excerpt plus a true length: `describe`
/// wants the full description (capped much higher, at
/// [`super::DESCRIBE_MAX_CHARS`]), and has no separate use for "how much more
/// there is" once it is serving all of it up to `cap`.
pub fn adf_text(v: &Value, cap: usize) -> Option<String> {
    let mut out = String::new();
    let mut total: i64 = 0;
    match v {
        Value::String(s) => out.push_str(s),
        Value::Object(_) => adf_walk(v, &mut out, &mut total, cap),
        _ => return None,
    }
    let text = out
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();
    (!text.is_empty()).then(|| text.chars().take(cap).collect())
}

/// The issue key a Jira URL path names.
pub(crate) fn key_in_path(path: &str) -> Option<String> {
    let (path, query) = path.split_once('?').unwrap_or((path, ""));
    let path = path.split('#').next().unwrap_or(path);
    if let Some(k) = path
        .strip_prefix("browse/")
        .map(|k| k.split('/').next().unwrap_or(k))
    {
        if is_key(k) {
            return Some(k.to_ascii_uppercase());
        }
    }
    query
        .split(['&', '#'])
        .filter_map(|kv| kv.split_once('='))
        .find(|(k, _)| *k == "selectedIssue")
        .map(|(_, v)| v)
        .filter(|v| is_key(v))
        .map(str::to_ascii_uppercase)
}

/// Longest project key. Cloud stops at 10; Data Center lets an admin raise
/// `jira.projectkey.maxlength`, so this is an upper bound on the shape, not
/// Cloud's limit. With the dash and up to 7 digits a key stays inside
/// [`KEY_MAX_CHARS`](super::sync::KEY_MAX_CHARS).
pub const KEY_PREFIX_MAX_CHARS: usize = 50;

/// `ABC-123`: a letter, then letters/digits/underscore (at most
/// [`KEY_PREFIX_MAX_CHARS`]), a dash, digits; the whole within
/// `KEY_MAX_CHARS`. Every Jira adapter's snapshot AND fetch go through this
/// one shape, so what the sync stores is what a lookup asks for.
pub(crate) fn is_key(s: &str) -> bool {
    let Some((p, n)) = s.split_once('-') else {
        return false;
    };
    s.len() <= super::sync::KEY_MAX_CHARS
        && p.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && (2..=KEY_PREFIX_MAX_CHARS).contains(&p.len())
        && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && (1..=7).contains(&n.len())
        && n.chars().all(|c| c.is_ascii_digit())
}

/// Keys with one of `prefixes` in free text, case-insensitive, bounded by
/// non-alphanumerics (C11), upper-cased, in order, without repeats.
pub fn keys_in_text(text: &str, prefixes: &[String]) -> Vec<String> {
    if prefixes.is_empty() {
        return Vec::new();
    }
    let alternation = prefixes
        .iter()
        .map(|p| regex::escape(p))
        .collect::<Vec<_>>()
        .join("|");
    // The regex crate has no lookaround; the boundary is checked by hand.
    let Ok(re) = regex::Regex::new(&format!(r"(?i)({alternation})-(\d{{1,7}})")) else {
        return Vec::new();
    };
    let bytes = text.as_bytes();
    let mut out: Vec<String> = Vec::new();
    for m in re.find_iter(text) {
        let before_ok = m.start() == 0 || !bytes[m.start() - 1].is_ascii_alphanumeric();
        let after_ok = m.end() == bytes.len() || !bytes[m.end()].is_ascii_alphanumeric();
        if before_ok && after_ok {
            let k = m.as_str().to_ascii_uppercase();
            if !out.contains(&k) {
                out.push(k);
            }
        }
    }
    out
}

/// [`keys_in_text`] over the words of `text` that are not URLs: a key in
/// another site's URL is not this tracker's. (Each adapter's `recognize`
/// reads its own site's URLs first, by host.)
pub fn keys_in_prose(text: &str, prefixes: &[String]) -> Vec<String> {
    // Words split on whitespace AND on the brackets and commas prose glues a
    // URL to a key with — `ABC-1(https://…/ABC-9)` — so dropping the
    // URL-bearing word keeps the key beside it.
    let prose: String = text
        .split(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | ','))
        .filter(|w| !w.is_empty() && !w.contains("://"))
        .collect::<Vec<_>>()
        .join(" ");
    keys_in_text(&prose, prefixes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn retry_after_is_bounded_for_jira_too() {
        let r = Response::new(429, "").with_header("Retry-After", "4000000000000000000");
        assert_eq!(
            check(&r, Call::Other),
            Err(TrackerError::RateLimited {
                retry_after_secs: Some(MAX_RETRY_AFTER_SECS)
            })
        );
        let r = Response::new(503, "").with_header("Retry-After", "7200");
        assert_eq!(
            check(&r, Call::View),
            Err(TrackerError::RateLimited {
                retry_after_secs: Some(MAX_RETRY_AFTER_SECS)
            })
        );
    }

    #[test]
    fn a_key_inside_a_url_is_not_prose() {
        let p = vec!["ABC".to_string()];
        assert_eq!(
            keys_in_prose(
                "ABC-1 then https://partner.atlassian.net/browse/ABC-9 and (https://x.example/b?selectedIssue=ABC-8) abc-2",
                &p
            ),
            vec!["ABC-1", "ABC-2"]
        );
    }

    /// A key glued to a URL by a bracket or a comma is still prose: only
    /// the URL-bearing word is dropped, not the key beside it.
    #[test]
    fn a_key_glued_to_a_url_by_a_bracket_or_comma_is_still_prose() {
        let p = vec!["ABC".to_string()];
        assert_eq!(
            keys_in_prose(
                "ABC-1(https://partner.atlassian.net/browse/ABC-9) then ABC-2,https://x.example/ABC-8, and (ABC-3)",
                &p
            ),
            vec!["ABC-1", "ABC-2", "ABC-3"]
        );
    }

    #[test]
    fn a_legacy_sprint_string_reads_like_an_object() {
        let v = json!([
            "com.atlassian.greenhopper.service.sprint.Sprint@1a2b[id=4,rapidViewId=1,state=CLOSED,name=PLAT Sprint 2,startDate=2026-08-01,endDate=<null>,sequence=4]",
            "com.atlassian.greenhopper.service.sprint.Sprint@3c4d[id=5,rapidViewId=1,state=ACTIVE,name=PLAT Sprint 3, the one,startDate=2026-09-15,endDate=<null>,sequence=5]"
        ]);
        assert_eq!(
            current_sprint(&v),
            (Some("PLAT Sprint 3, the one".into()), true)
        );
        let v = json!([{"state": "future", "name": "Next"}]);
        assert_eq!(current_sprint(&v), (Some("Next".into()), false));
        assert_eq!(current_sprint(&json!(["garbage"])), (None, false));
    }

    /// `adf_text` serves the WHOLE description, not the 2k excerpt: a body
    /// past [`DESCRIPTION_MAX_CHARS`] is not cut there, only at its own `cap`.
    #[test]
    fn adf_text_is_capped_at_its_own_ceiling_not_the_excerpts() {
        let long = json!({"type":"doc","content":[{"type":"paragraph","content":[
            {"type":"text","text": "x".repeat(DESCRIPTION_MAX_CHARS * 3)}]}]});
        let cap = DESCRIPTION_MAX_CHARS * 2;
        let text = adf_text(&long, cap).unwrap();
        assert_eq!(text.chars().count(), cap);
        // Under the cap: the whole thing, past DESCRIPTION_MAX_CHARS.
        let short_of_cap = json!({"type":"doc","content":[{"type":"paragraph","content":[
            {"type":"text","text": "x".repeat(DESCRIPTION_MAX_CHARS + 500)}]}]});
        let text = adf_text(&short_of_cap, cap).unwrap();
        assert_eq!(text.chars().count(), DESCRIPTION_MAX_CHARS + 500);
        assert_eq!(adf_text(&Value::Null, cap), None);
        assert_eq!(adf_text(&json!({"type":"doc","content":[]}), cap), None);
        assert_eq!(
            adf_text(&json!("plain string body"), cap).as_deref(),
            Some("plain string body")
        );
    }
}
