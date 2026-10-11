use super::super::status_map::{cases, parse_labels, report, run_providers, Provider, FIXTURE};
use super::*;
use crate::service::decide::jev::{BackendError, DecisionBackend, JevResponse};
use crate::service::settings;
use crate::store::{DecisionRunFilter, Secret, Store};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const KEY: &str = "tsk_test_0123456789abcdefghijklmnopqrstuv";

#[test]
fn one_yes_answers_two_or_none_abstain() {
    let n = |t: f64, i: f64, d: f64, x: f64| {
        [
            ("todo", t),
            ("in_progress", i),
            ("done", d),
            ("not_planned", x),
        ]
    };
    assert_eq!(
        combine(&n(0.1, 0.2, 0.9, 0.3)),
        (Some("done"), Some(("done", 0.9)))
    );
    // Two categories say yes: a conflict, an abstention naming the top one.
    assert_eq!(combine(&n(0.1, 0.7, 0.8, 0.3)), (None, Some(("done", 0.8))));
    // Nobody says yes.
    assert_eq!(combine(&n(0.1, 0.2, 0.3, 0.4)).0, None);
    // At the floor counts.
    assert_eq!(combine(&n(0.5, 0.1, 0.1, 0.1)).0, Some("todo"));
}

#[test]
fn each_noul_is_the_adapters_state_and_one_narrow_criterion() {
    let labels = parse_labels(FIXTURE).unwrap();
    let (cs, _) = cases(&labels);
    let c = &cs[0];
    let base = super::super::status_map::request_for(c, None);
    for cat in CATEGORIES {
        let r = noul_request(c, cat);
        assert_eq!(r.state, base.state);
        let Question::Noul { instructions, .. } = &r.question else {
            panic!("a noul");
        };
        let text = instructions.as_str().unwrap();
        assert!(text.starts_with(NOUL_PREFIX));
        assert!(text.ends_with(criterion(cat)), "{text}");
    }
}

/// Says yes (0.9) to the case's true category, no (0.1) to the rest.
struct NoulOracle {
    truth: HashMap<String, &'static str>,
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl DecisionBackend for NoulOracle {
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
        self.calls.fetch_add(1, Ordering::SeqCst);
        let truth = self.truth[&req.state.to_string()];
        let Question::Noul { instructions, .. } = &req.question else {
            panic!("the split asks nouls");
        };
        let yes = instructions.as_str().unwrap().ends_with(criterion(truth));
        Ok(serde_json::from_value(serde_json::json!({
            "model": "jev-1.13.0",
            "answers": { "q": { "type": "noul", "noul": if yes { 0.9 } else { 0.1 } } },
            "usage": { "input_tokens": 60, "output_tokens": 1 },
        }))
        .unwrap())
    }
}

#[tokio::test]
async fn the_split_asks_four_nouls_a_case_and_answers_the_one_yes() {
    let labels = parse_labels(FIXTURE).unwrap();
    let (cs, _) = cases(&labels);
    let cs: Vec<_> = cs.into_iter().take(6).collect();
    let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
    {
        let s = store.lock().unwrap();
        settings::set(&s, settings::DECIDE_JEV_ENABLED, "true").unwrap();
        settings::set(&s, settings::DECIDE_JEV_UNASSIGNED, "true").unwrap();
        s.set_decision_credential(Some(&Secret::new(KEY)), None)
            .unwrap();
    }
    let oracle = Arc::new(NoulOracle {
        truth: cs
            .iter()
            .map(|c| (noul_request(c, "todo").state.to_string(), c.expect))
            .collect(),
        calls: AtomicUsize::new(0),
    });
    let ctx = DecideCtx::new(Arc::clone(&store), oracle.clone());
    // 4 calls a case: 10 calls fit two whole cases, never half of a third.
    let outs = run_providers(&cs, &[Provider::JevSplit], Some(&ctx), 10).await;
    let o = &outs[&Provider::JevSplit];
    assert_eq!(oracle.calls.load(Ordering::SeqCst), 8);
    for (c, x) in cs.iter().zip(o).take(2) {
        assert_eq!(x.answer.as_deref(), Some(c.expect));
        assert_eq!(x.confidence, Some(0.9));
        assert_eq!(x.input_tokens, 240);
    }
    assert!(o[2..]
        .iter()
        .all(|x| x.reason.as_deref() == Some("max_calls")));
    let runs = store
        .lock()
        .unwrap()
        .list_decision_runs(&DecisionRunFilter {
            limit: 100,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(runs.len(), 8);
    assert!(runs
        .iter()
        .all(|r| r.question_version == QUESTION_VERSION && r.subject_kind == SUBJECT_KIND));
    assert!(runs
        .iter()
        .any(|r| r.subject_id == format!("{}.done", cs[0].id)));
    // The report carries it like any model provider, cost per right answer
    // included.
    let r = report(&cs, &outs, cs.len(), 0, true);
    let m = r
        .metrics
        .iter()
        .find(|m| m.provider == "jev-split")
        .unwrap();
    assert_eq!(m.calls, 2);
    assert_eq!(m.all.correct, 2);
    assert_eq!(
        m.cost_per_correct_microusd,
        crate::service::decide::bench::cost_per_correct(m.cost_microusd, 2)
    );
}
