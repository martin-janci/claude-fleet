//! `fleet-agent update`: the agent updates itself (update-channel design
//! §8.6, slice S9).
//!
//! One pass of `fleet_updater::binary`: ask the hub (`/update/check`, with
//! this host's own token — it already identifies the host as
//! `agent:<alias>`), and install what it decides — an operator's pin, a
//! required update, a rollback, or anything under `update.agent.mode =
//! automatic`. The tarball comes from the hub's mirror when it has one, else
//! from the release; it must be the bytes the signed manifest names. The new
//! release goes to `<root>/<version>/`, `<root>/current` points at it, the
//! unit is restarted, and it has to come back connected to its hub — or
//! `current` goes back.
//!
//! It runs from its own oneshot unit (`fleet-agent-update.service`, started
//! by `fleet-agent-update.timer`, which `install --auto-update` writes), so
//! restarting `fleet-agent.service` does not stop it halfway.

use std::path::{Path, PathBuf};

use fleet_update::wire::Speaks;
use fleet_update::Component;
use fleet_updater::binary::{BinState, BinaryConfig, BinaryUpdater, Outcome};
use fleet_updater::common::{trusted_keys, FileSequences};
use fleet_updater::net::{GitFetch, HubHttp};
use fleet_updater::systemd::SystemdHost;

use crate::config::Config;
use crate::install::{Scope, UNIT_NAME};

pub const UPDATE_SERVICE: &str = "fleet-agent-update.service";
pub const UPDATE_TIMER: &str = "fleet-agent-update.timer";
/// Starts a pass at once when the hub's `update_now` drops `update-now` in
/// the agent's runtime directory.
pub const UPDATE_PATH: &str = "fleet-agent-update.path";

/// Where the release directories and the updater's state live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdatePaths {
    pub root: PathBuf,
    pub state_dir: PathBuf,
    /// What the unit runs when nothing else says (a re-install from inside
    /// the layout).
    pub default_link: PathBuf,
}

impl UpdatePaths {
    pub fn system() -> Self {
        UpdatePaths {
            root: PathBuf::from("/opt/fleet-agent"),
            state_dir: PathBuf::from("/var/lib/fleet-agent-update"),
            default_link: PathBuf::from("/usr/local/bin/fleet-agent"),
        }
    }

    /// Under `home` (`$HOME`).
    pub fn user(home: &Path) -> Self {
        UpdatePaths {
            root: home.join(".local/lib/fleet-agent"),
            state_dir: home.join(".local/state/fleet-agent-update"),
            default_link: home.join(".local/bin/fleet-agent"),
        }
    }
}

/// The binary a unit's `ExecStart=` runs.
pub fn unit_binary(unit: &str) -> Option<PathBuf> {
    unit.lines()
        .find_map(|l| l.trim().strip_prefix("ExecStart="))
        .and_then(|cmd| cmd.split_whitespace().next())
        .map(PathBuf::from)
}

/// The path a unit should run: `binary`, unless it is a release directory
/// inside `root` (this binary was started through the layout's symlink, and
/// `current_exe` resolved it) — then the path the unit already runs, else
/// the layout's default. A unit pinned to one release dir would never move.
pub fn stable_binary(
    binary: &Path,
    root: &Path,
    existing: Option<PathBuf>,
    default_link: &Path,
) -> PathBuf {
    if !binary.starts_with(root) {
        return binary.to_path_buf();
    }
    existing
        .filter(|p| !p.starts_with(root))
        .unwrap_or_else(|| default_link.to_path_buf())
}

/// The oneshot unit the timer starts.
pub fn render_update_service(scope: &Scope, binary: &Path) -> Result<String, String> {
    let bin = binary
        .to_str()
        .filter(|b| {
            b.chars()
                .all(|c| c.is_ascii_alphanumeric() || "/._-+@".contains(c))
        })
        .ok_or_else(|| format!("{} is not a path a unit can hold", binary.display()))?;
    let user_flag = if *scope == Scope::User { " --user" } else { "" };
    Ok(format!(
        "# Written by `fleet-agent install --auto-update`; re-run it to change this file.\n\
         [Unit]\n\
         Description=claude-fleet agent: install the update its hub decides (one pass)\n\
         Wants=network-online.target\n\
         After=network-online.target\n\
         \n\
         [Service]\n\
         Type=oneshot\n\
         # The hub's update_now trigger, consumed: the path unit fires again only on a new one.\n\
         ExecStartPre=-/bin/rm -f %t/fleet-agent/update-now\n\
         ExecStart={bin} update{user_flag}\n"
    ))
}

/// Every six hours, spread over half an hour so a fleet does not ask at once.
pub fn render_update_timer() -> String {
    "# Written by `fleet-agent install --auto-update`.\n\
     [Unit]\n\
     Description=claude-fleet agent: check for an update\n\
     \n\
     [Timer]\n\
     OnBootSec=15min\n\
     OnUnitActiveSec=6h\n\
     RandomizedDelaySec=30min\n\
     Persistent=true\n\
     \n\
     [Install]\n\
     WantedBy=timers.target\n"
        .to_string()
}

/// The path unit: `update_now` on the hub → one pass now.
pub fn render_update_path() -> String {
    "# Written by `fleet-agent install --auto-update`.\n\
     [Unit]\n\
     Description=claude-fleet agent: install an update now, when the hub says so\n\
     \n\
     [Path]\n\
     PathExists=%t/fleet-agent/update-now\n\
     Unit=fleet-agent-update.service\n\
     \n\
     [Install]\n\
     WantedBy=paths.target\n"
        .to_string()
}

