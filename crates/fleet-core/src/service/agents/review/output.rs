//! The review the model answers, ported from claude-code-reviewer's
//! `parseStructuredReview` / `validateStructuredReview` (`claude.ts`) and
//! the placement in `reviewer.ts`, with their tests (`claude.test.ts`).
//!
//! * [`parse_review`] reads the answer in three tiers: the whole text, a
//!   ```` ```json ```` fence, then each `{` from the right to the last `}`.
//!   The first that validates wins. The original's freeform fallback (an
//!   issue comment with a regex verdict) is not ported: an answer that does
//!   not validate fails its job.
//! * Validation keeps what is well formed and drops the rest: a finding
//!   without a known severity, a path, a line ≥ 1 or a body is dropped, not
//!   fatal; the review itself needs a verdict, a summary and a findings
//!   array.
//! * [`place`] puts each finding on a commentable line of the diff (snapped
//!   within three lines) or in the review's body (`orphans`); praise always
//!   goes to the body, and a finding under the rule's confidence line is
//!   dropped and counted.

use super::diff::{snap_line, Commentable};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const VERDICTS: [&str; 3] = ["APPROVE", "REQUEST_CHANGES", "COMMENT"];
pub const SEVERITIES: [&str; 5] = ["issue", "suggestion", "nitpick", "question", "praise"];
const RISK_LEVELS: [&str; 4] = ["low", "medium", "high", "critical"];
const TEST_IMPORTANCE: [&str; 5] = ["none", "low", "medium", "high", "critical"];
const RESOLUTIONS: [&str; 3] = ["resolved", "wont_fix", "open"];

/// One finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub severity: String,
    pub blocking: bool,
    pub path: String,
    pub line: u32,
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub security_related: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_related: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_new: Option<bool>,
}

/// What a re-review says about one previous finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resolution {
    pub path: String,
    pub line: u32,
    pub body: String,
    /// resolved | wont_fix | open.
    pub resolution: String,
}

/// A validated review.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Review {
    pub verdict: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_summary: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_coverage: Option<Value>,
    pub findings: Vec<Finding>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resolutions: Vec<Resolution>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overall: Option<String>,
    /// The pull request's description, when the prompt asked for one
    /// (auto-description, step 8).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn line_of(v: &Value) -> Option<u32> {
    let n = v.get("line")?.as_f64()?;
    (n >= 1.0 && n.fract() == 0.0 && n <= f64::from(u32::MAX)).then_some(n as u32)
}

fn strings(v: Option<&Value>) -> Option<Vec<Value>> {
    v.and_then(Value::as_array).map(|a| {
        a.iter()
            .filter(|x| x.as_str().is_some_and(|s| !s.trim().is_empty()))
            .cloned()
            .collect()
    })
}

fn pr_summary(v: &Value) -> Option<Value> {
    let ps = v.get("prSummary")?;
    let num = |k: &str| ps.get(k).and_then(Value::as_f64);
    let ok = text(ps, "tldr").is_some()
        && num("filesChanged").is_some()
        && num("linesAdded").is_some()
        && num("linesRemoved").is_some()
        && ps.get("areasAffected").is_some_and(Value::is_array)
        && ps
            .get("riskLevel")
            .and_then(Value::as_str)
            .is_some_and(|r| RISK_LEVELS.contains(&r));
    if !ok {
        return None;
    }
    let mut out = serde_json::json!({
        "tldr": text(ps, "tldr"),
        "filesChanged": num("filesChanged"),
        "linesAdded": num("linesAdded"),
        "linesRemoved": num("linesRemoved"),
        "areasAffected": strings(ps.get("areasAffected")),
        "riskLevel": ps.get("riskLevel"),
    });
    if let Some(f) = strings(ps.get("riskFactors")) {
        out["riskFactors"] = Value::Array(f);
    }
    Some(out)
}

fn test_coverage(v: &Value) -> Option<Value> {
    let tc = v.get("testCoverage")?;
    let importance = tc
        .get("importance")
        .and_then(Value::as_str)
        .filter(|i| TEST_IMPORTANCE.contains(i))?;
    let rationale = text(tc, "rationale")?;
    let included = tc.get("testsIncluded")?.as_bool()?;
    let mut out = serde_json::json!({
        "importance": importance,
        "rationale": rationale,
        "testsIncluded": included,
    });
    if let Some(t) = strings(tc.get("suggestedTests")) {
        out["suggestedTests"] = Value::Array(t);
    }
    Some(out)
}

