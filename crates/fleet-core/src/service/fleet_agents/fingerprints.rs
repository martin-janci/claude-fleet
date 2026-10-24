//! `run.fingerprint` (restating the support agent, *Migration* step 2): the
//! support agent's fingerprint of an error (`agents::support::fingerprint`),
//! computed from the answer, kept in a fleet store, and what fleet knows of
//! it added to the answer before the actions are planned
//! ([`definition::FINGERPRINT_FIELDS`]):
//!
//! * `fingerprint`, `seenCount`: the record of kind [`KIND`] under the
//!   fingerprint, counted on every run that names it;
//! * `knownBug`: the bug a `tracker_issue` filed for it ([`set_bug`]), while
//!   the fingerprint was seen inside `window_mins`: incident aggregation;
//! * `similarBug` / `relatedBug`: else an open bug of `bugs.project` whose
//!   summary scores at least [`REUSE_AT`] / [`FOLLOW_UP_AT`].
//!
//! Hashing and scoring are fleet's, never the model's.

use super::definition::{self, FingerprintSpec};
use super::template;
use crate::service::agents::support::fingerprint::{
    fingerprint, similarity, FOLLOW_UP_AT, REUSE_AT,
};
use crate::service::trackers::IssueThread;
use crate::store::{NewAgentRecord, Store};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

/// The record kind fingerprints are kept under.
pub const KIND: &str = "fingerprint";
/// Open bugs one search reads.
pub const CANDIDATES_MAX: usize = 5;

/// What the store keeps under a fingerprint.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Seen {
    pub class: String,
    pub count: i64,
    pub first_seen: i64,
    pub last_seen: i64,
    /// The bug filed for it, and when.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bug: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bug_at: Option<i64>,
}

/// The error a run's answer names.
#[derive(Debug, Clone, PartialEq)]
pub struct Named {
    pub fingerprint: String,
    pub class: String,
    pub message: String,
    pub component: String,
}

/// PURE: the fingerprint of the error `ctx` (inputs, subject, answer) names;
/// `None` when the class renders empty.
pub fn named(spec: &FingerprintSpec, ctx: &Value) -> Option<Named> {
    let r = |t: Option<&String>| {
        t.map(|t| template::render(t, ctx).trim().to_string())
            .unwrap_or_default()
    };
    let class = r(Some(&spec.class));
    let message = r(spec.message.as_ref());
    let fp = fingerprint(
        &class,
        &message,
        &r(spec.stack.as_ref()),
        &r(spec.app_frame_prefix.as_ref()),
    )?;
    Some(Named {
        fingerprint: fp,
        class,
        message,
        component: r(spec.component.as_ref()),
    })
}

/// PURE: the JQL of the open bugs an exception class may already have.
pub fn candidates_jql(project: &str, issue_type: &str, class: &str) -> Option<String> {
    crate::service::agents::support::writes::candidates_jql(project, issue_type, class)
}

/// PURE: the best open bug for `n`: `(similarBug, relatedBug)`, the first
/// scoring at least [`REUSE_AT`], else the first at least [`FOLLOW_UP_AT`].
pub fn best_candidate(n: &Named, found: &[IssueThread]) -> (Option<String>, Option<String>) {
    let mut scored: Vec<(u32, &str)> = found
        .iter()
        .map(|t| {
            (
                similarity(&t.summary, &n.class, &n.component, &n.message),
                t.key.as_str(),
            )
        })
        .collect();
    scored.sort_by_key(|s| std::cmp::Reverse(s.0));
    match scored.first() {
        Some((s, k)) if *s >= REUSE_AT => (Some(k.to_string()), None),
        Some((s, k)) if *s >= FOLLOW_UP_AT => (None, Some(k.to_string())),
        _ => (None, None),
    }
}

/// Count a sighting of `n` in `namespace` and answer the record as it was
/// before it, and as it is now.
pub fn see(
    store: &Store,
    namespace: &str,
    n: &Named,
    job_id: Option<i64>,
    now: i64,
) -> Result<(Option<Seen>, Seen), String> {
    let before: Option<Seen> = store
        .agent_record(namespace, KIND, &n.fingerprint, now)
        .map_err(|e| e.to_string())?
        .and_then(|r| serde_json::from_str(&r.value).ok());
    let mut seen = before.clone().unwrap_or(Seen {
        class: n.class.clone(),
        first_seen: now,
        ..Default::default()
    });
    seen.count += 1;
    seen.last_seen = now;
    store
        .put_agent_record(
            &NewAgentRecord {
                namespace: namespace.into(),
                kind: KIND.into(),
                key: n.fingerprint.clone(),
                value: serde_json::to_string(&seen).unwrap_or_default(),
                expires_at: None,
                written_by_job: job_id,
            },
            now,
        )
        .map_err(|e| e.to_string())?;
    Ok((before, seen))
}

/// The bug filed for fingerprint `fp` in `namespace`: what the next run
/// inside the window links to. A fingerprint never seen is left alone.
pub fn set_bug(
    store: &Store,
    namespace: &str,
    fp: &str,
    bug: &str,
    now: i64,
) -> Result<(), String> {
    let Some(row) = store
        .agent_record(namespace, KIND, fp, now)
        .map_err(|e| e.to_string())?
    else {
        return Ok(());
    };
    let mut seen: Seen = serde_json::from_str(&row.value).map_err(|e| e.to_string())?;
    seen.bug = Some(bug.to_string());
    seen.bug_at = Some(now);
    store
        .put_agent_record(
            &NewAgentRecord {
                namespace: namespace.into(),
                kind: KIND.into(),
                key: fp.into(),
                value: serde_json::to_string(&seen).unwrap_or_default(),
                expires_at: None,
                written_by_job: row.written_by_job,
            },
            now,
        )
        .map_err(|e| e.to_string())
}

