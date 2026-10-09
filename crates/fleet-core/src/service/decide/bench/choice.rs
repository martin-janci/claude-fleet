//! The closed-choice benches (`fleet-hub decide bench <use case>`): one
//! labelled set per Jev use case that picks one option out of a few —
//! control_route (K2), duplicate (K4), related_session (N1),
//! work_placement (K5), host_placement (N5), routine_run_outcome (N6),
//! adopt_target (N4), restore_target (J10), main_ticket (J6) and
//! tracker_duplicate (J7).
//!
//! **A case** is one JSON line: `{id, input, label, by?, never?, trap?}`.
//! `input` is the use case's own facts (see each `prepare_*` below);
//! `label` is the right option, in the use case's words, or `unsure` where
//! nothing should be pre-selected. `by` asserts who answers: `rule` (the
//! rule layer decides, nobody is asked) or `jev` (the rules leave it to the
//! model). `never` lists options that must never be the answer (a host over
//! its limit, another org's task, a failed run read as `nothing`); `trap`
//! names the never-list target (`src/lib/ai_proposal.ts`'s
//! `NEVER_DECIDES`) a case baits. Lines starting with `#` are comments; a
//! `# synthetic` line marks the whole set as synthetic.
//!
//! **Each case is prepared through the use case's own code**: the same
//! rule functions and the same request builders the live adapter calls
//! (`control_route::unclear`, `duplicate::rank`, `related_session::eligible`,
//! `host_placement::candidates`, `routines::outcome::rule_outcome`, the Lost
//! and found path rule, …), so a case the rules decide never reaches a
//! model, and a case that reaches one sends exactly what the live adapter
//! would send.
//!
//! **Providers.** `rule`: the rule layer's answer, an abstention where it
//! leaves the case to the model. `baseline`: what fleet does without Jev
//! (the rule, else the live adapter's shadow baseline: `control`, `none`,
//! the first host, `did_work`, `unsure`). `jev`: the rule, else the model
//! through [`decide`] (subject `bench`, the use case's floor; nothing is
//! sent for a rule case).
//!
//! **Acceptance** (each use case alike, `docs/decisions.md`): proposal
//! precision ≥ [`ACCEPT_PRECISION`] over the answers that would show
//! something, judged from [`JUDGE_MIN_LABELLED`] labelled cases the rules
//! leave to the model; at most [`ACCEPT_UNSURE_PRESELECT`] of the
//! `unsure`-labelled cases pre-selected, judged from
//! [`JUDGE_MIN_UNSURE`]; and no answer on a case's `never` list. A
//! synthetic set is never judged: every verdict is NOT JUDGED.

use super::{gate_or_skip, pct, Verdict};
use crate::service::decide::{
    control_route, duplicate, host_placement, lost_target, main_ticket, related_session,
    routine_run_outcome, start_project, tracker_duplicate, turn_outcome, work_placement,
};
use crate::service::decide::{decide, DecideCtx, DecideRequest, Fallback, Feature, JevRequest};
use crate::store::{Secret, SessionRow, DECISION_BENCH_SUBJECT};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// The acceptance lines.
pub const ACCEPT_PRECISION: f64 = 0.9;
pub const ACCEPT_UNSURE_PRESELECT: f64 = 0.05;
/// Labelled cases (not `unsure`) the rules leave to the model, needed to
/// judge precision.
pub const JUDGE_MIN_LABELLED: u64 = 50;
/// `unsure`-labelled cases needed to judge the pre-select line.
pub const JUDGE_MIN_UNSURE: u64 = 20;
pub const DEFAULT_MAX_CALLS: usize = 500;
/// The option that proposes nothing, in every use case here.
pub const UNSURE: &str = "unsure";

/// A use case with a closed-choice bench.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UseCase {
    ControlRoute,
    Duplicate,
    RelatedSession,
    WorkPlacement,
    HostPlacement,
    RoutineRunOutcome,
    AdoptTarget,
    RestoreTarget,
    MainTicket,
    TrackerDuplicate,
}

macro_rules! fixture {
    ($f:literal) => {
        (
            include_str!(concat!("../../testdata/decide/", $f)),
            concat!("crates/fleet-core/src/service/testdata/decide/", $f),
        )
    };
}

impl UseCase {
    pub const ALL: &[UseCase] = &[
        UseCase::ControlRoute,
        UseCase::Duplicate,
        UseCase::RelatedSession,
        UseCase::WorkPlacement,
        UseCase::HostPlacement,
        UseCase::RoutineRunOutcome,
        UseCase::AdoptTarget,
        UseCase::RestoreTarget,
        UseCase::MainTicket,
        UseCase::TrackerDuplicate,
    ];

    pub fn feature(self) -> Feature {
        match self {
            UseCase::ControlRoute => Feature::ControlRoute,
            UseCase::Duplicate => Feature::Duplicate,
            UseCase::RelatedSession => Feature::RelatedSession,
            UseCase::WorkPlacement => Feature::WorkPlacement,
            UseCase::HostPlacement => Feature::HostPlacement,
            UseCase::RoutineRunOutcome => Feature::RoutineRunOutcome,
            UseCase::AdoptTarget => Feature::AdoptTarget,
            UseCase::RestoreTarget => Feature::RestoreTarget,
            UseCase::MainTicket => Feature::MainTicket,
            UseCase::TrackerDuplicate => Feature::TrackerDuplicate,
        }
    }

