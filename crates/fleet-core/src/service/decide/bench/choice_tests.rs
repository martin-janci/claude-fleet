//! The closed-choice benches: every built-in set loads, is marked
//! synthetic, says honestly who answers each case, never offers an option
//! on a case's never list, and its rule cases are answered without a call.
//! No test reaches TypeSafe.

use super::choice::*;
use super::Verdict;
use crate::service::decide::{
    BackendError, DecideCtx, DecisionBackend, JevRequest, JevResponse, Question, PROVIDER_JEV,
};
use crate::service::settings;
use crate::store::{Secret, Store};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const KEY: &str = "tsk_test_0123456789abcdefghijklmnopqrstuv";

/// Answers every question with its first option, and counts the calls.
#[derive(Default)]
struct FirstOption {
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl DecisionBackend for FirstOption {
    fn provider(&self) -> &'static str {
        PROVIDER_JEV
    }
    async fn ask(
        &self,
        _key: &Secret,
        _model: &str,
        req: &JevRequest,
        _timeout: Duration,
    ) -> Result<JevResponse, BackendError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let Question::Choice { criteria, .. } = &req.question else {
            return Err(BackendError::Transport("not a choice".into()));
        };
        let choice = criteria.keys().next().cloned().unwrap_or_default();
        Ok(serde_json::from_value(serde_json::json!({
            "model": "jev-1.13.0",
            "answers": { "q": {
                "type": "choice",
                "choice": choice,
                "probabilities": { choice.clone(): 0.9 },
                "confidence": 0.9,
            }},
            "usage": { "input_tokens": 50, "output_tokens": 2 },
        }))
        .unwrap())
    }
}

/// A store whose bench gate is open for every feature (no feature's live
/// mode is needed), and a context over the counting backend.
fn open_ctx() -> (DecideCtx, Arc<FirstOption>) {
    let s = Store::open_in_memory().unwrap();
    settings::set(&s, settings::DECIDE_JEV_ENABLED, "true").unwrap();
    settings::set(&s, settings::DECIDE_JEV_UNASSIGNED, "true").unwrap();
    settings::set(&s, settings::DECIDE_JEV_UNASSIGNED_REPLY, "true").unwrap();
    s.set_decision_credential(Some(&Secret::new(KEY)), None)
        .unwrap();
    let fake = Arc::new(FirstOption::default());
    let ctx = DecideCtx::new(Arc::new(Mutex::new(s)), fake.clone())
        .with_clock(Arc::new(crate::service::catalog::now_secs));
    (ctx, fake)
}

/// The plan's rule for every Jev use case: a benchmark set ships with it.
/// Each built-in set loads, says it is synthetic, holds rule cases, model
/// cases, `unsure` labels and never-list traps, and every `by` it claims is
/// what the use case's own rules do (`parse` refuses a contradiction).
#[test]
fn every_bench_set_loads_and_is_marked_synthetic() {
    // Every set's error at once, so a broken fixture names all its lines.
    let errors: Vec<String> = UseCase::ALL
        .iter()
        .filter_map(|&uc| fixture(uc).err().map(|e| format!("{}: {e}", uc.name())))
        .collect();
    assert!(errors.is_empty(), "{errors:#?}");
    for &uc in UseCase::ALL {
        let set = fixture(uc).unwrap_or_else(|e| panic!("{}: {e}", uc.name()));
        let n = uc.name();
        assert!(set.synthetic, "{n}: the built-in set is synthetic");
        assert!(set.cases.len() >= 10, "{n}: {} cases", set.cases.len());
        assert!(set.cases.iter().any(Case::by_rule), "{n}: a rule case");
        assert!(set.cases.iter().any(|c| !c.by_rule()), "{n}: a model case");
        assert!(
            set.cases.iter().any(|c| c.label == UNSURE),
            "{n}: an unsure label"
        );
        assert!(
            set.cases
                .iter()
                .any(|c| c.trap.is_some() || !c.never.is_empty()),
            "{n}: a never-list trap"
        );
        for c in &set.cases {
            assert!(c.by.is_some(), "{n}/{}: says who answers", c.id);
        }
        assert!(uc.fixture().starts_with("# synthetic"), "{n}: the header");
        assert!(
            uc.fixture_path().ends_with(&format!("{n}_cases.jsonl")),
            "{n}"
        );
        assert_eq!(UseCase::parse(&uc.command()), Some(uc));
    }
}

/// The use cases' fixed answer words (never a candidate the rules drop).
const FIXED_WORDS: &[&str] = &["did_work", "nothing", "needs_person", "control", "none"];

