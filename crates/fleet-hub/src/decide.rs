//! `fleet-hub decide …` — the hub operator's side of the decision envelope
//! (Jev evaluation, D35–D37; `fleet_core::service::decide`,
//! `docs/decisions.md`).
//!
//! * `set-key` / `clear-key` write the hub's database directly: the key
//!   never travels through a tool argument or reply, and the running hub
//!   reads it at its next call.
//!   **The key is never an argument** (`ps`, shell history): it is read from
//!   stdin, from a variable named by `--from-env`, or stored as a reference
//!   (`--ref env:NAME` / `--ref file:/run/secrets/jev`) the hub resolves at
//!   use.
//! * `status` and `runs` open the database read-only, like `fleet-hub
//!   census`: no running hub needed, nothing written, and nothing printed
//!   but ids, words and numbers — never the key.
//! * `proposals` (J3, `status_map`) reads the same way; the section names it
//!   prints come from the trackers' stored config, never from the runs.
//!   `proposals apply <run> [--as CATEGORY]` writes the tracker's section
//!   map over the running hub's `work_admin update`, like `fleet-hub tracker
//!   section-map`; `proposals reject <run>` writes only the run's follow-up,
//!   directly, like `set-key`.
//! * `enable` / `disable`, `mode`, `unassigned` and `set` change the
//!   `decide.*` settings over the running hub's `set_setting` (master token,
//!   loopback), like `fleet-hub org set` does for orgs: the hub validates
//!   the value and audits the change, and there is no other way to reach
//!   them on a hub (a paired desktop shows them read-only).

use crate::config::{self, HubOptions};
use crate::out;
use crate::serve;
use clap::{Subcommand, ValueEnum};
use fleet_core::service::decide;
use fleet_core::service::decide::status_map::{self, ProposalAction, ProposalOutcome};
use fleet_core::store::{DecisionRunFilter, DecisionRunRow, Secret, Store};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Subcommand, Debug)]
pub enum DecideCmd {
    /// Turn the kill switch on (`decide.jev.enabled`). Nothing is sent
    /// until a feature's mode (`fleet-hub decide mode`), an org's consent
    /// (`fleet-hub org set <id> --jev on`) and a key allow it too. Needs a
    /// running hub.
    Enable,
    /// Turn the kill switch off: every call stops at once, nothing in
    /// flight is retried. Needs a running hub.
    Disable,
    /// Set a feature's mode (`decide.jev.<feature>`): off, shadow (ask and
    /// record next to the rule, act on the rule) or assist (also propose
    /// the answer for a person to confirm). Needs a running hub.
    Mode {
        /// status_map or work_link.
        feature: String,
        /// off, shadow or assist.
        mode: String,
    },
    /// Whether sessions and items that belong to no org may be sent
    /// (`decide.jev.unassigned`, off by default). Needs a running hub.
    Unassigned {
        #[arg(value_enum)]
        value: Switch,
    },
    /// Set any other `decide.*` setting, e.g. `decide.jev.timeout_ms 2000`
    /// or `decide.jev.daily_token_budget 500000`; the hub checks the value.
    /// The table is in docs/decisions.md → Settings. Needs a running hub.
    Set {
        /// A key starting with `decide.`.
        key: String,
        value: String,
    },
    /// Store the TypeSafe (Jev) API key. Read from stdin (one line) unless
    /// --from-env or --ref says otherwise; never an argument. Nothing is
    /// sent until `decide.jev.enabled`, a feature's mode and an org's
    /// consent all allow it.
    SetKey {
        /// Read the key from this environment variable of THIS command
        /// (e.g. `read -rs JEV_KEY; export JEV_KEY`), and store it.
        #[arg(long, conflicts_with = "reference")]
        from_env: Option<String>,
        /// Store a reference instead of the key: env:NAME or
        /// file:/run/secrets/jev, read by the hub at each call.
        #[arg(long = "ref")]
        reference: Option<String>,
    },
    /// Forget the API key. Calls stop (`no_key`); the record stays.
    ClearKey,
    /// The flag, the modes, which orgs consented, whether a key is
    /// configured (never the key), the breaker, today's tokens and cost, and
    /// the runs per feature, provider, fallback and org.
    Status {
        /// The window of the run counts in days (1-365). [default: 7]
        #[arg(long)]
        days: Option<i64>,
        /// Read this database file instead of the hub's (a desktop's).
        #[arg(long, value_name = "FILE")]
        db: Option<PathBuf>,
        /// Print JSON instead of lines.
        #[arg(long)]
        json: bool,
    },
    /// Offline benchmarks of a use case (Jev evaluation phase 0): read-only
    /// and offline unless `--provider jev` or `--provider haiku` is given.
    /// See docs/decisions.md → "Benchmarking work_link".
    Bench {
        #[command(subcommand)]
        cmd: Box<crate::bench::BenchCmd>,
    },
    /// The most recent decision runs, newest first: ids, words and numbers
    /// only.
    Runs {
        /// Only this feature (status_map, work_link).
        #[arg(long)]
        feature: Option<String>,
        /// Rows to show (1-1000). [default: 50]
        #[arg(long)]
        limit: Option<u32>,
        /// Read this database file instead of the hub's (a desktop's).
        #[arg(long, value_name = "FILE")]
        db: Option<PathBuf>,
        /// Print JSON instead of lines.
        #[arg(long)]
        json: bool,
    },
    /// The status_map proposals (J3): per Asana tracker, the latest assist
    /// answer per section as `section → category (confidence)`, with the
    /// command a person runs to apply them, and in shadow the agreement
    /// with the keyword rule. Reads the database only; applies nothing.
    /// `apply <run>` / `reject <run>` decide one proposal.
    #[command(args_conflicts_with_subcommands = true)]
    Proposals {
        #[command(subcommand)]
        action: Option<ProposalCmd>,
        /// Only this tracker.
        #[arg(long)]
        tracker: Option<i64>,
        /// Read this database file instead of the hub's (a desktop's).
        #[arg(long, value_name = "FILE")]
        db: Option<PathBuf>,
        /// Print JSON instead of lines.
        #[arg(long)]
        json: bool,
    },
}

