//! `fleet-hub decide bench …` — offline benchmarks of the decision
//! envelope's use cases (Jev evaluation phase 0; the test map's cards J1
//! and J3; `fleet_core::service::decide::bench`).
//!
//! `status-map` (J3) reads labeled sections — a file, or the built-in
//! synthetic set — and opens no database at all unless `--provider jev`.
//!
//! By default the database is opened **read-only** and nothing is sent:
//! the providers `none` and `bm25` run in this process. `--provider jev` is
//! the one network path. It goes through the envelope, so a case is asked
//! only when `decide.jev.enabled` is on, the case's org consented, a key is
//! set and the breaker and budget allow it — the feature's live mode
//! (`decide.jev.<feature>`) may stay `off`; every call is
//! recorded in `decision_runs` (which is why that run opens the database
//! for writing). `--export-unlinked` writes the D39 hand-label file: new
//! only, `0600`, and it holds prompt and title text.
//!
//! `--provider haiku` (D33, both benches) asks the same request of `claude
//! -p` on the host named by `--haiku-host` (required): each case's redacted
//! state and options leave the hub over SSH for that host and reach
//! Anthropic through its Claude account. The host's org is read from the
//! database (read-only) and a case goes only to a host of its own org (no
//! org on both sides is the same; anything else is skipped as
//! `other_org`), so the built-in status-map set, which has no org, needs a
//! host with no org. The prompt goes on stdin, never in argv. A note on
//! stderr (and in the report) names the host and its org before anything is
//! sent; nothing is recorded in `decision_runs`.

use crate::census::create_private;
use crate::config::HubOptions;
use crate::dbarg::{db_path, open_read_only};
use crate::out;
use clap::Subcommand;
use fleet_core::service::decide::bench::choice::{self as ch, UseCase};
use fleet_core::service::decide::bench::perturb::Perturbation;
use fleet_core::service::decide::bench::status_map as sm;
use fleet_core::service::decide::bench::status_map_robust as robust;
use fleet_core::service::decide::bench::turn_outcome as to;
use fleet_core::service::decide::bench::work_link::{
    self as wl, BenchOptions, Provider, Shape, Split,
};
use fleet_core::service::decide::bench::work_link_robust as wlr;
use fleet_core::service::decide::haiku::{self, Haiku, HaikuConfig};
use fleet_core::service::decide::DecideCtx;
use fleet_core::service::nl::Detector;
use fleet_core::store::{now_unix, Store};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

#[derive(Subcommand, Debug)]
pub enum BenchCmd {
    /// Card J1: choosing a work item for a session. Cases are the links a
    /// person confirmed (manual / started) with their conversation's first
    /// prompt, redacted so the answer is never in it; each also becomes a
    /// "none of these" case. Reports accuracy on answered, coverage,
    /// coverage at precision 0.9, abstention, latency and cost per
    /// provider, by org, tracker, language, code and candidate-set size.
    ///
    /// Read-only and offline unless --provider jev (writes decision_runs)
    /// or --provider haiku (claude -p on --haiku-host; the database stays
    /// read-only) is given. No prompt or title is printed; counts under 5
    /// show as <5.
    WorkLink {
        /// Which cases to report: dev (oldest 60%), test (newest 40%) or
        /// all. Thresholds are always chosen on dev. [default: test]
        #[arg(long, value_parser = ["dev", "test", "all"])]
        split: Option<String>,
        /// A provider to run: none, bm25, jev, haiku (repeat it). [default:
        /// none and bm25]. jev sends each case's redacted prompt and
        /// candidate titles to TypeSafe, only for orgs that consented;
        /// haiku sends the same to claude -p on --haiku-host.
        #[arg(long = "provider", value_parser = ["none", "bm25", "jev", "haiku"])]
        providers: Vec<String>,
        #[command(flatten)]
        haiku: HaikuArgs,
        /// The window in days (1-730). [default: 365]
        #[arg(long)]
        days: Option<u32>,
        /// Only this org's cases (its id, from `fleet-hub org list`).
        #[arg(long)]
        org: Option<i64>,
        /// Newest links read (1-100000). [default: 20000]
        #[arg(long)]
        max_cases: Option<u32>,
        /// Jev calls at most in this run; later cases are skipped. [default: 500]
        #[arg(long)]
        max_calls: Option<usize>,
        /// Read this database file instead of the hub's, e.g. a desktop
        /// app's state.db.
        #[arg(long, value_name = "FILE")]
        db: Option<PathBuf>,
        /// Print JSON instead of lines.
        #[arg(long)]
        json: bool,
        /// Add the hand labels of this file (written by --export-unlinked,
        /// `label` filled in by a person) as dataset H.
        #[arg(long, value_name = "FILE", conflicts_with = "export_unlinked")]
        labels: Option<PathBuf>,
        /// Write N sessions with no link — redacted first prompt, candidates
        /// and `"label": null` — to --out for a person to label (D39). The
        /// file HOLDS PROMPT AND TITLE TEXT: created 0600, never overwritten.
        #[arg(long, value_name = "N", requires = "out", conflicts_with = "providers")]
        export_unlinked: Option<usize>,
        /// Where --export-unlinked writes (must not exist).
        #[arg(long, value_name = "FILE", requires = "export_unlinked")]
        out: Option<PathBuf>,
        /// How jev is asked: choice (one Choice over the candidates and
        /// "none") or choice+noul (then one Noul on the chosen item, kept
        /// above a threshold chosen on dev; two calls). [default: choice]
        #[arg(long, value_parser = ["choice", "choice+noul"])]
        shape: Option<String>,
        /// Also ask every asked case with its first prompt perturbed
        /// (dataset C) and compare each variant with its original at the
        /// provider's raw pick: fold (no diacritics), typo, code (fenced
        /// blocks replaced by `[code: <lang>, N lines]`, D42). Repeat it.
        /// Each perturbation is its own pass of at most --max-calls calls.
        #[arg(long = "perturb", value_parser = ["fold", "typo", "code"], conflicts_with = "export_unlinked")]
        perturb: Vec<String>,
    },
    /// Card J3: an Asana section's status category. Cases are labeled
    /// sections (--labels FILE, one JSON line each: section,
    /// project_sections, expect, lang, note, org_id) or the built-in
    /// synthetic set (--fixture). Reports accuracy on answered, coverage,
    /// coverage where the keyword rule abstains, done precision, the
    /// confusion matrix and calibration per provider, by language and
    /// ambiguous / clear, and card J3's acceptance.
    ///
    /// Offline unless --provider jev or --provider haiku (claude -p on
    /// --haiku-host) is given; only jev opens a database. No section name
    /// is printed.
    StatusMap {
        /// The labeled sections (JSON lines).
        #[arg(long, value_name = "FILE", conflicts_with = "fixture")]
        labels: Option<PathBuf>,
        /// Use the built-in synthetic set (LLM-written, D43; not yet
        /// spot-checked). Its rows have no org: a jev call's consent is
        /// decide.jev.unassigned.
        #[arg(long, conflicts_with = "paired_fixture")]
        fixture: bool,
        /// Use the built-in paired set (dataset B): 16 boards, each in en,
        /// sk, cs and de, one `pair` id per section; the report compares
        /// every language with English on the same pairs. LLM-written
        /// (D43), no org.
        #[arg(long, conflicts_with = "labels")]
        paired_fixture: bool,
        /// Also ask every case perturbed (dataset C) and compare each
        /// variant with its original: fold (no diacritics), typo, emoji,
        /// no-board, shuffle-board (repeat it). Only cases the perturbation
        /// changes are asked; each perturbation is its own pass of at most
        /// --max-calls calls.
        #[arg(long = "perturb", value_parser = ["fold", "typo", "emoji", "no-board", "shuffle-board"])]
        perturb: Vec<String>,
        /// Report jev's and haiku's numbers at every confidence floor
        /// (0-0.95); with --split all, also the lowest floor the dev boards
        /// would choose under card J3's precision lines, on test. No extra
        /// call: the answers under the floor are kept already.
        #[arg(long)]
        floor_sweep: bool,
        /// A provider to run: none, todo, rule, jev, haiku (repeat it).
        /// [default: todo and rule]. jev sends each section's name and its
        /// board's names to TypeSafe through the envelope's gate; haiku
        /// sends the same to claude -p on --haiku-host.
        #[arg(long = "provider", value_parser = ["none", "todo", "rule", "jev", "haiku"])]
        providers: Vec<String>,
        #[command(flatten)]
        haiku: HaikuArgs,
        /// Jev calls at most in this run; later cases are skipped. [default: 500]
        #[arg(long)]
        max_calls: Option<usize>,
        /// Which boards' sections to report: dev, test or all. By board,
        /// never by case: a board is dev when the SHA-256 of its normalised
        /// section names is under 3 modulo 10 (about 30%). Reword a question on dev,
        /// judge it once on test. [default: all]
        #[arg(long, value_parser = ["dev", "test", "all"])]
        split: Option<String>,
        /// Ask jev and haiku this question instead of the adapter's: JSON
        /// {version, instructions, options: {todo, in_progress, done,
        /// not_planned, unsure}}. The state sent stays the adapter's; jev's
        /// runs are recorded as status_map.bench.q.<version>. Repeat it to
        /// compare wordings: one report each, then a comparison.
        #[arg(long, value_name = "FILE")]
        question: Vec<PathBuf>,
        /// Compare the adapter's question with every built-in rewording
        /// (crates/fleet-core/src/service/testdata/decide/questions): one
        /// pass of at most --max-calls calls each. Only with --split dev:
        /// wordings are compared on dev and the chosen one is judged once
        /// on test.
        #[arg(long)]
        question_set: bool,
        /// The database jev runs are gated by and recorded in, instead of
        /// the hub's (only read with --provider jev).
        #[arg(long, value_name = "FILE")]
        db: Option<PathBuf>,
        /// Print JSON instead of lines.
        #[arg(long)]
        json: bool,
    },
    /// Card J2: what a turn came to when hooks said nothing. Cases are
    /// captured pane tails (--labels FILE, one JSON line each: pane_tail,
    /// label = finished | asked | stuck | working, optional id) or the
    /// built-in synthetic set (--fixture, LLM-written, D43).
    /// Reports coverage, accuracy and `asked` precision / recall per
    /// provider, and card J2's acceptance (asked precision >= 0.9, recall
    /// >= 0.8, judged from 50 labeled asked cases).
    ///
    /// Offline unless --provider jev. No pane text is printed.
    TurnOutcome {
        /// The labeled pane tails (JSON lines).
        #[arg(long, value_name = "FILE", conflicts_with = "fixture")]
        labels: Option<PathBuf>,
        /// Use the built-in synthetic set.
        #[arg(long)]
        fixture: bool,
        /// A provider to run: rule (the pane rules), qmark (ends with ?),
        /// jev (repeat it). [default: rule and qmark]. jev sends each tail,
        /// redacted, to TypeSafe through the envelope's gate: a case has no
        /// org, so decide.jev.unassigned AND decide.jev.unassigned_reply
        /// (D48) must be on.
        #[arg(long = "provider", value_parser = ["rule", "qmark", "jev"])]
        providers: Vec<String>,
        /// Jev calls at most in this run. [default: 500]
        #[arg(long)]
        max_calls: Option<usize>,
        /// The database jev runs are gated by and recorded in, instead of
        /// the hub's (only read with --provider jev).
        #[arg(long, value_name = "FILE")]
        db: Option<PathBuf>,
        /// Print JSON instead of lines.
        #[arg(long)]
        json: bool,
    },
    /// Card K2: which mission or session a Control message is about
    /// (control_route). Cases: a message and the targets a person may see.
    /// The closed-choice benches below share their flags and report: rule
    /// cases (answered without a call), coverage, accuracy, proposal
    /// precision, pre-selects on unsure cases and never-list answers, and
    /// the acceptance (precision >= 0.9 judged from 50 labelled model
    /// cases, at most 5% of 20 unsure cases pre-selected, no never-list
    /// answer). The built-in sets are synthetic and never judged. Offline
    /// unless --provider jev. No case text is printed.
    ControlRoute(ChoiceArgs),
    /// Card K4: a proposed task that repeats an open one (duplicate).
    Duplicate(ChoiceArgs),
    /// Card N1: another of the person's sessions on the same work
    /// (related_session).
    RelatedSession(ChoiceArgs),
    /// Card K5: the group of a task nobody placed (work_placement).
    WorkPlacement(ChoiceArgs),
    /// Card N5: the host of a project's new session (host_placement).
    HostPlacement(ChoiceArgs),
    /// Card N6: what a routine run came to (routine_run_outcome).
    RoutineRunOutcome(ChoiceArgs),
    /// Card N4: the project of a pane fleet did not start (adopt_target).
    AdoptTarget(ChoiceArgs),
    /// Card J10: the project of a found conversation (restore_target).
    RestoreTarget(ChoiceArgs),
    /// Card J6: the main ticket among several keys (main_ticket).
    MainTicket(ChoiceArgs),
    /// Card J7: a local task that repeats a tracker ticket
    /// (tracker_duplicate).
    TrackerDuplicate(ChoiceArgs),
}

