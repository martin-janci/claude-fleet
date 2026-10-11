use super::*;

fn floor(v: &str, model: &str, f: f64) -> FittedFloor {
    FittedFloor {
        question_version: v.into(),
        model: model.into(),
        floor: f,
        target: 0.95,
        labelled: 200,
        fitted_at: 1,
    }
}

fn floors(entries: &[(&str, FittedFloor)]) -> Floors {
    entries
        .iter()
        .map(|(k, f)| (k.to_string(), f.clone()))
        .collect()
}

#[test]
fn floors_round_trip_and_refuse_what_is_not_a_floor() {
    let fl = floors(&[("status_map", floor("sm.3", "jev-1.13.0", 0.9))]);
    assert_eq!(parse(&render(&fl)).unwrap(), fl);
    assert_eq!(parse("").unwrap(), Floors::new());
    assert_eq!(render(&Floors::new()), "");
    let bad = |f: FittedFloor| render(&floors(&[("status_map", f)]));
    assert!(parse(&bad(floor("sm.3", "jev-1.13.0", 1.5))).is_err());
    assert!(parse(&bad(floor("sm.3", "jev-1.13.0", f64::NAN))).is_err());
    assert!(parse(&bad(floor("a sentence", "jev-1.13.0", 0.9))).is_err());
    assert!(parse(&render(&floors(&[("nope", floor("v", "m", 0.9))]))).is_err());
    assert!(parse("[1]").is_err());
}

#[test]
fn a_fitted_floor_only_raises_and_only_for_its_version_and_model() {
    let fl = floors(&[
        ("status_map", floor("sm.3", "jev-1.13.0", 0.9)),
        ("work_link", floor("wl.2", "jev-1.13.0", 0.3)),
    ]);
    let eff = |adapter, feature, v: &str, m: &str| effective(adapter, &fl, feature, v, m);
    assert_eq!(
        eff(Some(0.5), Feature::StatusMap, "sm.3", "jev-1.13.0"),
        Some(0.9)
    );
    // Another question version or model: the adapter's floor stands.
    assert_eq!(
        eff(Some(0.5), Feature::StatusMap, "sm.4", "jev-1.13.0"),
        Some(0.5)
    );
    assert_eq!(
        eff(Some(0.5), Feature::StatusMap, "sm.3", "jev-latest"),
        Some(0.5)
    );
    // Never lower than the adapter's own.
    assert_eq!(
        eff(Some(0.5), Feature::WorkLink, "wl.2", "jev-1.13.0"),
        Some(0.5)
    );
    // No adapter floor: the fitted one applies; neither: none.
    assert_eq!(
        eff(None, Feature::StatusMap, "sm.3", "jev-1.13.0"),
        Some(0.9)
    );
    assert_eq!(eff(None, Feature::Duplicate, "d.1", "jev-1.13.0"), None);
}

#[test]
fn a_stored_floor_that_does_not_parse_counts_as_none() {
    let s = Store::open_in_memory().unwrap();
    assert!(read(&s).is_empty());
    s.set_setting(settings::DECIDE_JEV_FLOORS, "{not json")
        .unwrap();
    assert!(read(&s).is_empty());
    let fl = floors(&[("status_map", floor("sm.3", "jev-1.13.0", 0.9))]);
    s.set_setting(settings::DECIDE_JEV_FLOORS, &render(&fl))
        .unwrap();
    assert_eq!(read(&s), fl);
}

#[test]
fn the_floors_setting_refuses_what_does_not_parse() {
    let s = Store::open_in_memory().unwrap();
    assert!(settings::set(&s, settings::DECIDE_JEV_FLOORS, "{not json").is_err());
    let fl = floors(&[("status_map", floor("sm.3", "jev-1.13.0", 0.9))]);
    settings::set(&s, settings::DECIDE_JEV_FLOORS, &render(&fl)).unwrap();
    assert_eq!(read(&s), fl);
    settings::set(&s, settings::DECIDE_JEV_FLOORS, "").unwrap();
    assert!(read(&s).is_empty());
}