    /// `control_route`.
    pub fn name(self) -> &'static str {
        self.feature().as_str()
    }

    /// The subcommand: `control-route`.
    pub fn command(self) -> String {
        self.name().replace('_', "-")
    }

    pub fn parse(s: &str) -> Option<UseCase> {
        let s = s.replace('-', "_");
        UseCase::ALL.iter().copied().find(|u| u.name() == s)
    }

    /// The test map's / plan's card.
    pub fn card(self) -> &'static str {
        match self {
            UseCase::ControlRoute => "K2",
            UseCase::Duplicate => "K4",
            UseCase::RelatedSession => "N1",
            UseCase::WorkPlacement => "K5",
            UseCase::HostPlacement => "N5",
            UseCase::RoutineRunOutcome => "N6",
            UseCase::AdoptTarget => "N4",
            UseCase::RestoreTarget => "J10",
            UseCase::MainTicket => "J6",
            UseCase::TrackerDuplicate => "J7",
        }
    }

    /// The built-in set and its path in the repository.
    fn fixture_pair(self) -> (&'static str, &'static str) {
        match self {
            UseCase::ControlRoute => fixture!("control_route_cases.jsonl"),
            UseCase::Duplicate => fixture!("duplicate_cases.jsonl"),
            UseCase::RelatedSession => fixture!("related_session_cases.jsonl"),
            UseCase::WorkPlacement => fixture!("work_placement_cases.jsonl"),
            UseCase::HostPlacement => fixture!("host_placement_cases.jsonl"),
            UseCase::RoutineRunOutcome => fixture!("routine_run_outcome_cases.jsonl"),
            UseCase::AdoptTarget => fixture!("adopt_target_cases.jsonl"),
            UseCase::RestoreTarget => fixture!("restore_target_cases.jsonl"),
            UseCase::MainTicket => fixture!("main_ticket_cases.jsonl"),
            UseCase::TrackerDuplicate => fixture!("tracker_duplicate_cases.jsonl"),
        }
    }

    pub fn fixture(self) -> &'static str {
        self.fixture_pair().0
    }

    pub fn fixture_path(self) -> &'static str {
        self.fixture_pair().1
    }

    /// What the bench's Jev runs are recorded under.
    pub fn question_version(self) -> String {
        format!("{}.bench.v1", self.name())
    }

    /// Answers that show nothing (they are right or wrong, but never a
    /// proposal a person sees).
    pub fn quiet(self) -> &'static [&'static str] {
        match self {
            UseCase::ControlRoute => &[control_route::CONTROL],
            UseCase::Duplicate
            | UseCase::RelatedSession
            | UseCase::WorkPlacement
            | UseCase::TrackerDuplicate => &["none"],
            _ => &[],
        }
    }

    /// The live adapter's confidence floor.
    pub fn min_confidence(self) -> f64 {
        match self {
            UseCase::ControlRoute => control_route::MIN_CONFIDENCE,
            UseCase::Duplicate => duplicate::MIN_CONFIDENCE,
            UseCase::RelatedSession => related_session::MIN_CONFIDENCE,
            UseCase::WorkPlacement => work_placement::MIN_CONFIDENCE,
            UseCase::HostPlacement => host_placement::MIN_CONFIDENCE,
            UseCase::RoutineRunOutcome => routine_run_outcome::MIN_CONFIDENCE,
            UseCase::AdoptTarget | UseCase::RestoreTarget => lost_target::MIN_CONFIDENCE,
            UseCase::MainTicket => main_ticket::MIN_CONFIDENCE,
            UseCase::TrackerDuplicate => tracker_duplicate::MIN_CONFIDENCE,
        }
    }
}

/// What the use case's own code makes of a case's input.
#[derive(Debug, Clone, PartialEq)]
pub enum Prepared {
    /// The rule layer answers (an option, or `unsure`: nothing
    /// pre-selected); nobody is asked.
    Rule(String),
    /// The model would be asked `request`; `options` are the answers that
    /// name something, `baseline` what fleet does without it.
    Ask {
        request: JevRequest,
        options: Vec<String>,
        baseline: String,
    },
}

/// One labelled case, prepared.
#[derive(Debug, Clone, PartialEq)]
pub struct Case {
    pub id: String,
    /// The right option word (an alias already resolved), or `unsure`.
    pub label: String,
    /// `rule` / `jev`, when the case says who should answer.
    pub by: Option<String>,
    pub never: Vec<String>,
    pub trap: Option<String>,
    pub prepared: Prepared,
}

impl Case {
    pub fn by_rule(&self) -> bool {
        matches!(self.prepared, Prepared::Rule(_))
    }
}

/// A use case's labelled set.
#[derive(Debug, Clone, PartialEq)]
pub struct CaseSet {
    pub use_case: UseCase,
    /// A `# synthetic` line: written for the repository, not recorded.
    pub synthetic: bool,
    pub cases: Vec<Case>,
}

#[derive(Debug, Deserialize)]
struct RawCase {
    id: String,
    input: Value,
    label: String,
    #[serde(default)]
    by: Option<String>,
    #[serde(default)]
    never: Vec<String>,
    #[serde(default)]
    trap: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    note: Option<String>,
}

