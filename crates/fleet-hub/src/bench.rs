//! `fleet-hub decide bench …` — offline benchmarks of the decision
//! envelope's use cases (Jev evaluation phase 0; the test map's card J1;
//! `fleet_core::service::decide::bench`).
//!
//! By default the database is opened **read-only** and nothing is sent:
//! the providers `none` and `bm25` run in this process. `--provider jev` is
//! the one network path. It goes through the envelope, so a case is asked
//! only when `decide.jev.enabled` is on, `decide.jev.work_link` is `shadow`
//! or `assist`, the case's org consented and a key is set; every call is
//! recorded in `decision_runs` (which is why that run opens the database
//! for writing). `--export-unlinked` writes the D39 hand-label file: new
//! only, `0600`, and it holds prompt and title text.

use crate::census::create_private;
use crate::config::{self, HubOptions};
use crate::out;
use crate::serve;
use clap::Subcommand;
use fleet_core::service::decide::bench::work_link::{self as wl, BenchOptions, Provider, Split};
use fleet_core::service::decide::DecideCtx;
use fleet_core::service::nl::Detector;
use fleet_core::store::Store;
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
    /// Read-only and offline unless --provider jev is given. No prompt or
    /// title is printed; counts under 5 show as <5.
    WorkLink {
        /// Which cases to report: dev (oldest 60%), test (newest 40%) or
        /// all. Thresholds are always chosen on dev. [default: test]
        #[arg(long, value_parser = ["dev", "test", "all"])]
        split: Option<String>,
        /// A provider to run: none, bm25, jev (repeat it). [default: none
        /// and bm25]. jev sends each case's redacted prompt and candidate
        /// titles to TypeSafe, only for orgs that consented.
        #[arg(long = "provider", value_parser = ["none", "bm25", "jev"])]
        providers: Vec<String>,
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
    db: Option<&Path>,
    opts: &HubOptions,
    env: &HashMap<String, String>,
) -> Result<PathBuf, String> {
    match db {
        Some(p) if p.is_file() => Ok(p.to_path_buf()),
        Some(p) => Err(format!("no database at {}", p.display())),
        None => serve::existing_db(&config::resolve_data_dir(opts, env)),
    }
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
            days,
            org,
            max_cases,
            max_calls,
            db,
            json,
            labels,
            export_unlinked,
            out: out_path,
        } => {
            let split = Split::parse(split.as_deref().unwrap_or("test"))
                .ok_or("--split is dev, test or all")?;
            let providers = parse_providers(&providers)?;
            let o = BenchOptions::new(days, org, max_cases, split, providers, max_calls, now())
                .map_err(|e| e.message)?;
            let path = db_path(db.as_deref(), opts, env)?;

            if let (Some(n), Some(file)) = (export_unlinked, out_path) {
                let store = Store::open_read_only(&path)
                    .map_err(|e| format!("open {} read-only: {e}", path.display()))?;
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
                let loaded = {
                    let s = store.lock().map_err(|_| "store lock poisoned")?;
                    wl::load(&s, &detector, &o, labeled.as_deref()).map_err(|e| e.message)?
                };
                out::error(&format!(
                    "jev: asking only for cases whose org passes the gate (at most {} calls); \
                     each call is recorded in decision_runs",
                    o.max_calls
                ));
                let ctx = DecideCtx::jev(Arc::clone(&store));
                let outs = wl::run_providers(&loaded, &o, Some(&ctx)).await;
                wl::report(&loaded, &o, &outs)
            } else {
                let store = Store::open_read_only(&path)
                    .map_err(|e| format!("open {} read-only: {e}", path.display()))?;
                let loaded =
                    wl::load(&store, &detector, &o, labeled.as_deref()).map_err(|e| e.message)?;
                let outs = wl::run_providers(&loaded, &o, None).await;
                wl::report(&loaded, &o, &outs)
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
    }
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
            ..
        } = c;
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
        assert!(parse(&["work-link", "--provider", "haiku"]).is_err());
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
                .upsert_bg_session("h1", name, None, cid, None, now(), "bg", now())
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
}