fn row(at: i64, conf: Option<f64>, followup: Option<&str>) -> DecisionRunRow {
    DecisionRunRow {
        id: at,
        at,
        feature: "status_map".into(),
        org_id: None,
        subject_kind: "section".into(),
        subject_id: format!("3:{at}"),
        mode: "shadow".into(),
        provider: "jev".into(),
        model_version: Some("jev-1.13.0".into()),
        question_version: "sm.3".into(),
        input_fp: None,
        candidates: vec![],
        answer: Some("done".into()),
        probabilities: None,
        confidence: conf,
        fallback: None,
        baseline_answer: None,
        called: true,
        latency_ms: None,
        input_tokens: 0,
        cost_microusd: 0,
        followup: followup.map(str::to_string),
        followup_at: None,
        corrected_to: None,
    }
}

#[test]
fn only_person_followups_of_the_same_question_and_model_are_labels() {
    let mut rows = vec![
        row(3, Some(0.9), Some("confirmed")),
        row(1, Some(0.8), Some("corrected")),
        row(2, Some(0.7), Some("rejected")),
        row(4, Some(0.9), Some("ignored")),
        row(5, Some(0.9), None),
    ];
    let mut other_version = row(6, Some(0.9), Some("confirmed"));
    other_version.question_version = "sm.2".into();
    let mut other_model = row(7, Some(0.9), Some("confirmed"));
    other_model.model_version = Some("jev-1.14.0".into());
    let mut bench = row(8, Some(0.9), Some("confirmed"));
    bench.subject_kind = crate::store::DECISION_BENCH_SUBJECT.into();
    let mut unsure = row(9, Some(0.9), Some("confirmed"));
    unsure.answer = Some("unsure".into());
    rows.extend([other_version, other_model, bench, unsure]);
    let l = labelled(&rows, "sm.3", "jev-1.13.0");
    assert_eq!(
        l.iter().map(|l| (l.at, l.correct)).collect::<Vec<_>>(),
        vec![(1, false), (2, false), (3, true)]
    );
}

/// `n` runs, oldest first: right at or above `cut`, wrong below.
fn separable(n: i64, cut: f64) -> Vec<Labelled> {
    (0..n)
        .map(|i| {
            let c = 0.5 + 0.5 * (i % 50) as f64 / 50.0;
            Labelled {
                at: i,
                certainty: Some(c),
                correct: c >= cut,
            }
        })
        .collect()
}

#[test]
fn the_fit_picks_the_lowest_floor_that_holds_on_both_halves() {
    let f = fit(&separable(400, 0.8), 0.95, 20);
    assert_eq!(f.floor, Some(0.8));
    assert!(f.holds);
    let t = f.test.unwrap();
    assert_eq!(t.precision(), Some(1.0));
    assert!(f.lines(Feature::StatusMap).join("\n").contains("holds"));
}

#[test]
fn a_fit_that_the_newer_half_breaks_is_not_stored() {
    // The older half is right everywhere; the newer half wrong everywhere.
    let mut rows = separable(400, 0.0);
    for l in rows.iter_mut().skip(200) {
        l.correct = false;
    }
    let f = fit(&rows, 0.95, 20);
    assert_eq!(f.floor, Some(0.5));
    assert!(!f.holds);
    assert!(f.lines(Feature::StatusMap).join("\n").contains("NOT hold"));
}

#[test]
fn too_few_labels_fit_nothing() {
    let f = fit(&separable(20, 0.8), 0.95, 20);
    assert_eq!(f.floor, None);
    assert!(!f.holds);
    assert!(f
        .lines(Feature::StatusMap)
        .join("\n")
        .contains("no floor reaches"));
    // A run without a certainty is never answered at any floor.
    let none: Vec<Labelled> = (0..100)
        .map(|i| Labelled {
            at: i,
            certainty: None,
            correct: true,
        })
        .collect();
    assert_eq!(fit(&none, 0.5, 1).floor, None);
}