/// A never-list option is dropped by the rules before any model sees the
/// case: it is never one of the options asked, and never the rule's answer.
#[test]
fn a_never_option_is_never_offered() {
    for &uc in UseCase::ALL {
        for c in fixture(uc).unwrap().cases {
            match &c.prepared {
                Prepared::Rule(a) => {
                    assert!(!c.never.contains(a), "{}/{}: {a}", uc.name(), c.id)
                }
                Prepared::Ask { options, .. } => {
                    // A fixed answer word (N6's `nothing` on a screen that
                    // asks) is always an option: it is a trap for the
                    // model, counted by the metrics, not a candidate the
                    // rules drop.
                    for n in c
                        .never
                        .iter()
                        .filter(|n| !FIXED_WORDS.contains(&n.as_str()))
                    {
                        assert!(!options.contains(n), "{}/{}: {n} offered", uc.name(), c.id);
                    }
                }
            }
        }
    }
}

/// The rule layer answers its rule-decidable cases without a Jev call, and
/// each answer is the case's label; every other case costs one call.
#[tokio::test]
async fn the_rules_answer_their_cases_without_a_call() {
    for &uc in UseCase::ALL {
        let set = fixture(uc).unwrap();
        let rules = CaseSet {
            cases: set.cases.iter().filter(|c| c.by_rule()).cloned().collect(),
            ..set.clone()
        };
        let (ctx, fake) = open_ctx();
        let answers = run_jev(&ctx, &rules, 1_000).await;
        assert_eq!(fake.calls.load(Ordering::SeqCst), 0, "{}", uc.name());
        for (c, a) in rules.cases.iter().zip(&answers) {
            assert!(!a.called, "{}/{}", uc.name(), c.id);
            let want = (c.label != UNSURE).then(|| c.label.clone());
            assert_eq!(a.answer, want, "{}/{}", uc.name(), c.id);
        }
        let (ctx, fake) = open_ctx();
        let all = run_jev(&ctx, &set, 1_000).await;
        let asked = set.cases.iter().filter(|c| !c.by_rule()).count();
        assert_eq!(fake.calls.load(Ordering::SeqCst), asked, "{}", uc.name());
        assert_eq!(all.iter().filter(|a| a.called).count(), asked);
    }
}

#[tokio::test]
async fn a_closed_gate_skips_the_model_cases_and_still_answers_the_rules() {
    let s = Store::open_in_memory().unwrap();
    let fake = Arc::new(FirstOption::default());
    let ctx = DecideCtx::new(Arc::new(Mutex::new(s)), fake.clone());
    let set = fixture(UseCase::ControlRoute).unwrap();
    let answers = run_jev(&ctx, &set, 10).await;
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    for (c, a) in set.cases.iter().zip(&answers) {
        assert_eq!(a.skipped.is_some(), !c.by_rule(), "{}", c.id);
    }
    let m = metrics(Provider::Jev, &set, &answers);
    assert_eq!(
        m.cases as usize,
        set.cases.iter().filter(|c| c.by_rule()).count()
    );
}

/// Offline, nothing is called; on a synthetic set nothing is judged, and
/// the report says so without printing a case.
#[test]
fn the_offline_providers_on_the_synthetic_sets_are_never_judged() {
    for &uc in UseCase::ALL {
        let set = fixture(uc).unwrap();
        let mut all = Vec::new();
        for p in [Provider::Rule, Provider::Baseline] {
            let answers: Vec<Answer> = set.cases.iter().map(|c| offline(p, c)).collect();
            let m = metrics(p, &set, &answers);
            assert_eq!(m.calls, 0);
            assert_eq!(
                m.rule_decided as usize,
                set.cases.iter().filter(|c| c.by_rule()).count()
            );
            assert_eq!(m.never_violations, 0, "{} {}", uc.name(), p.as_str());
            for v in [m.precision_verdict, m.unsure_verdict, m.never_verdict] {
                assert_eq!(v, Verdict::NotJudged, "{}", uc.name());
            }
            all.push(m);
        }
        let l = lines(&all);
        assert_eq!(l.len(), 3);
        assert!(l[0].contains("SYNTHETIC"), "{}", l[0]);
    }
}

