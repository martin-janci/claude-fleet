//! The command line: `run`, `install`, `status`.
//!
//! Parsing and the decisions that follow from the flags are here and are pure;
//! `main.rs` supplies the environment (who is running, where the binary is)
//! and does the I/O.

use crate::config::{self, Config};
use crate::conn::Endpoint;
use crate::install::{Layout, Plan, Scope};
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
    Install(InstallArgs),
    /// Is the service running, and is it connected to its hub?
    Status(StatusArgs),
}

#[derive(Debug, Args)]
pub struct RunArgs {
    /// The config `install` wrote. The way the service runs: the token stays
    /// out of the process list.
    #[arg(long, conflicts_with_all = ["hub", "token"])]
    pub config: Option<PathBuf>,
    /// The hub's URL, e.g. https://hub.example.
    #[arg(long, requires = "token")]
    pub hub: Option<String>,
    /// This host's per-host bearer token.
    #[arg(long, requires = "hub")]
    pub token: Option<String>,
    /// Allow a plain http:// or ws:// hub. For a loopback test only: the
    /// token crosses the network in clear.
    #[arg(long)]
    pub insecure: bool,
    /// A PEM bundle to trust instead of the system roots.
    #[arg(long)]
    pub ca_file: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct InstallArgs {
    /// The hub's URL, e.g. https://hub.example.
    #[arg(long)]
    pub hub: String,
    /// This host's per-host bearer token (written to the 0600 config, never
    /// to the unit).
    #[arg(long)]
    pub token: String,
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
}

/// The config `run` will use: from `--config`, from `--hub`/`--token`, or
/// from `default_config` when neither is given.
pub fn run_config(args: &RunArgs, default_config: Option<PathBuf>) -> Result<Config, String> {
    let config = match (&args.hub, &args.token, &args.config) {
        (Some(hub), Some(token), _) => {
            let config = Config {
                hub: hub.clone(),
                token: token.clone(),
                insecure: args.insecure,
                ca_file: args.ca_file.clone(),
            };
            config::check_token(&config.token).map_err(|e| e.to_string())?;
            config
        }
        (_, _, Some(path)) => config::load(path).map_err(|e| e.to_string())?,
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
pub fn install_plan(
    args: &InstallArgs,
    env: &InstallEnv,
    owner_of: impl Fn(&str) -> Result<(u32, u32), String>,
) -> Result<Plan, String> {
    Endpoint::parse(&args.hub, args.insecure)?;
    config::check_token(&args.token).map_err(|e| e.to_string())?;
    let config = Config {
        hub: args.hub.clone(),
        token: args.token.clone(),
        insecure: args.insecure,
        ca_file: args.ca_file.clone(),
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
        let err = install_plan(&a, &env(true, Some("alice")), uid_1000).unwrap_err();
        assert!(err.contains("--insecure"), "{err}");

        let a = install_args(&[
            "install",
            "--hub",
            "http://127.0.0.1:1",
            "--token",
            "t",
            "--insecure",
        ]);
        let plan = install_plan(&a, &env(true, Some("alice")), uid_1000).unwrap();
        assert!(
            plan.config.insecure,
            "the flag reaches the config `run` reads"
        );
    }

    #[test]
    fn a_system_install_runs_as_the_sudo_user_and_gives_them_the_config() {
        let a = install_args(&["install", "--hub", "https://hub.example", "--token", "t"]);
        let plan = install_plan(&a, &env(true, Some("alice")), uid_1000).unwrap();
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

    #[test]
    fn a_system_install_needs_root() {
        let a = install_args(&["install", "--hub", "https://hub.example", "--token", "t"]);
        let err = install_plan(&a, &env(false, None), uid_1000).unwrap_err();
        assert!(err.contains("--user"), "it names the alternative: {err}");
    }

    /// Root with nobody behind sudo: the agent would drive root's tmux, which
    /// is never the sessions the operator meant.
    #[test]
    fn a_system_install_will_not_guess_root() {
        let a = install_args(&["install", "--hub", "https://hub.example", "--token", "t"]);
        let err = install_plan(&a, &env(true, None), uid_1000).unwrap_err();
        assert!(err.contains("--run-as"), "{err}");
        let err = install_plan(&a, &env(true, Some("root")), uid_1000).unwrap_err();
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
        let plan = install_plan(&a, &env(true, None), uid_1000).unwrap();
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
        let plan = install_plan(&a, &env(false, None), uid_1000).unwrap();
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
        let plan = install_plan(&a, &env(false, None), uid_1000).unwrap();
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
        assert!(install_plan(&a, &env(false, None), uid_1000).is_err());
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
        )
        .unwrap();
        assert_eq!(
            (c.hub.as_str(), c.token.as_str(), c.insecure),
            ("https://h.example", "t", false)
        );

        let err = run_config(
            &run(&["run", "--hub", "ws://h.example", "--token", "t"]),
            None,
        )
        .unwrap_err();
        assert!(err.contains("--insecure"), "{err}");

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.json");
        config::write(&path, &c, None).unwrap();
        let from_file =
            run_config(&run(&["run", "--config", path.to_str().unwrap()]), None).unwrap();
        assert_eq!(from_file, c);
        // No flags at all: the default config.
        let from_default = run_config(&run(&["run"]), Some(path)).unwrap();
        assert_eq!(from_default, c);
        assert!(run_config(&run(&["run"]), None).is_err());
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
