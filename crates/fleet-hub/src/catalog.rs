//! `fleet-hub catalog …` — point the hub at its asset catalog (a git
//! checkout of skills, agents, hooks, MCP servers and plugin refs on this
//! machine) and reload it. The desktop does this in its Assets tab; a paired
//! client cannot, because the checkout has to be on the hub's machine.
//!
//! Each subcommand works on `state.db` and the checkout directly, so it does
//! not need a running hub. A running hub notices at its next catalog call
//! (`catalog::ensure_fresh` compares the load recorded here with its own);
//! on a paired client that is the Assets tab's Refresh.

use crate::config::{self, HubOptions};
use crate::out;
use crate::serve;
use clap::Subcommand;
use fleet_core::service::catalog::{self, ConfigureArgs};
use fleet_core::store::Store;
use std::collections::HashMap;
use std::process::ExitCode;
use std::sync::Mutex;

#[derive(Subcommand, Debug)]
pub enum CatalogCmd {
    /// Print where the catalog is and what was last loaded.
    Show,
    /// Use the git checkout at PATH as the catalog, then load it.
    ///
    /// When PATH has no checkout and --remote is given, the remote is cloned
    /// into it (with this machine's git credentials — for an SSH remote, the
    /// key `fleet-hub ssh-key` prints must be allowed to read it).
    Set {
        /// The checkout, on this machine. `~/` is this user's home.
        path: String,
        /// Clone from this URL when PATH is not a checkout yet.
        #[arg(long)]
        remote: Option<String>,
    },
    /// Re-read the checkout, after editing it or pulling by hand.
    Reload {
        /// `git pull --ff-only` first.
        #[arg(long)]
        pull: bool,
    },
}

pub fn run(
    cmd: CatalogCmd,
    opts: &HubOptions,
    env: &HashMap<String, String>,
) -> Result<ExitCode, String> {
    serve::existing_db(&config::resolve_data_dir(opts, env))?;
    let store = Mutex::new(serve::open_store(opts, env)?);
    match cmd {
        CatalogCmd::Show => show(&store),
        CatalogCmd::Set { path, remote } => {
            catalog::configure(
                ConfigureArgs {
                    repo_path: path,
                    remote_url: remote,
                },
                &store,
            )
            .map_err(|e| e.message)?;
            load(&store, false)
        }
        CatalogCmd::Reload { pull } => load(&store, pull),
    }
}

fn show(store: &Mutex<Store>) -> Result<ExitCode, String> {
    match catalog::config(store).map_err(|e| e.message)? {
        None => out::line(
            "no asset catalog; set one with: fleet-hub catalog set <path> [--remote <url>]",
        ),
        Some(cfg) => {
            out::line(&format!("path    {}", cfg.repo_path));
            out::line(&format!(
                "remote  {}",
                cfg.remote_url.as_deref().unwrap_or("—")
            ));
            out::line(&format!(
                "head    {}",
                cfg.head_commit
                    .as_deref()
                    .filter(|h| !h.is_empty())
                    .map_or("— (not loaded yet)", |h| &h[..h.len().min(12)])
            ));
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn load(store: &Mutex<Store>, pull: bool) -> Result<ExitCode, String> {
    let s = catalog::load(pull, store).map_err(|e| e.message)?;
    out::line(&format!(
        "loaded {} asset(s) at {}{}; a running hub picks it up at its next catalog call (Refresh on a client)",
        s.asset_count,
        &s.head[..s.head.len().min(12)],
        if s.problem_count > 0 {
            format!(", {} problem(s)", s.problem_count)
        } else {
            String::new()
        }
    ));
    Ok(ExitCode::SUCCESS)
}
