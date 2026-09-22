//! The command line: `run`, `install`, `status`.
//!
//! Parsing and the decisions that follow from the flags are here and are pure;
//! `main.rs` supplies the environment (who is running, where the binary is)
//! and does the I/O.

use crate::config::{self, Config};
use crate::conn::Endpoint;
use crate::install::{Layout, Plan, Scope, SystemdStatus};
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "fleet-agent",
    version,
    about = "Dial a claude-fleet hub and run what it asks, for a host the hub cannot reach"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Dial the hub and serve it, reconnecting for as long as this runs.
    Run(RunArgs),
    /// Write a systemd unit that runs the agent, then enable and start it.
    /// Needs a Linux host with systemd already running; refuses cleanly,
    /// before writing anything, otherwise — see `run` for how to run the
    /// agent under your own supervisor instead.
    Install(InstallArgs),
    /// Is the service running, and is it connected to its hub?
    Status(StatusArgs),
}

#[derive(Debug, Args)]
pub struct RunArgs {
    /// The config `install` wrote. The way the service runs: the token stays
    /// out of the process list.
    #[arg(long, conflicts_with_all = ["hub", "token", "token_file"])]
    pub config: Option<PathBuf>,
    /// The hub's URL, e.g. https://hub.example.
    #[arg(long, requires = "token_source")]
    pub hub: Option<String>,
    /// This host's per-host bearer token. Visible in the process list and
    /// your shell history: prefer --token-file.
    #[arg(long, requires = "hub", group = "token_source")]
    pub token: Option<String>,
    /// A file holding the token, or `-` to read it from stdin.
    #[arg(long, requires = "hub", group = "token_source")]
    pub token_file: Option<PathBuf>,
    /// Allow a plain http:// or ws:// hub. For a loopback test only: the
    /// token crosses the network in clear.
    #[arg(long)]
    pub insecure: bool,
    /// A PEM bundle to trust instead of the system roots.
    #[arg(long)]
    pub ca_file: Option<PathBuf>,
}

#[derive(Debug, Args)]
#[command(group = clap::ArgGroup::new("token_source").required(true))]
pub struct InstallArgs {
    /// The hub's URL, e.g. https://hub.example.
    #[arg(long)]
    pub hub: String,
    /// This host's per-host bearer token (written to the 0600 config, never
    /// to the unit). Visible in the process list and your shell history:
    /// prefer --token-file.
    #[arg(long, group = "token_source")]
    pub token: Option<String>,
    /// A file holding the token, or `-` to read it from stdin — e.g.
    /// `fleet-hub agent-token <host>` on the hub, pasted here.
    #[arg(long, group = "token_source")]
    pub token_file: Option<PathBuf>,
    /// A user unit instead of a system one: no root needed, but it stops at
    /// logout unless lingering is enabled (`loginctl enable-linger`).
    #[arg(long)]
    pub user: bool,
    /// Who the system unit runs as. Defaults to the user who ran sudo.
    #[arg(long, conflicts_with = "user")]
    pub run_as: Option<String>,
    /// Allow a plain http:// or ws:// hub (loopback tests only).
    #[arg(long)]
    pub insecure: bool,
    /// A PEM bundle to trust instead of the system roots.
    #[arg(long)]
    pub ca_file: Option<PathBuf>,
    /// Where to write the config, instead of the scope's standard place.
    #[arg(long)]
    pub config: Option<PathBuf>,
    /// Write the files but leave systemd alone.
    #[arg(long)]
    pub no_start: bool,
}

#[derive(Debug, Args)]
pub struct StatusArgs {
    /// Ask about the user unit instead of the system one.
    #[arg(long)]
    pub user: bool,
}

