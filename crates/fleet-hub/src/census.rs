//! `fleet-hub census …` — local measurements: the language census for the
//! Jev evaluation
//! (`docs/superpowers/specs/2026-09-27-jev-language-census-design.md`), and
//! the limit census (how often every Claude account was at its usage limit
//! at once, `fleet_core::service::limit_census`).
//!
//! Reads `state.db` directly and read-only: no running hub, no network, no
//! write. A census prints counts only; `--export-sample` is the one path
//! that writes text, to a new local file the operator asked for (D46).

use crate::config::HubOptions;
use crate::dbarg::{db_path, open_read_only};
use crate::out;
use clap::Subcommand;
use fleet_core::service::limit_census;
use fleet_core::service::nl::{self, census, Detector};
use fleet_core::store::now_unix;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Subcommand, Debug)]
pub enum CensusCmd {
    /// Which languages the texts a decision model would see are written in
    /// (first prompts, ticket titles and descriptions, journal notes, and
    /// prompt × title of confirmed links), per org, as counts.
    ///
    /// Reads the database directly and never writes it; a running hub is not
    /// needed. No text is printed or kept, and counts under 5 show as <5.
    Languages {
        /// The window in days (1-730). [default: 90]
        #[arg(long)]
        days: Option<u32>,
        /// Only this org's rows (its id, from `fleet-hub org list`).
        #[arg(long)]
        org: Option<i64>,
        /// Newest rows read per source (1-100000). [default: 5000]
        #[arg(long)]
        max_per_source: Option<u32>,
        /// Read this database file instead of the hub's, e.g. a desktop
        /// app's state.db.
        #[arg(long, value_name = "FILE")]
        db: Option<PathBuf>,
        /// Print the answer as JSON instead of lines.
        #[arg(long)]
        json: bool,
        /// Measure the detector on a labeled JSON-lines file instead of
        /// counting (cases whose `checked` is false are left out).
        #[arg(long, value_name = "FILE", conflicts_with = "export_sample")]
        labels: Option<PathBuf>,
        /// Write N first prompts, spread over the window, with the
        /// detector's guess to --out for a person to correct. The file HOLDS
        /// PROMPT TEXT: it is created 0600 and never overwritten.
        #[arg(long, value_name = "N", requires = "out")]
        export_sample: Option<usize>,
        /// Where --export-sample writes (must not exist).
        #[arg(long, value_name = "FILE", requires = "export_sample")]
        out: Option<PathBuf>,
    },
    /// How often every Claude account was at its usage limit at the same
    /// time, from the usage history the hub keeps: the times, how long they
    /// lasted, and each account's own limits.
    ///
    /// Reads the database directly and never writes it; a running hub is not
    /// needed.
    Limits {
        /// The window in days (1-35, what the history keeps). [default: 28]
        #[arg(long)]
        days: Option<u32>,
        /// Read this database file instead of the hub's, e.g. a copy of
        /// another hub's state.db.
        #[arg(long, value_name = "FILE")]
        db: Option<PathBuf>,
        /// Print the answer as JSON instead of lines.
        #[arg(long)]
        json: bool,
    },
}

/// Create `path` for the sample: new only, owner-only on unix.
pub(crate) fn create_private(path: &Path) -> Result<std::fs::File, String> {
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::AlreadyExists => {
            format!("{} exists; this file is never written over", path.display())
        }
        _ => format!("create {}: {e}", path.display()),
    })
}