/// PURE: the cases of a JSONL set for `uc`, each prepared. Refused with
/// its line number: a line that does not parse, an input the use case
/// cannot read, a label that is no answer of the case, a `by` the rules
/// contradict, a duplicate id.
pub fn parse(uc: UseCase, text: &str) -> Result<CaseSet, String> {
    let mut synthetic = false;
    let mut cases: Vec<Case> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let n = i + 1;
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if let Some(c) = t.strip_prefix('#') {
            if c.trim().to_ascii_lowercase().starts_with("synthetic") {
                synthetic = true;
            }
            continue;
        }
        match parse_case(uc, t, &cases) {
            Ok(c) => cases.push(c),
            Err(e) => errors.push(format!("line {n}: {e}")),
        }
    }
    if !errors.is_empty() {
        return Err(errors.join("; "));
    }
    Ok(CaseSet {
        use_case: uc,
        synthetic,
        cases,
    })
}

/// One case line, checked against the cases before it.
fn parse_case(uc: UseCase, t: &str, before: &[Case]) -> Result<Case, String> {
    let raw: RawCase = serde_json::from_str(t).map_err(|e| e.to_string())?;
    if before.iter().any(|c| c.id == raw.id) {
        return Err(format!("the id {:?} is used twice", raw.id));
    }
    let prepared = prepare(uc, &raw.input)?;
    let label = label_aliases(uc, &raw.input)
        .into_iter()
        .find(|(l, _)| *l == raw.label)
        .map(|(_, o)| o)
        .unwrap_or(raw.label.clone());
    let known = match &prepared {
        Prepared::Rule(a) => label == *a || label == UNSURE || label_word(uc, &label),
        Prepared::Ask { options, .. } => {
            label == UNSURE || options.contains(&label) || label_word(uc, &label)
        }
    };
    if !known {
        return Err(format!(
            "the label {:?} is no answer of this case",
            raw.label
        ));
    }
    match (raw.by.as_deref(), &prepared) {
        (Some("rule"), Prepared::Ask { .. }) => {
            return Err("marked by: rule, but the rules leave it to the model".into())
        }
        (Some("jev"), Prepared::Rule(a)) => {
            return Err(format!("marked by: jev, but the rules answer {a:?}"))
        }
        (Some(b), _) if b != "rule" && b != "jev" => {
            return Err(format!("by is rule or jev, not {b:?}"))
        }
        _ => {}
    }
    Ok(Case {
        id: raw.id,
        label,
        by: raw.by,
        never: raw.never,
        trap: raw.trap,
        prepared,
    })
}

/// PURE: a label a case may carry although no option of its own names it:
/// routine_run_outcome's `failed` (the exit's answer, never the model's).
fn label_word(uc: UseCase, label: &str) -> bool {
    match uc {
        UseCase::RoutineRunOutcome => crate::store::ROUTINE_RUN_OUTCOMES.contains(&label),
        UseCase::ControlRoute => label == control_route::CONTROL,
        _ => false,
    }
}

/// PURE: labels a case may give in a person's words, each with its option
/// word: work_placement's group labels, main_ticket's keys.
fn label_aliases(uc: UseCase, input: &Value) -> Vec<(String, String)> {
    let strs = |k: &str| -> Vec<String> {
        input
            .get(k)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect()
    };
    match uc {
        UseCase::WorkPlacement => {
            let fp = bench_fp_key();
            let mut labels = strs("placements");
            labels.extend(strs("rules"));
            labels.extend(opt_text(input, "placed"));
            labels
                .into_iter()
                .map(|l| {
                    let o = work_placement::option_of(&fp, l.trim());
                    (l, o)
                })
                .collect()
        }
        UseCase::MainTicket => input
            .get("keys")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
            .filter_map(|(i, k)| {
                let key = k.get("key")?.as_str()?.to_string();
                Some((key, main_ticket::option_of(i as i64 + 1)))
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// The built-in set of `uc`.
pub fn fixture(uc: UseCase) -> Result<CaseSet, String> {
    parse(uc, uc.fixture())
}

// --- preparing a case: each use case's own code ------------------------------------

fn field<'a>(v: &'a Value, k: &str) -> Result<&'a Value, String> {
    v.get(k).ok_or_else(|| format!("input.{k} is missing"))
}

fn text(v: &Value, k: &str) -> Result<String, String> {
    field(v, k)?
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| format!("input.{k} is not text"))
}

fn opt_text(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_string)
}

fn list<'a>(v: &'a Value, k: &str) -> Result<&'a Vec<Value>, String> {
    field(v, k)?
        .as_array()
        .ok_or_else(|| format!("input.{k} is not a list"))
}

fn int(v: &Value, k: &str) -> Result<i64, String> {
    field(v, k)?
        .as_i64()
        .ok_or_else(|| format!("input.{k} is not a whole number"))
}

/// `base` with `over`'s keys laid on top (objects only).
fn merged(base: Value, over: Option<&Value>) -> Value {
    let mut b = base;
    if let (Some(bo), Some(Value::Object(o))) = (b.as_object_mut(), over) {
        for (k, v) in o {
            bo.insert(k.clone(), v.clone());
        }
    }
    b
}

/// A session row from a case's partial one: a running Claude session
/// unless the case says otherwise.
fn session_row(over: Option<&Value>) -> Result<SessionRow, String> {
    let base = json!({
        "id": 1, "row_version": 0, "prompt_submit_seq": 0, "tmux_name": "t",
        "host_alias": "local", "created_at": 0, "last_activity_at": 0,
        "status": "running", "kind": "work", "agent": "claude", "turn_seq": 0,
        "visibility": "private",
    });
    serde_json::from_value(merged(base, over)).map_err(|e| format!("a session: {e}"))
}

