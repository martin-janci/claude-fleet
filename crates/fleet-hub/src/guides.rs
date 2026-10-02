//! `fleet-hub guides …` — the hub operator's review of guides (declarative
//! pages, layout `guide`). A Claude session proposes a guide over the
//! control API (`guide { propose }`, usually through the `fleet-guides`
//! catalog skill); the operator reads it here and approves or rejects it.
//!
//! Reads and writes `state.db` directly, as the person at the hub's
//! console. A paired desktop or phone lists the live guides on its next
//! read of the Guides page.

use crate::config::{self, HubOptions};
use crate::out;
use crate::serve;
use clap::Subcommand;
use fleet_core::service::guides;
use fleet_core::service::settings::Actor;
use fleet_core::store::Store;
use std::collections::HashMap;
use std::process::ExitCode;
use std::sync::Arc;

#[derive(Subcommand, Debug)]
pub enum GuidesCmd {
    /// The live guides and the proposals waiting for review.
    List {
        /// Print the answer as JSON instead of lines.
        #[arg(long)]
        json: bool,
    },
    /// One proposal's guide, step by step, before deciding.
    Show { id: i64 },
    /// Approve a proposal by id (from `guides list`): it joins the pages.
    Approve { id: i64 },
    /// Reject a proposal by id.
    Reject { id: i64 },
    /// Take a live guide off the pages, by its id (guide.<name>).
    Remove { page_id: String },
}

fn open(opts: &HubOptions, env: &HashMap<String, String>) -> Result<Store, String> {
    let path = serve::existing_db(&config::resolve_data_dir(opts, env))?;
    Store::open_with_bus(&path, Arc::new(fleet_core::events::NoopEventBus))
        .map_err(|e| format!("open {}: {e}", path.display()))
}

