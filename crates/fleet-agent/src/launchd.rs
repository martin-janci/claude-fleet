//! `install` and `status` on macOS: a launchd LaunchAgent that runs
//! `fleet-agent run` for the user who installs it.
//!
//! The macOS counterpart of [`crate::install`]'s systemd user unit. A Mac is
//! the host this exists for: a laptop carried between networks, which no hub
//! can reach over SSH once it has left the one its alias points into, but
//! which can always dial the hub out over HTTPS.
//!
//! Like `install.rs`, everything that decides what to write is a pure
//! function, and every `launchctl` call goes through [`Launchctl`], so the
//! tests render into a directory they own and record the calls.

use crate::config::{self, Config};
use std::io::Write;
use std::path::{Path, PathBuf};

/// The job's label, and the plist's file name without `.plist`.
pub const LABEL: &str = "sk.rlt.fleet-agent";

/// Where the plist, the config and the log go for a user whose home is
/// `home` (config under `config_home`, the same `~/.config/fleet-agent` a
/// Linux user unit uses, so `fleet-agent run` with no flags finds it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub plist_path: PathBuf,
    pub config_path: PathBuf,
    pub log_path: PathBuf,
}

impl Layout {
    pub fn user(home: &Path, config_home: &Path) -> Self {
        Self {
            plist_path: home
                .join("Library/LaunchAgents")
                .join(format!("{LABEL}.plist")),
            config_path: config_home.join("fleet-agent/config.json"),
            log_path: home.join("Library/Logs/fleet-agent.log"),
        }
    }
}

/// Everything `install` needs, decided before anything is written.
#[derive(Debug, Clone)]
pub struct Plan {
    pub binary: PathBuf,
    pub home: PathBuf,
    pub layout: Layout,
    pub config: Config,
    /// The installing user's uid: the `gui/<uid>` domain the job loads into.
    pub uid: u32,
    /// Load (or reload) the job after writing it.
    pub start: bool,
    /// `--auto-update` was asked for. There is no timer to install on macOS:
    /// the agent updates itself in-process (`crate::self_update`), which is
    /// already on wherever no systemd timer is; `install` says so.
    pub auto_update: bool,
}

/// The one way this module reaches launchd.
pub trait Launchctl {
    fn run(&mut self, args: &[String]) -> Result<(), String>;
}

/// The real `launchctl`.
pub struct RealLaunchctl;

impl Launchctl for RealLaunchctl {
    fn run(&mut self, args: &[String]) -> Result<(), String> {
        let status = std::process::Command::new("launchctl")
            .args(args)
            .status()
            .map_err(|e| format!("could not run launchctl: {e}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("`launchctl {}` failed: {status}", args.join(" ")))
        }
    }
}

/// `text` as plist `<string>` content.
fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// A path as the plist may hold it: absolute and UTF-8. No quoting rules to
/// trip over (launchd does not run a shell), so nothing else is refused.
fn plist_path(path: &Path, what: &str) -> Result<String, String> {
    let text = path
        .to_str()
        .ok_or_else(|| format!("{what} path is not UTF-8: {}", path.display()))?;
    if !path.is_absolute() {
        return Err(format!("{what} path must be absolute: {text}"));
    }
    Ok(xml(text))
}

/// The LaunchAgent's plist.
///
/// What carries the weight:
///
/// - `KeepAlive` + `RunAtLoad`: started at login and whenever it exits, the
///   way `Restart=always` keeps the systemd unit up. `ThrottleInterval` is
///   `RestartSec`.
/// - `AbandonProcessGroup`: stopping or reloading the job (an upgrade, a
///   re-run of `install`) must not take the tmux servers and Claude sessions
///   the agent started with it — `KillMode=process` on Linux.
/// - `ProcessType` `Interactive`: launchd throttles CPU and I/O of an agent
///   it considers background work, and every Claude session on the Mac runs
///   under this job.
/// - `PATH`: launchd hands a job `/usr/bin:/bin:/usr/sbin:/sbin` and nothing
///   else, so tmux and claude from Homebrew or `~/.local/bin` would not be
///   found by the hub's non-login scripts.
///
/// The token is NOT in the plist: it is in the 0600 config the job points at.
pub fn render_plist(binary: &Path, layout: &Layout, home: &Path) -> Result<String, String> {
    let binary = plist_path(binary, "the fleet-agent binary")?;
    let config = plist_path(&layout.config_path, "the config")?;
    let log = plist_path(&layout.log_path, "the log")?;
    let home = plist_path(home, "the home directory")?;
    Ok(format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<!-- Written by `fleet-agent install`; re-run it to change this file. -->
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{LABEL}</string>
	<key>ProgramArguments</key>
	<array>
		<string>{binary}</string>
		<string>run</string>
		<string>--config</string>
		<string>{config}</string>
	</array>
	<key>RunAtLoad</key>
	<true/>
	<key>KeepAlive</key>
	<true/>
	<key>ThrottleInterval</key>
	<integer>5</integer>
	<key>AbandonProcessGroup</key>
	<true/>
	<key>ProcessType</key>
	<string>Interactive</string>
	<key>WorkingDirectory</key>
	<string>{home}</string>
	<key>EnvironmentVariables</key>
	<dict>
		<key>PATH</key>
		<string>{home}/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin</string>
	</dict>
	<key>StandardOutPath</key>
	<string>{log}</string>
	<key>StandardErrorPath</key>
	<string>{log}</string>
</dict>
</plist>
"#
    ))
}