fn work_item(over: &Value) -> Result<crate::store::WorkItemRow, String> {
    let base = json!({ "source": "local", "title": "", "created_at": 0, "updated_at": 0 });
    serde_json::from_value(merged(base, Some(over))).map_err(|e| format!("a task: {e}"))
}

/// PURE: what `uc`'s own code makes of `input`.
pub fn prepare(uc: UseCase, input: &Value) -> Result<Prepared, String> {
    match uc {
        UseCase::ControlRoute => prepare_control_route(input),
        UseCase::Duplicate => prepare_duplicate(input),
        UseCase::RelatedSession => prepare_related_session(input),
        UseCase::WorkPlacement => prepare_work_placement(input),
        UseCase::HostPlacement => prepare_host_placement(input),
        UseCase::RoutineRunOutcome => prepare_routine_run_outcome(input),
        UseCase::AdoptTarget => prepare_lost(input, lost_target::LostKind::Pane),
        UseCase::RestoreTarget => prepare_lost(input, lost_target::LostKind::Transcript),
        UseCase::MainTicket => prepare_main_ticket(input),
        UseCase::TrackerDuplicate => prepare_tracker_duplicate(input),
    }
}

/// K2: `{message, targets: [{kind: mission|session, id, name, detail}]}`
/// (the targets a person may see, in `control_route::targets`' order).
fn prepare_control_route(input: &Value) -> Result<Prepared, String> {
    use control_route::{Target, TargetKind};
    let message = text(input, "message")?;
    let mut targets = Vec::new();
    for t in list(input, "targets")? {
        let kind = match t.get("kind").and_then(Value::as_str) {
            Some("mission") => TargetKind::Mission,
            Some("session") => TargetKind::Session,
            k => return Err(format!("a target's kind is mission or session, not {k:?}")),
        };
        targets.push(Target {
            kind,
            id: int(t, "id")?,
            name: text(t, "name")?,
            detail: opt_text(t, "detail").unwrap_or_default(),
            org_id: None,
        });
    }
    targets.truncate(control_route::MAX_TARGETS);
    // `propose`'s order: a command or no target shows nothing; a short
    // message makes Control ask.
    if control_route::is_command(&message) || message.trim().is_empty() || targets.is_empty() {
        return Ok(Prepared::Rule(control_route::CONTROL.into()));
    }
    if control_route::unclear(&message) {
        return Ok(Prepared::Rule(UNSURE.into()));
    }
    let mut options: Vec<String> = targets.iter().map(Target::option).collect();
    options.push(control_route::CONTROL.into());
    Ok(Prepared::Ask {
        request: control_route::question_for(&message, &targets),
        options,
        baseline: control_route::CONTROL.into(),
    })
}

/// K4: `{proposed: {id, title, why?, parent_id?}, org?, open: [{id, title,
/// key?, org?}]}` — `open` are the org-blind open tasks; `duplicate::rank`
/// keeps the same org's, sharing a title word.
fn prepare_duplicate(input: &Value) -> Result<Prepared, String> {
    let p = field(input, "proposed")?;
    let mut proposed = work_item(p)?;
    proposed.proposal_why = opt_text(p, "why");
    let org = input.get("org").and_then(Value::as_i64);
    let mut open = Vec::new();
    let mut org_of: BTreeMap<i64, Option<i64>> = BTreeMap::new();
    for o in list(input, "open")? {
        let row = work_item(o)?;
        org_of.insert(row.id, o.get("org").and_then(Value::as_i64));
        open.push(row);
    }
    let found = duplicate::rank(&proposed, open, |i| {
        org_of.get(&i.id).copied().flatten() == org
    });
    if found.is_empty() {
        return Ok(Prepared::Rule(duplicate::NONE_OPTION.into()));
    }
    let mut options: Vec<String> = found.iter().map(|i| duplicate::option_id(i.id)).collect();
    options.push(duplicate::NONE_OPTION.into());
    Ok(Prepared::Ask {
        request: duplicate::request(&proposed, &found),
        options,
        baseline: duplicate::NONE_OPTION.into(),
    })
}

/// N1: `{session: {<session row fields>}, prompt, others: [{session:
/// {...}, prompt?}]}` — `related_session::eligible` drops another person's,
/// a gone one, another org's and one sharing the worktree.
fn prepare_related_session(input: &Value) -> Result<Prepared, String> {
    let row = session_row(input.get("session"))?;
    let prompt = text(input, "prompt")?;
    let mut found: Vec<(i64, String)> = Vec::new();
    for o in list(input, "others")? {
        let other = session_row(o.get("session"))?;
        let Some(p) = opt_text(o, "prompt").filter(|p| !p.trim().is_empty()) else {
            continue;
        };
        if related_session::eligible(&row, &other) {
            found.push((other.id, related_session::cut(&p)));
        }
    }
    found.truncate(related_session::MAX_CANDIDATES);
    if found.is_empty() {
        return Ok(Prepared::Rule(related_session::NONE_OPTION.into()));
    }
    let mut options: Vec<String> = found
        .iter()
        .map(|(id, _)| related_session::option_of(*id))
        .collect();
    options.push(related_session::NONE_OPTION.into());
    Ok(Prepared::Ask {
        request: related_session::request(&related_session::cut(&prompt), &found),
        options,
        baseline: related_session::NONE_OPTION.into(),
    })
}

