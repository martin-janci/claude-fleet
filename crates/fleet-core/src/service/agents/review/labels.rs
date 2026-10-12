//! Auto-label (step 8), ported from claude-code-reviewer's
//! `features/auto-label.ts`: the labels a review puts on its pull request,
//! from its verdict, the severities of its findings and the paths its diff
//! touches. Only labels the rule names are ever removed; a label someone
//! else put there is never touched.

use super::diff::glob_match;
use super::output::{Finding, SEVERITIES, VERDICTS};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The action that puts a review's labels on its pull request.
pub const PR_LABELS: &str = "pr_labels";
/// The action that writes a pull request's description (auto-description).
pub const PR_DESCRIPTION: &str = "pr_description";

/// A `pr_labels` action's payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabelsRequest {
    pub host: String,
    pub repo: String,
    pub number: i64,
    /// The head the review was of; a moved head expires it.
    pub sha: String,
    pub add: Vec<String>,
    pub remove: Vec<String>,
}

/// A `pr_description` action's payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DescriptionRequest {
    pub host: String,
    pub repo: String,
    pub number: i64,
    pub sha: String,
    pub body: String,
}

/// What the reviewer reads of a pull request for labels and description.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrMeta {
    pub body: String,
    pub labels: Vec<String>,
}

/// PURE: `gh api …/pulls/N --jq '{body, labels: [.labels[].name]}'`'s answer.
pub fn parse_meta(stdout: &str) -> Result<PrMeta, String> {
    let v: serde_json::Value =
        serde_json::from_str(stdout.trim()).map_err(|e| format!("pull request: {e}"))?;
    Ok(PrMeta {
        body: v
            .get("body")
            .and_then(|b| b.as_str())
            .unwrap_or_default()
            .to_string(),
        labels: v
            .get("labels")
            .and_then(|l| l.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|l| l.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// A label for pull requests whose diff touches `pattern` (a glob).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathLabel {
    pub pattern: String,
    pub label: String,
}

/// A rule's labels.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LabelRules {
    /// Verdict (`APPROVE`, `REQUEST_CHANGES`, `COMMENT`) → labels.
    pub verdict: BTreeMap<String, Vec<String>>,
    /// Finding severity (`issue`, `suggestion`, …) → labels.
    pub severity: BTreeMap<String, Vec<String>>,
    pub paths: Vec<PathLabel>,
}

fn label_ok(l: &str) -> bool {
    !l.trim().is_empty() && l.chars().count() <= 50 && !l.contains([',', '\n'])
}

impl LabelRules {
    pub fn is_empty(&self) -> bool {
        self.verdict.is_empty() && self.severity.is_empty() && self.paths.is_empty()
    }

    /// PURE: the rules checked.
    pub fn check(&self) -> Result<(), String> {
        if let Some(k) = self
            .verdict
            .keys()
            .find(|k| !VERDICTS.contains(&k.as_str()))
        {
            return Err(format!("labels.verdict: {k} is not a verdict"));
        }
        if let Some(k) = self
            .severity
            .keys()
            .find(|k| !SEVERITIES.contains(&k.as_str()))
        {
            return Err(format!("labels.severity: {k} is not a severity"));
        }
        if let Some(l) = self.managed().into_iter().find(|l| !label_ok(l)) {
            return Err(format!("label {l:?}: at most 50 characters, no comma"));
        }
        if self.paths.iter().any(|p| p.pattern.trim().is_empty()) {
            return Err("labels.paths: an empty pattern".into());
        }
        Ok(())
    }

    /// Every label the rules name: the only ones ever removed.
    pub fn managed(&self) -> BTreeSet<String> {
        self.verdict
            .values()
            .chain(self.severity.values())
            .flatten()
            .cloned()
            .chain(self.paths.iter().map(|p| p.label.clone()))
            .collect()
    }
}

/// The labels to add and to remove.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabelChange {
    pub add: Vec<String>,
    pub remove: Vec<String>,
}

impl LabelChange {
    pub fn is_empty(&self) -> bool {
        self.add.is_empty() && self.remove.is_empty()
    }
}

/// PURE: what the labels should become, against what the pull request has.
pub fn decide(
    rules: &LabelRules,
    verdict: &str,
    findings: &[Finding],
    paths: &[String],
    current: &[String],
) -> LabelChange {
    let mut want: BTreeSet<String> = BTreeSet::new();
    want.extend(rules.verdict.get(verdict).into_iter().flatten().cloned());
    for f in findings {
        want.extend(
            rules
                .severity
                .get(&f.severity)
                .into_iter()
                .flatten()
                .cloned(),
        );
    }
    for p in &rules.paths {
        if paths.iter().any(|path| glob_match(&p.pattern, path)) {
            want.insert(p.label.clone());
        }
    }
    let managed = rules.managed();
    LabelChange {
        add: want
            .iter()
            .filter(|l| !current.contains(l))
            .cloned()
            .collect(),
        remove: current
            .iter()
            .filter(|l| managed.contains(*l) && !want.contains(*l))
            .cloned()
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> LabelRules {
        serde_json::from_str(
            r#"{"verdict":{"APPROVE":["lgtm"],"REQUEST_CHANGES":["needs-work"]},
                "severity":{"issue":["has-issues"]},
                "paths":[{"pattern":"**/*.sql","label":"migration"}]}"#,
        )
        .unwrap()
    }

    fn finding(severity: &str) -> Finding {
        Finding {
            severity: severity.into(),
            blocking: false,
            path: "a.rs".into(),
            line: 1,
            body: "b".into(),
            confidence: None,
            security_related: None,
            test_related: None,
            is_new: None,
        }
    }

    #[test]
    fn labels_follow_the_verdict_severities_and_paths() {
        let c = decide(
            &rules(),
            "REQUEST_CHANGES",
            &[finding("issue"), finding("nitpick")],
            &["db/migrations/1.sql".into()],
            &["lgtm".into(), "bug".into()],
        );
        assert_eq!(c.add, ["has-issues", "migration", "needs-work"]);
        assert_eq!(c.remove, ["lgtm"], "only a managed label goes; `bug` stays");
        let none = decide(&rules(), "APPROVE", &[], &[], &["lgtm".into()]);
        assert!(none.is_empty());
    }

    #[test]
    fn a_pull_requests_body_and_labels_read_back() {
        let m = parse_meta(r#"{"body":null,"labels":["a","b"]}"#).unwrap();
        assert_eq!(
            m,
            PrMeta {
                body: String::new(),
                labels: vec!["a".into(), "b".into()]
            }
        );
        assert!(parse_meta("nope").is_err());
    }

    #[test]
    fn rules_are_checked() {
        assert!(rules().check().is_ok());
        for bad in [
            r#"{"verdict":{"LGTM":["x"]}}"#,
            r#"{"severity":{"blocker":["x"]}}"#,
            r#"{"verdict":{"APPROVE":["a,b"]}}"#,
            r#"{"paths":[{"pattern":"","label":"x"}]}"#,
        ] {
            let r: LabelRules = serde_json::from_str(bad).unwrap();
            assert!(r.check().is_err(), "{bad}");
        }
        assert!(serde_json::from_str::<LabelRules>(r#"{"nope":1}"#).is_err());
    }
}