/// The rule provider abstains wherever the model would be asked; the
/// baseline answers what fleet shows without Jev there.
#[test]
fn the_rule_abstains_and_the_baseline_is_todays_answer() {
    let set = fixture(UseCase::HostPlacement).unwrap();
    let c = set.cases.iter().find(|c| c.id == "history").unwrap();
    assert_eq!(offline(Provider::Rule, c).answer, None);
    assert_eq!(
        offline(Provider::Baseline, c).answer.as_deref(),
        Some("h:local"),
        "the first candidate, local first"
    );
    let set = fixture(UseCase::ControlRoute).unwrap();
    let c = set.cases.iter().find(|c| c.id == "session-api").unwrap();
    assert_eq!(
        offline(Provider::Baseline, c).answer.as_deref(),
        Some("control")
    );
    // A label in a person's words resolves to its option word.
    let set = fixture(UseCase::WorkPlacement).unwrap();
    let c = set.cases.iter().find(|c| c.id == "billing").unwrap();
    assert!(c.label.starts_with('g'), "{}", c.label);
    let set = fixture(UseCase::MainTicket).unwrap();
    let c = set
        .cases
        .iter()
        .find(|c| c.id == "fix-with-history")
        .unwrap();
    assert_eq!(c.label, "k1");
}

#[test]
fn a_set_that_contradicts_the_rules_or_its_labels_is_refused() {
    let uc = UseCase::ControlRoute;
    let t = r#"{"kind":"session","id":4,"name":"api","detail":"acme/api"}"#;
    let line = |msg: &str, label: &str, by: &str| {
        format!(
            r#"{{"id":"x","input":{{"message":"{msg}","targets":[{t}]}},"label":"{label}","by":"{by}"}}"#
        )
    };
    assert!(parse(uc, &line("yes", "unsure", "rule")).is_ok());
    let e = parse(uc, &line("yes", "unsure", "jev")).unwrap_err();
    assert!(e.contains("line 1") && e.contains("rules answer"), "{e}");
    let e = parse(uc, &line("fix the api login please now", "s4", "rule")).unwrap_err();
    assert!(e.contains("leave it to the model"), "{e}");
    let e = parse(uc, &line("fix the api login please now", "s9", "jev")).unwrap_err();
    assert!(e.contains("no answer"), "{e}");
    let two = format!(
        "{}\n{}",
        line("yes", "unsure", "rule"),
        line("no", "unsure", "rule")
    );
    assert!(parse(uc, &two).unwrap_err().contains("twice"));
    // Without a `# synthetic` line a set is recorded data.
    let set = parse(uc, &line("yes", "unsure", "rule")).unwrap();
    assert!(!set.synthetic);
}

/// Recorded data (no `# synthetic`) with enough cases is judged: precision
/// over the proposals, pre-selects on unsure cases, and the never list.
#[test]
fn a_recorded_set_is_judged() {
    let uc = UseCase::ControlRoute;
    let t = r#"{"kind":"session","id":4,"name":"api","detail":"acme/api"}"#;
    let mut text = String::new();
    for i in 0..60 {
        text.push_str(&format!(
            r#"{{"id":"a{i}","input":{{"message":"fix the api login bug {i}","targets":[{t}]}},"label":"s4","never":["control"]}}"#
        ));
        text.push('\n');
    }
    for i in 0..20 {
        text.push_str(&format!(
            r#"{{"id":"u{i}","input":{{"message":"look into this thing {i}","targets":[{t}]}},"label":"unsure"}}"#
        ));
        text.push('\n');
    }
    let set = parse(uc, &text).unwrap();
    assert!(!set.synthetic);
    let right: Vec<Answer> = set
        .cases
        .iter()
        .map(|c| Answer {
            answer: (c.label != UNSURE).then(|| c.label.clone()),
            called: true,
            skipped: None,
        })
        .collect();
    let m = metrics(Provider::Jev, &set, &right);
    assert_eq!(m.labelled_for_model, 60);
    assert_eq!(
        (m.precision_verdict, m.unsure_verdict, m.never_verdict),
        (Verdict::Pass, Verdict::Pass, Verdict::Pass)
    );
    // Proposing on every unsure case, and answering `control` (a never
    // option here) once, fails both lines.
    let mut wrong = right.clone();
    for a in wrong.iter_mut().skip(60) {
        a.answer = Some("s4".into());
    }
    wrong[0].answer = Some("control".into());
    let m = metrics(Provider::Jev, &set, &wrong);
    assert_eq!(m.preselected_on_unsure, 20);
    assert_eq!(m.unsure_verdict, Verdict::Fail);
    assert_eq!(m.never_violations, 1);
    assert_eq!(m.never_verdict, Verdict::Fail);
    assert_eq!(
        m.precision_verdict,
        Verdict::Fail,
        "59 of 79 proposals right"
    );
}
