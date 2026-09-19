//! `install` and `status`: a systemd unit that runs `fleet-agent run`.
//!
//! Everything that decides WHAT to write is a pure function of its inputs, and
//! everything that touches systemd goes through [`Systemctl`], so the tests
//! render into a directory they own and record the `systemctl` calls instead
//! of making them.

use crate::config::{self, Config};
use std::io::Write;
use std::path::{Path, PathBuf};

/// The unit's name, in both scopes.
pub const UNIT_NAME: &str = "fleet-agent.service";

/// Where the unit runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// A system unit (`/etc/systemd/system`), run as `run_as` — the user whose
    /// tmux and Claude sessions the agent drives. Needs root to install.
    System { run_as: String },
    /// A user unit (`~/.config/systemd/user`), run as whoever installs it.
    /// Stops at logout unless lingering is on.
    User,
}

/// Where the two files go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub unit_path: PathBuf,
    pub config_path: PathBuf,
}

impl Layout {
    /// The system scope's standard locations.
    pub fn system() -> Self {
        Self {
            unit_path: PathBuf::from("/etc/systemd/system").join(UNIT_NAME),
            config_path: PathBuf::from("/etc/fleet-agent/config.json"),
        }
    }

    /// The user scope's locations under `config_home` (`$XDG_CONFIG_HOME`,
    /// else `~/.config`).
    pub fn user(config_home: &Path) -> Self {
        Self {
            unit_path: config_home.join("systemd/user").join(UNIT_NAME),
            config_path: config_home.join("fleet-agent/config.json"),
        }
    }
}

/// Everything `install` needs, decided before anything is written.
#[derive(Debug, Clone)]
pub struct Plan {
    pub scope: Scope,
    pub binary: PathBuf,
    pub layout: Layout,
    pub config: Config,
    /// uid/gid the config file must belong to — the system scope's `run_as`,
    /// since a root-owned 0600 file is unreadable by the service. `None`
    /// leaves it with the installing user.
    pub owner: Option<(u32, u32)>,
    /// Enable and (re)start the unit after writing it.
    pub start: bool,
}

/// The one way this module reaches systemd.
pub trait Systemctl {
    fn run(&mut self, args: &[String]) -> Result<(), String>;
}

/// The real `systemctl`.
pub struct RealSystemctl;

