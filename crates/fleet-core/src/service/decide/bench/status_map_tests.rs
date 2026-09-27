//! The `status_map` benchmark's tests: the synthetic fixture is well formed,
//! the rule baseline's numbers on it are pinned, Jev is asked only through
//! the gate (with the adapter's own request, floor and record), the card's
//! acceptance is judged (or not) as registered, and no report carries a
//! section name.

use super::status_map::*;
use super::Verdict;
use crate::service::decide::status_map as sm;
use crate::service::decide::{
    BackendError, DecideCtx, DecisionBackend, JevRequest, JevResponse, Question,
};
use crate::service::settings;
use crate::store::{DecisionRunFilter, Secret, Store};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn fixture() -> (Vec<SectionLabel>, Vec<SectionCase>) {
    let labels = parse_labels(FIXTURE).unwrap();
    let (cases, dropped) = cases(&labels);
    assert_eq!(dropped, 0);
    (labels, cases)
}

async fn offline(cases: &[SectionCase]) -> Outcomes {
    run_providers(
        cases,
        &[Provider::None, Provider::Todo, Provider::Rule],
        None,
        DEFAULT_MAX_CALLS,
    )
    .await
}

fn metric<'a>(r: &'a Report, p: &str) -> &'a ProviderMetrics {
    r.metrics.iter().find(|m| m.provider == p).unwrap()
}

#[test]
fn the_fixture_is_well_formed() {
    let (labels, cases) = fixture();
    assert!(cases.len() >= 300, "{}", cases.len());
    for (l, c) in labels.iter().zip(&cases) {
        assert!(
            ["en", "sk", "cs", "de", "mixed"].contains(&c.lang.as_str()),
            "{l:?}"
        );
        // Every section is on its own board, which is at most the probe's
        // 30 names sent.
        assert!(c.board.contains(&c.key), "{l:?}");
        assert!(c.board.len() <= sm::MAX_PROJECT_SECTIONS);
    }
    for lang in ["en", "sk", "cs", "de", "mixed"] {
        assert!(
            cases.iter().filter(|c| c.lang == lang).count() >= 25,
            "{lang}"
        );
    }
    for cat in CATEGORIES {
        assert!(cases.iter().any(|c| c.expect == cat), "{cat}");
    }
    let ambiguous = cases.iter().filter(|c| c.ambiguous).count();
    assert!((30..=100).contains(&ambiguous), "{ambiguous}");
    // Emoji prefixes, diacritics and their absence are all in it.
    assert!(cases.iter().any(|c| c.key.starts_with('🚀')));
    assert!(cases.iter().any(|c| c.key == "rozpracované"));
    assert!(cases.iter().any(|c| c.key == "rozpracovane"));
    assert!(cases.iter().any(|c| c.key == "v řešení"));
    assert!(cases.iter().any(|c| c.key == "erledigt"));
}

#[test]
fn a_label_file_is_checked_row_by_row() {
    let ok = "// a comment\n\n{\"section\":\"Ideas\",\"project_sections\":[\"Ideas\",\"Done\"],\"expect\":\"todo\",\"lang\":\"en\",\"org_id\":3}\n";
    let rows = parse_labels(ok).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].org_id, Some(3));
    let e = parse_labels("{\"section\":\"x\",\"expect\":\"later\"}").unwrap_err();
    assert!(e.contains("line 1") && e.contains("expect"), "{e}");
    let e = parse_labels("\n{\"section\":\"  \",\"expect\":\"todo\"}").unwrap_err();
    assert!(e.contains("line 2"), "{e}");
    assert!(parse_labels("{not json").is_err());
    // Names are normalised like the probe's; the board de-duplicated.
    let (cs, dropped) = cases(&[
        SectionLabel {
            section: "  In Progress ".into(),
            project_sections: vec!["To do".into(), "In Progress".into(), "to do".into()],
            expect: "in_progress".into(),
            lang: String::new(),
            note: Some("Ambiguous: a test".into()),
            org_id: None,
        },
        SectionLabel {
            section: "Untitled section".into(),
            project_sections: vec![],
            expect: "todo".into(),
            lang: "en".into(),
            note: None,
            org_id: None,
        },
    ]);
    assert_eq!(dropped, 1);
    assert_eq!(cs[0].key, "in progress");
    assert_eq!(cs[0].board, vec!["to do", "in progress"]);
    assert_eq!(cs[0].lang, "unknown");
    assert!(cs[0].ambiguous);
    assert_eq!(cs[0].rule, Some("in_progress"));
}