/// What `install` needs from the environment it runs in.
#[derive(Debug, Clone)]
pub struct InstallEnv {
    /// This binary, absolute.
    pub binary: PathBuf,
    /// Running as root.
    pub root: bool,
    /// `$SUDO_USER`, the person behind `sudo`.
    pub sudo_user: Option<String>,
    /// `$XDG_CONFIG_HOME`, else `~/.config`.
    pub config_home: Option<PathBuf>,
    /// Whether this host can run a systemd unit — checked first, before any
    /// other input, so a host with no systemd is refused before anything
    /// else about the plan is even considered.
    pub systemd: SystemdStatus,
}

/// The token from `--token`, or read from `--token-file` (`-` for `stdin`),
/// with the line ending a file or a paste leaves on it removed.
pub fn read_token(
    token: Option<&str>,
    file: Option<&std::path::Path>,
    stdin: &mut dyn std::io::Read,
) -> Result<String, String> {
    let raw = match (token, file) {
        (Some(t), _) => t.to_string(),
        (None, Some(path)) if path.as_os_str() == "-" => {
            let mut text = String::new();
            stdin
                .read_to_string(&mut text)
                .map_err(|e| format!("reading the token from stdin: {e}"))?;
            text
        }
        (None, Some(path)) => std::fs::read_to_string(path)
            .map_err(|e| format!("reading the token from {}: {e}", path.display()))?,
        (None, None) => return Err("no token: pass --token-file (or --token)".into()),
    };
    let token = raw.trim_end_matches(['\n', '\r']).to_string();
    if token.is_empty() {
        return Err("the token is empty".into());
    }
    Ok(token)
}

/// The config `run` will use: from `--config`, from `--hub`/`--token`, or
/// from `default_config` when neither is given.
pub fn run_config(
    args: &RunArgs,
    default_config: Option<PathBuf>,
    stdin: &mut dyn std::io::Read,
) -> Result<Config, String> {
    let config = match (&args.hub, &args.config) {
        (Some(hub), _) => {
            let config = Config {
                hub: hub.clone(),
                token: read_token(args.token.as_deref(), args.token_file.as_deref(), stdin)?,
                insecure: args.insecure,
                ca_file: args.ca_file.clone(),
                report_errors: config::default_report_errors(),
            };
            config::check_token(&config.token).map_err(|e| e.to_string())?;
            config
        }
        (_, Some(path)) => config::load(path).map_err(|e| e.to_string())?,
        _ => {
            let path = default_config.ok_or(
                "no config: pass --config, or --hub and --token, or run `fleet-agent install`",
            )?;
            config::load(&path).map_err(|e| e.to_string())?
        }
    };
    // Checked here as well as at dial time, so a plain hub is refused at
    // startup rather than as a reconnect loop.
    Endpoint::parse(&config.hub, config.insecure)?;
    Ok(config)
}