impl Systemctl for RealSystemctl {
    fn run(&mut self, args: &[String]) -> Result<(), String> {
        let status = std::process::Command::new("systemctl")
            .args(args)
            .status()
            .map_err(|e| format!("could not run systemctl: {e}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("`systemctl {}` failed: {status}", args.join(" ")))
        }
    }
}

/// The unit file for `scope`.
///
/// Two lines carry the weight:
///
/// - `KillMode=process`. The agent starts tmux servers for the hub, and a
///   tmux server stays in the cgroup of whoever started it. systemd's default
///   (`control-group`) would therefore kill every Claude session on the host
///   each time the agent restarted or was upgraded. Only the agent itself is
///   stopped; what it started lives on, as it would over SSH.
/// - `NotifyAccess=main`, so systemd keeps the `STATUS=` line the agent
///   reports (`conn::Notifier`), which is how `status` knows whether it is
///   connected without the agent keeping a state file.
///
/// The token is NOT in the unit: units are world-readable. It is in the 0600
/// config `ExecStart` points at.
pub fn render_unit(scope: &Scope, binary: &Path, config_path: &Path) -> Result<String, String> {
    let binary = unit_path(binary, "the fleet-agent binary")?;
    let config_path = unit_path(config_path, "the config")?;
    let mut unit = String::new();
    unit.push_str("# Written by `fleet-agent install`; re-run it to change this file.\n");
    unit.push_str("[Unit]\n");
    unit.push_str("Description=claude-fleet agent: dials the hub and runs what it asks\n");
    unit.push_str("Wants=network-online.target\n");
    unit.push_str("After=network-online.target\n");
    unit.push('\n');
    unit.push_str("[Service]\n");
    unit.push_str("Type=simple\n");
    let wanted_by = match scope {
        Scope::System { run_as } => {
            check_user(run_as)?;
            unit.push_str(&format!("User={run_as}\n"));
            "multi-user.target"
        }
        Scope::User => "default.target",
    };
    unit.push_str(&format!("ExecStart={binary} run --config {config_path}\n"));
    // An ssh remote command starts in $HOME; so do the agent's children.
    unit.push_str("WorkingDirectory=~\n");
    unit.push_str("Restart=always\n");
    unit.push_str("RestartSec=5\n");
    unit.push_str("KillMode=process\n");
    unit.push_str("NotifyAccess=main\n");
    unit.push('\n');
    unit.push_str("[Install]\n");
    unit.push_str(&format!("WantedBy={wanted_by}\n"));
    Ok(unit)
}

/// A path as a unit line may hold it: absolute, and nothing systemd would
/// split, expand or read as a new directive. Refused rather than escaped.
fn unit_path(path: &Path, what: &str) -> Result<String, String> {
    let text = path
        .to_str()
        .ok_or_else(|| format!("{what} path is not UTF-8: {}", path.display()))?;
    if !path.is_absolute() {
        return Err(format!("{what} path must be absolute: {text}"));
    }
    if let Some(c) = text
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || "/._-+@".contains(*c)))
    {
        return Err(format!(
            "{what} path {text:?} has {c:?} in it, which a systemd unit would reinterpret; \
             move it somewhere plainer"
        ));
    }
    Ok(text.to_string())
}

/// A user name as `User=` may hold it.
fn check_user(name: &str) -> Result<(), String> {
    let ok = !name.is_empty()
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c));
    if ok {
        Ok(())
    } else {
        Err(format!(
            "{name:?} is not a user name a systemd unit can run as"
        ))
    }
}

/// The `systemctl` calls that load, enable and (re)start the unit. `restart`
/// rather than `enable --now`, so re-running install picks up a new config.
pub fn systemctl_calls(scope: &Scope) -> Vec<Vec<String>> {
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
        scoped(&["enable", UNIT_NAME]),
        scoped(&["restart", UNIT_NAME]),
    ]
}

/// Write the config and the unit, then hand the unit to systemd. Reports each
/// file it wrote and each call it made on `out`.
///
/// The unit is rendered FIRST, so a plan it refuses writes nothing — least of
/// all the token.
pub fn install(
    plan: &Plan,
    systemctl: &mut dyn Systemctl,
    out: &mut dyn Write,
) -> Result<(), String> {
    let unit = render_unit(&plan.scope, &plan.binary, &plan.layout.config_path)?;

    config::write(&plan.layout.config_path, &plan.config, plan.owner).map_err(|e| e.to_string())?;
    let _ = writeln!(
        out,
        "wrote {} (mode 0600: the hub URL and this host's token)",
        plan.layout.config_path.display()
    );

    let unit_path = &plan.layout.unit_path;
    if let Some(dir) = unit_path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(unit_path, &unit).map_err(|e| format!("{}: {e}", unit_path.display()))?;
    let _ = writeln!(out, "wrote {}:", unit_path.display());
    for line in unit.lines() {
        let _ = writeln!(out, "    {line}");
    }

    if !plan.start {
        let _ = writeln!(out, "not started (--no-start)");
        return Ok(());
    }
    for call in systemctl_calls(&plan.scope) {
        let _ = writeln!(out, "systemctl {}", call.join(" "));
        systemctl.run(&call)?;
    }
    if plan.scope == Scope::User {
        let _ = writeln!(
            out,
            "note: a user unit stops at logout unless lingering is on: loginctl enable-linger"
        );
    }
    Ok(())
}

/// What `systemctl show` says about the unit.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ServiceState {
    pub active: String,
    pub sub: String,
    /// What the agent last reported over `sd_notify` — see `conn::Notifier`.
    pub status_text: String,
}