/// The fingerprint key the bench's group options are made under (a
/// constant: the options only need to be stable within one run).
fn bench_fp_key() -> Secret {
    Secret::new("decide-bench-work-placement")
}

/// K5: `{title, key?, subtask?, placed?: <a person's or rule's group>,
/// placements: [<group label of each person's placement>], rules: [<an
/// enabled rule's group>]}`. A label is a group's label (or `none` /
/// `unsure`).
fn prepare_work_placement(input: &Value) -> Result<Prepared, String> {
    let title = text(input, "title")?;
    let key = opt_text(input, "key");
    let fp = bench_fp_key();
    if input.get("subtask").and_then(Value::as_bool) == Some(true) {
        return Ok(Prepared::Rule(work_placement::NONE_OPTION.into()));
    }
    if let Some(placed) = opt_text(input, "placed") {
        return Ok(Prepared::Rule(work_placement::option_of(&fp, &placed)));
    }
    let placements: Vec<crate::store::Placement> = input
        .get("placements")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(i, g)| {
            Some(crate::store::Placement {
                task_id: format!("item:{}", 10_000 + i),
                group: Some(g.as_str()?.to_string()),
                note: None,
                version: 1,
                updated_at: 0,
                updated_by: None,
            })
        })
        .collect();
    let rules: Vec<crate::store::WorkRule> = input
        .get("rules")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(i, g)| {
            Some(crate::store::WorkRule {
                id: i as i64 + 1,
                name: format!("rule {}", i + 1),
                enabled: true,
                version: 1,
                conditions: Default::default(),
                group: g.as_str()?.to_string(),
                created_at: 0,
                updated_at: 0,
            })
        })
        .collect();
    let labels = work_placement::groups(&placements, &rules);
    if labels.is_empty() {
        return Ok(Prepared::Rule(work_placement::NONE_OPTION.into()));
    }
    let pairs: Vec<(String, String)> = labels
        .into_iter()
        .map(|l| (work_placement::option_of(&fp, &l), l))
        .collect();
    let mut options: Vec<String> = pairs.iter().map(|(o, _)| o.clone()).collect();
    options.push(work_placement::NONE_OPTION.into());
    Ok(Prepared::Ask {
        request: work_placement::request(&title, key.as_deref(), &pairs),
        options,
        baseline: work_placement::NONE_OPTION.into(),
    })
}

/// N5: `{project: "owner/repo", org?, remembered?, pause_at_pct?,
/// disk_low_pct?, hosts: [{alias, reachable, hidden?, org?,
/// account_used_pct?, disk_free_pct?, latency_ms?, recent_starts?,
/// checked_out?, live_sessions?}]}` — through `host_placement::candidates`
/// (online, the project's org, not past its account limit) and
/// `past_disk_rule`.
fn prepare_host_placement(input: &Value) -> Result<Prepared, String> {
    use crate::service::account_usage::{
        AccountUsage, AccountUsageSnapshot, UsageOutcomeKind, Window,
    };
    const NOW: i64 = 1_790_510_400;
    let project = text(input, "project")?;
    let (owner, repo) = project
        .split_once('/')
        .ok_or_else(|| "input.project is owner/repo".to_string())?;
    let org = input.get("org").and_then(Value::as_i64);
    let pause_at = input
        .get("pause_at_pct")
        .and_then(Value::as_f64)
        .unwrap_or(90.0);
    let disk_low = input
        .get("disk_low_pct")
        .and_then(Value::as_u64)
        .unwrap_or(90) as u8;
    let mut hosts = Vec::new();
    let mut usage = Vec::new();
    let mut counts: BTreeMap<String, (i64, i64, bool)> = BTreeMap::new();
    let mut host_org: BTreeMap<String, Option<i64>> = BTreeMap::new();
    for h in list(input, "hosts")? {
        let alias = text(h, "alias")?;
        let used = h.get("account_used_pct").and_then(Value::as_f64);
        let account = used.map(|_| format!("acct-{alias}"));
        let free = h.get("disk_free_pct").and_then(Value::as_i64);
        let row: crate::store::HostRow = serde_json::from_value(json!({
            "alias": alias, "reachable": h.get("reachable").and_then(Value::as_bool).unwrap_or(true),
            "hidden": h.get("hidden").and_then(Value::as_bool).unwrap_or(false),
            "account_uuid": account, "provisioned": true, "transport": "ssh",
            "disk_home_total_kb": free.map(|_| 1_000_000), "disk_home_free_kb": free.map(|f| f * 10_000),
            "latency_ms": h.get("latency_ms").and_then(Value::as_i64),
        }))
        .map_err(|e| format!("a host: {e}"))?;
        if let (Some(a), Some(u)) = (account, used) {
            usage.push(AccountUsageSnapshot {
                account_uuid: a,
                usage: Some(AccountUsage {
                    five_hour: Some(Window {
                        utilization: u,
                        resets_at: Some(NOW + 3_600),
                    }),
                    seven_day: None,
                    seven_day_opus: None,
                    seven_day_sonnet: None,
                }),
                subscription: None,
                fetched_at: Some(NOW - 60),
                source_host: None,
                status: UsageOutcomeKind::Ok,
                detail: None,
                next_try_at: 0,
            });
        }
        counts.insert(
            alias.clone(),
            (
                h.get("live_sessions").and_then(Value::as_i64).unwrap_or(0),
                h.get("recent_starts").and_then(Value::as_i64).unwrap_or(0),
                h.get("checked_out")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            ),
        );
        host_org.insert(alias, h.get("org").and_then(Value::as_i64));
        hosts.push(row);
    }
    let mut candidates = host_placement::candidates(
        &hosts,
        &usage,
        pause_at,
        &counts,
        org,
        |a| host_org.get(a).copied().flatten(),
        NOW,
    );
    candidates.retain(|c| !host_placement::past_disk_rule(c, disk_low));
    if let Some(r) = opt_text(input, "remembered") {
        if candidates.iter().any(|c| c.alias == r) {
            return Ok(Prepared::Rule(host_placement::option_of(&r)));
        }
    }
    match candidates.len() {
        0 => return Ok(Prepared::Rule(UNSURE.into())),
        1 => {
            return Ok(Prepared::Rule(host_placement::option_of(
                &candidates[0].alias,
            )))
        }
        _ => {}
    }
    let options: Vec<String> = candidates
        .iter()
        .map(|c| host_placement::option_of(&c.alias))
        .collect();
    let baseline = options[0].clone();
    let input = host_placement::PlacementInput {
        project_id: 1,
        owner: owner.to_string(),
        repo: repo.to_string(),
        org_id: org,
        candidates,
    };
    Ok(Prepared::Ask {
        request: host_placement::question_for(&input),
        options,
        baseline,
    })
}

