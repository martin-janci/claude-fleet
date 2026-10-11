//! The review as GitHub shows it, ported from claude-code-reviewer's
//! `formatter.ts`: the review body (verdict, PR overview, test coverage,
//! summary, a collapsed list of every finding, the findings no line took,
//! previous resolutions, notes) and each inline comment in Conventional
//! Comments style. The body opens with [`MARKER`], which is how the
//! reviewer knows its own reviews and comments (and never reacts to them).

use super::output::{Finding, PlacedReview};
use serde_json::Value;

/// The invisible first line of every review the reviewer posts.
pub const MARKER: &str = "<!-- fleet-agent:review -->";

/// The marker the original service wrote; its reviews count as the
/// reviewer's own too.
pub const LEGACY_MARKER: &str = "<!-- claude-code-review -->";

const ORDER: [&str; 5] = ["issue", "suggestion", "nitpick", "question", "praise"];

fn severity_emoji(s: &str) -> &'static str {
    match s {
        "issue" => "🚨",
        "suggestion" => "💡",
        "nitpick" => "✨",
        "question" => "❓",
        "praise" => "👏",
        _ => "",
    }
}

fn level_emoji(s: &str) -> &'static str {
    match s {
        "none" => "⚪",
        "low" => "🟢",
        "medium" => "🟡",
        "high" => "🟠",
        "critical" => "🔴",
        _ => "⚪",
    }
}

fn plural(s: &str) -> &'static str {
    match s {
        "issue" => "Issues",
        "suggestion" => "Suggestions",
        "nitpick" => "Nitpicks",
        "question" => "Questions",
        _ => "Praise",
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max - 3).collect();
    out.push_str("...");
    out
}

fn tags(f: &Finding) -> String {
    let mut out = String::new();
    if f.blocking {
        out.push_str(" 🚫 **blocking**");
    }
    if f.security_related == Some(true) {
        out.push_str(" 🔐");
    }
    if f.test_related == Some(true) {
        out.push_str(" 🧪");
    }
    if f.is_new == Some(true) {
        out.push_str(" 🆕");
    }
    out
}

/// PURE: an inline comment.
pub fn inline_comment(f: &Finding) -> String {
    let confidence = match f.confidence {
        Some(c) if c < 100 => format!(" *({c}% confidence)*"),
        _ => String::new(),
    };
    format!(
        "{} **{}{}:** {}{confidence}",
        severity_emoji(&f.severity),
        f.severity,
        tags(f),
        f.body
    )
}

fn str_of<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or_default()
}

