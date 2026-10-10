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
        Command::Update(args) => match update_cmd(args) {
            Ok(true) => ExitCode::SUCCESS,
            Ok(false) => ExitCode::FAILURE,
            Err(why) => fail(&why),
        },
    }
}

/// `$HOME`, for the user scope's paths.
fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

fn update_cmd(args: cli::UpdateArgs) -> Result<bool, String> {
    let (layout, paths) = if args.user {
        let home = home().ok_or("--user needs $HOME")?;
        let config_home = config_home().ok_or("--user needs $XDG_CONFIG_HOME or $HOME")?;
        (
            Layout::user(&config_home),
            fleet_agent::update::UpdatePaths::user(&home),
        )
    } else {
        (Layout::system(), fleet_agent::update::UpdatePaths::system())
    };
    let config_path = args.config.clone().unwrap_or(layout.config_path.clone());
    let config = fleet_agent::config::load(&config_path)
        .map_err(|e| format!("{}: {e}", config_path.display()))?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("could not start the runtime: {e}"))?;
    runtime.block_on(fleet_agent::update::run(
        &config,
        &paths,
        &layout.unit_path,
        args.user,
        args.status,
        args.clear,
    ))
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
    use tracing_subscriber::layer::SubscriberExt as _;
    use tracing_subscriber::util::SubscriberInitExt as _;
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let fmt = tracing_subscriber::fmt::layer().with_writer(std::io::stderr);
    // The config is read after the subscriber is up (it logs its own
    // refusals), so the layer is installed unconditionally and
    // `report_errors` gates the FLUSH instead (`conn::serve`); a disabled
    // agent's ring wraps at RING_CAP and costs nothing.
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt)
        .with(fleet_agent::report::ReportLayer)
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
    // Settled before the runtime's threads exist: it may set
    // `RUNTIME_DIRECTORY`, which the hub's `update_now` line reads to know
    // where to drop `update-now`.
    let self_update = self_update_plan();
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => return fail(&format!("could not start the runtime: {e}")),
    };
    match runtime.block_on(fleet_agent::conn::run(config, self_update)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => fail(&why),
    }
}

/// The in-process updater, where no `fleet-agent-update.timer` runs one.
fn self_update_plan() -> Option<fleet_agent::self_update::SelfUpdate> {
    let argv: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let plan = fleet_agent::self_update::plan(
        &argv,
        home().as_deref(),
        config_home().as_deref(),
        is_root(),
        std::env::var(fleet_agent::self_update::ENV_SWITCH)
            .ok()
            .as_deref(),
        |p| p.exists(),
        std::env::var_os("PATH"),
        std::env::current_dir().ok(),
    )?;
    // Under systemd `RuntimeDirectory=` names where the poke lands already;
    // elsewhere it lands in the updater's own directory. `run` watches
    // whichever it is.
    let mut plan = plan;
    match std::env::var_os("RUNTIME_DIRECTORY").filter(|d| !d.is_empty()) {
        Some(d) => plan.poke_dir = PathBuf::from(d),
        None => {
            if std::fs::create_dir_all(&plan.poke_dir).is_ok() {
                std::env::set_var("RUNTIME_DIRECTORY", &plan.poke_dir);
            }
        }
    }
    Some(plan)
}

fn install_cmd(args: cli::InstallArgs) -> Result<(), String> {
    let binary = std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .map_err(|e| format!("where is this binary? {e}"))?;
    // Started through the update layout's symlink, `current_exe` names one
    // release's directory; the unit must keep running the symlink.
    let (layout, paths) = match (args.user, home(), config_home()) {
        (true, Some(h), Some(c)) => (Layout::user(&c), fleet_agent::update::UpdatePaths::user(&h)),
        _ => (Layout::system(), fleet_agent::update::UpdatePaths::system()),
    };
    let existing = std::fs::read_to_string(&layout.unit_path)
        .ok()
        .and_then(|u| fleet_agent::update::unit_binary(&u));
    let binary =
        fleet_agent::update::stable_binary(&binary, &paths.root, existing, &paths.default_link);
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