/// N6: `{run: {state, …}, session?: {<session row fields>}, screen}` —
/// `routines::outcome::rule_outcome` first (a failed exit, an open
/// question, a wedged REPL, J2's asked/stuck, a pull request).
fn prepare_routine_run_outcome(input: &Value) -> Result<Prepared, String> {
    let run: crate::store::RoutineRunRow = serde_json::from_value(merged(
        json!({ "id": 1, "routine_id": 1, "trigger": "cron", "state": "done", "started_at": 0 }),
        input.get("run"),
    ))
    .map_err(|e| format!("a run: {e}"))?;
    let row = match input.get("session") {
        Some(Value::Null) | None => None,
        Some(s) => Some(session_row(Some(s))?),
    };
    if let Some((o, _)) = crate::service::routines::outcome::rule_outcome(&run, row.as_ref()) {
        return Ok(Prepared::Rule(o.as_str().into()));
    }
    let screen = turn_outcome::prepare_tail(&text(input, "screen")?);
    Ok(Prepared::Ask {
        request: routine_run_outcome::request(&screen),
        options: vec!["did_work".into(), "nothing".into(), "needs_person".into()],
        baseline: routine_run_outcome::BASELINE.into(),
    })
}

/// N4 / J10: `{cwd, branch?, name?, projects: [{id, owner, repo,
/// base_path, consented?}]}` — the Lost and found path rule (a directory
/// inside a project is that project), then the projects whose org
/// consented (`consented: false` stays home, review r15).
fn prepare_lost(input: &Value, kind: lost_target::LostKind) -> Result<Prepared, String> {
    let cwd = text(input, "cwd")?;
    let mut rows = Vec::new();
    let mut fenced = Vec::new();
    for p in list(input, "projects")? {
        let row: crate::store::ProjectRow = serde_json::from_value(json!({
            "id": int(p, "id")?, "owner": text(p, "owner")?, "repo": text(p, "repo")?,
            "base_path": text(p, "base_path")?, "adopted": false,
        }))
        .map_err(|e| format!("a project: {e}"))?;
        if p.get("consented").and_then(Value::as_bool) != Some(false) {
            fenced.push(start_project::Candidate {
                project_id: row.id,
                owner: row.owner.clone(),
                repo: row.repo.clone(),
            });
        }
        rows.push(row);
    }
    if let Some(pid) = crate::service::sessions::project_for_local_path(&rows, &cwd) {
        return Ok(Prepared::Rule(start_project::option_of(pid)));
    }
    fenced.truncate(lost_target::MAX_CANDIDATES);
    if fenced.is_empty() {
        return Ok(Prepared::Rule(UNSURE.into()));
    }
    let options: Vec<String> = fenced
        .iter()
        .map(|c| start_project::option_of(c.project_id))
        .collect();
    let input = lost_target::LostInput {
        kind,
        subject: "bench".into(),
        org_id: None,
        cwd,
        git_branch: opt_text(input, "branch"),
        name: match kind {
            lost_target::LostKind::Pane => opt_text(input, "name"),
            lost_target::LostKind::Transcript => None,
        },
        candidates: fenced,
    };
    Ok(Prepared::Ask {
        request: lost_target::question_for(&input),
        options,
        baseline: UNSURE.into(),
    })
}

/// J6: `{prompt, branch?, keys: [{key, title?}]}` — the suggested keys in
/// detection's order; options `k1`, `k2` … (the live adapter's are link
/// ids). A label may be the key itself.
fn prepare_main_ticket(input: &Value) -> Result<Prepared, String> {
    let prompt = text(input, "prompt")?;
    let keys: Vec<main_ticket::KeyCandidate> = list(input, "keys")?
        .iter()
        .enumerate()
        .map(|(i, k)| {
            Ok(main_ticket::KeyCandidate {
                option: main_ticket::option_of(i as i64 + 1),
                key: text(k, "key")?,
                title: opt_text(k, "title").unwrap_or_default(),
            })
        })
        .collect::<Result<_, String>>()?;
    if let Some(a) = main_ticket::rule(&keys, opt_text(input, "branch").as_deref()) {
        return Ok(Prepared::Rule(a));
    }
    Ok(Prepared::Ask {
        request: main_ticket::request(&prompt, &keys),
        options: keys.iter().map(|k| k.option.clone()).collect(),
        baseline: UNSURE.into(),
    })
}

