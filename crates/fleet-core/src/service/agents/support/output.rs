//! The investigation the model answers, ported from support-agent's
//! claude-analyzer (`plugins/claude-analyzer`): its JSON fields, read in the
//! same three tiers as the reviewer's answer (the whole text, a ```` ```json
//! ```` fence, the object ending at the last `}`). An answer that is no
//! investigation fails its job; the original's last resort (the first 500
//! characters as a low-confidence root cause) is not ported.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const CONFIDENCES: [&str; 3] = ["high", "medium", "low"];
pub const SEVERITIES: [&str; 3] = ["critical", "major", "minor"];

/// A validated investigation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Investigation {
    pub summary: String,
    pub root_cause: String,
    pub confidence: String,
    pub severity: String,
    pub affected_component: String,
    pub affected_version: String,
    pub suggested_fix: String,
    pub diff: String,
    pub steps_to_reproduce: String,
    /// The question for the reporter; empty when the investigation has enough.
    pub needs_more_info: String,
    /// The bug's exception class, message and stack trace, when the
    /// investigation found one: the fingerprint's input (step 7). Empty
    /// otherwise.
    #[serde(default)]
    pub exception_class: String,
    #[serde(default)]
    pub error_message: String,
    #[serde(default)]
    pub stack_trace: String,
}

fn text(v: &Value, k: &str) -> String {
    v.get(k)
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
        .to_string()
}

/// PURE: `v` as an investigation: it needs a root cause, a known confidence
/// and a known severity; the rest may be empty.
pub fn validate(v: &Value) -> Option<Investigation> {
    let root_cause = text(v, "rootCause");
    let confidence = text(v, "confidence").to_lowercase();
    let severity = text(v, "severity").to_lowercase();
    if root_cause.is_empty()
        || !CONFIDENCES.contains(&confidence.as_str())
        || !SEVERITIES.contains(&severity.as_str())
    {
        return None;
    }
    Some(Investigation {
        summary: text(v, "summary"),
        root_cause,
        confidence,
        severity,
        affected_component: Some(text(v, "affectedComponent"))
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| "unknown".into()),
        affected_version: text(v, "affectedVersion"),
        suggested_fix: text(v, "suggestedFix"),
        diff: text(v, "diff"),
        steps_to_reproduce: text(v, "stepsToReproduce"),
        needs_more_info: text(v, "needsMoreInfo"),
        exception_class: text(v, "exceptionClass"),
        error_message: text(v, "errorMessage"),
        stack_trace: text(v, "stackTrace").chars().take(8000).collect(),
    })
}

/// PURE: the first JSON object in `answer` that `ok` takes, in the three
/// tiers.
pub fn json_object_in<T>(answer: &str, ok: impl Fn(&Value) -> Option<T>) -> Option<T> {
    let t = answer.trim();
    let try_parse = |s: &str| serde_json::from_str::<Value>(s).ok().and_then(|v| ok(&v));
    if let Some(r) = try_parse(t) {
        return Some(r);
    }
    if let Some(start) = t.find("```json") {
        let after = &t[start + 7..];
        if let Some(nl) = after.find('\n') {
            let body = &after[nl + 1..];
            if let Some(end) = body.find("```") {
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

/// PURE: the investigation in the model's answer.
pub fn parse_investigation(answer: &str) -> Option<Investigation> {
    json_object_in(answer, validate)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"{"summary":"NPE in split payment","rootCause":"Order not initialised","confidence":"High","severity":"major","affectedComponent":"payments-service","suggestedFix":"init the order","diff":"","needsMoreInfo":""}"#;

    #[test]
    fn three_tiers_and_the_fields_it_needs() {
        let i = parse_investigation(GOOD).unwrap();
        assert_eq!(
            (i.confidence.as_str(), i.severity.as_str()),
            ("high", "major")
        );
        assert_eq!(i.affected_component, "payments-service");
        assert_eq!(
            parse_investigation(&format!("Looked at logs.\n```json\n{GOOD}\n```"))
                .unwrap()
                .summary,
            "NPE in split payment"
        );
        assert!(parse_investigation(&format!("Found {{it}}: {GOOD}")).is_some());
        for bad in [
            r#"{"rootCause":"","confidence":"high","severity":"major"}"#,
            r#"{"rootCause":"x","confidence":"sure","severity":"major"}"#,
            r#"{"rootCause":"x","confidence":"low","severity":"blocker"}"#,
            "No idea.",
        ] {
            assert!(parse_investigation(bad).is_none(), "{bad}");
        }
        let unknown =
            parse_investigation(r#"{"rootCause":"x","confidence":"low","severity":"minor"}"#)
                .unwrap();
        assert_eq!(unknown.affected_component, "unknown");
        let v = serde_json::to_value(&unknown).unwrap();
        assert!(
            v.get("rootCause").is_some() && v.get("needsMoreInfo").is_some(),
            "camelCase as the original"
        );
    }
}