impl ServiceState {
    /// Running, and the agent says it is connected. A stopped unit's last
    /// report may still say "connected"; it is not.
    pub fn connected(&self) -> bool {
        self.active == "active" && self.status_text.starts_with(crate::conn::CONNECTED)
    }
}

/// Parse `systemctl show -p ActiveState -p SubState -p StatusText`.
pub fn parse_show(text: &str) -> ServiceState {
    let mut state = ServiceState::default();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "ActiveState" => state.active = value.to_string(),
            "SubState" => state.sub = value.to_string(),
            "StatusText" => state.status_text = value.to_string(),
            _ => {}
        }
    }
    state
}

/// The arguments `status` passes to `systemctl`.
pub fn show_args(scope_user: bool) -> Vec<String> {
    let mut v = Vec::new();
    if scope_user {
        v.push("--user".to_string());
    }
    for a in [
        "show",
        UNIT_NAME,
        "--no-pager",
        "-p",
        "ActiveState",
        "-p",
        "SubState",
        "-p",
        "StatusText",
    ] {
        v.push(a.to_string());
    }
    v
}

/// `user`'s uid and gid, from the system's user database (so LDAP and
/// friends count, not only `/etc/passwd`).
#[cfg(unix)]
pub fn lookup_user(user: &str) -> Result<(u32, u32), String> {
    let name = std::ffi::CString::new(user).map_err(|_| format!("{user:?} is not a user name"))?;
    let mut buf = vec![0u8; 16 * 1024];
    // SAFETY: every pointer is to a local that outlives the call, and `buf`'s
    // length is passed alongside it.
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut found: *mut libc::passwd = std::ptr::null_mut();
    let rc = unsafe {
        libc::getpwnam_r(
            name.as_ptr(),
            &mut pwd,
            buf.as_mut_ptr().cast(),
            buf.len(),
            &mut found,
        )
    };
    if rc != 0 || found.is_null() {
        return Err(format!("no such user: {user}"));
    }
    Ok((pwd.pw_uid, pwd.pw_gid))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    const TOKEN: &str = "per-host-token-abc123";

    fn config() -> Config {
        Config {
            hub: "https://hub.example".into(),
            token: TOKEN.into(),
            insecure: false,
            ca_file: None,
        }
    }

    #[derive(Default)]
    struct Recorder {
        calls: Vec<Vec<String>>,
    }

    impl Systemctl for Recorder {
        fn run(&mut self, args: &[String]) -> Result<(), String> {
            self.calls.push(args.to_vec());
            Ok(())
        }
    }

    fn line_in(unit: &str, want: &str) -> bool {
        unit.lines().any(|l| l == want)
    }

    #[test]
    fn a_system_unit_names_the_binary_the_config_and_the_user() {
        let unit = render_unit(
            &Scope::System {
                run_as: "alice".into(),
            },
            Path::new("/usr/local/bin/fleet-agent"),
            Path::new("/etc/fleet-agent/config.json"),
        )
        .unwrap();
        assert!(
            line_in(
                &unit,
                "ExecStart=/usr/local/bin/fleet-agent run --config /etc/fleet-agent/config.json"
            ),
            "{unit}"
        );
        assert!(line_in(&unit, "User=alice"), "{unit}");
        assert!(line_in(&unit, "WantedBy=multi-user.target"), "{unit}");
    }

    #[test]
    fn a_user_unit_has_no_user_line_and_starts_with_the_session() {
        let unit = render_unit(
            &Scope::User,
            Path::new("/home/alice/bin/fleet-agent"),
            Path::new("/home/alice/.config/fleet-agent/config.json"),
        )
        .unwrap();
        assert!(
            line_in(
                &unit,
                "ExecStart=/home/alice/bin/fleet-agent run --config /home/alice/.config/fleet-agent/config.json"
            ),
            "{unit}"
        );
        assert!(!unit.lines().any(|l| l.starts_with("User=")), "{unit}");
        assert!(line_in(&unit, "WantedBy=default.target"), "{unit}");
    }

    /// The agent starts tmux servers, and a tmux server lives in the cgroup of
    /// whoever started it. systemd's default `KillMode=control-group` would
    /// kill every Claude session on the host each time the agent restarts.
    #[test]
    fn restarting_the_agent_does_not_kill_what_it_started() {
        for scope in [Scope::User, Scope::System { run_as: "a".into() }] {
            let unit = render_unit(&scope, Path::new("/bin/fa"), Path::new("/c")).unwrap();
            assert!(line_in(&unit, "KillMode=process"), "{unit}");
            assert!(line_in(&unit, "Restart=always"), "{unit}");
            // Without it systemd ignores the agent's STATUS= reports, which
            // is what `status` reads.
            assert!(line_in(&unit, "NotifyAccess=main"), "{unit}");
        }
    }

    /// A unit line is parsed by systemd: a space splits an argument, `%` is a
    /// specifier, a newline starts a new directive. Refused rather than
    /// escaped — none of them belongs in a binary path or a user name.
    #[test]
    fn a_path_or_user_systemd_would_reinterpret_is_refused() {
        let ok = Path::new("/usr/bin/fleet-agent");
        for bad in [
            "/opt/my agent/fa",
            "/opt/a%h/fa",
            "/opt/a\nUser=root/fa",
            "relative/fa",
        ] {
            assert!(
                render_unit(&Scope::User, Path::new(bad), Path::new("/c")).is_err(),
                "binary {bad:?}"
            );
            assert!(
                render_unit(&Scope::User, ok, Path::new(bad)).is_err(),
                "config {bad:?}"
            );
        }
        for bad in ["", "al ice", "alice\nExecStart=/bin/sh", "-alice", "a%b"] {
            let scope = Scope::System { run_as: bad.into() };
            assert!(
                render_unit(&scope, ok, Path::new("/c")).is_err(),
                "user {bad:?}"
            );
        }
    }

    #[test]
    fn the_systemctl_calls_reload_enable_and_restart_in_the_right_scope() {
        let sys = systemctl_calls(&Scope::System {
            run_as: "alice".into(),
        });
        assert_eq!(
            sys,
            [
                vec!["daemon-reload".to_string()],
                vec!["enable".into(), UNIT_NAME.into()],
                vec!["restart".into(), UNIT_NAME.into()],
            ]
        );
        let user = systemctl_calls(&Scope::User);
        assert!(user.iter().all(|c| c[0] == "--user"), "{user:?}");
        assert_eq!(user.len(), 3);
    }

    fn plan_in(dir: &Path, start: bool) -> Plan {
        Plan {
            scope: Scope::User,
            binary: PathBuf::from("/usr/local/bin/fleet-agent"),
            layout: Layout::user(dir),
            config: config(),
            owner: None,
            start,
        }
    }

    #[test]
    fn install_writes_a_private_config_and_a_unit_without_the_token() {
        let dir = tempfile::tempdir().unwrap();
        let plan = plan_in(dir.path(), true);
        let mut systemctl = Recorder::default();
        let mut out = Vec::new();
        install(&plan, &mut systemctl, &mut out).unwrap();

        let cfg = &plan.layout.config_path;
        assert_eq!(
            std::fs::metadata(cfg).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(config::load(cfg).unwrap(), config());

        let unit = std::fs::read_to_string(&plan.layout.unit_path).unwrap();
        assert!(!unit.contains(TOKEN), "the unit is world-readable: {unit}");
        assert!(unit.contains(cfg.to_str().unwrap()));

        assert_eq!(systemctl.calls, systemctl_calls(&plan.scope));

        // It says what it wrote.
        let said = String::from_utf8(out).unwrap();
        assert!(said.contains(cfg.to_str().unwrap()), "{said}");
        assert!(
            said.contains(plan.layout.unit_path.to_str().unwrap()),
            "{said}"
        );
        assert!(!said.contains(TOKEN), "{said}");
    }

    /// The system scope's layout, rendered into a directory the test owns:
    /// root would create the config's directory for ANOTHER user, and that
    /// user must be able to open the file through it.
    #[test]
    fn a_system_install_leaves_the_config_reachable_by_the_user_it_runs_as() {
        use std::os::unix::fs::MetadataExt;
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: no preconditions.
        let me = unsafe { (libc::getuid(), libc::getgid()) };
        let plan = Plan {
            scope: Scope::System {
                run_as: "alice".into(),
            },
            binary: PathBuf::from("/usr/local/bin/fleet-agent"),
            layout: Layout {
                unit_path: dir.path().join("systemd/system").join(UNIT_NAME),
                config_path: dir.path().join("etc/fleet-agent/config.json"),
            },
            config: config(),
            owner: Some(me),
            start: false,
        };
        install(&plan, &mut Recorder::default(), &mut Vec::new()).unwrap();
        let cfg_dir = plan.layout.config_path.parent().unwrap();
        assert_eq!(
            std::fs::metadata(cfg_dir).unwrap().permissions().mode() & 0o777,
            0o755,
            "passable by the run-as user"
        );
        let file = std::fs::metadata(&plan.layout.config_path).unwrap();
        assert_eq!(file.permissions().mode() & 0o777, 0o600);
        assert_eq!((file.uid(), file.gid()), me, "the run-as user's file");
    }

    #[test]
    fn install_without_start_writes_the_files_and_leaves_systemd_alone() {
        let dir = tempfile::tempdir().unwrap();
        let plan = plan_in(dir.path(), false);
        let mut systemctl = Recorder::default();
        install(&plan, &mut systemctl, &mut Vec::new()).unwrap();
        assert!(plan.layout.unit_path.exists());
        assert!(systemctl.calls.is_empty(), "{:?}", systemctl.calls);
    }

    /// A refused unit writes nothing at all — not even the token.
    #[test]
    fn install_with_an_unrenderable_unit_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut plan = plan_in(dir.path(), true);
        plan.binary = PathBuf::from("/opt/has space/fleet-agent");
        let mut systemctl = Recorder::default();
        assert!(install(&plan, &mut systemctl, &mut Vec::new()).is_err());
        assert!(!plan.layout.config_path.exists());
        assert!(!plan.layout.unit_path.exists());
        assert!(systemctl.calls.is_empty());
    }

    #[test]
    fn a_user_is_looked_up_in_the_user_database() {
        assert_eq!(lookup_user("root").unwrap(), (0, 0));
        assert!(lookup_user("no-such-user-fleet-agent-test").is_err());
        assert!(lookup_user("nul\0byte").is_err());
    }

    #[test]
    fn status_reads_the_agent_s_own_report() {
        let s = parse_show(
            "ActiveState=active\nSubState=running\nStatusText=connected to wss://hub.example/agent since 2026-09-18 10:00:00 UTC\n",
        );
        assert_eq!(s.active, "active");
        assert_eq!(s.sub, "running");
        assert!(s.status_text.starts_with("connected to"));
        assert!(s.connected());

        let retrying = parse_show(
            "ActiveState=active\nSubState=running\nStatusText=reconnecting in 8s: 401 Unauthorized\n",
        );
        assert!(!retrying.connected());

        // A stopped unit's last report can still say "connected"; it is not.
        let stale = parse_show(
            "ActiveState=inactive\nSubState=dead\nStatusText=connected to wss://hub.example/agent since then\n",
        );
        assert!(!stale.connected());

        let stopped = parse_show("ActiveState=inactive\nSubState=dead\nStatusText=\n");
        assert!(!stopped.connected());
        assert_eq!(stopped.status_text, "");
    }
}