/// The keyword rule's numbers on the fixture. A change here is a change to
/// the rule or the fixture: look at it, then update the numbers.
#[tokio::test]
async fn the_rule_baseline_on_the_fixture_is_pinned() {
    let (labels, cases) = fixture();
    let outs = offline(&cases).await;
    let r = report(&cases, &outs, labels.len(), 0, true);
    assert_eq!(r.sizes.cases, 391);
    assert_eq!(r.sizes.rule_abstained, 317);
    assert_eq!(r.sizes.ambiguous, 60);
    let rule = metric(&r, "rule");
    assert_eq!(
        (rule.cases, rule.all.answered, rule.all.correct),
        (391, 74, 66)
    );
    assert_eq!(rule.all.accuracy_on_answered, Some(0.892));
    assert_eq!(rule.all.coverage, Some(0.189));
    assert_eq!(rule.rule_abstained.coverage, Some(0.0));
    // Of its 31 `done` answers, "almost done" and "to be released" hide
    // live work; "abandoned" and "closed - won't fix" apply as done anyway.
    assert_eq!(rule.done_answers, 31);
    assert_eq!(rule.done_precision, Some(0.935));
    assert_eq!(rule.done_precision_strict, Some(0.871));
    assert_eq!(rule.confusion["todo"]["in_progress"], 3);
    assert_eq!(rule.confusion["done"]["done"], 27);
    assert_eq!(rule.confusion["done"][ABSTAIN], 53);
    let todo = metric(&r, "todo");
    assert_eq!(todo.all.coverage, Some(1.0));
    assert_eq!(todo.all.accuracy_on_answered, Some(0.325));
    assert_eq!(todo.rule_abstained.accuracy_on_answered, Some(0.391));
    assert_eq!(todo.done_precision, None);
    let none = metric(&r, "none");
    assert_eq!(none.all.coverage, Some(0.0));
    // The rule's acceptance: done precision fails, and it never answers
    // where it abstains.
    let acc = r.acceptance.iter().find(|a| a.provider == "rule").unwrap();
    assert_eq!(acc.criteria[0].verdict, Verdict::Fail);
    assert_eq!(acc.criteria[2].verdict, Verdict::Fail);
    assert_eq!(acc.criteria[3].verdict, Verdict::NotJudged);
    assert_eq!(acc.overall, Verdict::Fail);
    assert!(r.acceptance.iter().all(|a| a.provider != "none"));
    // rule vs todo is compared over all cases, not where the rule abstains.
    assert!(r
        .diffs
        .iter()
        .any(|d| d.scope == "all" && d.a == "rule" && d.b == "todo"));
    assert!(!r
        .diffs
        .iter()
        .any(|d| d.scope == "rule_abstained" && (d.a == "rule" || d.b == "rule")));
    let lines = r.lines().join("\n");
    assert!(lines.contains("acceptance, card J3"), "{lines}");
    assert!(lines.contains("synthetic"), "{lines}");
}

#[tokio::test]
async fn a_small_set_is_not_judged() {
    let (_, cases) = fixture();
    let few: Vec<SectionCase> = cases.into_iter().take(40).collect();
    let outs = offline(&few).await;
    let r = report(&few, &outs, 40, 0, false);
    for a in &r.acceptance {
        assert!(
            a.criteria.iter().all(|c| c.verdict == Verdict::NotJudged),
            "{a:?}"
        );
        assert_eq!(a.overall, Verdict::NotJudged);
    }
    assert!(r.breakdown.iter().all(|c| !c.judged));
}

