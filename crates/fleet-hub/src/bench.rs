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
use crate::config::{self, HubOptions};
use crate::out;
use crate::serve;
use clap::Subcommand;
use fleet_core::service::decide::bench::status_map as sm;
use fleet_core::service::decide::bench::work_link::{
    self as wl, BenchOptions, Provider, Shape, Split,
};
use fleet_core::service::decide::haiku::{self, Haiku, HaikuConfig};
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
        #[arg(long)]
        fixture: bool,
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
        /// The database jev runs are gated by and recorded in, instead of
        /// the hub's (only read with --provider jev).
        #[arg(long, value_name = "FILE")]
        db: Option<PathBuf>,
        /// Print JSON instead of lines.
        #[arg(long)]
        json: bool,
    },
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
        } => {
            let split = Split::parse(split.as_deref().unwrap_or("test"))
                .ok_or("--split is dev, test or all")?;
            let shape = Shape::parse(shape.as_deref().unwrap_or("choice"))
                .ok_or("--shape is choice or choice+noul")?;
            let providers = parse_providers(&providers)?;
            let o = BenchOptions::new(days, org, max_cases, split, providers, max_calls, now())
                .map_err(|e| e.message)?
                .with_shape(shape);
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
                let mut r = wl::report(&loaded, &o, &outs);
                r.notes.extend(note);
                r
            } else {
                let store = Store::open_read_only(&path)
                    .map_err(|e| format!("open {} read-only: {e}", path.display()))?;
                let loaded =
                    wl::load(&store, &detector, &o, labeled.as_deref()).map_err(|e| e.message)?;
                let haiku = bind_haiku(&ssh, haiku_cfg, &store)?;
                let note = haiku.as_ref().map(Haiku::consent_note);
                let outs = wl::run_providers_with(&loaded, &o, None, haiku.as_ref()).await;
                let mut r = wl::report(&loaded, &o, &outs);
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
            providers,
            haiku,
            max_calls,
            db,
            json,
        } => {
            let ssh = fleet_core::ssh::SshClient::new();
            let report = status_map(
                labels.as_deref(),
                fixture,
                &providers,
                &haiku,
                &ssh,
                max_calls,
                db.as_deref(),
                opts,
                env,
            )
            .await?;
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

/// `decide bench status-map`: the labeled sections (or the built-in set)
/// through the providers. Only `--provider jev` opens a database — for
/// writing, as the envelope records every call. `--provider haiku` runs on
/// `ssh` against `--haiku-host`, whose org it reads from the database
/// (read-only).
#[allow(clippy::too_many_arguments)]
async fn status_map(
    labels: Option<&Path>,
    fixture: bool,
    providers: &[String],
    haiku_args: &HaikuArgs,
    ssh: &dyn fleet_core::ssh::SshExec,
    max_calls: Option<usize>,
    db: Option<&Path>,
    opts: &HubOptions,
    env: &HashMap<String, String>,
) -> Result<sm::Report, String> {
    let raw = match (labels, fixture) {
        (Some(file), _) => {
            std::fs::read_to_string(file).map_err(|e| format!("read {}: {e}", file.display()))?
        }
        (None, true) => sm::FIXTURE.to_string(),
        (None, false) => {
            return Err(
                "give the labeled sections with --labels FILE, or --fixture for the built-in \
                 synthetic set"
                    .into(),
            )
        }
    };
    let mut rows = sm::parse_labels(&raw).map_err(|e| match labels {
        Some(f) => format!("{}: {e}", f.display()),
        None => format!("the built-in set: {e}"),
    })?;
    let providers = parse_sm_providers(providers)?;
    let max_calls = max_calls.unwrap_or(sm::DEFAULT_MAX_CALLS);
    let haiku = match haiku_args.config(providers.contains(&sm::Provider::Haiku))? {
        Some(cfg) => {
            let path = db_path(db, opts, env)?;
            let s = Store::open_read_only(&path)
                .map_err(|e| format!("open {} read-only: {e}", path.display()))?;
            bind_haiku(ssh, Some(cfg), &s)?
        }
        None => None,
    };
    // Before anything is sent, each row's org comes from the database (the
    // gate's consent and the haiku org fence rest on it), never from the
    // file alone.
    if providers.iter().any(|p| p.is_model()) {
        let path = db_path(db, opts, env)?;
        let s = Store::open_read_only(&path)
            .map_err(|e| format!("open {} read-only: {e}", path.display()))?;
        sm::resolve_label_orgs(&s, &mut rows).map_err(|e| match labels {
            Some(f) => format!("{}: {e}", f.display()),
            None => format!("the built-in set: {e}"),
        })?;
    }
    let (cases, dropped) = sm::cases(&rows);
    let note = haiku.as_ref().map(Haiku::consent_note);
    let outs = if providers.contains(&sm::Provider::Jev) {
        let path = db_path(db, opts, env)?;
        let store = Store::open_with_bus(&path, Arc::new(fleet_core::events::NoopEventBus))
            .map_err(|e| format!("open {}: {e}", path.display()))?;
        out::error(&format!(
            "jev: asking only for sections whose org (or, with none, decide.jev.unassigned) passes \
             the gate, at most {max_calls} calls; each call is recorded in decision_runs"
        ));
        let ctx = DecideCtx::jev(Arc::new(Mutex::new(store)));
        sm::run_providers_with(&cases, &providers, Some(&ctx), haiku.as_ref(), max_calls).await
    } else {
        sm::run_providers_with(&cases, &providers, None, haiku.as_ref(), max_calls).await
    };
    let mut r = sm::report(&cases, &outs, rows.len(), dropped, labels.is_none());
    r.notes.extend(note);
    Ok(r)
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
            use std::os::unix::process::ExitStatusExt;
            self.hosts.lock().unwrap().push(host.to_string());
            self.commands.lock().unwrap().push(args.join(" "));
            self.stdins
                .lock()
                .unwrap()
                .push(String::from_utf8(stdin).unwrap());
            Ok(std::process::Output {
                status: std::process::ExitStatus::from_raw(0),
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
}
