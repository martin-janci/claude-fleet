//! `fleet-agent` — see the library (`src/lib.rs`) for what it is.
//!
//! This file only supplies the environment (who is running, where the binary
//! is, which config exists) and does the I/O; every decision is in the
//! library, where it is tested.

use clap::Parser;
use fleet_agent::cli::{self, Cli, Command, InstallEnv};
use fleet_agent::install::{self, Layout, RealSystemctl, SystemdStatus};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Run(args) => run(args),
        Command::Install(args) => match install_cmd(args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(why) => fail(&why),
        },
        Command::Status(args) => status(args.user),
    }
}

fn fail(why: &str) -> ExitCode {
    eprintln!("fleet-agent: {why}");
    ExitCode::FAILURE
}

/// `$XDG_CONFIG_HOME`, else `~/.config`.
fn config_home() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
}

fn run(args: cli::RunArgs) -> ExitCode {
    // The journal adds its own timestamps; tracing adds levels and targets.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();
    // With no flags: the user's own config if there is one, else the system's.
    let default_config = config_home()
        .map(|h| Layout::user(&h).config_path)
        .filter(|p| p.exists())
        .or_else(|| Some(Layout::system().config_path).filter(|p| p.exists()));
    let config = match cli::run_config(&args, default_config, &mut std::io::stdin()) {
        Ok(c) => c,
        Err(why) => return fail(&why),
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => return fail(&format!("could not start the runtime: {e}")),
    };
    match runtime.block_on(fleet_agent::conn::run(config)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => fail(&why),
    }
}

fn install_cmd(args: cli::InstallArgs) -> Result<(), String> {
    let binary = std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .map_err(|e| format!("where is this binary? {e}"))?;
    let env = InstallEnv {
        binary,
        root: is_root(),
        sudo_user: std::env::var("SUDO_USER").ok().filter(|u| !u.is_empty()),
        config_home: config_home(),
        systemd: systemd_status(),
    };
    let plan = cli::install_plan(&args, &env, install::lookup_user, &mut std::io::stdin())?;
    install::install(&plan, &mut RealSystemctl, &mut std::io::stdout())
}

/// Running as root.
#[cfg(unix)]
fn is_root() -> bool {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() == 0 }
}

/// `libc` is a unix-only dependency (see Cargo.toml): this crate is not
/// built for anything else yet, but an unconditional `libc::geteuid()` would
/// still fail to compile there, so the one call site is guarded.
#[cfg(not(unix))]
fn is_root() -> bool {
    false
}

/// The real probe behind [`install::SystemdStatus`]: is systemd this host's
/// running init, and does `systemctl` resolve on `$PATH`? Both are read
/// here, in `main`'s I/O, and handed to the library as data so
/// `cli::install_plan`'s refusal stays a pure function of its inputs.
fn systemd_status() -> SystemdStatus {
    SystemdStatus {
        init_running: Path::new("/run/systemd/system").exists(),
        systemctl_on_path: std::env::var_os("PATH")
            .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join("systemctl").is_file()))
            .unwrap_or(false),
    }
}

fn status(user: bool) -> ExitCode {
    let out = match std::process::Command::new("systemctl")
        .args(install::show_args(user))
        .output()
    {
        Ok(out) => out,
        Err(e) => return fail(&format!("could not run systemctl: {e}")),
    };
    let state = install::parse_show(&String::from_utf8_lossy(&out.stdout));
    let scope = if user { "user" } else { "system" };
    println!(
        "{} ({scope}): {} ({})",
        install::UNIT_NAME,
        if state.active.is_empty() {
            "unknown"
        } else {
            &state.active
        },
        state.sub
    );
    if !state.status_text.is_empty() {
        println!("agent: {}", state.status_text);
    }
    if state.connected() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(3)
    }
}
