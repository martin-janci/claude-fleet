//! `fleet-hub decide …` — the hub operator's side of the decision envelope
//! (Jev evaluation, D35–D37; `fleet_core::service::decide`,
//! `docs/decisions.md`).
//!
//! * `set-key` / `clear-key` write the hub's database directly, like
//!   `fleet-hub tracker webhook`: the key never travels through a tool
//!   argument or reply, and the running hub reads it at its next call.
//!   **The key is never an argument** (`ps`, shell history): it is read from
//!   stdin, from a variable named by `--from-env`, or stored as a reference
//!   (`--ref env:NAME` / `--ref file:/run/secrets/jev`) the hub resolves at
//!   use.
//! * `status` and `runs` open the database read-only, like `fleet-hub
//!   census`: no running hub needed, nothing written, and nothing printed
//!   but ids, words and numbers — never the key.
//! * `proposals` (J3, `status_map`) reads the same way; the section names it
//!   prints come from the trackers' stored config, never from the runs.

use crate::config::{self, HubOptions};
use crate::out;
use crate::serve;
use clap::Subcommand;
use fleet_core::service::decide;
use fleet_core::store::{DecisionRunFilter, DecisionRunRow, Secret, Store};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Subcommand, Debug)]
pub enum DecideCmd {
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
    /// and offline unless `--provider jev` is given. See docs/decisions.md
    /// → "Benchmarking work_link".
    Bench {
        #[command(subcommand)]
        cmd: crate::bench::BenchCmd,
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
    Proposals {
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
    match cmd {
        DecideCmd::Bench { cmd } => return crate::bench::run(cmd, opts, env).await,
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
        DecideCmd::Proposals { tracker, db, json } => {
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
}