pub fn run(
    cmd: GuidesCmd,
    opts: &HubOptions,
    env: &HashMap<String, String>,
) -> Result<ExitCode, String> {
    let s = open(opts, env)?;
    match cmd {
        GuidesCmd::List { json } => {
            let v = guides::view(&s, true).map_err(|e| e.message)?;
            if json {
                out::line(&serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?);
                return Ok(ExitCode::SUCCESS);
            }
            if v.guides.is_empty() {
                out::line("No guides are live.");
            }
            for g in &v.guides {
                out::line(&format!(
                    "live  {}  {} ({} steps)",
                    g.id,
                    g.title,
                    g.sections.len()
                ));
            }
            // An approved row that is NOT being served. It still holds a
            // MAX_APPROVED slot, and this is the only listing that prints the
            // page id `guides remove` needs — before this, such a guide was
            // invisible everywhere and unremovable in practice.
            for w in &v.withheld {
                out::line(&format!("held  {}  not served: {}", w.page_id, w.why));
                out::line(&format!(
                    "      remove it with: fleet-hub guides remove {}",
                    w.page_id
                ));
            }
            if v.proposals.is_empty() {
                out::line("No guide proposals are waiting for review.");
            }
            for p in &v.proposals {
                let who = p.source_detail.as_deref().unwrap_or(&p.source);
                let replaces = if p.replaces {
                    ", replaces the live one"
                } else {
                    ""
                };
                out::line(&format!(
                    "#{}  {}  {}  ({who}{replaces})",
                    p.id, p.page_id, p.title
                ));
                if let Some(why) = &p.why {
                    out::line(&format!("      why: {why}"));
                }
            }
            if !v.proposals.is_empty() {
                out::line("Read one with `fleet-hub guides show <id>`; then `approve <id>` or `reject <id>`.");
            }
            Ok(ExitCode::SUCCESS)
        }
        GuidesCmd::Show { id } => {
            let p = guides::pending(&s)
                .map_err(|e| e.message)?
                .into_iter()
                .find(|p| p.id == id)
                .ok_or_else(|| format!("no guide proposal #{id} waits"))?;
            out::line(&format!("{}  ({})", p.page.title, p.page_id));
            if let Some(i) = &p.page.intro {
                out::line(i);
            }
            for (n, step) in p.page.sections.iter().enumerate() {
                out::line(&format!("{}. {}", n + 1, step.title));
                for item in &step.items {
                    out::line(&format!(
                        "     {}",
                        serde_json::to_string(item).map_err(|e| e.to_string())?
                    ));
                }
            }
            let keys = guides::keys_of(&p.page);
            if !keys.is_empty() {
                out::line(&format!(
                    "Settings it lets a person change: {}",
                    keys.join(", ")
                ));
            }
            Ok(ExitCode::SUCCESS)
        }
        GuidesCmd::Approve { id } => {
            guides::decide(&s, id, true, Actor::Person).map_err(|e| e.message)?;
            out::line(&format!("approved #{id}"));
            Ok(ExitCode::SUCCESS)
        }
        GuidesCmd::Reject { id } => {
            guides::decide(&s, id, false, Actor::Person).map_err(|e| e.message)?;
            out::line(&format!("rejected #{id}"));
            Ok(ExitCode::SUCCESS)
        }
        GuidesCmd::Remove { page_id } => {
            guides::remove(&s, &page_id, Actor::Person).map_err(|e| e.message)?;
            out::line(&format!("removed {page_id}"));
            Ok(ExitCode::SUCCESS)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approve_puts_an_agents_guide_on_the_pages() {
        let dir = tempfile::tempdir().unwrap();
        let opts = HubOptions {
            data_dir: Some(dir.path().to_path_buf()),
            ..HubOptions::default()
        };
        let env = HashMap::new();
        let path = dir.path().join("state.db");
        let id = {
            let s =
                Store::open_with_bus(&path, Arc::new(fleet_core::events::NoopEventBus)).unwrap();
            guides::propose(&s, &guides::example(), None, Actor::Agent("host web-1"))
                .unwrap()
                .id
        };
        let show = run(GuidesCmd::Show { id }, &opts, &env).unwrap();
        assert_eq!(show, ExitCode::SUCCESS);
        run(GuidesCmd::Approve { id }, &opts, &env).unwrap();
        assert!(
            run(GuidesCmd::Approve { id }, &opts, &env).is_err(),
            "decided once"
        );
        let s = open(&opts, &env).unwrap();
        assert_eq!(guides::live(&s)[0].id, "guide.cleanup");
        drop(s);
        run(
            GuidesCmd::Remove {
                page_id: "guide.cleanup".into(),
            },
            &opts,
            &env,
        )
        .unwrap();
        assert!(guides::live(&open(&opts, &env).unwrap()).is_empty());
    }

    /// `List`, `Reject` and every unknown id — the half of the CLI no test
    /// drove.
    ///
    /// `List { json }` is the output a script parses and `Reject` is the
    /// decision an operator makes most, yet neither was ever called, so the
    /// `no guide proposal #{id} waits` and `no live guide` sentences were
    /// untested and a panic in either listing branch would have shipped.
    #[test]
    fn list_reject_and_the_unknown_id_paths() {
        let dir = tempfile::tempdir().unwrap();
        let opts = HubOptions {
            data_dir: Some(dir.path().to_path_buf()),
            ..HubOptions::default()
        };
        let env = HashMap::new();
        let path = dir.path().join("state.db");
        let bus = || Arc::new(fleet_core::events::NoopEventBus);

        // Empty: both "nothing here" branches.
        {
            let _ = Store::open_with_bus(&path, bus()).unwrap();
        }
        for json in [false, true] {
            assert_eq!(
                run(GuidesCmd::List { json }, &opts, &env).unwrap(),
                ExitCode::SUCCESS
            );
        }
        // Nothing waits, so every id is unknown — the three sentences.
        let e = run(GuidesCmd::Show { id: 404 }, &opts, &env).unwrap_err();
        assert!(e.contains("no guide proposal #404 waits"), "{e}");
        let e = run(GuidesCmd::Reject { id: 404 }, &opts, &env).unwrap_err();
        assert!(e.contains("no guide proposal 404 waits"), "{e}");
        let e = run(GuidesCmd::Approve { id: 404 }, &opts, &env).unwrap_err();
        assert!(e.contains("no guide proposal 404 waits"), "{e}");
        let e = run(
            GuidesCmd::Remove {
                page_id: "guide.nope".into(),
            },
            &opts,
            &env,
        )
        .unwrap_err();
        assert!(e.contains("no live guide `guide.nope`"), "{e}");

        // A live guide and a waiting proposal that replaces it, so each listing
        // branch runs with something in it — including the `why` line and the
        // "replaces the live one" note.
        let (waiting, live_id) = {
            let s = Store::open_with_bus(&path, bus()).unwrap();
            let live_id = guides::propose(&s, &guides::example(), None, Actor::Agent("host web-1"))
                .unwrap()
                .id;
            guides::decide(&s, live_id, true, Actor::Person).unwrap();
            let waiting = guides::propose(
                &s,
                &guides::example(),
                Some("people ask"),
                Actor::Agent("host web-1"),
            )
            .unwrap()
            .id;
            assert!(guides::pending(&s).unwrap()[0].replaces);
            (waiting, live_id)
        };
        for json in [false, true] {
            assert_eq!(
                run(GuidesCmd::List { json }, &opts, &env).unwrap(),
                ExitCode::SUCCESS
            );
        }

        // Reject moves the row, and says so only once.
        run(GuidesCmd::Reject { id: waiting }, &opts, &env).unwrap();
        {
            let s = open(&opts, &env).unwrap();
            assert_eq!(
                s.guide_proposal(waiting).unwrap().unwrap().state,
                "rejected",
                "the CLI's rejection reached the row"
            );
            assert!(guides::pending(&s).unwrap().is_empty());
            // Rejecting the revision leaves the live guide alone.
            assert_eq!(
                s.guide_proposal(live_id).unwrap().unwrap().state,
                "approved"
            );
            assert_eq!(guides::live(&s).len(), 1);
        }
        assert!(
            run(GuidesCmd::Reject { id: waiting }, &opts, &env).is_err(),
            "decided once"
        );
    }
}