#[tokio::test]
async fn the_report_holds_no_section_name() {
    let (labels, cases) = fixture();
    let outs = offline(&cases).await;
    let r = report(&cases, &outs, labels.len(), 0, true);
    let json = serde_json::to_string(&r).unwrap().to_lowercase();
    let lines = r.lines().join("\n").to_lowercase();
    for out in [json, lines] {
        for name in [
            "rozpracovan",
            "parking lot",
            "graveyard",
            "erledigt",
            "icebox",
            "hotovo",
        ] {
            assert!(!out.contains(name), "{name}");
        }
    }
}

// --- Jev through the envelope ----------------------------------------------------

const KEY: &str = "tsk_test_0123456789abcdefghijklmnopqrstuv";

/// Knows the right answer for every state (section and board); says
/// `unsure` for the ambiguous ones and answers under the floor on every
/// tenth call.
struct Oracle {
    truth: HashMap<String, (&'static str, bool)>,
    calls: AtomicUsize,
    seen: Mutex<Vec<JevRequest>>,
}

#[async_trait::async_trait]
impl DecisionBackend for Oracle {
    fn provider(&self) -> &'static str {
        crate::service::decide::PROVIDER_JEV
    }
    async fn ask(
        &self,
        _key: &Secret,
        _model: &str,
        req: &JevRequest,
        _timeout: Duration,
    ) -> Result<JevResponse, BackendError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        self.seen.lock().unwrap().push(req.clone());
        let (truth, ambiguous) = self
            .truth
            .get(&req.state.to_string())
            .copied()
            .unwrap_or(("todo", false));
        let (choice, conf) = if ambiguous {
            ("unsure", 0.8)
        } else if n % 10 == 9 {
            (truth, 0.4)
        } else {
            (truth, 0.95)
        };
        Ok(serde_json::from_value(serde_json::json!({
            "model": "jev-1.13.0",
            "answers": { "q": { "type": "choice", "choice": choice, "confidence": conf } },
            "usage": { "input_tokens": 80, "output_tokens": 2 },
        }))
        .unwrap())
    }
}

