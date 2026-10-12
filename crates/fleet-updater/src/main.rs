//! `fleet-updater`: the hub's update sidecar (update-channel design §8, S6).
//!
//! ```text
//! fleet-updater [run]          loop: check, install when told, gate, roll back
//! fleet-updater once           one pass, then exit (the systemd-timer variant)
//! fleet-updater status         the state file, as JSON
//! fleet-updater clear          after an operator sorted out a rollback_failed
//! fleet-updater pair <url>     redeem `fleet-hub pair --mode updater`'s code
//! fleet-updater --standalone … no hub above: read the published channel
//! ```
//!
//! Configuration is the environment (docs/updates.md → *fleet-updater*).

// `#[async_trait]` expands each async trait method into a `#[must_use]` fn that
// returns a boxed future, which is already `#[must_use]`; clippy 1.99 flags that
// macro output as `double_must_use`. It is not code we wrote — allow it crate-wide.
#![allow(clippy::double_must_use)]

mod docker;
mod engine;
mod spec;
mod state;

use fleet_updater::common::{self, trusted_keys, FileSequences};
use fleet_updater::{http, net};

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use fleet_update::decide::{Pin, Policy};
use fleet_update::{GitUpdateChannel, HubUpdateChannel, Mode, Track, UpdateChannel, Version};

use crate::docker::{Docker, EngineDocker};
use crate::engine::{log, Config, Outcome, Updater};
use crate::state::State;

const CHANNEL_BASE_URL: &str =
    "https://raw.githubusercontent.com/martin-janci/claude-fleet/update-channels/";

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

fn env_secs(key: &str, default: u64) -> Result<Duration, String> {
    match env(key) {
        None => Ok(Duration::from_secs(default)),
        Some(v) => v
            .trim()
            .parse()
            .map(Duration::from_secs)
            .map_err(|_| format!("{key}={v}: not a number of seconds")),
    }
}

/// The hub container: `FLEET_UPDATER_HUB_CONTAINER`, else the compose service
/// `FLEET_UPDATER_HUB_SERVICE` (default `fleet-hub`) in this container's own
/// compose project, else `fleet-hub`.
async fn hub_container(docker: &EngineDocker) -> String {
    if let Some(name) = env("FLEET_UPDATER_HUB_CONTAINER") {
        return name;
    }
    let service = env("FLEET_UPDATER_HUB_SERVICE").unwrap_or_else(|| "fleet-hub".into());
    let project = match env("HOSTNAME") {
        Some(me) => docker
            .inspect_container(&me)
            .await
            .ok()
            .flatten()
            .and_then(|c| {
                docker::str_at(&c, &["Config", "Labels", "com.docker.compose.project"])
                    .map(String::from)
            }),
        None => None,
    };
    if let Some(project) = project {
        let labels = [
            format!("com.docker.compose.project={project}"),
            format!("com.docker.compose.service={service}"),
        ];
        if let Ok(list) = docker.list_containers(&labels).await {
            if let Some(name) = list
                .first()
                .and_then(|c| c.get("Names")?.as_array()?.first()?.as_str())
            {
                return name.trim_start_matches('/').to_string();
            }
        }
    }
    "fleet-hub".into()
}

fn usage() -> ExitCode {
    eprintln!(
        "usage: fleet-updater [--standalone] [run|once|status|clear|pair <url-or-code>|--version]"
    );
    ExitCode::from(2)
}

#[tokio::main]
async fn main() -> ExitCode {
    let mut standalone = env("FLEET_UPDATER_STANDALONE").is_some_and(|v| v == "1" || v == "true");
    let mut cmd = None;
    let mut pair_code = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--standalone" => standalone = true,
            "--version" | "-V" => {
                println!("fleet-updater {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            "-h" | "--help" => {
                usage();
                return ExitCode::SUCCESS;
            }
            "run" | "once" | "status" | "clear" if cmd.is_none() => cmd = Some(a),
            "pair" if cmd.is_none() => {
                let Some(code) = args.next() else {
                    return usage();
                };
                pair_code = Some(code);
                cmd = Some(a);
            }
            _ => return usage(),
        }
    }
    let cmd = cmd.unwrap_or_else(|| "run".into());
    let result = match pair_code {
        Some(code) => pair(&code).await,
        None => real_main(&cmd, standalone).await,
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            log!("{e}");
            ExitCode::FAILURE
        }
    }
}

