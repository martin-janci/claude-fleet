//! Tests of the `status_map` benchmark's paired diagnostics and floor sweep:
//! variants change what they say and nothing else, the paired set is one
//! section per pair in four languages, the comparisons count what they say,
//! the sweep's dev choice is the lowest floor that meets the card, the
//! built-in questions parse, and no section name reaches a report.

use super::perturb::{Perturbation, PAIRED_MIN};
use super::status_map::*;
use super::status_map_robust::*;
use crate::service::decide::status_map as sm;
use std::collections::{BTreeMap, BTreeSet};

fn fixture_cases() -> Vec<SectionCase> {
    cases(&parse_labels(FIXTURE).unwrap()).0
}

fn paired_cases() -> Vec<SectionCase> {
    let (c, dropped) = cases(&parse_labels(PAIRED_FIXTURE).unwrap());
    assert_eq!(dropped, 0);
    c
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

// --- variants -----------------------------------------------------------------------

#[test]
fn a_variant_changes_only_what_its_perturbation_says() {
    let cases = fixture_cases();
    for p in Perturbation::STATUS_MAP {
        let vs = variants(&cases, p);
        assert!(!vs.is_empty(), "{p:?}");
        // Deterministic.
        assert_eq!(vs, variants(&cases, p), "{p:?}");
        for (i, v) in &vs {
            let c = &cases[*i];
            assert_eq!(v.id, p.case_id(&c.id));
            assert!(crate::store::is_decision_word(&v.id), "{}", v.id);
            assert_eq!(
                (v.expect, &v.lang, v.ambiguous, v.org_id, &v.pair),
                (c.expect, &c.lang, c.ambiguous, c.org_id, &c.pair)
            );
            assert_eq!(
                v.rule,
                crate::service::trackers::asana::infer_section(&v.key)
            );
            assert!(
                v.key != c.key || v.board != c.board,
                "{p:?} changed nothing"
            );
            match p {
                Perturbation::NoBoard => {
                    assert!(v.board.is_empty());
                    assert_eq!(v.key, c.key);
                }
                Perturbation::ShuffleBoard => {
                    assert_eq!(v.key, c.key);
                    let (a, b): (BTreeSet<_>, BTreeSet<_>) =
                        (v.board.iter().collect(), c.board.iter().collect());
                    assert_eq!(a, b);
                    assert_ne!(v.board, c.board);
                }
                _ => {
                    assert!(v.board.contains(&v.key), "{p:?}: {:?}", v.id);
                    assert_eq!(v.board.len(), c.board.len(), "{p:?}");
                }
            }
        }
    }
}

#[test]
fn folding_touches_only_names_with_diacritics_and_typos_keep_the_board() {
    let cases = fixture_cases();
    let fold = variants(&cases, Perturbation::Fold);
    // Only boards with a diacritic change; English is never folded.
    for (i, _) in &fold {
        let c = &cases[*i];
        assert!(
            c.board.iter().any(|n| !n.is_ascii()),
            "{} has no diacritic",
            c.id
        );
    }
    assert!(fold
        .iter()
        .all(|(i, _)| cases[*i].lang != "en" || cases[*i].board.iter().any(|n| !n.is_ascii())));
    let v = fold
        .iter()
        .find(|(i, _)| cases[*i].key == "rozpracované")
        .map(|(_, v)| v)
        .unwrap();
    assert_eq!(v.key, "rozpracovane");
    assert!(v
        .board
        .iter()
        .all(|n| n.is_ascii() || n.chars().any(|c| !c.is_alphabetic())));

    // A typo changes the section and its own entry on the board, nothing
    // else.
    for (i, v) in variants(&cases, Perturbation::Typo) {
        let c = &cases[i];
        let changed: Vec<usize> = (0..c.board.len())
            .filter(|&k| c.board[k] != v.board[k])
            .collect();
        assert!(changed.len() <= 1, "{}", c.id);
    }
    // The keyword rule is read again on the new name: some typo defeats it.
    let typo = variants(&cases, Perturbation::Typo);
    assert!(typo
        .iter()
        .any(|(i, v)| cases[*i].rule.is_some() && v.rule.is_none()));
}

#[tokio::test]
async fn robustness_pairs_each_variant_with_its_original() {
    let cases = fixture_cases();
    let outs = offline(&cases).await;
    let vs = variants(&cases, Perturbation::Typo);
    let vcases: Vec<SectionCase> = vs.iter().map(|(_, v)| v.clone()).collect();
    let vouts = offline(&vcases).await;
    let r = robustness(Perturbation::Typo, &cases, &outs, &vs, &vouts);
    assert_eq!(r.perturbation, "typo");
    assert_eq!((r.cases, r.changed), (cases.len() as u64, vs.len() as u64));
    assert!(r.rule_changed > 0);
    // `none` is never compared; `todo` never changes its answer.
    let names: Vec<&str> = r.providers.iter().map(|p| p.compare.provider).collect();
    assert_eq!(names, vec!["todo", "rule"]);
    let todo = &r.providers[0].compare;
    assert_eq!(todo.changed, 0);
    assert_eq!(todo.pairs, vs.len() as u64);
    assert_eq!(
        todo.verdict,
        if todo.pairs >= PAIRED_MIN {
            "no difference"
        } else {
            "not judged"
        }
    );
    // The rule loses answers to typos: its coverage falls.
    let rule = &r.providers[1].compare;
    assert!(rule.changed >= r.rule_changed.min(rule.pairs));
    assert!(rule.coverage < rule.ref_coverage, "{rule:?}");
}

// --- the paired set (dataset B) -------------------------------------------------------

#[test]
fn the_paired_set_is_one_section_in_four_languages_per_pair() {
    let cases = paired_cases();
    assert_eq!(cases.len(), 324);
    let mut by_pair: BTreeMap<String, Vec<&SectionCase>> = BTreeMap::new();
    for c in &cases {
        by_pair.entry(c.pair.clone().unwrap()).or_default().push(c);
        assert!(c.board.contains(&c.key), "{}", c.id);
        assert!(c.board.len() <= sm::MAX_PROJECT_SECTIONS);
    }
    assert_eq!(by_pair.len(), 81);
    assert!(by_pair.len() as u64 >= PAIRED_MIN);
    for (p, rows) in &by_pair {
        let langs: BTreeSet<&str> = rows.iter().map(|c| c.lang.as_str()).collect();
        assert_eq!(langs, ["cs", "de", "en", "sk"].into_iter().collect(), "{p}");
        // A translation keeps the label, the ambiguity and the position.
        let first = rows[0];
        for c in rows {
            assert_eq!(
                (c.expect, c.ambiguous),
                (first.expect, first.ambiguous),
                "{p}"
            );
            assert_eq!(
                c.board.iter().position(|n| *n == c.key),
                first.board.iter().position(|n| *n == first.key),
                "{p}"
            );
            assert_eq!(c.board.len(), first.board.len(), "{p}");
        }
    }
    for cat in CATEGORIES {
        assert!(cases.iter().any(|c| c.expect == cat), "{cat}");
    }
    // The keyword rule is English: it abstains on far more of the others.
    let abstains = |lang: &str| {
        cases
            .iter()
            .filter(|c| c.lang == lang && c.rule.is_none())
            .count()
    };
    assert!(abstains("sk") > abstains("en"));
}

#[tokio::test]
async fn languages_are_compared_with_english_on_the_same_pairs() {
    let cases = paired_cases();
    let outs = offline(&cases).await;
    let l = language_pairs(&cases, &outs).unwrap();
    assert_eq!((l.reference, l.pairs), ("en", 81));
    let langs: Vec<&str> = l.langs.iter().map(|x| x.lang.as_str()).collect();
    assert_eq!(langs, vec!["cs", "de", "sk"]);
    for lang in &l.langs {
        assert_eq!(lang.pairs, 81);
        let rule = lang
            .providers
            .iter()
            .find(|p| p.compare.provider == "rule")
            .unwrap();
        assert!(
            rule.compare.coverage < rule.compare.ref_coverage,
            "{}: {:?}",
            lang.lang,
            rule.compare
        );
    }
    // The report carries it by itself, and without pairs it is absent.
    let r = report(&cases, &outs, cases.len(), 0, true);
    assert!(r.languages.is_some());
    let plain = fixture_cases();
    let po = offline(&plain).await;
    let r = report(&plain, &po, plain.len(), 0, true);
    assert!(r.languages.is_none());
    let json = serde_json::to_value(&r).unwrap();
    for k in ["languages", "robustness", "floor_sweep"] {
        assert!(json.get(k).is_none(), "{k}");
    }
}

#[tokio::test]
async fn no_section_name_reaches_the_paired_or_robust_lines() {
    let cases = paired_cases();
    let outs = offline(&cases).await;
    let vs = variants(&cases, Perturbation::Fold);
    let vcases: Vec<SectionCase> = vs.iter().map(|(_, v)| v.clone()).collect();
    let vouts = offline(&vcases).await;
    let r = report(&cases, &outs, cases.len(), 0, true).with_robustness(vec![robustness(
        Perturbation::Fold,
        &cases,
        &outs,
        &vs,
        &vouts,
    )]);
    let text = r.lines().join("\n") + &serde_json::to_string(&r).unwrap();
    assert!(text.contains("languages (dataset B"));
    assert!(text.contains("robustness (dataset C"));
    // Words the report prints anyway (`not_planned`, …) are not names.
    let dummy: Vec<SectionCase> = cases
        .iter()
        .enumerate()
        .map(|(i, c)| SectionCase {
            key: format!("x{i}"),
            board: vec![format!("x{i}")],
            ..c.clone()
        })
        .collect();
    let d_outs = offline(&dummy).await;
    let vocabulary = report(&dummy, &d_outs, dummy.len(), 0, true)
        .with_robustness(vec![robustness(
            Perturbation::Fold,
            &dummy,
            &d_outs,
            &[],
            &d_outs,
        )])
        .lines()
        .join("\n")
        + &serde_json::to_string(&report(&dummy, &d_outs, dummy.len(), 0, true)).unwrap();
    let mut checked = 0;
    for c in cases.iter().chain(&vcases) {
        if c.key.chars().count() > 3 && !vocabulary.contains(&c.key) {
            checked += 1;
            assert!(!text.contains(&c.key), "{}", c.key);
        }
    }
    assert!(checked > 300, "{checked}");
}

// --- the floor sweep ------------------------------------------------------------------

/// A model that is right at confidence 0.9 and wrong at 0.4 on every third
/// case, and says `unsure` on every seventh.
fn swept(cases: &[SectionCase]) -> Vec<Outcome> {
    cases
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let (model, conf) = if i % 7 == 6 {
                ("unsure".to_string(), 0.9)
            } else if i % 3 == 2 {
                let wrong = CATEGORIES.iter().find(|x| **x != c.expect).unwrap();
                (wrong.to_string(), 0.4)
            } else {
                (c.expect.to_string(), 0.9)
            };
            Outcome {
                ran: true,
                answer: (conf >= sm::MIN_CONFIDENCE && model != "unsure").then(|| model.clone()),
                model: Some(model),
                confidence: Some(conf),
                ..Default::default()
            }
        })
        .collect()
}