fn finding(f: &Value) -> Option<Finding> {
    let severity = f
        .get("severity")
        .and_then(Value::as_str)
        .filter(|s| SEVERITIES.contains(s))?;
    let path = f
        .get("path")
        .and_then(Value::as_str)
        .filter(|p| !p.is_empty())?;
    let body = f
        .get("body")
        .and_then(Value::as_str)
        .filter(|b| !b.is_empty())?;
    Some(Finding {
        severity: severity.to_string(),
        blocking: f.get("blocking") == Some(&Value::Bool(true)),
        path: path.to_string(),
        line: line_of(f)?,
        body: body.to_string(),
        confidence: f
            .get("confidence")
            .and_then(Value::as_f64)
            .filter(|c| (0.0..=100.0).contains(c))
            .map(|c| c.round() as u8),
        security_related: f.get("securityRelated").and_then(Value::as_bool),
        test_related: f.get("testRelated").and_then(Value::as_bool),
        is_new: f.get("isNew").and_then(Value::as_bool),
    })
}

fn resolution(r: &Value) -> Option<Resolution> {
    let res = r
        .get("resolution")
        .and_then(Value::as_str)
        .filter(|x| RESOLUTIONS.contains(x))?;
    Some(Resolution {
        path: r
            .get("path")
            .and_then(Value::as_str)
            .filter(|p| !p.is_empty())?
            .to_string(),
        line: line_of(r)?,
        body: r
            .get("body")
            .and_then(Value::as_str)
            .filter(|b| !b.is_empty())?
            .to_string(),
        resolution: res.to_string(),
    })
}

/// PURE: `v` as a review, or `None` when it is not one.
pub fn validate(v: &Value) -> Option<Review> {
    let verdict = v
        .get("verdict")
        .and_then(Value::as_str)
        .filter(|x| VERDICTS.contains(x))?;
    let summary = text(v, "summary")?;
    let findings = v.get("findings")?.as_array()?;
    Some(Review {
        verdict: verdict.to_string(),
        summary,
        pr_summary: pr_summary(v),
        test_coverage: test_coverage(v),
        findings: findings.iter().filter_map(finding).collect(),
        resolutions: v
            .get("resolutions")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(resolution).collect())
            .unwrap_or_default(),
        overall: text(v, "overall"),
        description: text(v, "description"),
    })
}

/// PURE: the review in the model's answer: the whole text, else a
/// ```` ```json ```` fence, else the object ending at the last `}` that
/// starts furthest right and validates.
pub fn parse_review(answer: &str) -> Option<Review> {
    let t = answer.trim();
    let try_parse = |s: &str| {
        serde_json::from_str::<Value>(s)
            .ok()
            .and_then(|v| validate(&v))
    };
    if let Some(r) = try_parse(t) {
        return Some(r);
    }
    if let Some(start) = t.find("```json") {
        let after = &t[start + 7..];
        if let Some(nl) = after.find('\n') {
            let body = &after[nl + 1..];
            if let Some(end) = body.find("\n```").or_else(|| body.find("```")) {
                if let Some(r) = try_parse(body[..end].trim()) {
                    return Some(r);
                }
            }
        }
    }
    let last = t.rfind('}')?;
    let mut search = last;
    loop {
        let open = t[..=search].rfind('{')?;
        if let Some(r) = try_parse(&t[open..=last]) {
            return Some(r);
        }
        if open == 0 {
            return None;
        }
        search = open - 1;
    }
}

/// A review placed on the diff.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlacedReview {
    pub verdict: String,
    pub summary: String,
    /// Findings on a commentable line (their `line` snapped).
    pub inline: Vec<Finding>,
    /// Findings for the review's body: praise, and those no line took.
    pub orphans: Vec<Finding>,
    /// Findings under the rule's confidence line, left out.
    pub dropped: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_summary: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_coverage: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resolutions: Vec<Resolution>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overall: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// PURE: place `review`'s findings on `commentable`. A finding below
