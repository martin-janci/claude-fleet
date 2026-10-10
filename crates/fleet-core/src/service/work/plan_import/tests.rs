//! A plan's step table read into a mission: tasks, lanes, statuses and
//! edges, a second import that brings them in line, and the table's
//! refusals before anything is written.

use super::*;
use crate::service::work::missions::{save, MissionInput};
use crate::service::work::orchestrate;

fn store() -> Mutex<Store> {
    Mutex::new(Store::open_in_memory().unwrap())
}

fn scope() -> ViewScope {
    ViewScope::internal()
}

fn mission(st: &Mutex<Store>, mode: &str) -> crate::store::MissionRow {
    save(
        &WorkLinkArgs {
            action: "mission_save".into(),
            mission: Some(MissionInput {
                name: Some("Orbit Fleet".into()),
                goal: Some("the redesign".into()),
                mode: Some(mode.into()),
                ..Default::default()
            }),
            ..Default::default()
        },
        st,
        &scope(),
    )
    .unwrap()
}

fn row(step: &str, title: &str, lane: &str, needs: &[&str]) -> PlanRow {
    PlanRow {
        step: step.into(),
        title: title.into(),
        lane: Some(lane.into()),
        needs: needs.iter().map(|n| n.to_string()).collect(),
        status: None,
        project_id: None,
    }
}

fn import_rows(st: &Mutex<Store>, id: i64, plan: Vec<PlanRow>) -> Result<PlanImport, IpcError> {
    import(
        &WorkLinkArgs {
            action: "mission_import".into(),
            mission_id: Some(id),
            plan: Some(plan),
            ..Default::default()
        },
        st,
        &scope(),
    )
}

fn members(st: &Mutex<Store>, id: i64) -> HashMap<String, WorkItemRow> {
    let s = lock(st).unwrap();
    s.mission_items(id)
        .unwrap()
        .into_iter()
        .filter_map(|i| step_of(&i.title).map(|k| (k.to_string(), i.clone())))
        .collect()
}

fn deps_of(st: &Mutex<Store>, item: i64) -> Vec<i64> {
    let s = lock(st).unwrap();
    s.item_deps(&[item])
        .unwrap()
        .into_iter()
        .map(|d| d.depends_on)
        .collect()
}

fn plan() -> Vec<PlanRow> {
    vec![
        row("0.3", "Shortcut registry", "B", &[]),
        row("3.1", "Layout switch", "B", &["0.3"]),
        row("4.1", "Account pills", "E", &["3.1", "9.9"]),
        PlanRow {
            status: Some("done".into()),
            ..row("0.5", "Tokens", "C", &[])
        },
    ]
}

#[test]
fn a_plan_becomes_tasks_with_lanes_statuses_and_edges() {
    let st = store();
    let m = mission(&st, "plan");
    let out = import_rows(&st, m.id, plan()).unwrap();
    assert_eq!((out.created, out.updated, out.deps_added), (4, 0, 2));
    assert_eq!(out.unknown_needs, vec!["4.1 needs 9.9".to_string()]);
    let got = members(&st, m.id);
    assert_eq!(got["3.1"].title, "3.1 Layout switch");
    assert_eq!(got["3.1"].assignees, vec!["B".to_string()]);
    assert_eq!(got["3.1"].parent_id, m.root_item_id);
    assert_eq!(got["0.5"].status_category, "done");
    assert_eq!(deps_of(&st, got["3.1"].id), vec![got["0.3"].id]);
    assert_eq!(deps_of(&st, got["4.1"].id), vec![got["3.1"].id]);
}

#[test]
fn importing_again_brings_the_tasks_in_line_without_copies() {
    let st = store();
    let m = mission(&st, "plan");
    import_rows(&st, m.id, plan()).unwrap();
    let again = import_rows(&st, m.id, plan()).unwrap();
    assert_eq!((again.created, again.updated, again.unchanged), (0, 0, 4));
    assert_eq!((again.deps_added, again.deps_removed), (0, 0));

    // 3.1 moves lane, is done, and stops needing 0.3 but needs 0.5.
    let mut next = plan();
    next[1] = PlanRow {
        status: Some("done".into()),
        ..row("3.1", "Layout switch and rail", "A", &["0.5"])
    };
    let out = import_rows(&st, m.id, next).unwrap();
    assert_eq!((out.created, out.updated), (0, 1));
    assert_eq!((out.deps_added, out.deps_removed), (1, 1));
    let got = members(&st, m.id);
    assert_eq!(got.len(), 4);
    assert_eq!(got["3.1"].title, "3.1 Layout switch and rail");
    assert_eq!(got["3.1"].assignees, vec!["A".to_string()]);
    assert_eq!(got["3.1"].status_category, "done");
    assert_eq!(deps_of(&st, got["3.1"].id), vec![got["0.5"].id]);
}