/// The flags every closed-choice bench takes (`decide::bench::choice`).
#[derive(clap::Args, Debug, Clone, Default, PartialEq, Eq)]
pub struct ChoiceArgs {
    /// The labelled cases (JSON lines: id, input, label, by?, never?,
    /// trap?; `#` lines are comments, `# synthetic` marks the set).
    #[arg(long, value_name = "FILE", conflicts_with = "fixture")]
    pub labels: Option<PathBuf>,
    /// Use the built-in synthetic set.
    #[arg(long)]
    pub fixture: bool,
    /// A provider to run: rule (the rule layer alone), baseline (what fleet
    /// does without Jev), jev (the rule, else the decision model through
    /// the envelope's gate; a case has no org, so decide.jev.unassigned
    /// must be on, and decide.jev.unassigned_reply too for
    /// routine-run-outcome). [default: rule and baseline]
    #[arg(long = "provider", value_parser = ["rule", "baseline", "jev"])]
    pub providers: Vec<String>,
    /// Jev calls at most in this run. [default: 500]
    #[arg(long)]
    pub max_calls: Option<usize>,
    /// The database jev runs are gated by and recorded in, instead of the
    /// hub's (only read with --provider jev).
    #[arg(long, value_name = "FILE")]
    pub db: Option<PathBuf>,
    /// Print JSON instead of lines.
    #[arg(long)]
    pub json: bool,
}

/// The `claude -p haiku` baseline's flags (D33), shared by both benches.
#[derive(clap::Args, Debug, Clone, Default, PartialEq, Eq)]
pub struct HaikuArgs {
    /// The host whose `claude -p` answers for --provider haiku (required
    /// with it): each case's redacted state and options go over SSH to
    /// this host and on to Anthropic through its Claude account.
    #[arg(long, value_name = "ALIAS")]
    pub haiku_host: Option<String>,
    /// The model claude -p runs. [default: haiku]
    #[arg(long, value_parser = ["haiku", "sonnet", "opus"])]
    pub haiku_model: Option<String>,
    /// One call's wall clock in seconds (10-600). [default: 120]
    #[arg(long, value_name = "SECS")]
    pub haiku_timeout: Option<u64>,
}

impl HaikuArgs {
    /// The checked configuration when `wanted` (`--provider haiku`), which
    /// needs `--haiku-host`; the haiku flags without it are an error.
    fn config(&self, wanted: bool) -> Result<Option<HaikuConfig>, String> {
        match (&self.haiku_host, wanted) {
            (Some(host), true) => HaikuConfig::new(
                host,
                self.haiku_model.as_deref(),
                self.haiku_timeout,
            )
            .map(Some)
            .map_err(|e| format!("--haiku-*: {e}")),
            (None, true) => Err(
                "--provider haiku needs --haiku-host ALIAS: the host whose claude -p answers                  (each case's redacted state and options are sent there, and on to Anthropic                  through its Claude account)"
                    .into(),
            ),
            (_, false)
                if self.haiku_host.is_some()
                    || self.haiku_model.is_some()
                    || self.haiku_timeout.is_some() =>
            {
                Err("--haiku-host, --haiku-model and --haiku-timeout go with --provider haiku".into())
            }
            (_, false) => Ok(None),
        }
    }
}

/// Bind the haiku baseline (when asked for) to `ssh` and to its host's org,
/// read from `s` — a host the database does not know is refused, since no
/// case may cross the org boundary — and say where the data goes before any
/// of it is sent.
fn bind_haiku<'a>(
    ssh: &'a dyn fleet_core::ssh::SshExec,
    cfg: Option<HaikuConfig>,
    s: &Store,
) -> Result<Option<Haiku<'a>>, String> {
    let Some(cfg) = cfg else {
        return Ok(None);
    };
    let host_org = haiku::resolve_host_org(s, &cfg.host)?;
    let h = Haiku {
        exec: ssh,
        cfg,
        host_org,
    };
    out::error(&h.consent_note());
    Ok(Some(h))
}

fn parse_providers(v: &[String]) -> Result<Vec<Provider>, String> {
    v.iter()
        .map(|p| Provider::parse(p).ok_or_else(|| format!("unknown provider {p:?}")))
        .collect()
}