async fn real_main(cmd: &str, standalone: bool) -> Result<ExitCode, String> {
    let state_dir = state_dir();
    let state = State::load(&state_dir)?;
    if cmd == "status" {
        println!(
            "{}",
            serde_json::to_string_pretty(&state).map_err(|e| e.to_string())?
        );
        return Ok(ExitCode::SUCCESS);
    }

    let docker = EngineDocker::from_env()?;
    let container = hub_container(&docker).await;
    let data_mount =
        PathBuf::from(env("FLEET_UPDATER_HUB_DATA").unwrap_or_else(|| "/hub-data".into()));
    let mut cfg = Config::defaults(&container, &data_mount, &state_dir);
    cfg.ready_timeout = env_secs("FLEET_UPDATER_READY_TIMEOUT_SECS", 90)?;
    cfg.soak = env_secs("FLEET_UPDATER_SOAK_SECS", 120)?;
    cfg.interval = env_secs("FLEET_UPDATER_INTERVAL_SECS", 21_600)?.max(Duration::from_secs(60));
    if let Some(k) = env("FLEET_UPDATER_KEEP_BACKUPS") {
        cfg.keep_backups = k
            .parse()
            .map_err(|_| format!("FLEET_UPDATER_KEEP_BACKUPS={k}: not a number"))?;
    }
    let keys = trusted_keys();
    let seen = Box::new(FileSequences::open(&state_dir));

    let channel: Box<dyn UpdateChannel> = if standalone {
        let track = match env("FLEET_UPDATER_TRACK").as_deref() {
            None | Some("stable") => Track::Stable,
            Some("beta") => Track::Beta,
            Some("nightly") => Track::Nightly,
            Some("dev") => Track::Dev,
            Some(t) => {
                return Err(format!(
                    "FLEET_UPDATER_TRACK={t}: stable, beta, nightly or dev"
                ))
            }
        };
        let mode = match env("FLEET_UPDATER_MODE").as_deref() {
            None | Some("notify") => Mode::Notify,
            Some("automatic") => Mode::Automatic,
            Some("manual") => Mode::Manual,
            Some(m) => {
                return Err(format!(
                    "FLEET_UPDATER_MODE={m}: manual, notify or automatic"
                ))
            }
        };
        let pin = env("FLEET_UPDATER_PIN")
            .map(|v| {
                Version::parse(v.trim_start_matches('v'))
                    .map(|version| Pin {
                        version,
                        mandatory: false,
                    })
                    .map_err(|e| format!("FLEET_UPDATER_PIN={v}: {e}"))
            })
            .transpose()?;
        let base = env("FLEET_UPDATE_CHANNEL_URL");
        let fetch = net::GitFetch::new(base.as_deref())?;
        Box::new(GitUpdateChannel::new(
            fetch,
            base.unwrap_or_else(|| CHANNEL_BASE_URL.into()),
            track,
            Policy {
                mode,
                pin,
                ..Policy::default()
            },
            keys,
            seen,
        ))
    } else {
        let url = env("FLEET_UPDATER_HUB_URL").unwrap_or_else(|| "http://fleet-hub:4180".into());
        let token = token(&state_dir)?;
        let host = net::hub_host_header(
            env("FLEET_UPDATER_HUB_HOST").as_deref(),
            env("FLEET_HUB_PUBLIC_URL").as_deref(),
        );
        Box::new(HubUpdateChannel::new(
            net::HubHttp::new(&url, &token, host)?,
            keys,
            seen,
        ))
    };

    let mut u = Updater::new(docker, channel, cfg, state);
    if cmd == "clear" {
        u.clear();
        log!("cleared; the next pass checks again");
        return Ok(ExitCode::SUCCESS);
    }
    log!(
        "watching {container} ({}; soak {} s)",
        if standalone {
            "standalone, the published channel"
        } else {
            "the hub decides"
        },
        u.cfg.soak.as_secs()
    );
    if cmd == "once" {
        let (o, _) = u.tick().await;
        return Ok(match o {
            Outcome::Failed(_) | Outcome::RollbackFailed(_) | Outcome::RolledBack { .. } => {
                ExitCode::FAILURE
            }
            _ => ExitCode::SUCCESS,
        });
    }
    // `update_admin update_now` on the hub drops this in its data dir: the
    // next pass starts at once instead of after `wait`.
    let trigger = data_mount.join(common::UPDATE_NOW_FILE);
    common::take_trigger(&trigger);
    loop {
        let (o, wait) = u.tick().await;
        if o != Outcome::Nothing {
            log!("{o:?}; next pass in {} s", wait.as_secs());
        }
        tokio::select! {
            poked = common::sleep_or_poked(&trigger, wait) => {
                if poked {
                    log!("the hub asked for an update now");
                }
            }
            _ = shutdown() => {
                log!("stopping");
                return Ok(ExitCode::SUCCESS);
            }
        }
    }
}