#[test]
fn answer_at_a_floor() {
    let o = |m: &str, c: Option<f64>| Outcome {
        ran: true,
        model: Some(m.into()),
        confidence: c,
        ..Default::default()
    };
    assert_eq!(answer_at(&o("done", Some(0.6)), 0.6), Some("done"));
    assert_eq!(answer_at(&o("done", Some(0.59)), 0.6), None);
    assert_eq!(answer_at(&o("unsure", Some(0.99)), 0.0), None);
    // No confidence: it stands at every floor.
    assert_eq!(answer_at(&o("todo", None), 0.95), Some("todo"));
    let mut failed = o("done", Some(0.9));
    failed.reason = Some("timeout".into());
    assert_eq!(answer_at(&failed, 0.0), None);
}

#[test]
fn the_sweep_trades_coverage_for_accuracy_and_dev_picks_the_lowest_floor_that_meets_the_card() {
    let cases = fixture_cases();
    let outs: Outcomes = [
        (Provider::Jev, swept(&cases)),
        (Provider::Rule, vec![Outcome::default(); cases.len()]),
    ]
    .into_iter()
    .collect();
    let s = floor_sweep(&cases, &outs, true);
    assert_eq!(s.len(), 1, "only a model is swept");
    let s = &s[0];
    assert_eq!(s.provider, "jev");
    assert_eq!(s.rows.len(), SWEEP_FLOORS.len());
    assert_eq!(s.rows.iter().filter(|r| r.current).count(), 1);
    let at = |f: f64| s.rows.iter().find(|r| (r.floor - f).abs() < 1e-9).unwrap();
    // Up to 0.40 the wrong answers count; from 0.45 they do not.
    assert!(at(0.40).coverage > at(0.45).coverage);
    assert!(at(0.40).accuracy_on_answered < Some(0.9));
    assert_eq!(at(0.45).accuracy_on_answered, Some(1.0));
    assert_eq!(at(0.95).answered, 0);
    let d = s.dev_choice.as_ref().unwrap();
    assert_eq!(d.floor, Some(0.45));
    assert_eq!(d.dev_cases + d.test_cases, cases.len() as u64);
    let t = d.test.as_ref().unwrap();
    assert_eq!(t.rule_abstained_accuracy, Some(1.0));
    assert!(meets_card(t));
    let lines = floor_sweep_lines(std::slice::from_ref(s)).join("\n");
    assert!(
        lines.contains("dev (") && lines.contains("chooses floor 0.45"),
        "{lines}"
    );
    // One side only: no choice.
    let s = floor_sweep(&cases, &outs, false);
    assert!(s[0].dev_choice.is_none());
    assert!(floor_sweep_lines(&s).join("\n").contains("--split all"));
}

