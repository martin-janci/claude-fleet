//! What Jira Cloud (M3) and Jira Data Center (M6.5) share: the error table
//! (CAPTCHA included), status category and resolution, the sprint field,
//! ADF → text, and key recognition. Everything else — the API version,
//! paging, bulk fetch, the epic — is each adapter's own.

use super::{CallKind as Call, TrackerError, DESCRIPTION_MAX_CHARS, MAX_RETRY_AFTER_SECS};
use crate::net::https::Response;
use serde_json::Value;

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
/// [`DESCRIPTION_MAX_CHARS`] characters. A plain string (API v2, or a
/// renderer) is taken as is.
pub fn adf_excerpt(v: &Value) -> Option<String> {
    let mut out = String::new();
    match v {
        Value::String(s) => out.push_str(s),
        Value::Object(_) => adf_walk(v, &mut out),
        _ => return None,
    }
    let text = out
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();
    if text.is_empty() {
        return None;
    }
    Some(text.chars().take(DESCRIPTION_MAX_CHARS).collect())
}

fn adf_walk(node: &Value, out: &mut String) {
    if out.chars().count() > DESCRIPTION_MAX_CHARS {
        return;
    }
    let attrs = &node["attrs"];
    match node["type"].as_str().unwrap_or_default() {
        "text" => out.push_str(node["text"].as_str().unwrap_or_default()),
        "hardBreak" => out.push('\n'),
        "mention" => out.push_str(attrs["text"].as_str().unwrap_or("@someone")),
        "emoji" => out.push_str(attrs["shortName"].as_str().unwrap_or_default()),
        "inlineCard" | "blockCard" => out.push_str(attrs["url"].as_str().unwrap_or_default()),
        "status" => out.push_str(attrs["text"].as_str().unwrap_or_default()),
        "listItem" => out.push_str("- "),
        _ => {}
    }
    if let Some(children) = node["content"].as_array() {
        for c in children {
            adf_walk(c, out);
        }
    }
    if matches!(
        node["type"].as_str(),
        Some("paragraph" | "heading" | "codeBlock" | "blockquote" | "rule" | "listItem")
    ) && !out.ends_with('\n')
    {
        out.push('\n');
    }
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

/// `ABC-123`: a letter, then letters/digits/underscore, a dash, digits.
pub(crate) fn is_key(s: &str) -> bool {
    let Some((p, n)) = s.split_once('-') else {
        return false;
    };
    p.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && (2..=10).contains(&p.len())
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
    let prose: String = text
        .split_whitespace()
        .filter(|w| !w.contains("://"))
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
}