/// J7: `{local: {id, title, why?}, org?, open: [{id, title, key?,
/// tracker_id?, source?, org?}]}` — tracker tickets of the same org
/// (`tracker_duplicate::candidates`), the key rule first.
fn prepare_tracker_duplicate(input: &Value) -> Result<Prepared, String> {
    let l = field(input, "local")?;
    let mut local = work_item(l)?;
    local.proposal_why = opt_text(l, "why");
    let org = input.get("org").and_then(Value::as_i64);
    let mut open = Vec::new();
    let mut org_of: BTreeMap<i64, Option<i64>> = BTreeMap::new();
    for o in list(input, "open")? {
        let row = work_item(o)?;
        org_of.insert(row.id, o.get("org").and_then(Value::as_i64));
        open.push(row);
    }
    let found = tracker_duplicate::candidates(&local, open, |i| {
        org_of.get(&i.id).copied().flatten() == org
    });
    if let Some(a) = tracker_duplicate::rule(&local, &found) {
        return Ok(Prepared::Rule(a));
    }
    let mut options: Vec<String> = found.iter().map(|i| duplicate::option_id(i.id)).collect();
    options.push(tracker_duplicate::NONE_OPTION.into());
    Ok(Prepared::Ask {
        request: tracker_duplicate::request(&local, &found),
        options,
        baseline: tracker_duplicate::NONE_OPTION.into(),
    })
}

// --- providers ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Rule,
    Baseline,
    Jev,
}

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Provider::Rule => "rule",
            Provider::Baseline => "baseline",
            Provider::Jev => "jev",
        }
    }
    pub fn parse(s: &str) -> Option<Provider> {
        [Provider::Rule, Provider::Baseline, Provider::Jev]
            .into_iter()
            .find(|p| p.as_str() == s)
    }
}

/// One provider's answer to one case: `None` is an abstention (`unsure`
/// included); `called` says whether a model was asked; `skipped` why
/// nothing was asked (a gate fallback, `max_calls`).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Answer {
    pub answer: Option<String>,
    pub called: bool,
    pub skipped: Option<String>,
}

fn word(a: &str) -> Option<String> {
    (a != UNSURE).then(|| a.to_string())
}

/// PURE: an offline provider's answer (`jev` answers only rule cases
/// offline and abstains on the rest).
pub fn offline(p: Provider, c: &Case) -> Answer {
    let answer = match (&c.prepared, p) {
        (Prepared::Rule(a), _) => word(a),
        (Prepared::Ask { baseline, .. }, Provider::Baseline) => word(baseline),
        (Prepared::Ask { .. }, _) => None,
    };
    Answer {
        answer,
        called: false,
        skipped: None,
    }
}

/// The rule where it decides, else Jev through the envelope (recorded as a
/// benchmark run); at most `max_calls` calls. A rule case sends nothing.
pub async fn run_jev(ctx: &DecideCtx, set: &CaseSet, max_calls: usize) -> Vec<Answer> {
    let uc = set.use_case;
    let mut out = Vec::with_capacity(set.cases.len());
    let mut calls = 0usize;
    for (i, c) in set.cases.iter().enumerate() {
        let (request, options, baseline) = match &c.prepared {
            Prepared::Rule(_) => {
                out.push(offline(Provider::Rule, c));
                continue;
            }
            Prepared::Ask {
                request,
                options,
                baseline,
                ..
            } => (request, options, baseline),
        };
        if let Err(r) = gate_or_skip(ctx, uc.feature(), None, calls, max_calls) {
            out.push(Answer {
                answer: None,
                called: false,
                skipped: Some(r.to_string()),
            });
            continue;
        }
        calls += 1;
        let res = decide(
            ctx,
            DecideRequest {
                feature: uc.feature(),
                subject_kind: DECISION_BENCH_SUBJECT.into(),
                subject_id: format!("bench:{}:{i}", uc.name()),
                org_id: None,
                request: request.clone(),
                baseline: Some(baseline.clone()),
                question_version: uc.question_version(),
                min_confidence: Some(uc.min_confidence()),
            },
        )
        .await;
        let skipped = res
            .fallback
            .filter(|f| *f != Fallback::LowConfidence)
            .map(|f| f.as_str().to_string());
        let answer = res
            .usable()
            .map(|a| a.value.clone())
            .filter(|v| v != UNSURE)
            // An answer outside the case's options proposes nothing.
            .filter(|v| options.contains(v));
        out.push(Answer {
            answer,
            called: true,
            skipped,
        });
    }
    out
}

// --- metrics -----------------------------------------------------------------------