/// The job's target in the user's GUI domain.
pub fn service_target(uid: u32) -> String {
    format!("gui/{uid}/{LABEL}")
}

/// The calls that (re)load the job: enable it (a job once disabled stays
/// disabled across loads), take out a running copy — allowed to fail, there
/// may be none — and load the plist, which starts it (`RunAtLoad`).
///
/// Each call is `(args, may_fail)`.
pub fn launchctl_calls(uid: u32, plist: &Path) -> Vec<(Vec<String>, bool)> {
    let target = service_target(uid);
    vec![
        (vec!["enable".into(), target.clone()], false),
        (vec!["bootout".into(), target], true),
        (
            vec![
                "bootstrap".into(),
                format!("gui/{uid}"),
                plist.to_string_lossy().into_owned(),
            ],
            false,
        ),
    ]
}

/// Write the config and the plist, then hand the plist to launchd. Reports
/// each file it wrote and each call it made on `out`.
///
/// The plist is rendered FIRST, so a plan it refuses writes nothing — least
/// of all the token.
pub fn install(
    plan: &Plan,
    launchctl: &mut dyn Launchctl,
    out: &mut dyn Write,
) -> Result<(), String> {
    let plist = render_plist(&plan.binary, &plan.layout, &plan.home)?;

    config::write(&plan.layout.config_path, &plan.config, None).map_err(|e| e.to_string())?;
    let _ = writeln!(
        out,
        "wrote {} (mode 0600: the hub URL and this host's token)",
        plan.layout.config_path.display()
    );

    for path in [&plan.layout.plist_path, &plan.layout.log_path] {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
    }
    let plist_path = &plan.layout.plist_path;
    std::fs::write(plist_path, &plist).map_err(|e| format!("{}: {e}", plist_path.display()))?;
    let _ = writeln!(out, "wrote {}", plist_path.display());

    if plan.auto_update {
        let _ = writeln!(
            out,
            "note: no update timer on macOS; the agent updates itself in-process \
             (set FLEET_AGENT_SELF_UPDATE=off in the plist to leave updates to someone else)"
        );
    }
    if !plan.start {
        let _ = writeln!(out, "not started (--no-start)");
        return Ok(());
    }
    for (call, may_fail) in launchctl_calls(plan.uid, plist_path) {
        let _ = writeln!(out, "launchctl {}", call.join(" "));
        match launchctl.run(&call) {
            Ok(()) => {}
            Err(_) if may_fail => {}
            Err(why) => {
                return Err(format!(
                    "{why}. The job loads into your login session (gui/{uid}): install it \
                     from a session where you are logged in to the Mac, or run \
                     `launchctl bootstrap gui/{uid} {}` after you log in",
                    plist_path.display(),
                    uid = plan.uid
                ))
            }
        }
    }
    let _ = writeln!(
        out,
        "note: the agent runs while you are logged in; it logs to {}",
        plan.layout.log_path.display()
    );
    Ok(())
}

/// The arguments `status` passes to `launchctl`.
pub fn print_args(uid: u32) -> Vec<String> {
    vec!["print".into(), service_target(uid)]
}