pub fn run(
    cmd: CensusCmd,
    opts: &HubOptions,
    env: &HashMap<String, String>,
) -> Result<ExitCode, String> {
    match cmd {
        CensusCmd::Languages {
            days,
            org,
            max_per_source,
            db,
            json,
            labels,
            export_sample,
            out: out_path,
        } => {
            let detector = Detector::new();
            if let Some(file) = labels {
                let raw = std::fs::read_to_string(&file)
                    .map_err(|e| format!("read {}: {e}", file.display()))?;
                let cases =
                    nl::parse_cases(&raw).map_err(|e| format!("{}: {e}", file.display()))?;
                let eval = nl::evaluate(&detector, &cases);
                print(json, || serde_json::to_string_pretty(&eval), eval.lines())?;
                return Ok(ExitCode::SUCCESS);
            }
            let store = open_read_only(&db_path(db.as_deref(), opts, env)?)?;
            let o = census::CensusOptions::new(days, org, max_per_source, now_unix())
                .map_err(|e| e.message)?;
            if let (Some(n), Some(path)) = (export_sample, out_path) {
                let cases =
                    census::sample_prompts(&store, &detector, &o, n).map_err(|e| e.message)?;
                let mut f = create_private(&path)?;
                use std::io::Write;
                for c in &cases {
                    let line = serde_json::to_string(c).map_err(|e| e.to_string())?;
                    writeln!(f, "{line}").map_err(|e| format!("write {}: {e}", path.display()))?;
                }
                out::line(&format!(
                    "wrote {} prompt(s) to {}. It holds prompt text: keep it on this machine. \
                     Correct each `expect` (and `folded`), set `checked` to true, then run \
                     `fleet-hub census languages --labels {}`.",
                    cases.len(),
                    path.display(),
                    path.display()
                ));
                return Ok(ExitCode::SUCCESS);
            }
            let report = census::census(&store, &detector, &o).map_err(|e| e.message)?;
            print(
                json,
                || serde_json::to_string_pretty(&report),
                report.lines(),
            )?;
            Ok(ExitCode::SUCCESS)
        }
        CensusCmd::Limits { days, db, json } => {
            let o =
                limit_census::LimitCensusOptions::new(days, now_unix()).map_err(|e| e.message)?;
            let store = open_read_only(&db_path(db.as_deref(), opts, env)?)?;
            let report = limit_census::limit_census(&store, &o).map_err(|e| e.message)?;
            print(
                json,
                || serde_json::to_string_pretty(&report),
                report.lines(),
            )?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// The JSON (rendered only when asked for) or the lines.
fn print(
    json: bool,
    render: impl FnOnce() -> serde_json::Result<String>,
    lines: Vec<String>,
) -> Result<(), String> {
    if json {
        out::line(&render().map_err(|e| e.to_string())?);
    } else {
        for l in lines {
            out::line(&l);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct T {
        #[command(subcommand)]
        cmd: CensusCmd,
    }

    #[test]
    fn languages_parses_on_the_command_line() {
        let t =
            T::try_parse_from(["t", "languages", "--days", "30", "--org", "2", "--json"]).unwrap();
        assert!(matches!(
            t.cmd,
            CensusCmd::Languages {
                days: Some(30),
                org: Some(2),
                json: true,
                labels: None,
                export_sample: None,
                ..
            }
        ));
    }

    #[test]
    fn limits_parses_on_the_command_line() {
        let t = T::try_parse_from(["t", "limits"]).unwrap();
        assert!(matches!(
            t.cmd,
            CensusCmd::Limits {
                days: None,
                db: None,
                json: false
            }
        ));
        let t =
            T::try_parse_from(["t", "limits", "--days", "14", "--db", "x.db", "--json"]).unwrap();
        let CensusCmd::Limits { days, db, json } = t.cmd else {
            panic!("not limits");
        };
        assert_eq!(
            (days, db, json),
            (Some(14), Some(PathBuf::from("x.db")), true)
        );
        assert!(T::try_parse_from(["t", "limits", "--org", "2"]).is_err());
    }

    /// The whole path a person runs: a database file with a host, a login
    /// and its readings, opened read-only and counted.
    #[test]
    fn limits_counts_a_database_file_and_refuses_days_the_history_does_not_keep() {
        use fleet_core::service::account_usage::{AccountUsage, Window};
        use fleet_core::store::{AccountRow, Store, UsageSnapshotRow};
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("state.db");
        {
            let s =
                Store::open_with_bus(&db, std::sync::Arc::new(fleet_core::events::NoopEventBus))
                    .unwrap();
            s.insert_host("h1", Some("h1")).unwrap();
            s.upsert_account(&AccountRow {
                uuid: "main".into(),
                ..Default::default()
            })
            .unwrap();
            s.set_host_account("h1", Some("main")).unwrap();
            let now = now_unix();
            for at in [now - 86_400, now - 3_600] {
                s.insert_usage_snapshot(&UsageSnapshotRow {
                    account_uuid: "main".into(),
                    fetched_at: at,
                    usage: AccountUsage {
                        five_hour: Some(Window {
                            utilization: 100.0,
                            resets_at: Some(at + 1_800),
                        }),
                        ..Default::default()
                    },
                    subscription: None,
                    source_host: Some("h1".into()),
                })
                .unwrap();
            }
        }
        let run_limits = |days, json| {
            run(
                CensusCmd::Limits {
                    days,
                    db: Some(db.clone()),
                    json,
                },
                &HubOptions::default(),
                &HashMap::new(),
            )
        };
        assert_eq!(run_limits(None, false), Ok(ExitCode::SUCCESS));
        assert_eq!(run_limits(Some(35), true), Ok(ExitCode::SUCCESS));
        let e = run_limits(Some(36), false).unwrap_err();
        assert!(e.contains("days must be 1-35"), "{e}");
        let e = run(
            CensusCmd::Limits {
                days: None,
                db: Some(dir.path().join("none.db")),
                json: false,
            },
            &HubOptions::default(),
            &HashMap::new(),
        )
        .unwrap_err();
        assert!(e.contains("no database at"), "{e}");
    }

    #[test]
    fn a_sample_needs_a_file_and_a_file_needs_a_sample() {
        assert!(T::try_parse_from(["t", "languages", "--export-sample", "50"]).is_err());
        assert!(T::try_parse_from(["t", "languages", "--out", "x.jsonl"]).is_err());
        assert!(T::try_parse_from([
            "t",
            "languages",
            "--export-sample",
            "50",
            "--out",
            "x.jsonl"
        ])
        .is_ok());
    }

    #[test]
    fn labels_and_a_sample_are_one_or_the_other() {
        assert!(T::try_parse_from([
            "t",
            "languages",
            "--labels",
            "a.jsonl",
            "--export-sample",
            "5",
            "--out",
            "b.jsonl"
        ])
        .is_err());
    }

    #[test]
    fn the_sample_file_is_new_and_private() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        drop(create_private(&p).unwrap());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let e = create_private(&p).unwrap_err();
        assert!(e.contains("never written over"), "{e}");
    }
}
