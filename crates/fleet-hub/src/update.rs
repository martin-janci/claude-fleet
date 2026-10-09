//! `fleet-hub update …` — the hub's own updates, from the command line
//! (update-channel design §7, slice S3).
//!
//! `check` is Git mode: it reads the published release channel directly,
//! verifies it against the compiled-in release key, and decides for THIS
//! build under the hub's own `update.*` settings and pin. It needs no
//! running hub and installs nothing.
//!
//! `apply` installs, for a hub run from `fleet-hub.service` without Docker
//! (S9; a Docker hub has `fleet-updater`): one pass of
//! `fleet_updater::binary` — ask the running hub with an `updater` token
//! (`pair`), download and check the tarball, back up, switch
//! `/usr/local/lib/fleet-hub/current`, restart the unit, gate it, and roll back (with
//! the database) when it fails. `deploy/hub/fleet-hub-update.timer` runs it.

use crate::config::{self, HubOptions};
use crate::out;
use crate::serve;
use clap::Subcommand;
use fleet_core::service::update::{self as upd, GitCheck, HttpsFetch};
use fleet_core::store::Store;
use fleet_update::channel::CheckOutcome;
use fleet_update::wire::Installed;
use fleet_update::{Artifact, Component, Track, Version};
use serde::Serialize;
use std::collections::HashMap;
use std::process::ExitCode;

#[derive(Subcommand, Debug)]
pub enum UpdateCmd {
    /// What this hub build should run, read straight from the published
    /// release channel and verified against the release key. Uses the
    /// hub's `update.*` settings and pin when its database exists; needs no
    /// running hub and changes nothing.
    ///
    /// Exits 0 with any answer (up to date, an update, a hold); 1 when there
    /// is none: no channel published, unverified, or unreachable. `--json`
    /// prints the whole decision.
    Check {
        /// The track to read instead of `update.track`: stable, beta or
        /// nightly.
        #[arg(long)]
        track: Option<String>,
        /// Print the decision as JSON instead of lines.
        #[arg(long)]
        json: bool,
    },
    /// One update pass for a hub run from `fleet-hub.service` without
    /// Docker: ask the running hub, and install what it decides — a pin, a
    /// required update, a rollback, or anything under `automatic` — gated
    /// and rolled back (with the database) when it fails. Run as root, by
    /// `fleet-hub-update.timer`. Exits 1 when an install failed.
    Apply {
        #[command(flatten)]
        bin: ApplyArgs,
        /// Print the state file and exit.
        #[arg(long, conflicts_with = "clear")]
        status: bool,
        /// After an operator sorted out a rollback failure: forget it (and
        /// the releases that failed here), so the next pass checks again.
        #[arg(long)]
        clear: bool,
    },
    /// Redeem `fleet-hub pair --name updater --mode updater`'s URL or code
    /// for `apply`, keeping the token in the state dir (mode 0600).
    Pair {
        /// What `fleet-hub pair` printed: the URL or the bare code.
        code: String,
        #[command(flatten)]
        bin: ApplyArgs,
    },
}

/// Where a bare-binary hub lives (deploy/hub/fleet-hub.service).
#[derive(clap::Args, Debug, Clone)]
pub struct ApplyArgs {
    /// The running hub's URL [default: FLEET_HUB_PUBLIC_URL when the hub
    /// terminates TLS itself, else http://127.0.0.1:$FLEET_HUB_PORT].
    #[arg(long)]
    pub hub_url: Option<String>,
    /// Where the release directories and `current` live (not /opt/fleet-hub:
    /// that is the Docker deploy's compose directory).
    #[arg(long, default_value = "/usr/local/lib/fleet-hub")]
    pub root: std::path::PathBuf,
    /// The path the unit runs; it becomes a symlink through `current`.
    #[arg(long, default_value = "/usr/local/bin/fleet-hub")]
    pub link: std::path::PathBuf,
    #[arg(long, default_value = "fleet-hub.service")]
    pub unit: String,
    /// The state file, the replay guard and the updater token.
    #[arg(long, default_value = "/var/lib/fleet-hub-update")]
    pub state_dir: std::path::PathBuf,
    /// The unit's `User=`: its healthcheck and backups run as it.
    #[arg(long, default_value = "fleet")]
    pub user: String,
    /// The unit's `EnvironmentFile=`.
    #[arg(long, default_value = "/etc/fleet-hub.env")]
    pub env_file: std::path::PathBuf,
}

/// Where `pair` keeps the token, in [`ApplyArgs::state_dir`].
const TOKEN_FILE: &str = "token";