pub async fn run(
    cmd: BenchCmd,
    opts: &HubOptions,
    env: &HashMap<String, String>,
) -> Result<ExitCode, String> {
    match cmd {
        BenchCmd::WorkLink {
            split,
            providers,
            haiku: haiku_args,
            days,
            org,
            max_cases,
            max_calls,
            db,
            json,
            labels,
            export_unlinked,
            out: out_path,
            shape,
            perturb,
        } => {
            let perturb = parse_wl_perturb(&perturb)?;
            let split = Split::parse(split.as_deref().unwrap_or("test"))
                .ok_or("--split is dev, test or all")?;
            let shape = Shape::parse(shape.as_deref().unwrap_or("choice"))
                .ok_or("--shape is choice or choice+noul")?;
            let providers = parse_providers(&providers)?;
            let o = BenchOptions::new(
                days,
                org,
                max_cases,
                split,
                providers,
                max_calls,
                now_unix(),
            )
            .map_err(|e| e.message)?
            .with_shape(shape);
            let path = db_path(db.as_deref(), opts, env)?;

            if let (Some(n), Some(file)) = (export_unlinked, out_path) {
                let store = open_read_only(&path)?;
                let rows = wl::export_unlinked(&store, &o, n).map_err(|e| e.message)?;
                let mut f = create_private(&file)?;
                use std::io::Write;
                for r in &rows {
                    let line = serde_json::to_string(r).map_err(|e| e.to_string())?;
                    writeln!(f, "{line}").map_err(|e| format!("write {}: {e}", file.display()))?;
                }
                out::line(&format!(
                    "wrote {} session(s) to {}. It holds prompt and title text: keep it on this \
                     machine. Set each `label` to one candidate's id or \"none\", then run \
                     `fleet-hub decide bench work-link --labels {}`.",
                    rows.len(),
                    file.display(),
                    file.display()
                ));
                return Ok(ExitCode::SUCCESS);
            }

            let haiku_cfg = haiku_args.config(o.providers.contains(&Provider::Haiku))?;
            let ssh = fleet_core::ssh::SshClient::new();
            let labeled = match &labels {
                Some(file) => {
                    let raw = std::fs::read_to_string(file)
                        .map_err(|e| format!("read {}: {e}", file.display()))?;
                    Some(wl::parse_labels(&raw).map_err(|e| format!("{}: {e}", file.display()))?)
                }
                None => None,
            };
            let detector = Detector::new();
            let report = if o.providers.contains(&Provider::Jev) {
                // Every call is recorded: this run opens the database for
                // writing (the envelope's record, nothing else).
                let store = Store::open_with_bus(&path, Arc::new(fleet_core::events::NoopEventBus))
                    .map_err(|e| format!("open {}: {e}", path.display()))?;
                let store = Arc::new(Mutex::new(store));
                let (loaded, haiku) = {
                    let s = store.lock().map_err(|_| "store lock poisoned")?;
                    let loaded =
                        wl::load(&s, &detector, &o, labeled.as_deref()).map_err(|e| e.message)?;
                    (loaded, bind_haiku(&ssh, haiku_cfg, &s)?)
                };
                out::error(&format!(
                    "jev: asking only for cases whose org passes the gate (at most {} calls); \
                     each call is recorded in decision_runs",
                    o.max_calls
                ));
                let ctx = DecideCtx::jev(Arc::clone(&store));
                let note = haiku.as_ref().map(Haiku::consent_note);
                let outs = wl::run_providers_with(&loaded, &o, Some(&ctx), haiku.as_ref()).await;
                let robustness =
                    wl_robustness(&loaded, &o, &outs, Some(&ctx), haiku.as_ref(), &perturb).await;
                let mut r = wl::report(&loaded, &o, &outs).with_robustness(robustness);
                r.notes.extend(note);
                r
            } else {
                let store = open_read_only(&path)?;
                let loaded =
                    wl::load(&store, &detector, &o, labeled.as_deref()).map_err(|e| e.message)?;
                let haiku = bind_haiku(&ssh, haiku_cfg, &store)?;
                let note = haiku.as_ref().map(Haiku::consent_note);
                let outs = wl::run_providers_with(&loaded, &o, None, haiku.as_ref()).await;
                let robustness =
                    wl_robustness(&loaded, &o, &outs, None, haiku.as_ref(), &perturb).await;
                let mut r = wl::report(&loaded, &o, &outs).with_robustness(robustness);
                r.notes.extend(note);
                r
            };
            if json {
                out::line(&serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?);
            } else {
                for l in report.lines() {
                    out::line(&l);
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        BenchCmd::StatusMap {
            labels,
            fixture,
            paired_fixture,
            perturb,
            floor_sweep,
            providers,
            haiku,
            max_calls,
            split,
            question,
            question_set,
            db,
            json,
        } => {
            let split = sm_split(split.as_deref())?;
            let ssh = fleet_core::ssh::SshClient::new();
            let extras = SmExtras {
                fixture: match (fixture, paired_fixture) {
                    (_, true) => Some(SmFixture::Paired),
                    (true, false) => Some(SmFixture::Sections),
                    _ => None,
                },
                perturb: parse_perturb(&perturb)?,
                floor_sweep,
                questions: question.iter().map(PathBuf::as_path).collect(),
                question_set,
            };
            let reports = status_map_with(
                labels.as_deref(),
                &extras,
                &providers,
                &haiku,
                &ssh,
                max_calls,
                db.as_deref(),
                split,
                opts,
                env,
            )
            .await?;
            let compared: Vec<robust::QuestionRow> = if reports.len() > 1 {
                reports.iter().flat_map(robust::question_rows).collect()
            } else {
                Vec::new()
            };
            if json {
                let v = match reports.as_slice() {
                    [one] => serde_json::to_value(one),
                    many => serde_json::to_value(serde_json::json!({
                        "reports": many,
                        "questions": compared,
                    })),
                }
                .map_err(|e| e.to_string())?;
                out::line(&serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?);
            } else {
                for (i, r) in reports.iter().enumerate() {
                    if i > 0 {
                        out::line("");
                        out::line(&"=".repeat(78));
                    }
                    for l in r.lines() {
                        out::line(&l);
                    }
                }
                if !compared.is_empty() {
                    out::line("");
                    for l in robust::question_lines(&compared) {
                        out::line(&l);
                    }
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        BenchCmd::TurnOutcome {
            labels,
            fixture,
            providers,
            max_calls,
            db,
            json,
        } => {
            let all = turn_outcome(
                labels.as_deref(),
                fixture,
                &providers,
                max_calls,
                db.as_deref(),
                opts,
                env,
            )
            .await?;
            if json {
                out::line(&serde_json::to_string_pretty(&all).map_err(|e| e.to_string())?);
            } else {
                for l in to::lines(&all) {
                    out::line(&l);
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        BenchCmd::ControlRoute(a) => choice_cmd(UseCase::ControlRoute, a, opts, env).await,
        BenchCmd::Duplicate(a) => choice_cmd(UseCase::Duplicate, a, opts, env).await,
        BenchCmd::RelatedSession(a) => choice_cmd(UseCase::RelatedSession, a, opts, env).await,
        BenchCmd::WorkPlacement(a) => choice_cmd(UseCase::WorkPlacement, a, opts, env).await,
        BenchCmd::HostPlacement(a) => choice_cmd(UseCase::HostPlacement, a, opts, env).await,
        BenchCmd::RoutineRunOutcome(a) => {
            choice_cmd(UseCase::RoutineRunOutcome, a, opts, env).await
        }
        BenchCmd::AdoptTarget(a) => choice_cmd(UseCase::AdoptTarget, a, opts, env).await,
        BenchCmd::RestoreTarget(a) => choice_cmd(UseCase::RestoreTarget, a, opts, env).await,
        BenchCmd::MainTicket(a) => choice_cmd(UseCase::MainTicket, a, opts, env).await,
        BenchCmd::TrackerDuplicate(a) => choice_cmd(UseCase::TrackerDuplicate, a, opts, env).await,
    }
}

/// One closed-choice bench, printed.
async fn choice_cmd(
    uc: UseCase,
    a: ChoiceArgs,
    opts: &HubOptions,
    env: &HashMap<String, String>,
) -> Result<ExitCode, String> {
    let all = choice(uc, &a, opts, env).await?;
    if a.json {
        out::line(&serde_json::to_string_pretty(&all).map_err(|e| e.to_string())?);
    } else {
        for l in ch::lines(&all) {
            out::line(&l);
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// `decide bench <use case>`: the labelled cases (or the built-in set)
/// through the providers. Only `--provider jev` opens a database — for
/// writing, as the envelope records every call; a rule case sends nothing.
async fn choice(
    uc: UseCase,
    a: &ChoiceArgs,
    opts: &HubOptions,
    env: &HashMap<String, String>,
) -> Result<Vec<ch::Metrics>, String> {
    let text = match (a.labels.as_deref(), a.fixture) {
        (Some(f), _) => std::fs::read_to_string(f).map_err(|e| format!("{}: {e}", f.display()))?,
        (None, true) => uc.fixture().to_string(),
        (None, false) => return Err("give --labels FILE or --fixture".into()),
    };
    let set = ch::parse(uc, &text)?;
    let mut ps: Vec<ch::Provider> = a
        .providers
        .iter()
        .map(|p| ch::Provider::parse(p).ok_or_else(|| format!("unknown provider {p}")))
        .collect::<Result<_, _>>()?;
    if ps.is_empty() {
        ps = vec![ch::Provider::Rule, ch::Provider::Baseline];
    }
    ps.dedup();
    let mut all = Vec::with_capacity(ps.len());
    for p in ps {
        let answers = if p == ch::Provider::Jev {
            let path = db_path(a.db.as_deref(), opts, env)?;
            let store = Store::open_with_bus(&path, Arc::new(fleet_core::events::NoopEventBus))
                .map_err(|e| format!("open {}: {e}", path.display()))?;
            let max = a.max_calls.unwrap_or(ch::DEFAULT_MAX_CALLS);
            out::error(&format!(
                "jev: asking only the cases no rule decides, when decide.jev.unassigned passes \
                 the gate, at most {max} calls; each call is recorded in decision_runs"
            ));
            let ctx = DecideCtx::jev(Arc::new(Mutex::new(store)));
            ch::run_jev(&ctx, &set, max).await
        } else {
            set.cases.iter().map(|c| ch::offline(p, c)).collect()
        };
        all.push(ch::metrics(p, &set, &answers));
    }
    Ok(all)
}

/// `decide bench turn-outcome`: the labeled tails (or the built-in set)
/// through the providers. Only `--provider jev` opens a database — for
/// writing, as the envelope records every call.
async fn turn_outcome(
    labels: Option<&Path>,
    fixture: bool,
    providers: &[String],
    max_calls: Option<usize>,
    db: Option<&Path>,
    opts: &HubOptions,
    env: &HashMap<String, String>,
) -> Result<Vec<to::Metrics>, String> {
    let text = match (labels, fixture) {
        (Some(f), _) => std::fs::read_to_string(f).map_err(|e| format!("{}: {e}", f.display()))?,
        (None, true) => to::FIXTURE.to_string(),
        (None, false) => return Err("give --labels FILE or --fixture".into()),
    };
    let cases = to::parse_labels(&text)?;
    let synthetic = to::is_synthetic(&text);
    let mut ps: Vec<to::Provider> = providers
        .iter()
        .map(|p| to::Provider::parse(p).ok_or_else(|| format!("unknown provider {p}")))
        .collect::<Result<_, _>>()?;
    if ps.is_empty() {
        ps = vec![to::Provider::Rule, to::Provider::Qmark];
    }
    ps.dedup();
    let mut all = Vec::with_capacity(ps.len());
    for p in ps {
        let answers = if p == to::Provider::Jev {
            let path = db_path(db, opts, env)?;
            let store = Store::open_with_bus(&path, Arc::new(fleet_core::events::NoopEventBus))
                .map_err(|e| format!("open {}: {e}", path.display()))?;
            let max = max_calls.unwrap_or(to::DEFAULT_MAX_CALLS);
            out::error(&format!(
                "jev: asking only when decide.jev.unassigned and decide.jev.unassigned_reply \
                 pass the gate, at most {max} calls; each call is recorded in decision_runs"
            ));
            let ctx = DecideCtx::jev(Arc::new(Mutex::new(store)));
            to::run_jev(&ctx, &cases, max).await
        } else {
            cases.iter().map(|c| to::offline(p, &c.pane_tail)).collect()
        };
        all.push(to::metrics(p, &cases, &answers, synthetic));
    }
    Ok(all)
}

fn parse_wl_perturb(v: &[String]) -> Result<Vec<Perturbation>, String> {
    let mut ps: Vec<Perturbation> = v
        .iter()
        .map(|p| {
            Perturbation::parse(p)
                .filter(|p| Perturbation::WORK_LINK.contains(p))
                .ok_or_else(|| format!("unknown perturbation {p:?}"))
        })
        .collect::<Result<_, _>>()?;
    ps.sort();
    ps.dedup();
    Ok(ps)
}

/// Each perturbation's pass over the asked cases (`work-link --perturb`),
/// compared with the originals' outcomes `outs`.
async fn wl_robustness(
    loaded: &wl::Loaded,
    o: &BenchOptions,
    outs: &wl::Outcomes,
    ctx: Option<&DecideCtx>,
    haiku: Option<&Haiku<'_>>,
    perturb: &[Perturbation],
) -> Vec<wlr::WlRobustness> {
    let mut v = Vec::with_capacity(perturb.len());
    for &p in perturb {
        let vl = wlr::variants(loaded, o, p);
        let vouts = wl::run_providers_with(&vl, o, ctx, haiku).await;
        v.push(wlr::robustness(p, loaded, o, outs, &vl, &vouts));
    }
    v
}

fn parse_sm_providers(v: &[String]) -> Result<Vec<sm::Provider>, String> {
    let mut ps: Vec<sm::Provider> = v
        .iter()
        .map(|p| sm::Provider::parse(p).ok_or_else(|| format!("unknown provider {p:?}")))
        .collect::<Result<_, _>>()?;
    if ps.is_empty() {
        ps = vec![sm::Provider::Todo, sm::Provider::Rule];
    }
    ps.sort();
    ps.dedup();
    Ok(ps)
}

/// `--split` of `status-map`: every case unless told otherwise.
fn sm_split(v: Option<&str>) -> Result<Split, String> {
    Split::parse(v.unwrap_or("all")).ok_or_else(|| "--split is dev, test or all".to_string())
}

/// `--question FILE`, read and checked (an error names the file). Only a
/// model is asked a question, so it needs `jev` or `haiku` among `providers`.
fn read_question(
    file: Option<&Path>,
    providers: &[sm::Provider],
) -> Result<Option<sm::QuestionOverride>, String> {
    let Some(file) = file else {
        return Ok(None);
    };
    if !providers.iter().any(|p| p.is_model()) {
        return Err(
            "--question goes with --provider jev or --provider haiku (only a model is asked it)"
                .into(),
        );
    }
    let raw = std::fs::read_to_string(file).map_err(|e| format!("read {}: {e}", file.display()))?;
    sm::parse_question(&raw)
        .map(Some)
        .map_err(|e| format!("{}: {e}", file.display()))
}

/// The built-in label sets `status-map` reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SmFixture {
    /// The synthetic sections (D43).
    Sections,
    /// The paired set (dataset B).
    Paired,
}

/// What a `status-map` run asks beyond one pass of the labeled sections.
#[derive(Debug, Default)]
struct SmExtras<'a> {
    fixture: Option<SmFixture>,
    perturb: Vec<Perturbation>,
    floor_sweep: bool,
    questions: Vec<&'a Path>,
    question_set: bool,
}

fn parse_perturb(v: &[String]) -> Result<Vec<Perturbation>, String> {
    let mut ps: Vec<Perturbation> = v
        .iter()
        .map(|p| {
            Perturbation::parse(p)
                .filter(|p| Perturbation::STATUS_MAP.contains(p))
                .ok_or_else(|| format!("unknown perturbation {p:?}"))
        })
        .collect::<Result<_, _>>()?;
    ps.sort();
    ps.dedup();
    Ok(ps)
}

/// The questions a run asks, in order: `None` is the adapter's. With the
/// question set, the adapter's and every built-in rewording (only on dev),
/// then the files; else the files, or the adapter's alone. Two wordings of
/// the same version are refused (their runs would be recorded as one).
fn sm_questions(
    x: &SmExtras<'_>,
    providers: &[sm::Provider],
    split: Split,
) -> Result<Vec<Option<sm::QuestionOverride>>, String> {
    let mut qs: Vec<Option<sm::QuestionOverride>> = Vec::new();
    if x.question_set {
        if !providers.iter().any(|p| p.is_model()) {
            return Err(
                "--question-set goes with --provider jev or --provider haiku (only a model is asked it)"
                    .into(),
            );
        }
        if split != Split::Dev {
            return Err(
                "--question-set runs only with --split dev: compare wordings on dev, then judge the \
                 chosen one once with --split test --question FILE"
                    .into(),
            );
        }
        qs.push(None);
        for (file, json) in robust::QUESTION_SET {
            qs.push(Some(
                sm::parse_question(json).map_err(|e| format!("built-in {file}: {e}"))?,
            ));
        }
    }
    for f in &x.questions {
        qs.push(read_question(Some(f), providers)?);
    }
    if qs.is_empty() {
        qs.push(None);
    }
    let mut seen = std::collections::BTreeSet::new();
    for q in &qs {
        let v = q.as_ref().map_or_else(
            || sm::QUESTION_VERSION.to_string(),
            |q| q.recorded_version(),
        );
        if !seen.insert(v.clone()) {
            return Err(format!("two questions have the same version {v}"));
        }
    }
    Ok(qs)
}

/// `decide bench status-map`, one report (the adapter's question, or one
/// question file) — what the tests drive.
#[allow(clippy::too_many_arguments)]
#[cfg(test)]
async fn status_map(
    labels: Option<&Path>,
    fixture: bool,
    providers: &[String],
    haiku_args: &HaikuArgs,
    ssh: &dyn fleet_core::ssh::SshExec,
    max_calls: Option<usize>,
    db: Option<&Path>,
    split: Split,
    question: Option<&Path>,
    opts: &HubOptions,
    env: &HashMap<String, String>,
) -> Result<sm::Report, String> {
    let x = SmExtras {
        fixture: fixture.then_some(SmFixture::Sections),
        questions: question.into_iter().collect(),
        ..Default::default()
    };
    let mut v = status_map_with(
        labels, &x, providers, haiku_args, ssh, max_calls, db, split, opts, env,
    )
    .await?;
    Ok(v.remove(0))
}

/// `decide bench status-map`: the labeled sections (or a built-in set)
/// through the providers, one report per question ([`sm_questions`]). Only
/// `--provider jev` opens a database — for writing, as the envelope records
/// every call. `--provider haiku` runs on `ssh` against `--haiku-host`,
/// whose org it reads from the database (read-only). Only the `split`
/// side's boards are asked. Each perturbation of `x` asks the cases it
/// changes once more (its own pass, at most `max_calls` calls) and compares
/// them with the originals; `x.floor_sweep` adds the model providers'
/// numbers at every floor (no call).
#[allow(clippy::too_many_arguments)]
async fn status_map_with(
    labels: Option<&Path>,
    x: &SmExtras<'_>,
    providers: &[String],
    haiku_args: &HaikuArgs,
    ssh: &dyn fleet_core::ssh::SshExec,
    max_calls: Option<usize>,
    db: Option<&Path>,
    split: Split,
    opts: &HubOptions,
    env: &HashMap<String, String>,
) -> Result<Vec<sm::Report>, String> {
    let raw =
        match (labels, x.fixture) {
            (Some(file), _) => std::fs::read_to_string(file)
                .map_err(|e| format!("read {}: {e}", file.display()))?,
            (None, Some(SmFixture::Sections)) => sm::FIXTURE.to_string(),
            (None, Some(SmFixture::Paired)) => robust::PAIRED_FIXTURE.to_string(),
            (None, None) => return Err(
                "give the labeled sections with --labels FILE, or --fixture (or --paired-fixture) \
                 for a built-in synthetic set"
                    .into(),
            ),
        };
    let mut rows = sm::parse_labels(&raw).map_err(|e| match labels {
        Some(f) => format!("{}: {e}", f.display()),
        None => format!("the built-in set: {e}"),
    })?;
    let providers = parse_sm_providers(providers)?;
    let questions = sm_questions(x, &providers, split)?;
    if x.floor_sweep && !providers.iter().any(|p| p.is_model()) {
        return Err(
            "--floor-sweep goes with --provider jev or --provider haiku (only a model states a \
             confidence)"
                .into(),
        );
    }
    let max_calls = max_calls.unwrap_or(sm::DEFAULT_MAX_CALLS);
    let haiku = match haiku_args.config(providers.contains(&sm::Provider::Haiku))? {
        Some(cfg) => {
            let path = db_path(db, opts, env)?;
            let s = open_read_only(&path)?;
            bind_haiku(ssh, Some(cfg), &s)?
        }
        None => None,
    };
    // Before anything is sent, each row's org comes from the database (the
    // gate's consent and the haiku org fence rest on it), never from the
    // file alone.
    if providers.iter().any(|p| p.is_model()) {
        let path = db_path(db, opts, env)?;
        let s = open_read_only(&path)?;
        sm::resolve_label_orgs(&s, &mut rows).map_err(|e| match labels {
            Some(f) => format!("{}: {e}", f.display()),
            None => format!("the built-in set: {e}"),
        })?;
    }
    let (cases, dropped) = sm::cases(&rows);
    let (cases, split_sizes) = sm::split_cases(cases, split);
    let note = haiku.as_ref().map(Haiku::consent_note);
    let ctx = if providers.contains(&sm::Provider::Jev) {
        let path = db_path(db, opts, env)?;
        let store = Store::open_with_bus(&path, Arc::new(fleet_core::events::NoopEventBus))
            .map_err(|e| format!("open {}: {e}", path.display()))?;
        let passes = questions.len() * (1 + x.perturb.len());
        out::error(&format!(
            "jev: asking only for sections whose org (or, with none, decide.jev.unassigned) passes \
             the gate, at most {max_calls} calls a pass ({passes} pass(es)); each call is recorded \
             in decision_runs"
        ));
        Some(DecideCtx::jev(Arc::new(Mutex::new(store))))
    } else {
        None
    };
    let variants: Vec<(Perturbation, Vec<(usize, sm::SectionCase)>)> = x
        .perturb
        .iter()
        .map(|&p| (p, robust::variants(&cases, p)))
        .collect();
    let mut reports = Vec::with_capacity(questions.len());
    for question in &questions {
        let outs = sm::run_providers_with(
            &cases,
            &providers,
            ctx.as_ref(),
            haiku.as_ref(),
            max_calls,
            question.as_ref(),
        )
        .await;
        let mut robustness = Vec::with_capacity(variants.len());
        for (p, vs) in &variants {
            let vcases: Vec<sm::SectionCase> = vs.iter().map(|(_, v)| v.clone()).collect();
            let vouts = sm::run_providers_with(
                &vcases,
                &providers,
                ctx.as_ref(),
                haiku.as_ref(),
                max_calls,
                question.as_ref(),
            )
            .await;
            robustness.push(robust::robustness(*p, &cases, &outs, vs, &vouts));
        }
        let sweep = if x.floor_sweep {
            robust::floor_sweep(&cases, &outs, split == Split::All)
        } else {
            Vec::new()
        };
        let mut r = sm::report(&cases, &outs, rows.len(), dropped, labels.is_none())
            .with_split(split, split_sizes.clone())
            .with_question(question.as_ref())
            .with_robustness(robustness)
            .with_floor_sweep(sweep);
        if labels.is_none() && x.fixture == Some(SmFixture::Paired) {
            r.source = "paired-fixture";
            r.notes.retain(|n| !n.contains(sm::FIXTURE_PATH));
            r.notes.push(format!(
                "the built-in paired set is synthetic ({}): LLM-written (D43), not yet spot-checked \
                 by the owner — its language comparison is indicative",
                robust::PAIRED_FIXTURE_PATH
            ));
        }
        if !x.perturb.is_empty() {
            r.notes.push(
                "each perturbation asks the cases it changes once more (its own pass of at most \
                 --max-calls calls); its comparison is a diagnostic, not an acceptance line"
                    .into(),
            );
        }
        r.notes.extend(note.clone());
        reports.push(r);
    }
    Ok(reports)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct T {
        #[command(subcommand)]
        cmd: BenchCmd,
    }

    fn parse(args: &[&str]) -> Result<BenchCmd, clap::Error> {
        let mut all = vec!["t"];
        all.extend_from_slice(args);
        T::try_parse_from(all).map(|t| t.cmd)
    }

    #[tokio::test]
    async fn turn_outcome_runs_the_baselines_on_the_fixture_offline() {
        let Ok(BenchCmd::TurnOutcome {
            fixture, providers, ..
        }) = parse(&["turn-outcome", "--fixture"])
        else {
            panic!("parses");
        };
        let env = HashMap::new();
        let all = turn_outcome(
            None,
            fixture,
            &providers,
            None,
            None,
            &HubOptions::default(),
            &env,
        )
        .await
        .unwrap();
        let names: Vec<&str> = all.iter().map(|m| m.provider.as_str()).collect();
        assert_eq!(names, ["rule", "qmark"]);
        assert!(all.iter().all(|m| m.synthetic && m.asked_labeled == 51));
        assert!(
            turn_outcome(None, false, &[], None, None, &HubOptions::default(), &env)
                .await
                .is_err()
        );
        assert!(parse(&["turn-outcome", "--fixture", "--provider", "haiku"]).is_err());
    }

    /// Every closed-choice use case is a subcommand that runs its built-in
    /// set offline: the rule and the baseline, nothing judged.
    #[tokio::test]
    async fn every_choice_bench_runs_its_fixture_offline() {
        let env = HashMap::new();
        for &uc in UseCase::ALL {
            let cmd = uc.command();
            let Ok(parsed) = parse(&[&cmd, "--fixture"]) else {
                panic!("{cmd} parses");
            };
            let a = match parsed {
                BenchCmd::ControlRoute(a)
                | BenchCmd::Duplicate(a)
                | BenchCmd::RelatedSession(a)
                | BenchCmd::WorkPlacement(a)
                | BenchCmd::HostPlacement(a)
                | BenchCmd::RoutineRunOutcome(a)
                | BenchCmd::AdoptTarget(a)
                | BenchCmd::RestoreTarget(a)
                | BenchCmd::MainTicket(a)
                | BenchCmd::TrackerDuplicate(a) => a,
                other => panic!("{cmd}: {other:?}"),
            };
            let all = choice(uc, &a, &HubOptions::default(), &env).await.unwrap();
            let names: Vec<&str> = all.iter().map(|m| m.provider.as_str()).collect();
            assert_eq!(names, ["rule", "baseline"], "{cmd}");
            assert!(all.iter().all(|m| m.synthetic && m.calls == 0), "{cmd}");
            assert!(ch::lines(&all)[0].contains("SYNTHETIC"), "{cmd}");
            let none = ChoiceArgs::default();
            assert!(choice(uc, &none, &HubOptions::default(), &env)
                .await
                .is_err());
        }
        assert!(parse(&["host-placement", "--fixture", "--provider", "haiku"]).is_err());
    }

    #[test]
    fn work_link_parses_its_flags() {
        let c = parse(&[
            "work-link",
            "--split",
            "all",
            "--provider",
            "bm25",
            "--provider",
            "jev",
            "--org",
            "3",
            "--max-calls",
            "20",
            "--json",
        ])
        .unwrap();
        let BenchCmd::WorkLink {
            split,
            providers,
            org,
            max_calls,
            json,
            shape,
            ..
        } = c
        else {
            panic!("work-link");
        };
        assert_eq!(shape, None);
        assert_eq!(split.as_deref(), Some("all"));
        assert_eq!(providers, vec!["bm25", "jev"]);
        assert_eq!((org, max_calls, json), (Some(3), Some(20), true));
        assert_eq!(
            parse_providers(&providers).unwrap(),
            vec![Provider::Bm25, Provider::Jev]
        );
    }

    #[test]
    fn unknown_values_are_refused() {
        assert!(parse(&["work-link", "--split", "train"]).is_err());
        assert!(parse(&["work-link", "--provider", "sonnet"]).is_err());
        assert!(parse(&["status-map", "--haiku-model", "claude-opus-5-5"]).is_err());
        assert!(parse(&["work-link", "--shape", "noul"]).is_err());
        assert!(parse(&["status-map", "--provider", "bm25"]).is_err());
        assert!(parse(&["status-map", "--fixture", "--labels", "x.jsonl"]).is_err());
    }

    #[test]
    fn work_link_takes_its_perturbations() {
        let Ok(BenchCmd::WorkLink { perturb, .. }) = parse(&[
            "work-link",
            "--perturb",
            "code",
            "--perturb",
            "fold",
            "--perturb",
            "code",
        ]) else {
            panic!("work-link");
        };
        assert_eq!(
            parse_wl_perturb(&perturb).unwrap(),
            vec![Perturbation::Fold, Perturbation::Code]
        );
        assert!(parse(&["work-link", "--perturb", "no-board"]).is_err());
        assert!(parse(&[
            "work-link",
            "--perturb",
            "typo",
            "--export-unlinked",
            "5",
            "--out",
            "x"
        ])
        .is_err());
    }

    #[test]
    fn the_shapes_parse() {
        let Ok(BenchCmd::WorkLink { shape, .. }) =
            parse(&["work-link", "--shape", "choice+noul", "--provider", "jev"])
        else {
            panic!("work-link");
        };
        assert_eq!(shape.as_deref(), Some("choice+noul"));
        assert_eq!(Shape::parse("choice+noul"), Some(Shape::ChoiceNoul));
    }

    /// A transport that answers every command with `stdout` and records
    /// the host, the command line and the stdin of each call.
    #[derive(Default)]
    struct Canned {
        stdout: String,
        hosts: Mutex<Vec<String>>,
        commands: Mutex<Vec<String>>,
        stdins: Mutex<Vec<String>>,
    }

    impl Canned {
        fn hosts(&self) -> Vec<String> {
            self.hosts.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl fleet_core::ssh::SshExec for Canned {
        async fn run(
            &self,
            _: &str,
            _: &[&str],
            _: std::time::Duration,
        ) -> Result<std::process::Output, fleet_core::ipc_error::IpcError> {
            unreachable!("haiku sends its prompt on stdin")
        }
        async fn run_bounded(
            &self,
            _: &str,
            _: &[&str],
            _: std::time::Duration,
            _: std::time::Duration,
        ) -> Result<std::process::Output, fleet_core::ipc_error::IpcError> {
            unreachable!("haiku sends its prompt on stdin")
        }
        async fn run_with_stdin(
            &self,
            host: &str,
            args: &[&str],
            stdin: Vec<u8>,
            _: std::time::Duration,
            _: std::time::Duration,
            _: usize,
        ) -> Result<std::process::Output, fleet_core::ipc_error::IpcError> {
            self.hosts.lock().unwrap().push(host.to_string());
            self.commands.lock().unwrap().push(args.join(" "));
            self.stdins
                .lock()
                .unwrap()
                .push(String::from_utf8(stdin).unwrap());
            Ok(std::process::Output {
                status: fleet_core::agent::transport::exit_status(0),
                stdout: self.stdout.clone().into_bytes(),
                stderr: Vec::new(),
            })
        }
        async fn run_cancellable(
            &self,
            _: &str,
            _: &[&str],
            _: std::time::Duration,
            _: tokio_util::sync::CancellationToken,
        ) -> Result<std::process::Output, fleet_core::ipc_error::IpcError> {
            unreachable!()
        }
        async fn run_bounded_cancellable(
            &self,
            _: &str,
            _: &[&str],
            _: std::time::Duration,
            _: std::time::Duration,
            _: tokio_util::sync::CancellationToken,
        ) -> Result<std::process::Output, fleet_core::ipc_error::IpcError> {
            unreachable!()
        }
        async fn upload_file(
            &self,
            _: &str,
            _: &Path,
            _: &str,
            _: std::time::Duration,
        ) -> Result<(), fleet_core::ipc_error::IpcError> {
            unreachable!()
        }
        async fn remote_home(&self, _: &str) -> Result<String, fleet_core::ipc_error::IpcError> {
            unreachable!()
        }
    }

    /// `status_map` without the haiku baseline (its flags unset, a
    /// transport that must never be used).
    async fn status_map(
        labels: Option<&Path>,
        fixture: bool,
        providers: &[String],
        max_calls: Option<usize>,
        db: Option<&Path>,
        opts: &HubOptions,
        env: &HashMap<String, String>,
    ) -> Result<sm::Report, String> {
        let never = Canned::default();
        let r = super::status_map(
            labels,
            fixture,
            providers,
            &HaikuArgs::default(),
            &never,
            max_calls,
            db,
            Split::All,
            None,
            opts,
            env,
        )
        .await;
        assert!(never.hosts().is_empty());
        r
    }

    #[test]
    fn haiku_needs_its_host_and_its_flags_need_it() {
        let Ok(BenchCmd::StatusMap {
            haiku, providers, ..
        }) = parse(&[
            "status-map",
            "--fixture",
            "--provider",
            "haiku",
            "--haiku-host",
            "gpu1",
            "--haiku-model",
            "sonnet",
            "--haiku-timeout",
            "30",
        ])
        else {
            panic!("status-map");
        };
        assert_eq!(providers, vec!["haiku"]);
        let cfg = haiku.config(true).unwrap().unwrap();
        assert_eq!(
            (cfg.host.as_str(), cfg.model.as_str(), cfg.timeout.as_secs()),
            ("gpu1", "sonnet", 30)
        );
        // The flags without the provider, and the provider without a host.
        assert!(haiku
            .config(false)
            .unwrap_err()
            .contains("--provider haiku"));
        let e = HaikuArgs::default().config(true).unwrap_err();
        assert!(e.contains("--haiku-host") && e.contains("Anthropic"), "{e}");
        assert_eq!(HaikuArgs::default().config(false), Ok(None));
        let bad = |host: &str, timeout: Option<u64>| HaikuArgs {
            haiku_host: Some(host.into()),
            haiku_timeout: timeout,
            ..Default::default()
        };
        assert!(bad("-oProxyCommand=id", None).config(true).is_err());
        assert!(bad("gpu1", Some(5)).config(true).is_err());
        assert!(bad("gpu1", None).config(true).is_ok());
        let Ok(BenchCmd::WorkLink { haiku, .. }) =
            parse(&["work-link", "--provider", "haiku", "--haiku-host", "gpu1"])
        else {
            panic!("work-link");
        };
        assert_eq!(haiku.haiku_host.as_deref(), Some("gpu1"));
    }

    #[tokio::test]
    async fn status_map_with_haiku_asks_the_named_host_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sections.jsonl");
        std::fs::write(
            &file,
            "{\"section\":\"Hotovo\",\"project_sections\":[\"Nové\",\"Hotovo\"],\"expect\":\"done\",\"lang\":\"sk\"}\n\
             {\"section\":\"Nové\",\"project_sections\":[\"Nové\",\"Hotovo\"],\"expect\":\"todo\",\"lang\":\"sk\"}\n",
        )
        .unwrap();
        let envelope = serde_json::json!({
            "type": "result",
            "result": "{\"choice\": \"done\", \"confidence\": 0.9}",
            "usage": { "input_tokens": 50, "output_tokens": 5 },
            "total_cost_usd": 0.001,
        });
        // The hub's database: gpu1 has no org (like the rows), acme-box
        // has one.
        let db = dir.path().join("state.db");
        {
            let s = Store::open_with_bus(&db, Arc::new(fleet_core::events::NoopEventBus)).unwrap();
            s.upsert_host("gpu1").unwrap();
            s.upsert_host("acme-box").unwrap();
            let acme = s.add_org("Acme", None, false).unwrap().id;
            s.set_host_org("acme-box", Some(acme)).unwrap();
        }
        let canned = || Canned {
            stdout: format!("fleet-haiku=run\n{envelope}\n"),
            ..Default::default()
        };
        let on = |host: &str| HaikuArgs {
            haiku_host: Some(host.into()),
            ..Default::default()
        };
        let providers = ["rule".to_string(), "haiku".to_string()];
        let fake = canned();
        let r = super::status_map(
            Some(&file),
            false,
            &providers,
            &on("gpu1"),
            &fake,
            None,
            Some(&db),
            Split::All,
            None,
            &HubOptions::default(),
            &HashMap::new(),
        )
        .await
        .unwrap();
        assert_eq!(fake.hosts(), vec!["gpu1", "gpu1"]);
        // The prompt went on stdin; no command line carries it.
        let stdins = fake.stdins.lock().unwrap().clone();
        assert!(stdins.iter().all(|p| p.contains("hotovo")), "{stdins:?}");
        assert!(fake
            .commands
            .lock()
            .unwrap()
            .iter()
            .all(|c| !c.contains("hotovo") && !c.contains("<state>")));
        let m = r.metrics.iter().find(|m| m.provider == "haiku").unwrap();
        assert_eq!((m.calls, m.all.answered, m.all.correct), (2, 2, 1));
        assert_eq!((m.input_tokens, m.cost_microusd), (100, 2000));
        assert!(
            r.notes.iter().any(|n| n.contains("host gpu1")
                && n.contains("Anthropic")
                && n.contains("no org")),
            "{:?}",
            r.notes
        );
        // A host of an org gets none of these org-less rows.
        let fake = canned();
        let r = super::status_map(
            Some(&file),
            false,
            &providers,
            &on("acme-box"),
            &fake,
            None,
            Some(&db),
            Split::All,
            None,
            &HubOptions::default(),
            &HashMap::new(),
        )
        .await
        .unwrap();
        assert!(fake.hosts().is_empty());
        let m = r.metrics.iter().find(|m| m.provider == "haiku").unwrap();
        assert_eq!((m.calls, m.skipped.get("other_org").copied()), (0, Some(2)));
        // A host the database does not know: refused, nothing sent.
        let fake = canned();
        let e = super::status_map(
            Some(&file),
            false,
            &providers,
            &on("stranger"),
            &fake,
            None,
            Some(&db),
            Split::All,
            None,
            &HubOptions::default(),
            &HashMap::new(),
        )
        .await
        .unwrap_err();
        assert!(e.contains("stranger"), "{e}");
        assert!(fake.hosts().is_empty());
        // A row whose org the database does not bear out: refused before
        // anything is sent (the file alone never sets a case's org).
        let bad = dir.path().join("bad.jsonl");
        std::fs::write(
            &bad,
            "{\"section\":\"Hotovo\",\"expect\":\"done\",\"lang\":\"sk\",\"org_id\":4242}\n",
        )
        .unwrap();
        let fake = canned();
        let e = super::status_map(
            Some(&bad),
            false,
            &providers,
            &on("acme-box"),
            &fake,
            None,
            Some(&db),
            Split::All,
            None,
            &HubOptions::default(),
            &HashMap::new(),
        )
        .await
        .unwrap_err();
        assert!(e.contains("row 1") && e.contains("no org 4242"), "{e}");
        assert!(fake.hosts().is_empty());
        // Without the host nothing is sent.
        let fake = Canned::default();
        let e = super::status_map(
            Some(&file),
            false,
            &providers,
            &HaikuArgs::default(),
            &fake,
            None,
            None,
            Split::All,
            None,
            &HubOptions::default(),
            &HashMap::new(),
        )
        .await
        .unwrap_err();
        assert!(e.contains("--haiku-host"), "{e}");
        assert!(fake.hosts().is_empty());
    }

    #[tokio::test]
    async fn status_map_runs_offline_on_the_fixture_or_a_labels_file() {
        let opts = HubOptions::default();
        let env = HashMap::new();
        // Neither: an error that names both.
        let e = status_map(None, false, &[], None, None, &opts, &env)
            .await
            .unwrap_err();
        assert!(e.contains("--labels") && e.contains("--fixture"), "{e}");
        // The fixture, no database anywhere.
        let r = status_map(
            None,
            true,
            &[],
            None,
            Some(Path::new("/nonexistent/state.db")),
            &opts,
            &env,
        )
        .await
        .unwrap();
        assert_eq!(r.providers, vec!["todo", "rule"]);
        assert_eq!(r.source, "fixture");
        assert!(r.sizes.cases >= 300);
        // A labels file.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sections.jsonl");
        std::fs::write(
            &file,
            "{\"section\":\"Hotovo\",\"project_sections\":[\"Nové\",\"Hotovo\"],\"expect\":\"done\",\"lang\":\"sk\"}\n",
        )
        .unwrap();
        let r = status_map(
            Some(&file),
            false,
            &["rule".into(), "none".into()],
            None,
            None,
            &opts,
            &env,
        )
        .await
        .unwrap();
        assert_eq!((r.source, r.sizes.cases), ("labels", 1));
        assert_eq!(r.providers, vec!["none", "rule"]);
        std::fs::write(&file, "{\"section\":\"x\",\"expect\":\"soon\"}\n").unwrap();
        let e = status_map(Some(&file), false, &[], None, None, &opts, &env)
            .await
            .unwrap_err();
        assert!(e.contains("sections.jsonl") && e.contains("line 1"), "{e}");
        // jev needs a database to gate and record in.
        let e = status_map(
            None,
            true,
            &["jev".into()],
            None,
            Some(Path::new("/nonexistent/state.db")),
            &opts,
            &env,
        )
        .await
        .unwrap_err();
        assert!(e.contains("/nonexistent/state.db"), "{e}");
    }

    #[tokio::test]
    async fn status_map_with_jev_is_gated_by_the_database() {
        // A fresh hub database: the flag is off, so every case is skipped
        // and nothing is sent.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        drop(Store::open_with_bus(&path, Arc::new(fleet_core::events::NoopEventBus)).unwrap());
        let r = status_map(
            None,
            true,
            &["jev".into(), "rule".into()],
            Some(10),
            Some(&path),
            &HubOptions::default(),
            &HashMap::new(),
        )
        .await
        .unwrap();
        let jev = r.metrics.iter().find(|m| m.provider == "jev").unwrap();
        assert_eq!(jev.cases, 0);
        assert_eq!(jev.calls, 0);
        assert_eq!(jev.skipped.get("flag_off").copied(), Some(r.sizes.cases));
    }

    #[test]
    fn status_map_parses_its_split_and_question_flags() {
        let Ok(BenchCmd::StatusMap {
            split, question, ..
        }) = parse(&[
            "status-map",
            "--fixture",
            "--split",
            "dev",
            "--question",
            "q.json",
        ])
        else {
            panic!("status-map");
        };
        assert_eq!(split.as_deref(), Some("dev"));
        assert_eq!(question, vec![PathBuf::from("q.json")]);
        let Ok(BenchCmd::StatusMap {
            split, question, ..
        }) = parse(&["status-map", "--fixture"])
        else {
            panic!("status-map");
        };
        assert_eq!((split, question), (None, vec![]));
        // The default is every case; a split is a board-level one.
        assert_eq!(sm_split(None), Ok(Split::All));
        assert_eq!(sm_split(Some("test")), Ok(Split::Test));
        assert!(parse(&["status-map", "--fixture", "--split", "train"]).is_err());
        assert!(sm_split(Some("train")).is_err());
    }

    const QUESTION: &str = r#"{"version": "v2-draft1",
      "instructions": "Decide the category of state.section.",
      "options": {"todo": "Not started.", "in_progress": "Started, or waiting after it started.",
                  "done": "Finished.", "not_planned": "Will not be done.", "unsure": "Cannot tell."}}"#;

    #[tokio::test]
    async fn status_map_takes_a_split_and_a_question_file() {
        let opts = HubOptions::default();
        let env = HashMap::new();
        let never = Canned::default();
        let no_haiku = HaikuArgs::default();
        let run = |split: Split| {
            super::status_map(
                None,
                true,
                &[],
                &no_haiku,
                &never,
                None,
                None,
                split,
                None,
                &opts,
                &env,
            )
        };
        // The fixture's dev and test boards: disjoint halves of every case.
        let all = run(Split::All).await.unwrap();
        let dev = run(Split::Dev).await.unwrap();
        let test = run(Split::Test).await.unwrap();
        assert_eq!((all.split, dev.split, test.split), ("all", "dev", "test"));
        assert_eq!(dev.split_sizes, all.split_sizes);
        assert_eq!(dev.sizes.cases, all.split_sizes.dev_cases);
        assert_eq!(test.sizes.cases, all.split_sizes.test_cases);
        assert_eq!(dev.sizes.cases + test.sizes.cases, all.sizes.cases);
        assert!(dev.sizes.cases > 0 && test.sizes.cases > 0);
        assert!(never.hosts().is_empty());

        // A question file reaches haiku's prompt, and the report names it.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sections.jsonl");
        std::fs::write(
            &file,
            "{\"section\":\"Hotovo\",\"project_sections\":[\"Nové\",\"Hotovo\"],\"expect\":\"done\",\"lang\":\"sk\"}\n",
        )
        .unwrap();
        let qfile = dir.path().join("q.json");
        std::fs::write(&qfile, QUESTION).unwrap();
        let db = dir.path().join("state.db");
        {
            let s = Store::open_with_bus(&db, Arc::new(fleet_core::events::NoopEventBus)).unwrap();
            s.upsert_host("gpu1").unwrap();
        }
        let envelope = serde_json::json!({
            "type": "result",
            "result": "{\"choice\": \"done\", \"confidence\": 0.9}",
            "usage": { "input_tokens": 50, "output_tokens": 5 },
            "total_cost_usd": 0.001,
        });
        let canned = || Canned {
            stdout: format!("fleet-haiku=run\n{envelope}\n"),
            ..Default::default()
        };
        let gpu1 = HaikuArgs {
            haiku_host: Some("gpu1".into()),
            ..Default::default()
        };
        let haiku = ["haiku".to_string()];
        let fake = canned();
        let r = super::status_map(
            Some(&file),
            false,
            &haiku,
            &gpu1,
            &fake,
            None,
            Some(&db),
            Split::All,
            Some(&qfile),
            &opts,
            &env,
        )
        .await
        .unwrap();
        let stdins = fake.stdins.lock().unwrap().clone();
        assert_eq!(stdins.len(), 1);
        assert!(
            stdins[0].contains("Decide the category of state.section.")
                && stdins[0].contains("waiting after it started")
                && stdins[0].contains("hotovo"),
            "{stdins:?}"
        );
        assert_eq!(r.question, "file v2-draft1");
        assert_eq!(r.question_version, "status_map.bench.q.v2-draft1");

        // A bad file is refused, naming itself, before anything is sent.
        let bad = dir.path().join("bad.json");
        std::fs::write(&bad, QUESTION.replace("\"not_planned\"", "\"wontfix\"")).unwrap();
        let fake = canned();
        let e = super::status_map(
            Some(&file),
            false,
            &haiku,
            &gpu1,
            &fake,
            None,
            Some(&db),
            Split::All,
            Some(&bad),
            &opts,
            &env,
        )
        .await
        .unwrap_err();
        assert!(e.contains("bad.json") && e.contains("wontfix"), "{e}");
        assert!(fake.hosts().is_empty());
        let e = super::status_map(
            Some(&file),
            false,
            &haiku,
            &gpu1,
            &fake,
            None,
            Some(&db),
            Split::All,
            Some(&dir.path().join("absent.json")),
            &opts,
            &env,
        )
        .await
        .unwrap_err();
        assert!(e.contains("absent.json"), "{e}");
        assert!(fake.hosts().is_empty());
        // Only a model reads a question: with offline providers it is an
        // error, not a report that names a question nobody was asked.
        let e = super::status_map(
            Some(&file),
            false,
            &["rule".to_string()],
            &HaikuArgs::default(),
            &never,
            None,
            None,
            Split::All,
            Some(&qfile),
            &opts,
            &env,
        )
        .await
        .unwrap_err();
        assert!(e.contains("--question"), "{e}");
    }

    #[test]
    fn an_export_needs_a_file_and_is_never_a_benchmark_run() {
        assert!(parse(&["work-link", "--export-unlinked", "150"]).is_err());
        assert!(parse(&["work-link", "--out", "x.jsonl"]).is_err());
        assert!(parse(&["work-link", "--export-unlinked", "150", "--out", "x.jsonl"]).is_ok());
        assert!(parse(&[
            "work-link",
            "--export-unlinked",
            "150",
            "--out",
            "x.jsonl",
            "--provider",
            "jev"
        ])
        .is_err());
        assert!(parse(&[
            "work-link",
            "--export-unlinked",
            "150",
            "--out",
            "x.jsonl",
            "--labels",
            "y.jsonl"
        ])
        .is_err());
    }

    /// A hub database with three person-decided links and one unlinked
    /// session, written through the store's public API only.
    fn seeded_db(dir: &Path) -> PathBuf {
        use fleet_core::store::{StartSource, WorkTarget};
        let path = dir.join("state.db");
        let s = Store::open_with_bus(&path, Arc::new(fleet_core::events::NoopEventBus)).unwrap();
        s.upsert_host("h1").unwrap();
        let session = |name: &str, cid: &str, prompt: &str| {
            let sid = s
                .upsert_bg_session("h1", name, None, cid, None, now_unix(), "bg", now_unix())
                .unwrap();
            s.rebind_conversation(sid, cid, StartSource::Startup, None, None)
                .unwrap();
            s.conversation_set_first_prompt(sid, cid, prompt).unwrap();
            sid
        };
        for (i, (key, title, prompt)) in [
            (
                "LOC-1",
                "Login redirect loops",
                "the login keeps redirecting",
            ),
            ("LOC-2", "Billing export to CSV", "export billing as csv"),
            (
                "LOC-3",
                "Upgrade database driver",
                "bump the database driver",
            ),
        ]
        .iter()
        .enumerate()
        {
            let item = s.create_local_work_item(Some(key), title).unwrap().id;
            let sid = session(&format!("s{i}"), &format!("c{i}"), prompt);
            s.link_session_work(sid, WorkTarget::Item(item), "manual")
                .unwrap();
        }
        session("u1", "u1", "look at the csv billing thing");
        path
    }

    #[tokio::test]
    async fn a_run_reads_a_database_and_an_export_is_new_and_private() {
        let dir = tempfile::tempdir().unwrap();
        let db = seeded_db(dir.path());
        let opts = HubOptions::default();
        let env = HashMap::new();
        let db_s = db.to_str().unwrap();
        run(
            parse(&["work-link", "--split", "all", "--db", db_s, "--json"]).unwrap(),
            &opts,
            &env,
        )
        .await
        .unwrap();
        // haiku without its host: refused before anything is read or sent.
        let e = run(
            parse(&["work-link", "--db", db_s, "--provider", "haiku"]).unwrap(),
            &opts,
            &env,
        )
        .await
        .unwrap_err();
        assert!(e.contains("--haiku-host"), "{e}");
        let e = run(
            parse(&["work-link", "--db", db_s, "--haiku-host", "gpu1"]).unwrap(),
            &opts,
            &env,
        )
        .await
        .unwrap_err();
        assert!(e.contains("--provider haiku"), "{e}");
        let file = dir.path().join("h.jsonl");
        let file_s = file.to_str().unwrap();
        let export = || {
            parse(&[
                "work-link",
                "--db",
                db_s,
                "--export-unlinked",
                "5",
                "--out",
                file_s,
            ])
            .unwrap()
        };
        run(export(), &opts, &env).await.unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let rows = wl::parse_labels(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].label.is_none());
        let e = run(export(), &opts, &env).await.unwrap_err();
        assert!(e.contains("never written over"), "{e}");
        // The labels file reads back into a run.
        run(
            parse(&["work-link", "--db", db_s, "--labels", file_s]).unwrap(),
            &opts,
            &env,
        )
        .await
        .unwrap();
        let e = run(
            parse(&["work-link", "--db", "/nonexistent/state.db"]).unwrap(),
            &opts,
            &env,
        )
        .await
        .unwrap_err();
        assert!(e.contains("/nonexistent/state.db"), "{e}");
    }

    // --- perturbations, the paired set, the floor sweep, the question set ---------

    #[test]
    fn status_map_parses_its_diagnostic_flags() {
        let Ok(BenchCmd::StatusMap {
            perturb,
            floor_sweep,
            paired_fixture,
            question_set,
            ..
        }) = parse(&[
            "status-map",
            "--paired-fixture",
            "--perturb",
            "typo",
            "--perturb",
            "no-board",
            "--floor-sweep",
            "--question-set",
        ])
        else {
            panic!("status-map");
        };
        assert!(floor_sweep && paired_fixture && question_set);
        assert_eq!(
            parse_perturb(&perturb).unwrap(),
            vec![Perturbation::Typo, Perturbation::NoBoard]
        );
        // `code` is work-link's; nothing else is a perturbation.
        assert!(parse(&["status-map", "--fixture", "--perturb", "code"]).is_err());
        assert!(parse(&["status-map", "--fixture", "--perturb", "upper"]).is_err());
        assert!(parse(&["status-map", "--fixture", "--paired-fixture"]).is_err());
        assert!(parse(&["status-map", "--labels", "x.jsonl", "--paired-fixture"]).is_err());
    }

    async fn sm_with(
        x: &SmExtras<'_>,
        providers: &[&str],
        haiku: &HaikuArgs,
        ssh: &dyn fleet_core::ssh::SshExec,
        db: Option<&Path>,
        split: Split,
    ) -> Result<Vec<sm::Report>, String> {
        let providers: Vec<String> = providers.iter().map(|p| p.to_string()).collect();
        status_map_with(
            None,
            x,
            &providers,
            haiku,
            ssh,
            None,
            db,
            split,
            &HubOptions::default(),
            &HashMap::new(),
        )
        .await
    }

    #[tokio::test]
    async fn status_map_perturbs_and_compares_languages_offline() {
        let never = Canned::default();
        let x = SmExtras {
            fixture: Some(SmFixture::Paired),
            perturb: vec![Perturbation::Fold, Perturbation::Typo],
            ..Default::default()
        };
        let r = sm_with(
            &x,
            &["rule"],
            &HaikuArgs::default(),
            &never,
            None,
            Split::All,
        )
        .await
        .unwrap();
        assert_eq!(r.len(), 1);
        let r = &r[0];
        assert_eq!(r.source, "paired-fixture");
        assert_eq!(r.sizes.cases, 324);
        assert!(r
            .notes
            .iter()
            .any(|n| n.contains(robust::PAIRED_FIXTURE_PATH)));
        assert!(!r.notes.iter().any(|n| n.contains(sm::FIXTURE_PATH)));
        let l = r.languages.as_ref().unwrap();
        assert_eq!(l.pairs, 81);
        let kinds: Vec<&str> = r.robustness.iter().map(|x| x.perturbation).collect();
        assert_eq!(kinds, vec!["fold", "typo"]);
        assert!(r.robustness.iter().all(|x| x.changed > 0));
        let text = r.lines().join("\n");
        assert!(text.contains("robustness (dataset C") && text.contains("languages (dataset B"));
        assert!(never.hosts().is_empty());
    }

    #[tokio::test]
    async fn the_sweep_and_the_question_set_need_a_model_and_the_set_needs_dev() {
        let never = Canned::default();
        let no = HaikuArgs::default();
        let sweep = SmExtras {
            fixture: Some(SmFixture::Sections),
            floor_sweep: true,
            ..Default::default()
        };
        let e = sm_with(&sweep, &["rule"], &no, &never, None, Split::All)
            .await
            .unwrap_err();
        assert!(e.contains("--floor-sweep"), "{e}");
        let set = SmExtras {
            fixture: Some(SmFixture::Sections),
            question_set: true,
            ..Default::default()
        };
        let e = sm_with(&set, &["rule"], &no, &never, None, Split::Dev)
            .await
            .unwrap_err();
        assert!(e.contains("--question-set"), "{e}");
        let e = sm_with(&set, &["jev"], &no, &never, None, Split::All)
            .await
            .unwrap_err();
        assert!(e.contains("--split dev"), "{e}");
        assert!(never.hosts().is_empty());
    }

    #[tokio::test]
    async fn the_question_set_asks_every_wording_on_dev_and_compares_them() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("state.db");
        {
            let s = Store::open_with_bus(&db, Arc::new(fleet_core::events::NoopEventBus)).unwrap();
            s.upsert_host("gpu1").unwrap();
        }
        let envelope = serde_json::json!({
            "type": "result",
            "result": "{\"choice\": \"done\", \"confidence\": 0.9}",
            "usage": { "input_tokens": 50, "output_tokens": 5 },
            "total_cost_usd": 0.001,
        });
        let fake = Canned {
            stdout: format!("fleet-haiku=run\n{envelope}\n"),
            ..Default::default()
        };
        let gpu1 = HaikuArgs {
            haiku_host: Some("gpu1".into()),
            ..Default::default()
        };
        let x = SmExtras {
            fixture: Some(SmFixture::Sections),
            question_set: true,
            floor_sweep: true,
            ..Default::default()
        };
        let reports = sm_with(&x, &["haiku"], &gpu1, &fake, Some(&db), Split::Dev)
            .await
            .unwrap();
        assert_eq!(reports.len(), 1 + robust::QUESTION_SET.len());
        assert_eq!(
            reports[0].question,
            format!("the adapter's {}", "status_map.v1")
        );
        let dev = reports[0].sizes.cases as usize;
        // Every wording asked every dev case once, and its words went out.
        assert_eq!(fake.hosts().len(), dev * reports.len());
        let stdins = fake.stdins.lock().unwrap().clone();
        for (_, json) in robust::QUESTION_SET {
            let q = sm::parse_question(json).unwrap();
            assert!(stdins.iter().any(|p| p.contains(&q.instructions[..60])));
        }
        let rows: Vec<robust::QuestionRow> =
            reports.iter().flat_map(robust::question_rows).collect();
        let names: Vec<&str> = rows.iter().map(|r| r.question.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "adapter",
                "v2-position",
                "v2-multilingual",
                "v2-careful-done"
            ]
        );
        // The sweep is there, without a dev choice (dev only).
        assert_eq!(reports[0].floor_sweep.len(), 1);
        assert!(reports[0].floor_sweep[0].dev_choice.is_none());
    }
}