/// One provider's numbers on one set.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Metrics {
    pub use_case: UseCase,
    pub provider: Provider,
    pub synthetic: bool,
    pub cases: u64,
    pub skipped: u64,
    /// Cases the rule layer answered (nobody asked).
    pub rule_decided: u64,
    /// Calls a model got.
    pub calls: u64,
    pub answered: u64,
    pub correct: u64,
    pub coverage: Option<f64>,
    pub accuracy_on_answered: Option<f64>,
    /// Answers that would show something (not `unsure`, not quiet), and the
    /// right ones among them.
    pub proposals: u64,
    pub proposals_right: u64,
    pub proposal_precision: Option<f64>,
    /// Labelled (not `unsure`) cases the rules leave to the model.
    pub labelled_for_model: u64,
    pub unsure_labelled: u64,
    /// `unsure`-labelled cases that got a proposal.
    pub preselected_on_unsure: u64,
    pub traps: u64,
    /// Answers on a case's `never` list.
    pub never_violations: u64,
    pub precision_verdict: Verdict,
    pub unsure_verdict: Verdict,
    pub never_verdict: Verdict,
}

/// PURE: the metrics of `answers` against `set` (same order).
pub fn metrics(p: Provider, set: &CaseSet, answers: &[Answer]) -> Metrics {
    let uc = set.use_case;
    let quiet = uc.quiet();
    let mut m = Metrics {
        use_case: uc,
        provider: p,
        synthetic: set.synthetic,
        cases: 0,
        skipped: 0,
        rule_decided: 0,
        calls: 0,
        answered: 0,
        correct: 0,
        coverage: None,
        accuracy_on_answered: None,
        proposals: 0,
        proposals_right: 0,
        proposal_precision: None,
        labelled_for_model: 0,
        unsure_labelled: 0,
        preselected_on_unsure: 0,
        traps: 0,
        never_violations: 0,
        precision_verdict: Verdict::NotJudged,
        unsure_verdict: Verdict::NotJudged,
        never_verdict: Verdict::NotJudged,
    };
    for (c, a) in set.cases.iter().zip(answers) {
        if a.skipped.is_some() {
            m.skipped += 1;
            continue;
        }
        m.cases += 1;
        m.rule_decided += u64::from(c.by_rule());
        m.calls += u64::from(a.called);
        let unsure = c.label == UNSURE;
        m.unsure_labelled += u64::from(unsure);
        m.labelled_for_model += u64::from(!unsure && !c.by_rule());
        m.traps += u64::from(c.trap.is_some() || !c.never.is_empty());
        let Some(ans) = a.answer.as_deref() else {
            continue;
        };
        m.answered += 1;
        m.correct += u64::from(ans == c.label);
        m.never_violations += u64::from(c.never.iter().any(|n| n == ans));
        let shows = !quiet.contains(&ans);
        if shows {
            m.proposals += 1;
            m.proposals_right += u64::from(ans == c.label);
            m.preselected_on_unsure += u64::from(unsure);
        }
    }
    m.coverage = pct(m.answered, m.cases);
    m.accuracy_on_answered = pct(m.correct, m.answered);
    m.proposal_precision = pct(m.proposals_right, m.proposals);
    let real = !set.synthetic;
    m.precision_verdict = Verdict::at_least(
        m.proposal_precision,
        ACCEPT_PRECISION,
        real && m.labelled_for_model >= JUDGE_MIN_LABELLED,
    );
    let rate = pct(m.preselected_on_unsure, m.unsure_labelled);
    m.unsure_verdict = Verdict::at_least(
        rate.map(|r| 1.0 - r),
        1.0 - ACCEPT_UNSURE_PRESELECT,
        real && m.unsure_labelled >= JUDGE_MIN_UNSURE,
    );
    m.never_verdict = if real && m.traps > 0 {
        if m.never_violations == 0 {
            Verdict::Pass
        } else {
            Verdict::Fail
        }
    } else {
        Verdict::NotJudged
    };
    m
}

fn f3(v: Option<f64>) -> String {
    v.map_or_else(|| "-".to_string(), |v| format!("{v:.3}"))
}

/// The report's lines: the acceptance, then one line per provider. No case
/// text is printed.
pub fn lines(all: &[Metrics]) -> Vec<String> {
    let Some(first) = all.first() else {
        return Vec::new();
    };
    let uc = first.use_case;
    let mut out = vec![format!(
        "{} {}: proposal precision >= {ACCEPT_PRECISION} (judged from {JUDGE_MIN_LABELLED} \
         labelled cases the rules leave to the model), at most {ACCEPT_UNSURE_PRESELECT} of \
         unsure cases pre-selected (judged from {JUDGE_MIN_UNSURE}), no answer on a never \
         list{}",
        uc.card(),
        uc.name(),
        if first.synthetic {
            "; SYNTHETIC set: not judged"
        } else {
            ""
        }
    )];
    for m in all {
        out.push(format!(
            "{:<8} cases {}  skipped {}  rule {}  calls {}  coverage {}  accuracy {}  \
             proposals: precision {} ({}/{})  unsure pre-selected {}/{}  never {}/{}  \
             -> precision {}, unsure {}, never {}",
            m.provider.as_str(),
            m.cases,
            m.skipped,
            m.rule_decided,
            m.calls,
            f3(m.coverage),
            f3(m.accuracy_on_answered),
            f3(m.proposal_precision),
            m.proposals_right,
            m.proposals,
            m.preselected_on_unsure,
            m.unsure_labelled,
            m.never_violations,
            m.traps,
            m.precision_verdict.as_str(),
            m.unsure_verdict.as_str(),
            m.never_verdict.as_str(),
        ));
    }
    out
}