/// The hub's URL as this box reaches it.
fn hub_url(a: &ApplyArgs, env: &HashMap<String, String>) -> String {
    if let Some(u) = &a.hub_url {
        return u.clone();
    }
    let tls = env
        .get("FLEET_HUB_TLS")
        .is_some_and(|v| !v.trim().is_empty());
    match env.get("FLEET_HUB_PUBLIC_URL") {
        Some(public) if tls => public.clone(),
        _ => format!(
            "http://127.0.0.1:{}",
            env.get("FLEET_HUB_PORT")
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
                .unwrap_or_else(|| fleet_core::mcp::DEFAULT_PORT.to_string())
        ),
    }
}

async fn apply(
    a: ApplyArgs,
    status: bool,
    clear: bool,
    opts: &HubOptions,
    env: &HashMap<String, String>,
) -> Result<ExitCode, String> {
    use fleet_updater::binary::{BinState, BinaryConfig, BinaryUpdater, Outcome};
    use fleet_updater::common::{trusted_keys, FileSequences};
    use fleet_updater::net::{hub_host_header, GitFetch, HubHttp};
    use fleet_updater::systemd::{parse_env_file, HubCommands, SystemdHost};

    let state = BinState::load(&a.state_dir)?;
    if status {
        out::line(&serde_json::to_string_pretty(&state).map_err(|e| e.to_string())?);
        return Ok(ExitCode::SUCCESS);
    }
    // The unit's environment, then this process's (the timer's unit loads
    // the same file), so the healthcheck and the backup see what serve sees.
    let mut unit_env = std::fs::read_to_string(&a.env_file)
        .map(|t| parse_env_file(&t))
        .unwrap_or_default();
    let mut all = env.clone();
    for (k, v) in &unit_env {
        all.entry(k.clone()).or_insert_with(|| v.clone());
    }
    let data_dir = config::resolve_data_dir(opts, &all);
    // `update_now`'s trigger, consumed: fleet-hub-update.path fires again
    // only on a new one.
    fleet_updater::common::take_trigger(&data_dir.join(fleet_updater::common::UPDATE_NOW_FILE));
    unit_env.push(("FLEET_HUB_DATA_DIR".into(), data_dir.display().to_string()));

    let url = hub_url(&a, &all);
    let token = std::fs::read_to_string(a.state_dir.join(TOKEN_FILE))
        .map(|t| t.trim().to_string())
        .map_err(|_| {
            format!(
                "no updater token in {}: run `fleet-hub pair --name updater --mode updater`,                  then `fleet-hub update pair <the URL it printed>`",
                a.state_dir.display()
            )
        })?;
    let host_header = hub_host_header(None, all.get("FLEET_HUB_PUBLIC_URL").map(String::as_str));
    let hub = HubHttp::new(&url, &token, host_header)?;
    let channel = fleet_update::HubUpdateChannel::new(
        hub.clone(),
        trusted_keys(),
        Box::new(FileSequences::open(&a.state_dir)),
    );
    let host = SystemdHost {
        unit: a.unit.clone(),
        user_scope: false,
        hub: Some(HubCommands {
            user: Some(a.user.clone()),
            env: unit_env,
        }),
        link: a.link.clone(),
        git: GitFetch::new(None)?,
        mirror: Some(hub),
    };
    let mut cfg = BinaryConfig::new(Component::Hub, "fleet-hub", &a.root, &a.link, &a.state_dir);
    cfg.data_dir = Some(data_dir);
    cfg.name = "fleet-hub update".into();
    let mut u = BinaryUpdater::new(host, Box::new(channel), cfg, state);
    if clear {
        u.clear();
        out::line("cleared; the next pass checks again");
        return Ok(ExitCode::SUCCESS);
    }
    let (o, _) = u.tick().await;
    Ok(match o {
        Outcome::Failed(_) | Outcome::RollbackFailed(_) | Outcome::RolledBack { .. } => {
            ExitCode::FAILURE
        }
        _ => ExitCode::SUCCESS,
    })
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn parse_track(s: &str) -> Result<Track, String> {
    match s {
        "stable" => Ok(Track::Stable),
        "beta" => Ok(Track::Beta),
        "nightly" => Ok(Track::Nightly),
        "dev" => Ok(Track::Dev),
        other => Err(format!(
            "--track {other:?}: expected stable, beta or nightly"
        )),
    }
}

/// A closed-set value as it is spelled on the wire.
fn snake(v: &impl Serialize) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default()
}