/// Turn `install`'s flags into a [`Plan`], refusing anything wrong BEFORE a
/// file is written. `owner_of` resolves the system scope's user to a uid/gid.
///
/// The systemd check comes first, ahead of every other check: a host with no
/// systemd to install a unit on is refused before a bad hub URL or an empty
/// token even gets a chance to complain about itself, and — the point that
/// matters — before anything is written.
pub fn install_plan(
    args: &InstallArgs,
    env: &InstallEnv,
    owner_of: impl Fn(&str) -> Result<(u32, u32), String>,
    stdin: &mut dyn std::io::Read,
) -> Result<Plan, String> {
    if !env.systemd.available() {
        return Err(env.systemd.refusal());
    }
    Endpoint::parse(&args.hub, args.insecure)?;
    let token = read_token(args.token.as_deref(), args.token_file.as_deref(), stdin)?;
    config::check_token(&token).map_err(|e| e.to_string())?;
    let config = Config {
        hub: args.hub.clone(),
        token,
        insecure: args.insecure,
        ca_file: args.ca_file.clone(),
        report_errors: config::default_report_errors(),
    };
    let (scope, mut layout, owner) = if args.user {
        let home = env
            .config_home
            .clone()
            .ok_or("--user needs $XDG_CONFIG_HOME or $HOME to find ~/.config")?;
        (Scope::User, Layout::user(&home), None)
    } else {
        if !env.root {
            return Err(
                "a system unit needs root: run this with sudo, or pass --user \
                        for a unit of your own"
                    .into(),
            );
        }
        // The agent drives ONE user's tmux and Claude. Root's are never the
        // sessions the operator meant, so root is not guessed.
        let run_as = args
            .run_as
            .clone()
            .or_else(|| env.sudo_user.clone())
            .filter(|u| u != "root")
            .ok_or(
                "whose sessions should the agent run? pass --run-as <user> \
                 (it defaults to the user who ran sudo, and never to root)",
            )?;
        let owner = owner_of(&run_as)?;
        (Scope::System { run_as }, Layout::system(), Some(owner))
    };
    if let Some(path) = &args.config {
        layout.config_path = path.clone();
    }
    Ok(Plan {
        scope,
        binary: env.binary.clone(),
        layout,
        config,
        owner,
        start: !args.no_start,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("fleet-agent").chain(args.iter().copied()))
    }

    fn install_args(args: &[&str]) -> InstallArgs {
        match parse(args).unwrap().command {
            Command::Install(a) => a,
            other => panic!("{other:?}"),
        }
    }

    fn env(root: bool, sudo_user: Option<&str>) -> InstallEnv {
        InstallEnv {
            binary: PathBuf::from("/usr/local/bin/fleet-agent"),
            root,
            sudo_user: sudo_user.map(str::to_string),
            config_home: Some(PathBuf::from("/home/alice/.config")),
            systemd: SystemdStatus {
                init_running: true,
                systemctl_on_path: true,
            },
        }
    }

    fn uid_1000(_: &str) -> Result<(u32, u32), String> {
        Ok((1000, 1000))
    }

    #[test]
    fn the_command_line_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn install_refuses_a_plain_hub_without_insecure() {
        let a = install_args(&["install", "--hub", "ws://hub.example", "--token", "t"]);
        let err = install_plan(
            &a,
            &env(true, Some("alice")),
            uid_1000,
            &mut std::io::empty(),
        )
        .unwrap_err();
        assert!(err.contains("--insecure"), "{err}");

        let a = install_args(&[
            "install",
            "--hub",
            "http://127.0.0.1:1",
            "--token",
            "t",
            "--insecure",
        ]);
        let plan = install_plan(
            &a,
            &env(true, Some("alice")),
            uid_1000,
            &mut std::io::empty(),
        )
        .unwrap();
        assert!(
            plan.config.insecure,
            "the flag reaches the config `run` reads"
        );
    }

    #[test]
    fn a_system_install_runs_as_the_sudo_user_and_gives_them_the_config() {
        let a = install_args(&["install", "--hub", "https://hub.example", "--token", "t"]);
        let plan = install_plan(
            &a,
            &env(true, Some("alice")),
            uid_1000,
            &mut std::io::empty(),
        )
        .unwrap();
        assert_eq!(
            plan.scope,
            Scope::System {
                run_as: "alice".into()
            }
        );
        assert_eq!(plan.layout, Layout::system());
        assert_eq!(plan.owner, Some((1000, 1000)));
        assert_eq!(plan.binary, PathBuf::from("/usr/local/bin/fleet-agent"));
        assert_eq!(plan.config.token, "t");
        assert!(plan.start);
    }

    /// The guard fires before anything else: no Plan is built, so nothing
    /// `install` could write ever gets a chance to.
    #[test]
    fn install_refuses_before_anything_else_when_there_is_no_systemd() {
        let a = install_args(&["install", "--hub", "https://hub.example", "--token", "t"]);
        let mut e = env(true, Some("alice"));
        e.systemd = SystemdStatus {
            init_running: false,
            systemctl_on_path: true,
        };
        let err = install_plan(&a, &e, uid_1000, &mut std::io::empty()).unwrap_err();
        assert!(err.contains("/run/systemd/system does not exist"), "{err}");
        assert!(err.contains("nothing was written"), "{err}");
        assert!(err.contains("fleet-agent run"), "{err}");

        let mut e = env(true, Some("alice"));
        e.systemd = SystemdStatus {
            init_running: true,
            systemctl_on_path: false,
        };
        let err = install_plan(&a, &e, uid_1000, &mut std::io::empty()).unwrap_err();
        assert!(err.contains("systemctl is not"), "{err}");
    }

    /// The literal requirement: after a no-systemd refusal, the path
    /// `install` would have written the config to does not exist.
    #[test]
    fn no_systemd_means_the_config_path_never_exists() {
        let dir = tempfile::tempdir().unwrap();
        let a = install_args(&[
            "install",
            "--hub",
            "https://hub.example",
            "--token",
            "t",
            "--user",
        ]);
        let mut e = env(false, None);
        e.config_home = Some(dir.path().to_path_buf());
        e.systemd = SystemdStatus {
            init_running: false,
            systemctl_on_path: false,
        };
        let expected_config = Layout::user(dir.path()).config_path;
        assert!(install_plan(&a, &e, uid_1000, &mut std::io::empty()).is_err());
        assert!(!expected_config.exists());
    }

    #[test]
    fn a_system_install_needs_root() {
        let a = install_args(&["install", "--hub", "https://hub.example", "--token", "t"]);
        let err = install_plan(&a, &env(false, None), uid_1000, &mut std::io::empty()).unwrap_err();
        assert!(err.contains("--user"), "it names the alternative: {err}");
    }

    /// Root with nobody behind sudo: the agent would drive root's tmux, which
    /// is never the sessions the operator meant.
    #[test]
    fn a_system_install_will_not_guess_root() {
        let a = install_args(&["install", "--hub", "https://hub.example", "--token", "t"]);
        let err = install_plan(&a, &env(true, None), uid_1000, &mut std::io::empty()).unwrap_err();
        assert!(err.contains("--run-as"), "{err}");
        let err = install_plan(
            &a,
            &env(true, Some("root")),
            uid_1000,
            &mut std::io::empty(),
        )
        .unwrap_err();
        assert!(err.contains("--run-as"), "{err}");

        let a = install_args(&[
            "install",
            "--hub",
            "https://hub.example",
            "--token",
            "t",
            "--run-as",
            "bob",
        ]);
        let plan = install_plan(&a, &env(true, None), uid_1000, &mut std::io::empty()).unwrap();
        assert_eq!(
            plan.scope,
            Scope::System {
                run_as: "bob".into()
            }
        );
    }

    #[test]
    fn a_user_install_writes_under_the_config_home_and_keeps_its_own_owner() {
        let a = install_args(&[
            "install",
            "--hub",
            "https://hub.example",
            "--token",
            "t",
            "--user",
        ]);
        let plan = install_plan(&a, &env(false, None), uid_1000, &mut std::io::empty()).unwrap();
        assert_eq!(plan.scope, Scope::User);
        assert_eq!(
            plan.layout,
            Layout::user(std::path::Path::new("/home/alice/.config"))
        );
        assert_eq!(plan.owner, None);
    }

    #[test]
    fn install_flags_override_the_config_path_and_starting() {
        let a = install_args(&[
            "install",
            "--hub",
            "https://hub.example",
            "--token",
            "t",
            "--user",
            "--config",
            "/srv/agent.json",
            "--no-start",
        ]);
        let plan = install_plan(&a, &env(false, None), uid_1000, &mut std::io::empty()).unwrap();
        assert_eq!(plan.layout.config_path, PathBuf::from("/srv/agent.json"));
        assert!(!plan.start);
    }

    #[test]
    fn install_refuses_a_token_that_cannot_be_a_header() {
        let a = install_args(&[
            "install",
            "--hub",
            "https://hub.example",
            "--token",
            "a b",
            "--user",
        ]);
        assert!(install_plan(&a, &env(false, None), uid_1000, &mut std::io::empty()).is_err());
    }

    #[test]
    fn run_takes_its_hub_from_the_flags_or_the_config() {
        let run = |args: &[&str]| match parse(args).unwrap().command {
            Command::Run(a) => a,
            other => panic!("{other:?}"),
        };
        let c = run_config(
            &run(&["run", "--hub", "https://h.example", "--token", "t"]),
            None,
            &mut std::io::empty(),
        )
        .unwrap();
        assert_eq!(
            (c.hub.as_str(), c.token.as_str(), c.insecure),
            ("https://h.example", "t", false)
        );

        let err = run_config(
            &run(&["run", "--hub", "ws://h.example", "--token", "t"]),
            None,
            &mut std::io::empty(),
        )
        .unwrap_err();
        assert!(err.contains("--insecure"), "{err}");

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.json");
        config::write(&path, &c, None).unwrap();
        let from_file = run_config(
            &run(&["run", "--config", path.to_str().unwrap()]),
            None,
            &mut std::io::empty(),
        )
        .unwrap();
        assert_eq!(from_file, c);
        // No flags at all: the default config.
        let from_default = run_config(&run(&["run"]), Some(path), &mut std::io::empty()).unwrap();
        assert_eq!(from_default, c);
        assert!(run_config(&run(&["run"]), None, &mut std::io::empty()).is_err());
    }

    /// The token need not be an argument, where it would sit in the process
    /// list and the shell history: a file, or stdin.
    #[test]
    fn the_token_can_come_from_a_file_or_stdin_instead_of_the_command_line() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("token");
        std::fs::write(&file, "from-a-file\n").unwrap();
        let path = file.to_str().unwrap();

        let a = install_args(&[
            "install",
            "--hub",
            "https://hub.example",
            "--token-file",
            path,
            "--user",
        ]);
        let plan = install_plan(&a, &env(false, None), uid_1000, &mut std::io::empty()).unwrap();
        assert_eq!(
            plan.config.token, "from-a-file",
            "the line ending is not the token"
        );

        let a = install_args(&[
            "install",
            "--hub",
            "https://hub.example",
            "--token-file",
            "-",
            "--user",
        ]);
        let plan =
            install_plan(&a, &env(false, None), uid_1000, &mut &b"from-stdin\r\n"[..]).unwrap();
        assert_eq!(plan.config.token, "from-stdin");

        let run = match parse(&["run", "--hub", "https://h.example", "--token-file", "-"])
            .unwrap()
            .command
        {
            Command::Run(a) => a,
            other => panic!("{other:?}"),
        };
        let c = run_config(&run, None, &mut &b"piped\n"[..]).unwrap();
        assert_eq!(c.token, "piped");

        let a = install_args(&[
            "install",
            "--hub",
            "https://hub.example",
            "--token-file",
            "-",
            "--user",
        ]);
        assert!(
            install_plan(&a, &env(false, None), uid_1000, &mut std::io::empty()).is_err(),
            "an empty token is refused"
        );
        assert!(read_token(
            None,
            Some(&dir.path().join("missing")),
            &mut std::io::empty()
        )
        .is_err());
    }

    #[test]
    fn install_needs_exactly_one_token_source() {
        assert!(parse(&["install", "--hub", "https://h"]).is_err());
        assert!(parse(&[
            "install",
            "--hub",
            "https://h",
            "--token",
            "t",
            "--token-file",
            "-"
        ])
        .is_err());
        assert!(parse(&[
            "run",
            "--hub",
            "https://h",
            "--token",
            "t",
            "--token-file",
            "-"
        ])
        .is_err());
    }

    #[test]
    fn run_rejects_half_a_hub() {
        assert!(parse(&["run", "--hub", "https://h.example"]).is_err());
        assert!(parse(&["run", "--token", "t"]).is_err());
        assert!(parse(&[
            "run",
            "--config",
            "/c",
            "--hub",
            "https://h",
            "--token",
            "t"
        ])
        .is_err());
    }
}