/// The `systemctl` calls that turn the timer and the path unit on.
pub fn update_systemctl_calls(scope: &Scope) -> Vec<Vec<String>> {
    let scoped = |args: &[&str]| -> Vec<String> {
        let mut v = Vec::new();
        if *scope == Scope::User {
            v.push("--user".to_string());
        }
        v.extend(args.iter().map(|a| a.to_string()));
        v
    };
    vec![
        scoped(&["daemon-reload"]),
        scoped(&["enable", "--now", UPDATE_TIMER]),
        scoped(&["enable", "--now", UPDATE_PATH]),
    ]
}

/// One pass. `Ok(true)` unless an install failed (or rolled back).
pub async fn run(
    config: &Config,
    paths: &UpdatePaths,
    unit_path: &Path,
    user_scope: bool,
    status: bool,
    clear: bool,
) -> Result<bool, String> {
    let state = BinState::load(&paths.state_dir)?;
    if status {
        println!(
            "{}",
            serde_json::to_string_pretty(&state).map_err(|e| e.to_string())?
        );
        return Ok(true);
    }
    let link = std::fs::read_to_string(unit_path)
        .ok()
        .and_then(|u| unit_binary(&u))
        .ok_or_else(|| {
            format!(
                "{} names no binary: run `fleet-agent install` first",
                unit_path.display()
            )
        })?;
    let hub = HubHttp::with_ca(&config.hub, &config.token, None, config.ca_file.as_deref())?;
    let channel = fleet_update::HubUpdateChannel::new(
        hub.clone(),
        trusted_keys(),
        Box::new(FileSequences::open(&paths.state_dir)),
    );
    let host = SystemdHost {
        unit: UNIT_NAME.to_string(),
        user_scope,
        hub: None,
        link: link.clone(),
        git: GitFetch::new(None)?,
        mirror: Some(hub),
    };
    let mut cfg = BinaryConfig::new(
        Component::Agent,
        "fleet-agent",
        &paths.root,
        &link,
        &paths.state_dir,
    );
    cfg.speaks = Speaks {
        contract_accepts: None,
        agent_proto: Some(fleet_proto::PROTO_VERSION),
    };
    cfg.name = "fleet-agent update".into();
    let mut u = BinaryUpdater::new(host, Box::new(channel), cfg, state);
    if clear {
        u.clear();
        println!("cleared; the next pass checks again");
        return Ok(true);
    }
    let (o, _) = u.tick().await;
    Ok(!matches!(
        o,
        Outcome::Failed(_) | Outcome::RollbackFailed(_) | Outcome::RolledBack { .. }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_unit_names_its_binary() {
        assert_eq!(
            unit_binary("[Service]\nExecStart=/usr/local/bin/fleet-agent run --config /etc/x\n"),
            Some(PathBuf::from("/usr/local/bin/fleet-agent"))
        );
        assert_eq!(unit_binary("[Service]\n"), None);
    }

    #[test]
    fn a_reinstall_from_inside_the_layout_keeps_the_units_path() {
        let root = Path::new("/opt/fleet-agent");
        let link = Path::new("/usr/local/bin/fleet-agent");
        let inside = Path::new("/opt/fleet-agent/0.5.4/fleet-agent");
        assert_eq!(
            stable_binary(Path::new("/home/u/fleet-agent"), root, None, link),
            Path::new("/home/u/fleet-agent")
        );
        assert_eq!(
            stable_binary(inside, root, Some("/home/u/bin/fleet-agent".into()), link),
            Path::new("/home/u/bin/fleet-agent")
        );
        assert_eq!(stable_binary(inside, root, None, link), link);
        assert_eq!(stable_binary(inside, root, Some(inside.into()), link), link);
    }

    #[test]
    fn the_timer_starts_one_pass_in_the_units_scope() {
        let s = render_update_service(&Scope::User, Path::new("/home/u/.local/bin/fleet-agent"))
            .unwrap();
        assert!(s.contains("Type=oneshot\n"));
        assert!(s.contains("ExecStart=/home/u/.local/bin/fleet-agent update --user\n"));
        let s = render_update_service(
            &Scope::System { run_as: "u".into() },
            Path::new("/usr/local/bin/fleet-agent"),
        )
        .unwrap();
        assert!(s.contains("ExecStart=/usr/local/bin/fleet-agent update\n"));
        assert!(
            !s.contains("User="),
            "the system pass runs as root: it writes /opt and restarts the unit"
        );
        assert!(render_update_service(&Scope::User, Path::new("/tmp/a b")).is_err());
        assert!(render_update_timer().contains("OnUnitActiveSec=6h"));
        assert!(s.contains("ExecStartPre=-/bin/rm -f %t/fleet-agent/update-now\n"));
        let p = render_update_path();
        assert!(p.contains("PathExists=%t/fleet-agent/update-now\n"));
        assert!(p.contains("Unit=fleet-agent-update.service\n"));
        assert_eq!(
            update_systemctl_calls(&Scope::User),
            vec![
                vec!["--user", "daemon-reload"],
                vec!["--user", "enable", "--now", UPDATE_TIMER],
                vec!["--user", "enable", "--now", UPDATE_PATH],
            ]
        );
    }
}