/// `on` / `off` for a boolean `decide.*` setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Switch {
    On,
    Off,
}

/// The `decide.*` setting a settings subcommand writes, checked before the
/// hub is called: `None` for every other subcommand.
fn setting_for(cmd: &DecideCmd) -> Result<Option<(String, String)>, String> {
    use fleet_core::service::settings as st;
    let pair = |k: &str, v: &str| Ok(Some((k.to_string(), v.to_string())));
    match cmd {
        DecideCmd::Enable => pair(st::DECIDE_JEV_ENABLED, "true"),
        DecideCmd::Disable => pair(st::DECIDE_JEV_ENABLED, "false"),
        DecideCmd::Mode { feature, mode } => {
            let f = decide::Feature::parse(feature).ok_or_else(|| {
                format!(
                    "the feature is one of {}, not {feature:?}",
                    decide::Feature::ALL
                        .iter()
                        .map(|f| f.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;
            if !st::DECIDE_MODES.contains(&mode.as_str()) {
                return Err(format!(
                    "the mode is one of {}, not {mode:?} (auto is not offered: no feature has passed acceptance)",
                    st::DECIDE_MODES.join(", ")
                ));
            }
            pair(f.setting_key(), mode)
        }
        DecideCmd::Unassigned { value } => pair(
            st::DECIDE_JEV_UNASSIGNED,
            if *value == Switch::On {
                "true"
            } else {
                "false"
            },
        ),
        DecideCmd::Set { key, value } => {
            if !key.starts_with("decide.") {
                return Err(format!(
                    "`fleet-hub decide set` changes decide.* settings only, not {key:?}"
                ));
            }
            pair(key, value)
        }
        _ => Ok(None),
    }
}

/// What still stops a call after a settings change, in one line.
fn after_setting(key: &str, value: &str) -> String {
    use fleet_core::service::settings as st;
    match (key, value) {
        (k, "false") if k == st::DECIDE_JEV_ENABLED => {
            "every call is stopped; the record and the key stay".to_string()
        }
        (k, "true") if k == st::DECIDE_JEV_ENABLED => "a call also needs an org's consent \
             (fleet-hub org set <id> --jev on) and a key (fleet-hub decide set-key); a live \
             feature also its mode (fleet-hub decide mode status_map shadow), which a benchmark \
             does not need; fleet-hub decide status shows what is missing"
            .to_string(),
        _ => "fleet-hub decide status shows the flag, the modes, the consents and the key"
            .to_string(),
    }
}

/// `fleet-hub decide proposals apply|reject <run>`: a person decides one
/// proposal, named by its decision run (the `run N` of the listing).
#[derive(Subcommand, Debug)]
pub enum ProposalCmd {
    /// Put the proposal's category (not_planned → done), or --as another
    /// one (a correction), into the tracker's section map and confirm it
    /// — over the running hub's work_admin update, as `fleet-hub tracker
    /// section-map` does. The run is marked confirmed or corrected.
    Apply {
        /// The proposal's run id.
        run: i64,
        /// todo, in_progress or done instead of the proposal's category.
        #[arg(long = "as", value_name = "CATEGORY")]
        category: Option<String>,
    },
    /// "Not this": mark the run rejected. The section stays unmapped, and
    /// the answer is not proposed again until a new one exists (another
    /// input, question version or model). Writes the hub's database only.
    Reject {
        /// The proposal's run id.
        run: i64,
    },
}

/// The action a `ProposalCmd` names, and its run.
fn proposal_action(cmd: &ProposalCmd) -> Result<(i64, ProposalAction), String> {
    match cmd {
        ProposalCmd::Apply { run, category } => {
            let action = match category {
                None => ProposalAction::parse("apply", None),
                Some(c) => ProposalAction::parse("apply_as", Some(c)),
            }
            .map_err(|e| e.message)?;
            Ok((*run, action))
        }
        ProposalCmd::Reject { run } => Ok((*run, ProposalAction::Reject)),
    }
}

/// One decided proposal as a line.
fn outcome_line(o: &ProposalOutcome) -> String {
    match (&o.category, o.followup.as_deref()) {
        (None, _) => format!(
            "rejected run {}: {:?} of tracker {} stays unmapped; not proposed again until a new \
             answer",
            o.run_id, o.section, o.tracker_id
        ),
        (Some(c), f) => format!(
            "tracker {}: {:?}={c} in your section map (run {} {})",
            o.tracker_id,
            o.section,
            o.run_id,
            match (f, &o.corrected_to) {
                (Some(f), Some(to)) => format!("{f} to {to}"),
                (Some(f), None) => f.to_string(),
                (None, _) => "not marked".into(),
            }
        ),
    }
}

/// `apply` / `reject`. A rejection writes only the run's follow-up, in the
/// hub's database, like `set-key`. An apply reads the proposal from the
/// database, writes the tracker's settings over the running hub's
/// `work_admin update` (as `fleet-hub tracker section-map`: the hub's
/// validation, event and follow-up), then makes sure the run carries its
/// follow-up.
async fn decide_one(
    cmd: ProposalCmd,
    opts: &HubOptions,
    env: &HashMap<String, String>,
) -> Result<ProposalOutcome, String> {
    let (run, action) = proposal_action(&cmd)?;
    serve::existing_db(&config::resolve_data_dir(opts, env))?;
    if action == ProposalAction::Reject {
        let store = serve::open_store(opts, env)?;
        return status_map::reject_proposal(&store, run, now()).map_err(|e| e.message);
    }
    let (p, category) = {
        let ro = open_read_only(&db_path(None, opts, env)?)?;
        let p = status_map::pending_proposal(&ro, run).map_err(|e| e.message)?;
        let category = p
            .category(&action)
            .map_err(|e| e.message)?
            .ok_or("an apply needs a category")?;
        (p, category)
    };
    let conn = crate::pair::hub_conn(opts, env)?;
    let listed =
        crate::pair::call_tool(&conn, "work_admin", serde_json::json!({ "action": "list" }))
            .await?;
    let row: fleet_core::store::TrackerRow = listed
        .as_array()
        .into_iter()
        .flatten()
        .find(|t| t["id"].as_i64() == Some(p.tracker.id))
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|e| format!("read the tracker: {e}"))?
        .ok_or_else(|| format!("no tracker {}", p.tracker.id))?;
    let args = status_map::apply_one_args(&row, &p.section, &category);
    crate::pair::call_tool(&conn, "work_admin", args).await?;
    let store = serve::open_store(opts, env)?;
    status_map::record_applied(&store, &p, &action, &category, now()).map_err(|e| e.message)
}

/// Where `set-key` gets the key from.
#[derive(Debug, PartialEq, Eq)]
enum Source {
    Stdin,
    Env(String),
    Reference(String),
}

fn source(from_env: Option<String>, reference: Option<String>) -> Source {
    match (from_env, reference) {
        (_, Some(r)) => Source::Reference(r),
        (Some(v), None) => Source::Env(v),
        (None, None) => Source::Stdin,
    }
}

/// What to store: `(key, reference)`, exactly one set. `read_stdin` is
/// injected so a test never touches a terminal.
fn key_to_store(
    src: &Source,
    env: &HashMap<String, String>,
    read_stdin: impl FnOnce() -> Result<String, String>,
) -> Result<(Option<Secret>, Option<String>), String> {
    match src {
        Source::Reference(r) => Ok((None, Some(r.clone()))),
        Source::Env(var) => env
            .get(var)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .map(|v| (Some(Secret::new(v)), None))
            .ok_or_else(|| format!("${var} is not set (or empty) in this shell")),
        Source::Stdin => {
            let v = read_stdin()?.trim().to_string();
            if v.is_empty() {
                return Err("no key on stdin; pipe it in, or use --from-env / --ref".into());
            }
            Ok((Some(Secret::new(v)), None))
        }
    }
}

fn read_one_line() -> Result<String, String> {
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| format!("read the key from stdin: {e}"))?;
    Ok(line)
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `--db`, else the hub's own `state.db` (which must exist).
fn db_path(
    db: Option<PathBuf>,
    opts: &HubOptions,
    env: &HashMap<String, String>,
) -> Result<PathBuf, String> {
    match db {
        Some(p) if p.is_file() => Ok(p),
        Some(p) => Err(format!("no database at {}", p.display())),
        None => serve::existing_db(&config::resolve_data_dir(opts, env)),
    }
}

fn open_read_only(path: &Path) -> Result<Store, String> {
    Store::open_read_only(path).map_err(|e| format!("open {} read-only: {e}", path.display()))
}

/// One run as a line: ids, words and numbers only.
fn run_line(r: &DecisionRunRow) -> String {
    let fmt_f = |v: Option<f64>| v.map(|c| format!("{c:.2}")).unwrap_or_else(|| "-".into());
    format!(
        "{:>6}  {}  {:<10} {:<6} org {:<4} {}:{}  {:<6} {:<15} answer {} (conf {})  baseline {}  {} tok  {} ms  model {}  q {}{}",
        r.id,
        crate::pair::fmt_time(Some(r.at)),
        r.feature,
        r.mode,
        r.org_id.map(|o| o.to_string()).unwrap_or_else(|| "-".into()),
        r.subject_kind,
        r.subject_id,
        r.provider,
        r.fallback.as_deref().unwrap_or("answered"),
        r.answer.as_deref().unwrap_or("-"),
        fmt_f(r.confidence),
        r.baseline_answer.as_deref().unwrap_or("-"),
        r.input_tokens,
        r.latency_ms.map(|l| l.to_string()).unwrap_or_else(|| "-".into()),
        r.model_version.as_deref().unwrap_or("-"),
        r.question_version,
        match (&r.followup, &r.corrected_to) {
            (Some(f), Some(c)) => format!("  → {f} {c}"),
            (Some(f), None) => format!("  → {f}"),
            _ => String::new(),
        }
    )
}

pub async fn run(
    cmd: DecideCmd,
    opts: &HubOptions,
    env: &HashMap<String, String>,
) -> Result<ExitCode, String> {
    if let Some((key, value)) = setting_for(&cmd)? {
        let conn = crate::pair::hub_conn(opts, env)?;
        crate::pair::call_tool(
            &conn,
            "set_setting",
            serde_json::json!({ "key": key, "value": value }),
        )
        .await?;
        out::line(&format!("{key} = {value}"));
        out::line(&after_setting(&key, &value));
        return Ok(ExitCode::SUCCESS);
    }
    match cmd {
        DecideCmd::Enable
        | DecideCmd::Disable
        | DecideCmd::Mode { .. }
        | DecideCmd::Unassigned { .. }
        | DecideCmd::Set { .. } => unreachable!("settings subcommands return above"),
        DecideCmd::Bench { cmd } => return crate::bench::run(*cmd, opts, env).await,
        DecideCmd::SetKey {
            from_env,
            reference,
        } => {
            let (key, reference) = key_to_store(&source(from_env, reference), env, read_one_line)?;
            serve::existing_db(&config::resolve_data_dir(opts, env))?;
            let store = serve::open_store(opts, env)?;
            store
                .set_decision_credential(key.as_ref(), reference.as_deref())
                .map_err(|e| e.message)?;
            let st = decide::status(&store, now(), 1).map_err(|e| e.message)?;
            out::line(if reference.is_some() {
                "stored a reference to the Jev key; the hub reads it at each call"
            } else {
                "stored the Jev key"
            });
            if !st.enabled {
                out::line(
                    "decide.jev.enabled is off: nothing is sent until it is on, a feature's \
                     mode is shadow or assist, and an org consents (fleet-hub org set <id> --jev on)",
                );
            }
        }
        DecideCmd::ClearKey => {
            serve::existing_db(&config::resolve_data_dir(opts, env))?;
            let store = serve::open_store(opts, env)?;
            let was = store.clear_decision_credential().map_err(|e| e.message)?;
            out::line(if was {
                "cleared the Jev key; no call is made until a new one is set"
            } else {
                "no Jev key was set"
            });
        }
        DecideCmd::Status { days, db, json } => {
            let days = days.unwrap_or(7);
            if !(1..=365).contains(&days) {
                return Err(format!("--days must be between 1 and 365, got {days}"));
            }
            let store = open_read_only(&db_path(db, opts, env)?)?;
            let st = decide::status(&store, now(), days).map_err(|e| e.message)?;
            if json {
                out::line(&serde_json::to_string_pretty(&st).map_err(|e| e.to_string())?);
            } else {
                for l in st.lines() {
                    out::line(&l);
                }
            }
        }
        DecideCmd::Runs {
            feature,
            limit,
            db,
            json,
        } => {
            if let Some(f) = &feature {
                if decide::Feature::parse(f).is_none() {
                    return Err(format!(
                        "--feature is one of {}, not {f:?}",
                        decide::Feature::ALL
                            .iter()
                            .map(|f| f.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
            }
            let limit = limit.unwrap_or(50);
            if !(1..=1000).contains(&limit) {
                return Err(format!("--limit must be between 1 and 1000, got {limit}"));
            }
            let store = open_read_only(&db_path(db, opts, env)?)?;
            let rows = store
                .list_decision_runs(&DecisionRunFilter {
                    feature,
                    limit,
                    ..Default::default()
                })
                .map_err(|e| e.message)?;
            if json {
                out::line(&serde_json::to_string_pretty(&rows).map_err(|e| e.to_string())?);
            } else if rows.is_empty() {
                out::line("no decision runs recorded");
            } else {
                for r in &rows {
                    out::line(&run_line(r));
                }
            }
        }
        DecideCmd::Proposals {
            action: Some(cmd),
            json,
            ..
        } => {
            let o = decide_one(cmd, opts, env).await?;
            if json {
                out::line(&serde_json::to_string_pretty(&o).map_err(|e| e.to_string())?);
            } else {
                out::line(&outcome_line(&o));
            }
        }
        DecideCmd::Proposals {
            action: None,
            tracker,
            db,
            json,
        } => {
            let store = open_read_only(&db_path(db, opts, env)?)?;
            let all = decide::status_map::proposals(&store, tracker).map_err(|e| e.message)?;
            if json {
                out::line(&serde_json::to_string_pretty(&all).map_err(|e| e.to_string())?);
            } else if all.is_empty() {
                out::line("no Asana trackers");
            } else {
                for l in all.iter().flat_map(|t| t.lines()) {
                    out::line(&l);
                }
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct T {
        #[command(subcommand)]
        cmd: DecideCmd,
    }

    fn parse(args: &[&str]) -> Result<DecideCmd, clap::Error> {
        let mut all = vec!["t"];
        all.extend_from_slice(args);
        T::try_parse_from(all).map(|t| t.cmd)
    }

    fn setting(args: &[&str]) -> Result<Option<(String, String)>, String> {
        setting_for(&parse(args).map_err(|e| e.to_string())?)
    }

    fn kv(k: &str, v: &str) -> Option<(String, String)> {
        Some((k.to_string(), v.to_string()))
    }

    #[test]
    fn the_settings_subcommands_name_their_decide_key() {
        assert_eq!(
            setting(&["enable"]).unwrap(),
            kv("decide.jev.enabled", "true")
        );
        assert_eq!(
            setting(&["disable"]).unwrap(),
            kv("decide.jev.enabled", "false")
        );
        assert_eq!(
            setting(&["mode", "status_map", "shadow"]).unwrap(),
            kv("decide.jev.status_map", "shadow")
        );
        assert_eq!(
            setting(&["mode", "work_link", "off"]).unwrap(),
            kv("decide.jev.work_link", "off")
        );
        assert_eq!(
            setting(&["unassigned", "on"]).unwrap(),
            kv("decide.jev.unassigned", "true")
        );
        assert_eq!(
            setting(&["unassigned", "off"]).unwrap(),
            kv("decide.jev.unassigned", "false")
        );
        assert_eq!(
            setting(&["set", "decide.jev.timeout_ms", "2000"]).unwrap(),
            kv("decide.jev.timeout_ms", "2000")
        );
        // Every other subcommand writes no setting.
        assert_eq!(setting(&["status"]).unwrap(), None);
        assert_eq!(setting(&["clear-key"]).unwrap(), None);
    }

    #[test]
    fn a_bad_feature_mode_or_key_is_refused_before_the_hub_is_called() {
        let e = setting(&["mode", "tidy", "shadow"]).unwrap_err();
        assert!(e.contains("status_map, work_link"), "{e}");
        let e = setting(&["mode", "status_map", "auto"]).unwrap_err();
        assert!(
            e.contains("off, shadow, assist") && e.contains("auto is not offered"),
            "{e}"
        );
        let e = setting(&["set", "work.auto_tidy", "true"]).unwrap_err();
        assert!(e.contains("decide.* settings only"), "{e}");
        assert!(parse(&["unassigned", "maybe"]).is_err());
        assert!(parse(&["mode", "status_map"]).is_err());
    }

    #[test]
    fn after_a_change_the_cli_says_what_else_a_call_needs() {
        let on = after_setting("decide.jev.enabled", "true");
        assert!(on.contains("fleet-hub decide mode"), "{on}");
        assert!(
            !on.contains("  "),
            "no runs of spaces from a wrapped literal: {on}"
        );
        assert!(after_setting("decide.jev.enabled", "false").contains("every call is stopped"));
        assert!(
            after_setting("decide.jev.status_map", "shadow").contains("fleet-hub decide status")
        );
    }

    const KEY: &str = "tsk_live_0123456789abcdefghijklmnop";

    #[test]
    fn the_key_is_never_an_argument() {
        // No positional key, no --key: clap refuses both.
        assert!(parse(&["set-key", KEY]).is_err());
        assert!(parse(&["set-key", "--key", KEY]).is_err());
        assert!(parse(&["set-key", "--from-env", "A", "--ref", "env:B"]).is_err());
        assert!(matches!(
            parse(&["set-key"]).unwrap(),
            DecideCmd::SetKey {
                from_env: None,
                reference: None
            }
        ));
    }

    #[test]
    fn the_key_comes_from_stdin_the_environment_or_a_reference() {
        let env = HashMap::from([("JEV_KEY".to_string(), format!(" {KEY}\n"))]);
        let (k, r) = key_to_store(&Source::Stdin, &env, || Ok(format!("{KEY}\n"))).unwrap();
        assert_eq!(k.unwrap().expose(), KEY);
        assert!(r.is_none());
        let (k, _) = key_to_store(&Source::Env("JEV_KEY".into()), &env, || {
            panic!("stdin is not read for --from-env")
        })
        .unwrap();
        assert_eq!(k.unwrap().expose(), KEY);
        assert!(key_to_store(&Source::Env("MISSING".into()), &env, || Ok(String::new())).is_err());
        assert!(key_to_store(&Source::Stdin, &env, || Ok("  \n".into())).is_err());
        let (k, r) = key_to_store(
            &Source::Reference("file:/run/secrets/jev".into()),
            &env,
            || panic!("stdin is not read for --ref"),
        )
        .unwrap();
        assert!(k.is_none());
        assert_eq!(r.as_deref(), Some("file:/run/secrets/jev"));
        assert_eq!(source(None, None), Source::Stdin);
        assert_eq!(source(Some("A".into()), None), Source::Env("A".into()));
    }

    #[test]
    fn status_and_runs_read_a_database_and_never_print_the_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        {
            let s =
                Store::open_with_bus(&path, std::sync::Arc::new(fleet_core::events::NoopEventBus))
                    .unwrap();
            s.set_decision_credential(Some(&Secret::new(KEY)), None)
                .unwrap();
            s.insert_decision_run(&fleet_core::store::NewDecisionRun {
                at: now(),
                feature: "work_link".into(),
                subject_kind: "session".into(),
                subject_id: "7".into(),
                mode: "off".into(),
                provider: "jev".into(),
                question_version: "wl.1".into(),
                fallback: Some("flag_off".into()),
                ..Default::default()
            })
            .unwrap();
        }
        let ro = open_read_only(&path).unwrap();
        let st = decide::status(&ro, now(), 7).unwrap();
        assert!(st.key.configured);
        let text = format!(
            "{}\n{}",
            st.lines().join("\n"),
            serde_json::to_string(&st).unwrap()
        );
        assert!(!text.contains(KEY));
        assert!(text.contains("flag_off"), "{text}");
        let rows = ro
            .list_decision_runs(&DecisionRunFilter {
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        let line = run_line(&rows[0]);
        assert!(
            line.contains("work_link") && line.contains("session:7"),
            "{line}"
        );
        assert!(line.contains("flag_off"), "{line}");
        assert!(db_path(
            Some(dir.path().join("nope.db")),
            &HubOptions::default(),
            &HashMap::new()
        )
        .is_err());
    }

    #[tokio::test]
    async fn runs_takes_a_known_feature_and_a_bounded_limit() {
        assert!(matches!(
            parse(&["runs", "--feature", "work_link", "--limit", "5", "--json"]).unwrap(),
            DecideCmd::Runs {
                limit: Some(5),
                json: true,
                ..
            }
        ));
        let opts = HubOptions::default();
        let env = HashMap::new();
        let e = run(parse(&["runs", "--feature", "nope"]).unwrap(), &opts, &env)
            .await
            .unwrap_err();
        assert!(e.contains("work_link"), "{e}");
        let e = run(parse(&["runs", "--limit", "0"]).unwrap(), &opts, &env)
            .await
            .unwrap_err();
        assert!(e.contains("--limit"), "{e}");
        let e = run(parse(&["status", "--days", "0"]).unwrap(), &opts, &env)
            .await
            .unwrap_err();
        assert!(e.contains("--days"), "{e}");
    }

    #[test]
    fn proposals_name_sections_from_the_tracker_and_print_the_apply_line() {
        use fleet_core::service::decide::status_map;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let tracker;
        {
            let s =
                Store::open_with_bus(&path, std::sync::Arc::new(fleet_core::events::NoopEventBus))
                    .unwrap();
            tracker = s
                .add_tracker("asana", "B", "https://app.asana.com")
                .unwrap()
                .id;
            let cfg = fleet_core::store::TrackerConfig {
                unmapped_sections: vec!["ideas".into()],
                ..Default::default()
            };
            s.set_tracker_probe(tracker, None, &cfg).unwrap();
            let key = s.decision_fp_key().unwrap();
            s.insert_decision_run(&fleet_core::store::NewDecisionRun {
                at: now(),
                feature: "status_map".into(),
                subject_kind: status_map::SUBJECT_KIND.into(),
                subject_id: status_map::subject_id(
                    tracker,
                    &status_map::section_id(&key, tracker, "ideas"),
                ),
                mode: "assist".into(),
                provider: "jev".into(),
                question_version: status_map::QUESTION_VERSION.into(),
                answer: Some("todo".into()),
                confidence: Some(0.9),
                baseline_answer: Some("none".into()),
                called: true,
                ..Default::default()
            })
            .unwrap();
        }
        let ro = open_read_only(&path).unwrap();
        let all = status_map::proposals(&ro, Some(tracker)).unwrap();
        let text = all[0].lines().join("\n");
        assert!(text.contains("\"ideas\" → todo (0.90)"), "{text}");
        assert!(
            text.contains(&format!(
                "fleet-hub tracker section-map {tracker} --set 'ideas=todo'"
            )),
            "{text}"
        );
        assert!(parse(&["proposals", "--tracker", "3", "--json"]).is_ok());
    }

    #[test]
    fn proposals_apply_and_reject_name_a_run_and_a_category_of_the_map() {
        assert!(matches!(
            parse(&["proposals", "reject", "42"]).unwrap(),
            DecideCmd::Proposals {
                action: Some(ProposalCmd::Reject { run: 42 }),
                ..
            }
        ));
        let DecideCmd::Proposals {
            action: Some(cmd), ..
        } = parse(&["proposals", "apply", "7", "--as", "in_progress"]).unwrap()
        else {
            panic!("apply parses");
        };
        assert_eq!(
            proposal_action(&cmd).unwrap(),
            (7, ProposalAction::ApplyAs("in_progress".into()))
        );
        let DecideCmd::Proposals {
            action: Some(cmd), ..
        } = parse(&["proposals", "apply", "7"]).unwrap()
        else {
            panic!("apply parses");
        };
        assert_eq!(proposal_action(&cmd).unwrap(), (7, ProposalAction::Apply));
        // not_planned is an answer, not a category a map takes.
        let DecideCmd::Proposals {
            action: Some(cmd), ..
        } = parse(&["proposals", "apply", "7", "--as", "not_planned"]).unwrap()
        else {
            panic!("apply parses");
        };
        assert!(proposal_action(&cmd).unwrap_err().contains("todo"));
        // A listing's filters do not go with a decision; a run is needed.
        assert!(parse(&["proposals", "--tracker", "3", "reject", "42"]).is_err());
        assert!(parse(&["proposals", "reject"]).is_err());
    }

    /// A hub database with one Asana tracker and assist proposals for
    /// `backlog` (todo) and `ideas` (unsure). Returns (tracker, backlog run,
    /// ideas run).
    fn hub_with_proposals(dir: &Path) -> (i64, i64, i64) {
        use fleet_core::service::decide::status_map;
        let s = Store::open_with_bus(
            &dir.join("state.db"),
            std::sync::Arc::new(fleet_core::events::NoopEventBus),
        )
        .unwrap();
        let tracker = s
            .add_tracker("asana", "B", "https://app.asana.com")
            .unwrap()
            .id;
        let cfg = fleet_core::store::TrackerConfig {
            unmapped_sections: vec!["backlog".into(), "ideas".into()],
            ..Default::default()
        };
        s.set_tracker_probe(tracker, None, &cfg).unwrap();
        let key = s.decision_fp_key().unwrap();
        let mut ids = Vec::new();
        for (name, answer) in [("backlog", "todo"), ("ideas", "unsure")] {
            ids.push(
                s.insert_decision_run(&fleet_core::store::NewDecisionRun {
                    at: now(),
                    feature: "status_map".into(),
                    subject_kind: status_map::SUBJECT_KIND.into(),
                    subject_id: status_map::subject_id(
                        tracker,
                        &status_map::section_id(&key, tracker, name),
                    ),
                    mode: "assist".into(),
                    provider: "jev".into(),
                    model_version: Some("jev-1.13.0".into()),
                    question_version: status_map::QUESTION_VERSION.into(),
                    input_fp: Some(format!("{:0>64}", name.len())),
                    answer: Some(answer.into()),
                    confidence: Some(0.9),
                    baseline_answer: Some("none".into()),
                    called: true,
                    ..Default::default()
                })
                .unwrap(),
            );
        }
        (tracker, ids[0], ids[1])
    }

    #[tokio::test]
    async fn reject_marks_the_run_in_the_hubs_database_and_hides_it() {
        let dir = tempfile::tempdir().unwrap();
        let (tracker, backlog, _) = hub_with_proposals(dir.path());
        let opts = HubOptions {
            data_dir: Some(dir.path().to_path_buf()),
            ..Default::default()
        };
        let env = HashMap::new();
        run(
            parse(&["proposals", "reject", &backlog.to_string()]).unwrap(),
            &opts,
            &env,
        )
        .await
        .unwrap();
        let ro = open_read_only(&dir.path().join("state.db")).unwrap();
        let r = ro.get_decision_run(backlog).unwrap().unwrap();
        assert_eq!(r.followup.as_deref(), Some("rejected"));
        let all = status_map::proposals(&ro, Some(tracker)).unwrap();
        assert_eq!(all[0].rejected, 1);
        assert!(all[0].proposals.iter().all(|p| p.section != "backlog"));
        // The tracker is untouched: the section stays unmapped.
        assert!(ro
            .require_tracker(tracker)
            .unwrap()
            .settings
            .section_map
            .is_empty());
        // Twice is refused, as is a run that is not a proposal.
        let e = run(
            parse(&["proposals", "reject", &backlog.to_string()]).unwrap(),
            &opts,
            &env,
        )
        .await
        .unwrap_err();
        assert!(e.contains("already rejected"), "{e}");
        let e = run(
            parse(&["proposals", "reject", "9999"]).unwrap(),
            &opts,
            &env,
        )
        .await
        .unwrap_err();
        assert!(e.contains("no decision run"), "{e}");
    }

    #[tokio::test]
    async fn apply_checks_the_proposal_before_it_reaches_the_hub() {
        let dir = tempfile::tempdir().unwrap();
        let (_, _, ideas) = hub_with_proposals(dir.path());
        let opts = HubOptions {
            data_dir: Some(dir.path().to_path_buf()),
            ..Default::default()
        };
        // unsure proposes nothing: refused from the database, no hub needed.
        let e = run(
            parse(&["proposals", "apply", &ideas.to_string()]).unwrap(),
            &opts,
            &HashMap::new(),
        )
        .await
        .unwrap_err();
        assert!(e.contains("proposes nothing"), "{e}");
        let line = outcome_line(&ProposalOutcome {
            run_id: 5,
            tracker_id: 3,
            section: "ideas".into(),
            action: "apply_as".into(),
            category: Some("done".into()),
            followup: Some("corrected".into()),
            corrected_to: Some("done".into()),
        });
        assert_eq!(
            line,
            "tracker 3: \"ideas\"=done in your section map (run 5 corrected to done)"
        );
    }
}
