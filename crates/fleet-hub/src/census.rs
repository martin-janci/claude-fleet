//! `fleet-hub census …` — local measurements for the Jev evaluation
//! (`docs/superpowers/specs/2026-09-27-jev-language-census-design.md`).
//!
//! Reads `state.db` directly and read-only: no running hub, no network, no
//! write. The census prints counts only; `--export-sample` is the one path
//! that writes text, to a new local file the operator asked for (D46).

use crate::config::{self, HubOptions};
use crate::out;
use crate::serve;
use clap::Subcommand;
use fleet_core::service::nl::{self, census, Detector};
use fleet_core::store::Store;
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

fn open(path: &Path) -> Result<Store, String> {
    Store::open_read_only(path).map_err(|e| format!("open {} read-only: {e}", path.display()))
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
            let store = open(&db_path(db, opts, env)?)?;
            let o = census::CensusOptions::new(days, org, max_per_source, now())
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

    #[test]
    fn a_missing_db_is_named() {
        let e = db_path(
            Some(PathBuf::from("/nonexistent/state.db")),
            &HubOptions::default(),
            &HashMap::new(),
        )
        .unwrap_err();
        assert!(e.contains("/nonexistent/state.db"), "{e}");
    }
}