#[test]
fn a_floor_needs_enough_answers_and_both_precision_lines() {
    let row = |ans: u64, acc: Option<f64>, done: Option<f64>| FloorRow {
        floor: 0.5,
        answered: ans,
        coverage: None,
        accuracy_on_answered: None,
        rule_abstained_answered: ans,
        rule_abstained_coverage: None,
        rule_abstained_accuracy: acc,
        done_answers: 0,
        done_precision: done,
        current: true,
    };
    assert!(meets_card(&row(20, Some(0.9), None)));
    assert!(meets_card(&row(20, Some(0.95), Some(0.97))));
    assert!(!meets_card(&row(19, Some(1.0), None)));
    assert!(!meets_card(&row(50, Some(0.89), None)));
    assert!(!meets_card(&row(50, Some(0.99), Some(0.96))));
}

// --- the question set -----------------------------------------------------------------

#[test]
fn the_built_in_questions_parse_under_their_file_names() {
    let mut versions = BTreeSet::new();
    for (file, json) in QUESTION_SET {
        let q = parse_question(json).unwrap_or_else(|e| panic!("{file}: {e}"));
        assert_eq!(file, format!("status_map.{}.json", q.version));
        assert_ne!(q.recorded_version(), QUESTION_VERSION);
        assert!(crate::store::is_decision_word(&q.recorded_version()));
        assert!(versions.insert(q.version.clone()), "{file}");
        // A rewording, not the adapter's words again.
        assert_ne!(q.instructions, sm::INSTRUCTIONS, "{file}");
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(QUESTION_SET_DIR)
            .join(file);
        assert!(path.is_file(), "{}", path.display());
    }
    // Every file in the directory is in the set.
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(QUESTION_SET_DIR);
    let on_disk: BTreeSet<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    let listed: BTreeSet<String> = QUESTION_SET.iter().map(|(f, _)| f.to_string()).collect();
    assert_eq!(on_disk, listed);
}

#[test]
fn a_question_row_is_a_models_numbers_where_the_rule_abstains() {
    let cases = fixture_cases();
    let outs: Outcomes = [
        (Provider::Jev, swept(&cases)),
        (
            Provider::Rule,
            cases
                .iter()
                .map(|c| Outcome {
                    ran: true,
                    answer: c.rule.map(str::to_string),
                    ..Default::default()
                })
                .collect(),
        ),
    ]
    .into_iter()
    .collect();
    let q = parse_question(QUESTION_SET[0].1).unwrap();
    let r = report(&cases, &outs, cases.len(), 0, true).with_question(Some(&q));
    let rows = question_rows(&r);
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (rows[0].question.as_str(), rows[0].provider),
        ("v2-position", "jev")
    );
    let m = r.metrics.iter().find(|m| m.provider == "jev").unwrap();
    assert_eq!(
        rows[0].rule_abstained_accuracy,
        m.rule_abstained.accuracy_on_answered
    );
    let plain = report(&cases, &outs, cases.len(), 0, true);
    assert_eq!(question_rows(&plain)[0].question, "adapter");
    assert!(question_lines(&rows).join("\n").contains("v2-position"));
}