#[tokio::test]
async fn jev_is_asked_only_through_the_gate_with_the_adapters_request() {
    let (labels, cases) = fixture();
    let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
    let oracle = Arc::new(Oracle {
        truth: cases
            .iter()
            .map(|c| {
                (
                    sm::question_for(&c.key, &c.board).state.to_string(),
                    (c.expect, c.ambiguous),
                )
            })
            .collect(),
        calls: AtomicUsize::new(0),
        seen: Mutex::new(Vec::new()),
    });
    let ctx = DecideCtx::new(Arc::clone(&store), oracle.clone());
    let ps = [Provider::Rule, Provider::Jev];
    let runs = |s: &Arc<Mutex<Store>>| {
        s.lock()
            .unwrap()
            .list_decision_runs(&DecisionRunFilter {
                limit: 10_000,
                ..Default::default()
            })
            .unwrap()
    };

    // The defaults: nothing sent, nothing recorded.
    let outs = run_providers(&cases, &ps, Some(&ctx), DEFAULT_MAX_CALLS).await;
    assert_eq!(oracle.calls.load(Ordering::SeqCst), 0);
    assert!(outs[&Provider::Jev]
        .iter()
        .all(|o| !o.ran && o.reason.as_deref() == Some("flag_off")));
    assert!(runs(&store).is_empty());

    // Flag, mode and key, but rows with no org are not allowed: org_off.
    {
        let s = store.lock().unwrap();
        settings::set(&s, settings::DECIDE_JEV_ENABLED, "true").unwrap();
        settings::set(&s, settings::DECIDE_JEV_STATUS_MAP, "shadow").unwrap();
        s.set_decision_credential(Some(&Secret::new(KEY)), None)
            .unwrap();
    }
    let outs = run_providers(&cases, &ps, Some(&ctx), DEFAULT_MAX_CALLS).await;
    assert_eq!(oracle.calls.load(Ordering::SeqCst), 0);
    assert!(outs[&Provider::Jev]
        .iter()
        .all(|o| o.reason.as_deref() == Some("org_off")));

    // decide.jev.unassigned on: every case is asked and recorded.
    settings::set(
        &store.lock().unwrap(),
        settings::DECIDE_JEV_UNASSIGNED,
        "true",
    )
    .unwrap();
    let outs = run_providers(&cases, &ps, Some(&ctx), DEFAULT_MAX_CALLS).await;
    assert_eq!(oracle.calls.load(Ordering::SeqCst), cases.len());
    let recorded = runs(&store);
    assert_eq!(recorded.len(), cases.len());
    assert!(recorded.iter().all(|r| r.feature == "status_map"
        && r.subject_kind == SUBJECT_KIND
        && r.question_version == QUESTION_VERSION
        && r.mode == "shadow"
        && r.org_id.is_none()
        && r.baseline_answer.is_some()));
    // What was sent is exactly the adapter's request for the normalised
    // section and board.
    {
        let seen = oracle.seen.lock().unwrap();
        for (c, req) in cases.iter().zip(seen.iter()) {
            assert_eq!(req, &sm::question_for(&c.key, &c.board));
            let Question::Choice { criteria, .. } = &req.question else {
                panic!("a choice");
            };
            assert!(criteria.contains_key(sm::UNSURE));
        }
    }
    let jev = &outs[&Provider::Jev];
    let low = jev
        .iter()
        .filter(|o| o.model.is_some() && o.confidence == Some(0.4))
        .count();
    assert!(low > 0);
    // Under the floor and `unsure` are abstentions, not skips.
    assert!(jev.iter().all(|o| o.usable()));
    for (c, o) in cases.iter().zip(jev) {
        if c.ambiguous || o.confidence == Some(0.4) {
            assert_eq!(o.answer, None, "{c:?}");
        } else {
            assert_eq!(o.answer.as_deref(), Some(c.expect));
        }
    }
    let r = report(&cases, &outs, labels.len(), 0, true);
    let m = metric(&r, "jev");
    assert_eq!(m.all.accuracy_on_answered, Some(1.0));
    assert_eq!(m.done_precision, Some(1.0));
    assert_eq!(m.calls, cases.len() as u64);
    assert_eq!(m.input_tokens, 80 * cases.len() as i64);
    assert!(m.latency_p95_ms.is_some());
    // Calibration: every category answer (the low ones too; not `unsure`).
    let answered_with_conf = cases.iter().filter(|c| !c.ambiguous).count() as u64;
    assert_eq!(m.calibration.n.0, answered_with_conf);
    assert!(m.calibration.ece.is_some() && m.calibration.brier.is_some());
    let acc = r.acceptance.iter().find(|a| a.provider == "jev").unwrap();
    assert_eq!(acc.criteria[0].verdict, Verdict::Pass);
    assert_eq!(acc.criteria[1].verdict, Verdict::Pass);
    assert_eq!(acc.criteria[2].verdict, Verdict::Pass);
    // The haiku comparison is not built: the whole is not judged.
    assert_eq!(acc.overall, Verdict::NotJudged);
    assert!(r.diffs.iter().any(|d| d.a == "jev" && d.b == "rule"));
    assert!(r.notes.iter().any(|n| n.contains("decide.jev.unassigned")));

    // The call cap.
    let before = oracle.calls.load(Ordering::SeqCst);
    let outs = run_providers(&cases, &[Provider::Jev], Some(&ctx), 3).await;
    assert_eq!(oracle.calls.load(Ordering::SeqCst) - before, 3);
    assert_eq!(
        outs[&Provider::Jev]
            .iter()
            .filter(|o| o.reason.as_deref() == Some("max_calls"))
            .count(),
        cases.len() - 3
    );

    // An org's consent is its own: a labeled row of an org that did not
    // consent is skipped even with unassigned on.
    let org = store
        .lock()
        .unwrap()
        .add_org("Acme", None, false)
        .unwrap()
        .id;
    let mut one = cases[0].clone();
    one.org_id = Some(org);
    let outs = run_jev(&ctx, &[one], 5).await;
    assert_eq!(outs[0].reason.as_deref(), Some("org_off"));
}