fn state_dir() -> PathBuf {
    PathBuf::from(env("FLEET_UPDATER_STATE_DIR").unwrap_or_else(|| "/var/lib/fleet-updater".into()))
}

/// Where `pair` keeps the token: the state volume, beside the state file.
const TOKEN_FILE: &str = "token";

/// `FLEET_UPDATER_TOKEN`, else `FLEET_UPDATER_TOKEN_FILE`, else the token
/// `fleet-updater pair` saved.
fn token(state_dir: &Path) -> Result<String, String> {
    if let Some(t) = env("FLEET_UPDATER_TOKEN") {
        return Ok(t);
    }
    let f = env("FLEET_UPDATER_TOKEN_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| state_dir.join(TOKEN_FILE));
    match std::fs::read_to_string(&f) {
        Ok(t) if !t.trim().is_empty() => Ok(t.trim().to_string()),
        Ok(_) => Err(format!("{} is empty", f.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(
            "no updater token: run `fleet-hub pair --name updater --mode updater` in the hub, then \
             `fleet-updater pair <the URL it printed>` (or set FLEET_UPDATER_TOKEN), or run --standalone"
                .into(),
        ),
        Err(e) => Err(format!("{}: {e}", f.display())),
    }
}

/// `fleet-updater pair <url-or-code>`: redeem the code at the hub's `/pair`
/// and keep the token in the state volume, readable by root only.
async fn pair(arg: &str) -> Result<ExitCode, String> {
    let url = env("FLEET_UPDATER_HUB_URL").unwrap_or_else(|| "http://fleet-hub:4180".into());
    let (token, name) = common::redeem(&url, arg).await?;
    let path = common::save_token(&state_dir(), TOKEN_FILE, &token)?;
    log!("paired as `{name}`; the token is in {}", path.display());
    Ok(ExitCode::SUCCESS)
}

async fn shutdown() {
    use tokio::signal::unix::{signal, SignalKind};
    match signal(SignalKind::terminate()) {
        Ok(mut term) => {
            tokio::select! {
                _ = term.recv() => {}
                _ = tokio::signal::ctrl_c() => {}
            }
        }
        Err(_) => {
            let _ = tokio::signal::ctrl_c().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_token_comes_from_the_state_volume_when_nothing_else_names_one() {
        let d = tempfile::tempdir().unwrap();
        assert!(token(d.path()).unwrap_err().contains("fleet-updater pair"));
        std::fs::write(d.path().join(TOKEN_FILE), "tok\n").unwrap();
        assert_eq!(token(d.path()).unwrap(), "tok");
    }
}