/// The job's `state = …` from `launchctl print` (`running`, `waiting`, …),
/// or `None` when the output has none (the job is not loaded).
pub fn parse_print(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let (key, value) = line.trim().split_once('=')?;
        (key.trim() == "state").then(|| value.trim().to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    const TOKEN: &str = "per-host-token-abc123";

    #[derive(Default)]
    struct Recorder {
        calls: Vec<Vec<String>>,
        fail_on: Option<&'static str>,
    }

    impl Launchctl for Recorder {
        fn run(&mut self, args: &[String]) -> Result<(), String> {
            self.calls.push(args.to_vec());
            match self.fail_on {
                Some(verb) if args.first().map(String::as_str) == Some(verb) => {
                    Err(format!("{verb} failed"))
                }
                _ => Ok(()),
            }
        }
    }

    fn plan_in(dir: &Path, start: bool) -> Plan {
        let home = dir.join("home");
        Plan {
            binary: PathBuf::from("/Users/dev/.local/bin/fleet-agent"),
            layout: Layout::user(&home, &home.join(".config")),
            home,
            config: Config {
                hub: "https://hub.example".into(),
                token: TOKEN.into(),
                insecure: false,
                ca_file: None,
                report_errors: true,
                hook_port: config::default_hook_port(),
                proxy: None,
            },
            uid: 501,
            start,
            auto_update: false,
        }
    }

    #[test]
    fn the_plist_keeps_the_sessions_alive_and_the_token_out() {
        let dir = tempfile::tempdir().unwrap();
        let plan = plan_in(dir.path(), true);
        let plist = render_plist(&plan.binary, &plan.layout, &plan.home).unwrap();
        assert!(plist.contains(&format!("<string>{LABEL}</string>")));
        assert!(plist.contains("<string>/Users/dev/.local/bin/fleet-agent</string>"));
        assert!(plist.contains("<string>run</string>"));
        assert!(plist.contains(&format!(
            "<string>{}</string>",
            plan.layout.config_path.display()
        )));
        for key in ["RunAtLoad", "KeepAlive", "AbandonProcessGroup"] {
            assert!(
                plist.contains(&format!("<key>{key}</key>\n\t<true/>")),
                "{key} must be true"
            );
        }
        assert!(plist.contains("<string>Interactive</string>"));
        assert!(plist.contains("/opt/homebrew/bin"));
        assert!(!plist.contains(TOKEN));
    }

    #[test]
    fn a_path_with_markup_is_escaped_and_a_relative_one_refused() {
        let home = PathBuf::from("/Users/a&b");
        let layout = Layout::user(&home, &home.join(".config"));
        let plist = render_plist(Path::new("/opt/<x>/fleet-agent"), &layout, &home).unwrap();
        assert!(plist.contains("/opt/&lt;x&gt;/fleet-agent"));
        assert!(plist.contains("/Users/a&amp;b/.config/fleet-agent/config.json"));
        assert!(render_plist(Path::new("fleet-agent"), &layout, &home).is_err());
    }

    #[test]
    fn install_writes_a_private_config_and_loads_the_job_into_the_gui_domain() {
        let dir = tempfile::tempdir().unwrap();
        let plan = plan_in(dir.path(), true);
        let mut rec = Recorder::default();
        let mut out = Vec::new();
        install(&plan, &mut rec, &mut out).unwrap();

        let mode = std::fs::metadata(&plan.layout.config_path)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        let plist = std::fs::read_to_string(&plan.layout.plist_path).unwrap();
        assert!(!plist.contains(TOKEN));
        assert!(plan
            .layout
            .plist_path
            .ends_with("Library/LaunchAgents/sk.rlt.fleet-agent.plist"));
        assert_eq!(
            rec.calls,
            vec![
                vec!["enable".to_string(), "gui/501/sk.rlt.fleet-agent".into()],
                vec!["bootout".to_string(), "gui/501/sk.rlt.fleet-agent".into()],
                vec![
                    "bootstrap".to_string(),
                    "gui/501".into(),
                    plan.layout.plist_path.to_string_lossy().into_owned()
                ],
            ]
        );
        let out = String::from_utf8(out).unwrap();
        assert!(!out.contains(TOKEN));
    }

    #[test]
    fn nothing_running_to_boot_out_is_not_an_error_but_a_failed_load_is() {
        let dir = tempfile::tempdir().unwrap();
        let plan = plan_in(dir.path(), true);
        let mut rec = Recorder {
            fail_on: Some("bootout"),
            ..Default::default()
        };
        install(&plan, &mut rec, &mut Vec::new()).unwrap();

        let mut rec = Recorder {
            fail_on: Some("bootstrap"),
            ..Default::default()
        };
        let err = install(&plan, &mut rec, &mut Vec::new()).unwrap_err();
        assert!(err.contains("gui/501"), "{err}");
    }

    #[test]
    fn without_start_launchd_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let plan = plan_in(dir.path(), false);
        let mut rec = Recorder::default();
        install(&plan, &mut rec, &mut Vec::new()).unwrap();
        assert!(rec.calls.is_empty());
        assert!(plan.layout.plist_path.is_file());
    }

    #[test]
    fn status_reads_the_job_state() {
        let text = "gui/501/sk.rlt.fleet-agent = {\n\tactive count = 1\n\tpath = /x.plist\n\tstate = running\n\n\tprogram = /x\n";
        assert_eq!(parse_print(text).as_deref(), Some("running"));
        assert_eq!(parse_print("Could not find service"), None);
        assert_eq!(print_args(501), vec!["print", "gui/501/sk.rlt.fleet-agent"]);
    }
}