/// PURE: the bug filed for a fingerprint that still counts: seen inside
/// `window_mins` before this run (0 is always).
pub fn known_bug(before: Option<&Seen>, window_mins: i64, now: i64) -> Option<String> {
    let b = before?;
    if window_mins > 0 && b.last_seen + window_mins * 60 < now {
        return None;
    }
    b.bug.clone()
}

/// PURE: the answer with the fingerprint's fields added.
pub fn with_fields(
    mut answer: Value,
    n: &Named,
    seen: &Seen,
    known: Option<String>,
    similar: Option<String>,
    related: Option<String>,
) -> Value {
    if let Some(m) = answer.as_object_mut() {
        let mut add = Map::new();
        add.insert("fingerprint".into(), json!(n.fingerprint));
        add.insert("seenCount".into(), json!(seen.count));
        for (k, v) in [
            ("knownBug", known),
            ("similarBug", similar),
            ("relatedBug", related),
        ] {
            if let Some(v) = v {
                add.insert(k.into(), json!(v));
            }
        }
        for (name, _) in definition::FINGERPRINT_FIELDS {
            if let Some(v) = add.remove(name) {
                m.insert(name.into(), v);
            }
        }
    }
    answer
}

/// PURE: whether `s` is a fingerprint (64 hex digits).
pub fn is_fingerprint(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> FingerprintSpec {
        FingerprintSpec {
            store: "memory".into(),
            class: "{{answer.exceptionClass}}".into(),
            message: Some("{{answer.errorMessage}}".into()),
            stack: None,
            component: None,
            app_frame_prefix: None,
            window_mins: 120,
            bugs: None,
        }
    }

    fn thread(key: &str, summary: &str) -> IssueThread {
        IssueThread {
            key: key.into(),
            summary: summary.into(),
            description: String::new(),
            assignee_id: None,
            status: "Open".into(),
            updated: None,
            comments: Vec::new(),
        }
    }

    #[test]
    fn the_answer_names_its_error_and_its_numbers_do_not_count() {
        let ctx = |msg: &str| json!({ "answer": { "exceptionClass": "KeyExpiredException", "errorMessage": msg } });
        let a = named(&spec(), &ctx("key 42 expired")).unwrap();
        let b = named(&spec(), &ctx("key 7 expired")).unwrap();
        assert_eq!(a.fingerprint, b.fingerprint);
        assert!(is_fingerprint(&a.fingerprint));
        assert!(
            named(&spec(), &json!({ "answer": {} })).is_none(),
            "no class"
        );
    }

    #[test]
    fn the_best_open_bug_is_the_same_one_or_a_related_one() {
        let n = Named {
            fingerprint: "x".into(),
            class: "KeyExpiredException".into(),
            message: "gateway key expired".into(),
            component: "payments".into(),
        };
        let same = [
            thread("BUG-1", "Unrelated"),
            thread(
                "BUG-2",
                "[payments] KeyExpiredException: gateway key expired",
            ),
        ];
        assert_eq!(best_candidate(&n, &same), (Some("BUG-2".into()), None));
        let related = [thread("BUG-3", "KeyExpiredException in checkout")];
        assert_eq!(best_candidate(&n, &related), (None, None), "50 is neither");
        let n2 = Named {
            message: "x".into(),
            component: String::new(),
            ..n.clone()
        };
        let follow = [thread("BUG-4", "keyexpiredexception x")];
        assert_eq!(
            best_candidate(&n2, &follow),
            (None, Some("BUG-4".into())),
            "70"
        );
        assert_eq!(best_candidate(&n, &[]), (None, None));
    }

    #[test]
    fn a_sighting_counts_and_a_filed_bug_counts_inside_its_window() {
        let s = Store::open_in_memory().unwrap();
        let n = Named {
            fingerprint: "f".repeat(64),
            class: "E".into(),
            message: String::new(),
            component: String::new(),
        };
        let (before, seen) = see(&s, "rule:1", &n, None, 1000).unwrap();
        assert!(before.is_none());
        assert_eq!((seen.count, seen.first_seen), (1, 1000));
        set_bug(&s, "rule:1", &n.fingerprint, "BUG-9", 1001).unwrap();
        let (before, seen) = see(&s, "rule:1", &n, None, 2000).unwrap();
        assert_eq!(seen.count, 2);
        assert_eq!(
            known_bug(before.as_ref(), 120, 2000).as_deref(),
            Some("BUG-9")
        );
        assert_eq!(
            known_bug(before.as_ref(), 1, 2000),
            None,
            "outside the window"
        );
        assert_eq!(
            known_bug(before.as_ref(), 0, 99_999).as_deref(),
            Some("BUG-9")
        );
        // Never seen: nothing to file the bug against.
        set_bug(&s, "rule:1", &"0".repeat(64), "BUG-1", 3000).unwrap();
        assert!(s
            .agent_record("rule:1", KIND, &"0".repeat(64), 3000)
            .unwrap()
            .is_none());
    }

    #[test]
    fn the_fields_join_the_answer() {
        let n = Named {
            fingerprint: "a".repeat(64),
            class: "E".into(),
            message: String::new(),
            component: String::new(),
        };
        let seen = Seen {
            count: 3,
            ..Default::default()
        };
        let a = with_fields(
            json!({ "summary": "s" }),
            &n,
            &seen,
            Some("BUG-1".into()),
            None,
            None,
        );
        assert_eq!(a["seenCount"], 3);
        assert_eq!(a["knownBug"], "BUG-1");
        assert!(a.get("similarBug").is_none());
        assert_eq!(a["summary"], "s");
    }
}