/// `confidence_threshold` (when it is above 0) is dropped; a finding with
/// no confidence counts as 100.
pub fn place(review: Review, commentable: &Commentable, confidence_threshold: u8) -> PlacedReview {
    let (mut inline, mut orphans, mut dropped) = (Vec::new(), Vec::new(), 0);
    for mut f in review.findings {
        if confidence_threshold > 0 && f.confidence.unwrap_or(100) < confidence_threshold {
            dropped += 1;
            continue;
        }
        if f.severity == "praise" {
            orphans.push(f);
            continue;
        }
        match snap_line(commentable, &f.path, f.line) {
            Some(line) => {
                f.line = line;
                inline.push(f);
            }
            None => orphans.push(f),
        }
    }
    PlacedReview {
        verdict: review.verdict,
        summary: review.summary,
        inline,
        orphans,
        dropped,
        pr_summary: review.pr_summary,
        test_coverage: review.test_coverage,
        resolutions: review.resolutions,
        overall: review.overall,
        description: review.description,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::agents::review::diff::commentable_lines;

    const VALID: &str = r#"{"verdict":"REQUEST_CHANGES","summary":"Found a bug.","findings":[{"severity":"issue","blocking":true,"path":"src/a.ts","line":11,"body":"Null deref.","confidence":90}]}"#;

    #[test]
    fn three_tiers_and_the_first_valid_wins() {
        assert_eq!(parse_review(VALID).unwrap().verdict, "REQUEST_CHANGES");
        let fenced = format!("Here you go:\n```json\n{VALID}\n```\nThanks");
        assert_eq!(parse_review(&fenced).unwrap().findings.len(), 1);
        let trailing = format!("I looked at {{the code}} carefully.\n{VALID}");
        assert_eq!(parse_review(&trailing).unwrap().summary, "Found a bug.");
        // A nested object is not taken for the review.
        let nested =
            r#"thinking {"a":{"b":1}} then {"verdict":"APPROVE","summary":"ok","findings":[]}"#;
        assert_eq!(parse_review(nested).unwrap().verdict, "APPROVE");
        assert!(parse_review("LGTM, approved!").is_none());
        assert!(parse_review("").is_none());
    }

    #[test]
    fn a_review_needs_a_verdict_a_summary_and_findings() {
        for bad in [
            r#"{"verdict":"LGTM","summary":"x","findings":[]}"#,
            r#"{"verdict":"APPROVE","summary":"  ","findings":[]}"#,
            r#"{"verdict":"APPROVE","summary":"x"}"#,
            r#"{"verdict":"APPROVE","summary":"x","findings":{}}"#,
            r#"[1,2]"#,
        ] {
            assert!(parse_review(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn a_malformed_finding_is_dropped_not_fatal() {
        let r = parse_review(
            r#"{"verdict":"APPROVE","summary":"ok","findings":[
              {"severity":"issue","path":"a","line":1,"body":"ok"},
              {"severity":"blocker","path":"a","line":1,"body":"bad severity"},
              {"severity":"issue","path":"","line":1,"body":"no path"},
              {"severity":"issue","path":"a","line":0,"body":"line 0"},
              {"severity":"issue","path":"a","line":1.5,"body":"half a line"},
              {"severity":"issue","path":"a","line":2},
              {"severity":"nitpick","path":"a","line":3,"body":"b","blocking":"yes","confidence":150}
            ]}"#,
        )
        .unwrap();
        assert_eq!(r.findings.len(), 2);
        let n = &r.findings[1];
        assert!(!n.blocking, "only a literal true blocks");
        assert_eq!(n.confidence, None, "out of range");
    }

    #[test]
    fn optional_parts_survive_only_when_well_formed() {
        let r = parse_review(
            r#"{"verdict":"APPROVE","summary":"ok","findings":[],
                "prSummary":{"tldr":"t","filesChanged":2,"linesAdded":3,"linesRemoved":1,
                             "areasAffected":["api",7],"riskLevel":"low"},
                "testCoverage":{"importance":"high","rationale":"r","testsIncluded":false,
                                "suggestedTests":["a",""]},
                "resolutions":[{"path":"a","line":2,"body":"fixed","resolution":"resolved"},
                               {"path":"a","line":2,"body":"x","resolution":"maybe"}],
                "overall":"  "}"#,
        )
        .unwrap();
        assert_eq!(
            r.pr_summary.unwrap()["areasAffected"],
            serde_json::json!(["api"])
        );
        assert_eq!(
            r.test_coverage.unwrap()["suggestedTests"],
            serde_json::json!(["a"])
        );
        assert_eq!(r.resolutions.len(), 1);
        assert_eq!(r.overall, None);
        let r = parse_review(
            r#"{"verdict":"APPROVE","summary":"ok","findings":[],
                "prSummary":{"tldr":"t","riskLevel":"extreme"},
                "testCoverage":{"importance":"high"}}"#,
        )
        .unwrap();
        assert!(r.pr_summary.is_none() && r.test_coverage.is_none());
    }

    #[test]
    fn placement_snaps_or_moves_to_the_body() {
        let diff = "diff --git a/src/a.ts b/src/a.ts\n--- a/src/a.ts\n+++ b/src/a.ts\n@@ -10,2 +10,3 @@\n ctx\n+new\n+new2\n";
        let c = commentable_lines(diff);
        let f = |sev: &str, line: u32, confidence: Option<u8>| Finding {
            severity: sev.into(),
            blocking: false,
            path: "src/a.ts".into(),
            line,
            body: "b".into(),
            confidence,
            security_related: None,
            test_related: None,
            is_new: None,
        };
        let review = Review {
            verdict: "APPROVE".into(),
            summary: "s".into(),
            pr_summary: None,
            test_coverage: None,
            findings: vec![
                f("issue", 11, None),
                f("suggestion", 14, Some(90)),
                f("nitpick", 40, Some(95)),
                f("praise", 11, None),
                f("question", 11, Some(20)),
            ],
            resolutions: vec![],
            overall: None,
            description: None,
        };
        let p = place(review.clone(), &c, 50);
        assert_eq!(
            p.inline.iter().map(|x| x.line).collect::<Vec<_>>(),
            vec![11, 12]
        );
        assert_eq!(p.orphans.len(), 2, "the far nitpick and the praise");
        assert_eq!(p.dropped, 1);
        assert_eq!(place(review, &c, 0).dropped, 0, "0 keeps everything");
    }
}