#[test]
fn a_plan_holds_more_than_the_loop_runs() {
    let st = store();
    let m = mission(&st, "plan");
    let rows: Vec<PlanRow> = (1..=60)
        .map(|i| row(&format!("1.{i}"), "Step", "L", &[]))
        .collect();
    assert_eq!(import_rows(&st, m.id, rows.clone()).unwrap().created, 60);

    let finite = mission(&st, "finite");
    let e = import_rows(&st, finite.id, rows).unwrap_err();
    assert!(e.message.contains("of 30 items"), "{}", e.message);
    assert!(members(&st, finite.id).is_empty(), "nothing written");
}

#[test]
fn the_loop_never_runs_a_plan() {
    let st = store();
    let m = mission(&st, "plan");
    import_rows(&st, m.id, plan()).unwrap();
    let s = lock(&st).unwrap();
    s.set_mission_state(m.id, None, "active", "fleet").unwrap();
    let m = s.get_mission(m.id).unwrap().unwrap();
    assert!(orchestrate::plan_for(&s, &m).unwrap().is_none());
    drop(s);
    let e = orchestrate::grant(
        &WorkLinkArgs {
            action: "mission_grant".into(),
            mission_id: Some(m.id),
            level: Some(1),
            ..Default::default()
        },
        &st,
        &scope(),
    )
    .unwrap_err();
    assert!(e.message.contains("is a plan"), "{}", e.message);
}

#[test]
fn a_plan_too_big_for_the_loop_cannot_become_finite() {
    let st = store();
    let m = mission(&st, "plan");
    let rows: Vec<PlanRow> = (1..=31)
        .map(|i| row(&format!("1.{i}"), "Step", "L", &[]))
        .collect();
    import_rows(&st, m.id, rows).unwrap();
    let e = save(
        &WorkLinkArgs {
            action: "mission_save".into(),
            mission_id: Some(m.id),
            mission: Some(MissionInput {
                mode: Some("finite".into()),
                ..Default::default()
            }),
            ..Default::default()
        },
        &st,
        &scope(),
    )
    .unwrap_err();
    assert!(e.message.contains("at most 30"), "{}", e.message);
}

#[test]
fn the_table_is_checked_before_anything_is_written() {
    let bad = |rows: Vec<PlanRow>| check_plan(&rows).unwrap_err().message;
    assert!(bad(vec![]).contains("at least one row"));
    assert!(bad(vec![row("3 1", "x", "B", &[])]).contains("step id"));
    assert!(bad(vec![row("abc", "x", "B", &[])]).contains("step id"));
    assert!(bad(vec![row("1.1", "x", "B", &[]), row("1.1", "y", "B", &[])]).contains("twice"));
    assert!(bad(vec![row("1.1", " ", "B", &[])]).contains("no title"));
    assert!(bad(vec![row("1.1", "x", "B", &["1.1"])]).contains("needs itself"));
    assert!(bad(vec![
        row("1.1", "x", "B", &["1.3"]),
        row("1.2", "y", "B", &["1.1"]),
        row("1.3", "z", "B", &["1.2"]),
    ])
    .contains("circle"));
    let st = PlanRow {
        status: Some("blocked".into()),
        ..row("1.1", "x", "B", &[])
    };
    assert!(bad(vec![st]).contains("a status is one of"));
    // A need outside the table is not a cycle and not an error.
    assert!(check_plan(&[row("1.1", "x", "B", &["9.9"])]).is_ok());
}

#[test]
fn a_step_id_leads_the_title() {
    assert_eq!(step_of("3.1 Layout switch"), Some("3.1"));
    assert_eq!(step_of("M14.2 Phone rows"), Some("M14.2"));
    assert_eq!(step_of("Payments v2"), None);
    assert_eq!(step_of("3.1"), None);
}

#[test]
fn a_row_names_the_repository_its_new_task_works_in() {
    let st = store();
    let m = mission(&st, "plan");
    let pid = st
        .lock()
        .unwrap()
        .upsert_project("acme", "api", "/src/api")
        .unwrap();
    let plan = vec![
        PlanRow {
            project_id: Some(pid),
            ..row("1.1", "Schema", "A", &[])
        },
        row("1.2", "Docs", "A", &["1.1"]),
    ];
    import_rows(&st, m.id, plan).unwrap();
    let got = members(&st, m.id);
    assert_eq!(got["1.1"].project_id, Some(pid));
    assert_eq!(got["1.2"].project_id, None);
}

#[test]
fn a_repository_that_is_not_there_refuses_the_whole_table() {
    let st = store();
    let m = mission(&st, "plan");
    let plan = vec![
        row("1.1", "Schema", "A", &[]),
        PlanRow {
            project_id: Some(9999),
            ..row("1.2", "Docs", "A", &[])
        },
    ];
    let err = import_rows(&st, m.id, plan).unwrap_err();
    assert_eq!(err.code, codes::E_NOTFOUND);
    assert!(err.message.contains("step 1.2"), "{}", err.message);
    assert!(members(&st, m.id).is_empty(), "nothing was written");
}