pub async fn run(
    cmd: UpdateCmd,
    opts: &HubOptions,
    env: &HashMap<String, String>,
) -> Result<ExitCode, String> {
    match cmd {
        UpdateCmd::Apply { bin, status, clear } => apply(bin, status, clear, opts, env).await,
        UpdateCmd::Pair { code, bin } => {
            let (token, name) = fleet_updater::common::redeem(&hub_url(&bin, env), &code).await?;
            let path = fleet_updater::common::save_token(&bin.state_dir, TOKEN_FILE, &token)?;
            out::line(&format!(
                "paired as `{name}`; the token is in {}",
                path.display()
            ));
            Ok(ExitCode::SUCCESS)
        }
        UpdateCmd::Check { track, json } => {
            let track = track.as_deref().map(parse_track).transpose()?;
            // The hub's settings when it has a database; a hub that was never
            // initialised checks under the defaults.
            let store = serve::existing_db(&config::resolve_data_dir(opts, env))
                .ok()
                .map(|p| {
                    Store::open_read_only(&p)
                        .map_err(|e| format!("open {} read-only: {e}", p.display()))
                })
                .transpose()?;
            let setup = GitCheck::from_store(store.as_ref(), Component::Hub, "hub:self", track)
                .map_err(|e| e.message)?;
            drop(store);
            let version = Version::parse(env!("CARGO_PKG_VERSION")).map_err(|e| e.to_string())?;
            let installed = Installed {
                version,
                commit: Some(crate::ready::COMMIT.to_string()),
                build_id: Some(crate::ready::BUILD_ID.to_string()),
                digest: None,
            };
            let req = upd::hub_self_request(installed);
            let fetch = HttpsFetch::new(Some(&setup.base_url));
            let outcome = upd::git_check(fetch, setup, upd::trusted_keys(), &req, now())
                .await
                .map_err(|e| format!("{}: {}", e.code, e.message))?;
            if json {
                out::line(
                    &serde_json::to_string_pretty(&outcome.decision).map_err(|e| e.to_string())?,
                );
            } else {
                let p = &req.platform;
                let platform = format!("{}, {}", p.os_arch(), p.variant);
                for l in render(&platform, &outcome) {
                    out::line(&l);
                }
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// Where the target comes from, in one line.
fn artifact_line(a: &Artifact, url: Option<&str>) -> String {
    match a {
        Artifact::Oci { image, digest, .. } => format!("image  {image}@{digest}"),
        _ => match url {
            Some(u) => format!("file   {u}"),
            None => "file   (no download URL)".into(),
        },
    }
}

/// The decision, for a person.
fn render(platform: &str, o: &CheckOutcome) -> Vec<String> {
    let d = &o.decision;
    let mut lines = vec![format!(
        "fleet-hub {} ({platform}) on {}: {}",
        d.installed,
        d.track.as_str(),
        snake(&d.status)
    )];
    lines.push(format!(
        "  why    {} — {}",
        snake(&d.reason.code),
        d.reason.text
    ));
    if let Some(t) = &d.target {
        let mandatory = if t.mandatory {
            match &t.deadline {
                Some(dl) => format!(", mandatory by {dl}"),
                None => ", mandatory".into(),
            }
        } else {
            String::new()
        };
        lines.push(format!("  target {}{mandatory}", t.version));
        lines.push(format!(
            "  {}",
            artifact_line(&t.artifact, t.url.as_deref())
        ));
        if let Some(v) = &o.verified {
            lines.push(format!(
                "  signed release manifest and channel #{} verified against the release key",
                v.channel_sequence
            ));
        }
    }
    lines.push(format!("  mode   {} (update.hub.mode)", snake(&d.mode)));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hub_is_reached_where_it_listens() {
        let a = |url: Option<&str>| ApplyArgs {
            hub_url: url.map(String::from),
            root: "/usr/local/lib/fleet-hub".into(),
            link: "/usr/local/bin/fleet-hub".into(),
            unit: "fleet-hub.service".into(),
            state_dir: "/var/lib/fleet-hub-update".into(),
            user: "fleet".into(),
            env_file: "/etc/fleet-hub.env".into(),
        };
        let env = |kv: &[(&str, &str)]| {
            kv.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<HashMap<_, _>>()
        };
        assert_eq!(hub_url(&a(None), &env(&[])), "http://127.0.0.1:4180");
        assert_eq!(
            hub_url(
                &a(None),
                &env(&[
                    ("FLEET_HUB_PORT", "5000"),
                    ("FLEET_HUB_PUBLIC_URL", "https://h.example")
                ])
            ),
            "http://127.0.0.1:5000"
        );
        assert_eq!(
            hub_url(
                &a(None),
                &env(&[
                    ("FLEET_HUB_TLS", "cert"),
                    ("FLEET_HUB_PUBLIC_URL", "https://h.example")
                ])
            ),
            "https://h.example"
        );
        assert_eq!(hub_url(&a(Some("http://x:1")), &env(&[])), "http://x:1");
    }

    #[test]
    fn tracks_parse() {
        assert_eq!(parse_track("beta"), Ok(Track::Beta));
        assert!(parse_track("edge").is_err());
    }
}