/// PURE: the review body. `verdict` is the one being posted (after any
/// escalation), which may differ from the model's.
pub fn review_body(r: &PlacedReview, verdict: &str, head_sha: &str) -> String {
    let mut p: Vec<String> = vec![MARKER.into(), String::new()];
    let (emoji, text) = match verdict {
        "APPROVE" => ("✅", "Approved"),
        "REQUEST_CHANGES" => ("🔄", "Changes Requested"),
        "COMMENT" => ("💬", "Commented"),
        _ => ("❔", "Review Complete"),
    };
    p.push(format!("## {emoji} {text}"));
    p.push(String::new());
    if r.dropped > 0 {
        p.push(format!(
            "> {} finding(s) under the confidence threshold were left out.",
            r.dropped
        ));
        p.push(String::new());
    }
    if let Some(ps) = &r.pr_summary {
        let risk = str_of(ps, "riskLevel");
        p.push("### 📋 PR Overview".into());
        p.push(String::new());
        p.push(format!("> {}", str_of(ps, "tldr")));
        p.push(String::new());
        p.push("| 📊 Metric | Value |".into());
        p.push("|-----------|-------|".into());
        let n = |k: &str| ps.get(k).and_then(Value::as_f64).unwrap_or(0.0) as i64;
        p.push(format!("| 📁 Files Changed | {} |", n("filesChanged")));
        p.push(format!(
            "| 📝 Lines | +{} / -{} |",
            n("linesAdded"),
            n("linesRemoved")
        ));
        let areas: Vec<&str> = ps
            .get("areasAffected")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        p.push(format!("| 🎯 Areas | {} |", areas.join(", ")));
        p.push(format!(
            "| {} Risk Level | **{}** |",
            level_emoji(risk),
            risk.to_uppercase()
        ));
        if let Some(factors) = ps.get("riskFactors").and_then(Value::as_array) {
            if !factors.is_empty() {
                p.push(String::new());
                p.push("⚠️ **Risk Factors:**".into());
                for f in factors.iter().filter_map(Value::as_str) {
                    p.push(format!("- {f}"));
                }
            }
        }
        p.push(String::new());
    }
    if let Some(tc) = &r.test_coverage {
        let imp = str_of(tc, "importance");
        p.push("### 🧪 Test Coverage".into());
        p.push(String::new());
        if imp == "none" {
            p.push(format!(
                "{} **Importance: NONE** (exempt) — {}",
                level_emoji(imp),
                str_of(tc, "rationale")
            ));
        } else {
            let status = if tc.get("testsIncluded") == Some(&Value::Bool(true)) {
                "✅ tests included"
            } else {
                "⚠️ no tests in this PR"
            };
            p.push(format!(
                "{} **Importance: {}** — {status}",
                level_emoji(imp),
                imp.to_uppercase()
            ));
            p.push(String::new());
            p.push(format!("> {}", str_of(tc, "rationale")));
            if let Some(tests) = tc.get("suggestedTests").and_then(Value::as_array) {
                if !tests.is_empty() {
                    p.push(String::new());
                    p.push("**Suggested tests:**".into());
                    for t in tests.iter().filter_map(Value::as_str) {
                        p.push(format!("- [ ] {t}"));
                    }
                }
            }
        }
        p.push(String::new());
    }
    p.push("### 📝 Review Summary".into());
    p.push(r.summary.clone());
    p.push(String::new());

    let all: Vec<&Finding> = r.inline.iter().chain(&r.orphans).collect();
    let counts: Vec<String> = ORDER
        .iter()
        .filter_map(|sev| {
            let n = all.iter().filter(|f| f.severity == *sev).count();
            (n > 0).then(|| {
                format!(
                    "{} {n} {sev}{}",
                    severity_emoji(sev),
                    if n > 1 { "s" } else { "" }
                )
            })
        })
        .collect();
    if !counts.is_empty() {
        let blocking = all.iter().filter(|f| f.blocking).count();
        let suffix = if blocking > 0 {
            format!(" — ⚠️ {blocking} blocking")
        } else {
            String::new()
        };
        p.push("<details>".into());
        p.push(format!(
            "<summary>🔍 Findings ({}){suffix}</summary>",
            counts.join(", ")
        ));
        p.push(String::new());
        for sev in ORDER {
            let group: Vec<&&Finding> = all.iter().filter(|f| f.severity == sev).collect();
            if group.is_empty() {
                continue;
            }
            p.push(format!("### {} {}", severity_emoji(sev), plural(sev)));
            for f in group {
                let confidence = match f.confidence {
                    Some(c) if c < 100 => format!(" ({c}% confidence)"),
                    _ => String::new(),
                };
                p.push(format!(
                    "- `{}:{}` — {}{}{confidence}",
                    f.path,
                    f.line,
                    truncate(&f.body, 120),
                    tags(f)
                ));
            }
            p.push(String::new());
        }
        p.push("</details>".into());
        p.push(String::new());
    }
    if !r.orphans.is_empty() {
        p.push("### 📌 Additional Findings".into());
        p.push(String::new());
        p.push(
            "> The following findings couldn't be placed as inline comments (line out of range \
             or file removed):"
                .into(),
        );
        p.push(String::new());
        for f in &r.orphans {
            p.push(format!(
                "{} **{}{}:** `{}:{}`",
                severity_emoji(&f.severity),
                f.severity,
                tags(f),
                f.path,
                f.line
            ));
            p.push(String::new());
            p.push(f.body.clone());
            p.push(String::new());
        }
    }
    if !r.resolutions.is_empty() {
        p.push("### ✅ Previous Finding Resolutions".into());
        p.push(String::new());
        let count = |k: &str| r.resolutions.iter().filter(|x| x.resolution == k).count();
        let mut status = Vec::new();
        if count("resolved") > 0 {
            status.push(format!("✅ {} resolved", count("resolved")));
        }
        if count("wont_fix") > 0 {
            status.push(format!("⏭️ {} won't fix", count("wont_fix")));
        }
        if count("open") > 0 {
            status.push(format!("⏸️ {} still open", count("open")));
        }
        if !status.is_empty() {
            p.push(format!("> {}", status.join(" • ")));
            p.push(String::new());
        }
        for x in &r.resolutions {
            let icon = match x.resolution.as_str() {
                "resolved" => "✅",
                "wont_fix" => "⏭️",
                _ => "⏸️",
            };
            p.push(format!(
                "{icon} `{}:{}` — **{}**",
                x.path,
                x.line,
                x.resolution.replacen('_', " ", 1).to_uppercase()
            ));
            p.push(x.body.clone());
            p.push(String::new());
        }
    }
    if let Some(o) = &r.overall {
        p.push("### 💭 Additional Notes".into());
        p.push(String::new());
        p.push(o.clone());
        p.push(String::new());
    }
    p.push("---".into());
    p.push(format!(
        "🤖 *Reviewed by Claude Code (claude-fleet) at commit [`{}`](../../commit/{head_sha})*",
        head_sha.get(..7).unwrap_or(head_sha)
    ));
    p.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::agents::review::output::Resolution;

    fn f(sev: &str, blocking: bool, confidence: Option<u8>) -> Finding {
        Finding {
            severity: sev.into(),
            blocking,
            path: "src/a.rs".into(),
            line: 3,
            body: "Body text.".into(),
            confidence,
            security_related: Some(sev == "issue"),
            test_related: None,
            is_new: None,
        }
    }

    #[test]
    fn an_inline_comment_is_a_conventional_comment() {
        assert_eq!(
            inline_comment(&f("issue", true, Some(80))),
            "🚨 **issue 🚫 **blocking** 🔐:** Body text. *(80% confidence)*"
        );
        assert_eq!(
            inline_comment(&f("nitpick", false, Some(100))),
            "✨ **nitpick:** Body text."
        );
    }

    #[test]
    fn the_body_has_every_section_it_has_content_for() {
        let r = PlacedReview {
            verdict: "COMMENT".into(),
            summary: "One bug.".into(),
            inline: vec![f("issue", true, None)],
            orphans: vec![f("praise", false, None)],
            dropped: 2,
            pr_summary: Some(
                serde_json::json!({"tldr":"Adds b","filesChanged":1,"linesAdded":2,
                "linesRemoved":0,"areasAffected":["core"],"riskLevel":"high"}),
            ),
            test_coverage: Some(serde_json::json!({"importance":"high","rationale":"logic",
                "testsIncluded":false,"suggestedTests":["b returns 2"]})),
            resolutions: vec![Resolution {
                path: "src/a.rs".into(),
                line: 1,
                body: "Fixed.".into(),
                resolution: "wont_fix".into(),
            }],
            overall: Some("Nice.".into()),
        };
        let b = review_body(&r, "REQUEST_CHANGES", "0123456789abcdef");
        assert!(b.starts_with(MARKER));
        assert!(
            b.contains("## 🔄 Changes Requested"),
            "the posted verdict, not the model's"
        );
        assert!(b.contains("> 2 finding(s) under the confidence threshold"));
        assert!(b.contains("| 🟠 Risk Level | **HIGH** |"));
        assert!(b.contains("🟠 **Importance: HIGH** — ⚠️ no tests in this PR"));
        assert!(b.contains("- [ ] b returns 2"));
        assert!(
            b.contains("<summary>🔍 Findings (🚨 1 issue, 👏 1 praise) — ⚠️ 1 blocking</summary>")
        );
        assert!(b.contains("### 📌 Additional Findings"));
        assert!(b.contains("⏭️ `src/a.rs:1` — **WONT FIX**"));
        assert!(b.contains("### 💭 Additional Notes"));
        assert!(b.ends_with("at commit [`0123456`](../../commit/0123456789abcdef)*"));
        assert_eq!(truncate(&"x".repeat(130), 120).chars().count(), 120);
    }
}
