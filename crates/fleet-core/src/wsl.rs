//! WSL distributions as fleet hosts, on a Windows desktop.
//!
//! A Windows machine has no tmux of its own, but a WSL distribution does: a
//! user with tmux and Claude Code installed in Ubuntu-on-WSL can run fleet
//! sessions on their own machine. Each distribution appears in host discovery
//! as `wsl-<name>` (lower-case, `[a-z0-9._-]`), and a command for that host
//! runs through `wsl.exe --distribution <name> --exec sh -c <command>` instead
//! of `ssh -- <host> <command>`. `ssh` hands the remote login shell the
//! command words joined by spaces; `sh -c` of the same joined string is the
//! same contract, so every script fleet sends works unchanged. No sshd, keys
//! or `~/.ssh/config` entry is needed.
//!
//! What does not carry over: the reverse tunnel (`ssh -R`) that brings a
//! host's Claude Code hooks back to this machine. WSL1 and WSL2 in
//! `networkingMode=mirrored` share `127.0.0.1` with Windows, so the hooks
//! reach the Control API directly; under WSL2's default NAT networking they
//! do not, and sessions run without them (see docs/windows.md).
//!
//! An alias that `~/.ssh/config` also defines stays an SSH host: discovery
//! never shadows a host the user configured.
//!
//! Everywhere but Windows this module finds no distributions and every lookup
//! answers `None`, so nothing else changes.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::RwLock;
use std::time::{Duration, Instant};

/// Host aliases this module owns start with this.
pub const ALIAS_PREFIX: &str = "wsl-";

/// The distributions found by the last [`refresh`]: `(alias, distribution)`.
static DISTROS: RwLock<Vec<(String, String)>> = RwLock::new(Vec::new());

/// A detection started by [`refresh_in_background`] has not finished yet.
/// Until it does, a `wsl-` alias cannot be told apart from an SSH host, so
/// the callers that route by alias wait for it ([`settled_for`]).
static PENDING: AtomicBool = AtomicBool::new(false);

/// How long a command for a `wsl-` host waits on a detection still running.
/// Past it the command goes ahead with whatever the table holds.
pub const SETTLE_WAIT: Duration = Duration::from_secs(20);

/// Detection has been started on this machine at least once (the Windows
/// desktop's startup does it). Until then a missing `wsl-` alias is never a
/// reason to run `wsl.exe`: on every other platform nothing is detected.
static ENABLED: AtomicBool = AtomicBool::new(false);

/// The `~/.ssh/config` aliases the last detection left out: a miss on one of
/// them is an SSH host, not a distribution to look for again.
static TAKEN: RwLock<Vec<String>> = RwLock::new(Vec::new());

/// When the last detection ended, and whether `wsl.exe` answered.
static LAST_DETECTION: std::sync::Mutex<Option<(Instant, bool)>> = std::sync::Mutex::new(None);

/// After a detection that answered, a `wsl-` alias missing from the table
/// runs another one at most this often: a distribution installed since, or
/// one renamed, is found without a restart, and a host that is simply gone
/// does not spawn `wsl.exe` on every tick.
pub const REDETECT_EVERY: Duration = Duration::from_secs(60);

/// After a detection that did not answer (a WSL service still starting when
/// the app did), the next miss may try again after this long. Without it a
/// slow first `wsl.exe` would send every `wsl-` alias to `ssh` for the rest
/// of the session.
pub const REDETECT_AFTER_FAILURE: Duration = Duration::from_secs(10);

/// The deadline of a re-detection (the startup one is the caller's).
const REDETECT_TIMEOUT: Duration = Duration::from_secs(15);

/// The fleet alias for a distribution name: `wsl-` plus the name
/// lower-cased, every character outside `[a-z0-9._-]` turned into `-`.
/// `None` when nothing usable is left.
pub fn alias_for(distro: &str) -> Option<String> {
    let slug: String = distro
        .trim()
        .chars()
        .map(|c| {
            let c = c.to_ascii_lowercase();
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        return None;
    }
    let alias = format!("{ALIAS_PREFIX}{slug}");
    crate::validate::host_alias_syntax(&alias).ok()?;
    Some(alias)
}

/// Parse `wsl.exe --list --quiet`: one distribution per line. wsl.exe writes
/// UTF-16LE (with or without a BOM) unless `WSL_UTF8=1`, so both are read.
/// Docker Desktop's internal distributions are not hosts anyone runs
/// sessions on and are left out.
pub fn parse_list(raw: &[u8]) -> Vec<String> {
    // UTF-8 text never holds a NUL; UTF-16LE of anything but CJK does, in
    // every other byte. A non-ASCII name breaks the strict every-odd-byte
    // test, so any NUL at all decides it.
    let looks_utf16 = raw.starts_with(&[0xFF, 0xFE]) || raw.contains(&0);
    let text = if looks_utf16 {
        let units: Vec<u16> = (0..raw.len() / 2)
            .map(|i| u16::from_le_bytes([raw[2 * i], raw[2 * i + 1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(raw).into_owned()
    };
    text.trim_start_matches('\u{feff}')
        .lines()
        .map(|l| l.trim().trim_matches('\0').trim())
        .filter(|l| !l.is_empty())
        .filter(|l| !l.to_ascii_lowercase().starts_with("docker-desktop"))
        .map(str::to_string)
        .collect()
}

/// Pair each distribution with its alias, leaving out any alias `taken` (an
/// `~/.ssh/config` host) and any name that yields no alias or a duplicate.
pub fn assign_aliases(distros: &[String], taken: &[String]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for d in distros {
        let Some(alias) = alias_for(d) else { continue };
        if taken.contains(&alias) || out.iter().any(|(a, _)| *a == alias) {
            continue;
        }
        out.push((alias, d.clone()));
    }
    out
}

/// The distribution behind `alias`, when it is one [`refresh`] found.
pub fn distro_for(alias: &str) -> Option<String> {
    if !alias.starts_with(ALIAS_PREFIX) {
        return None;
    }
    DISTROS
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .find(|(a, _)| a == alias)
        .map(|(_, d)| d.clone())
}

/// Whether `alias` is a WSL host (see [`distro_for`]).
pub fn is_wsl_host(alias: &str) -> bool {
    distro_for(alias).is_some()
}

/// The WSL hosts [`refresh`] last found, as `(alias, distribution)`.
pub fn hosts() -> Vec<(String, String)> {
    DISTROS
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// Re-detect the distributions (blocking, bounded by `timeout`) and replace
/// the table. Aliases in `taken` stay SSH hosts. A failed or timed-out
/// `wsl.exe` keeps the previous table rather than dropping hosts in use.
pub fn refresh(taken: &[String], timeout: Duration) {
    *TAKEN
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = taken.to_vec();
    let found = detect(timeout);
    *LAST_DETECTION
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) =
        Some((Instant::now(), found.is_some()));
    let Some(found) = found else {
        tracing::warn!(
            timeout_ms = timeout.as_millis() as u64,
            "[wsl] wsl.exe --list did not answer; keeping the previous host table"
        );
        return;
    };
    let table = assign_aliases(&found, taken);
    tracing::info!(hosts = ?table, "[wsl] distributions found");
    *DISTROS
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = table;
}

/// [`refresh`] on a thread of its own, so a WSL service that takes seconds
/// to start (the first `wsl.exe` after a boot does) never holds up the app's
/// startup. Until it finishes, [`settled_for`] holds back commands for
/// `wsl-` aliases, which could otherwise go to `ssh` as an unknown host.
///
/// `taken` is called on that thread too: reading `~/.ssh/config` (and what it
/// `Include`s) can itself stall on a redirected profile that is offline.
pub fn refresh_in_background(
    taken: impl FnOnce() -> Vec<String> + Send + 'static,
    timeout: Duration,
) {
    ENABLED.store(true, Ordering::Release);
    PENDING.store(true, Ordering::Release);
    std::thread::spawn(move || {
        refresh(&taken(), timeout);
        PENDING.store(false, Ordering::Release);
    });
}

/// Whether a background detection is still running.
pub fn pending() -> bool {
    PENDING.load(Ordering::Acquire)
}

/// Whether a `wsl-` alias the table does not hold should start another
/// detection: not while one is running, not for an `~/.ssh/config` alias,
/// and not sooner than [`REDETECT_EVERY`] after a detection that answered
/// ([`REDETECT_AFTER_FAILURE`] after one that did not). `last` is when the
/// previous detection ended and whether it answered; `None` is never.
pub fn redetect_due(
    alias: &str,
    known: bool,
    taken: &[String],
    pending: bool,
    last: Option<(Instant, bool)>,
    now: Instant,
) -> bool {
    if !alias.starts_with(ALIAS_PREFIX) || known || pending || taken.iter().any(|t| t == alias) {
        return false;
    }
    match last {
        None => true,
        Some((at, answered)) => {
            let every = if answered {
                REDETECT_EVERY
            } else {
                REDETECT_AFTER_FAILURE
            };
            now.saturating_duration_since(at) >= every
        }
    }
}

/// Start a background detection when `alias` is a `wsl-` alias the table
/// does not hold and [`redetect_due`] says one is due. Only once detection
/// is in use on this machine ([`refresh_in_background`] ran).
fn redetect_if_missing(alias: &str) {
    if !ENABLED.load(Ordering::Acquire) || !alias.starts_with(ALIAS_PREFIX) {
        return;
    }
    // Held across the decision and the start, so two callers missing at once
    // start one detection, not two.
    let last = LAST_DETECTION
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let due = redetect_due(
        alias,
        distro_for(alias).is_some(),
        &TAKEN
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        pending(),
        *last,
        Instant::now(),
    );
    if !due {
        return;
    }
    tracing::info!(alias, "[wsl] alias not in the host table; detecting again");
    // Marked pending before the lock is released: the next caller sees it.
    PENDING.store(true, Ordering::Release);
    drop(last);
    std::thread::spawn(|| {
        let taken: Vec<String> = crate::ssh_config::load_user_config()
            .into_iter()
            .map(|h| h.alias)
            .collect();
        refresh(&taken, REDETECT_TIMEOUT);
        PENDING.store(false, Ordering::Release);
    });
}

/// Wait, at most [`SETTLE_WAIT`], for a running detection when `alias` could
/// be one of its hosts; return at once for any other alias, or when nothing
/// is running. A `wsl-` alias the table does not hold first starts another
/// detection when one is due ([`redetect_due`]), and waits for that.
pub async fn settled_for(alias: &str) {
    if !alias.starts_with(ALIAS_PREFIX) {
        return;
    }
    redetect_if_missing(alias);
    let deadline = Instant::now() + SETTLE_WAIT;
    while pending() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// [`settled_for`] for a caller already off the async runtime (the PTY
/// attach runs on a blocking thread).
pub fn settled_for_blocking(alias: &str) {
    if !alias.starts_with(ALIAS_PREFIX) {
        return;
    }
    redetect_if_missing(alias);
    let deadline = Instant::now() + SETTLE_WAIT;
    while pending() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// `wsl.exe`: `%SystemRoot%\System32\wsl.exe` when it is there, else the
/// name for a PATH lookup.
pub fn wsl_binary() -> std::path::PathBuf {
    std::env::var_os("SystemRoot")
        .map(|r| std::path::PathBuf::from(r).join("System32").join("wsl.exe"))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| std::path::PathBuf::from("wsl.exe"))
}

/// Run `wsl.exe --list --quiet` with a deadline. `None` when it could not be
/// run or overran (a WSL service still starting can take seconds), and then
/// the process is killed rather than left behind; `Some(empty)` when WSL is
/// there with no distribution (that exits non-zero).
#[cfg(windows)]
fn detect(timeout: Duration) -> Option<Vec<String>> {
    let mut cmd = crate::proc::std_command(wsl_binary());
    cmd.args(["--list", "--quiet"]);
    list_with_deadline(cmd, timeout)
}

#[cfg(not(windows))]
fn detect(_timeout: Duration) -> Option<Vec<String>> {
    None
}

/// Run a `--list`-shaped command with a deadline and parse what it printed.
/// stdout is read on its own thread so a full pipe never stalls the child.
#[cfg_attr(not(windows), allow(dead_code))]
fn list_with_deadline(mut cmd: std::process::Command, timeout: Duration) -> Option<Vec<String>> {
    use std::io::Read;
    let mut child = cmd
        .env("WSL_UTF8", "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    let out = reader.join().unwrap_or_default();
    if !status.success() {
        return Some(Vec::new());
    }
    Some(parse_list(&out))
}

/// The `wsl.exe` arguments that run `args` in `distro` the way `ssh` would
/// run them on a host: the words joined by spaces, read by a shell.
///
/// `--cd ~` starts it in the Linux user's home, where `ssh` would: without
/// it wsl.exe keeps the Windows working directory (`/mnt/c/...`), and every
/// relative path fleet sends would resolve there.
pub fn exec_args(distro: &str, args: &[&str]) -> Vec<String> {
    vec![
        "--distribution".into(),
        distro.into(),
        "--cd".into(),
        "~".into(),
        "--exec".into(),
        "sh".into(),
        "-c".into(),
        args.join(" "),
    ]
}

/// A `tokio` command running `args` in `distro` (see [`exec_args`]).
/// `WSL_UTF8=1` makes wsl.exe's own messages (a distribution that no longer
/// exists, a WSL service that failed) UTF-8 instead of UTF-16, so they read
/// as text in an error rather than as NUL-riddled bytes.
pub fn command(distro: &str, args: &[&str]) -> tokio::process::Command {
    let mut cmd = crate::proc::command(wsl_binary());
    cmd.args(exec_args(distro, args));
    cmd.env("WSL_UTF8", "1");
    cmd.stdin(std::process::Stdio::null());
    cmd
}

/// The interactive attach for a WSL host: `wsl.exe` running `bash -lc` with
/// the attach script, the same script `ssh -tt` carries to a remote host.
pub fn attach_argv(distro: &str, script: &str) -> Vec<String> {
    vec![
        wsl_binary().to_string_lossy().into_owned(),
        "--distribution".into(),
        distro.into(),
        "--cd".into(),
        "~".into(),
        "--exec".into(),
        "bash".into(),
        "-lc".into(),
        script.into(),
    ]
}

/// Held by every test that swaps the process-wide table, so two of them
/// never see each other's hosts.
#[cfg(test)]
pub(crate) static TEST_TABLE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Replace the table directly, for tests elsewhere in the crate (hold
/// [`TEST_TABLE_LOCK`]).
#[cfg(test)]
pub(crate) fn set_for_tests(table: Vec<(String, String)>) {
    *DISTROS
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = table;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16le(s: &str, bom: bool) -> Vec<u8> {
        let mut out = if bom { vec![0xFF, 0xFE] } else { Vec::new() };
        for u in s.encode_utf16() {
            out.extend(u.to_le_bytes());
        }
        out
    }

    #[test]
    fn the_list_is_read_as_utf16_with_or_without_a_bom_and_as_utf8() {
        let listing = "Ubuntu-22.04\r\nDebian\r\ndocker-desktop\r\ndocker-desktop-data\r\n\r\n";
        let want = vec!["Ubuntu-22.04".to_string(), "Debian".to_string()];
        assert_eq!(parse_list(&utf16le(listing, true)), want);
        assert_eq!(parse_list(&utf16le(listing, false)), want);
        assert_eq!(parse_list(listing.as_bytes()), want);
        assert!(parse_list(b"").is_empty());
    }

    #[test]
    fn a_non_ascii_name_in_utf16_without_a_bom_is_still_utf16() {
        let listing = "Übuntu\r\nDebian\r\n";
        assert_eq!(
            parse_list(&utf16le(listing, false)),
            vec!["Übuntu".to_string(), "Debian".to_string()]
        );
    }

    /// The deadline kills an overrunning `wsl.exe` instead of leaving it
    /// behind; a non-zero exit is "no distributions", not a failure.
    #[cfg(unix)]
    #[test]
    fn listing_has_a_deadline_and_reads_a_failure_as_none_installed() {
        let run = |script: &str, ms: u64| {
            let mut cmd = std::process::Command::new("sh");
            cmd.args(["-c", script]);
            list_with_deadline(cmd, Duration::from_millis(ms))
        };
        assert_eq!(
            run("printf 'Ubuntu\\nDebian\\n'", 5_000),
            Some(vec!["Ubuntu".to_string(), "Debian".to_string()])
        );
        assert_eq!(run("echo 'no distributions'; exit 1", 5_000), Some(vec![]));
        let started = Instant::now();
        assert_eq!(run("sleep 30", 200), None);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "killed, not waited out"
        );
    }

    #[test]
    fn only_a_wsl_alias_waits_on_a_running_detection() {
        let _table = TEST_TABLE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        PENDING.store(true, Ordering::Release);
        let started = Instant::now();
        settled_for_blocking("mefistos");
        assert!(started.elapsed() < Duration::from_millis(100));
        let waiter = std::thread::spawn(|| {
            let started = Instant::now();
            settled_for_blocking("wsl-ubuntu");
            started.elapsed()
        });
        std::thread::sleep(Duration::from_millis(150));
        PENDING.store(false, Ordering::Release);
        let waited = waiter.join().unwrap();
        assert!(waited >= Duration::from_millis(100), "{waited:?}");
        assert!(waited < SETTLE_WAIT, "{waited:?}");
    }

    /// A slow first `wsl.exe` must not pin `wsl-` aliases to `ssh` for the
    /// session: a miss after a failed detection tries again soon, after one
    /// that answered only once a minute, and never for an SSH alias, another
    /// alias, a known host or while a detection runs.
    #[test]
    fn a_missing_wsl_alias_redetects_rate_limited() {
        let now = Instant::now() + Duration::from_secs(3600);
        let ago = |s: u64| now - Duration::from_secs(s);
        let due = |alias, known, pending, last| redetect_due(alias, known, &[], pending, last, now);
        assert!(due("wsl-ubuntu", false, false, None));
        assert!(due("wsl-ubuntu", false, false, Some((ago(11), false))));
        assert!(!due("wsl-ubuntu", false, false, Some((ago(5), false))));
        assert!(!due("wsl-ubuntu", false, false, Some((ago(30), true))));
        assert!(due("wsl-ubuntu", false, false, Some((ago(61), true))));
        assert!(!due("wsl-ubuntu", true, false, None), "known");
        assert!(!due("wsl-ubuntu", false, true, None), "already running");
        assert!(!due("mefistos", false, false, None), "not a wsl- alias");
        assert!(
            !redetect_due("wsl-box", false, &["wsl-box".into()], false, None, now),
            "an ~/.ssh/config alias stays an SSH host"
        );
    }

    #[test]
    fn aliases_are_lower_case_slugs_under_the_prefix() {
        assert_eq!(
            alias_for("Ubuntu-22.04").as_deref(),
            Some("wsl-ubuntu-22.04")
        );
        assert_eq!(
            alias_for("My Distro (dev)").as_deref(),
            Some("wsl-my-distro--dev")
        );
        assert_eq!(alias_for("  Arch  ").as_deref(), Some("wsl-arch"));
        assert_eq!(alias_for("***"), None);
        assert_eq!(alias_for(""), None);
        // Whatever comes out is a valid host alias.
        let a = alias_for("Ünïcode Dìstro").unwrap();
        assert!(crate::validate::host_alias_syntax(&a).is_ok(), "{a}");
    }

    #[test]
    fn an_ssh_config_alias_and_a_duplicate_slug_stay_out() {
        let distros = vec![
            "Ubuntu".to_string(),
            "ubuntu".to_string(),
            "Debian".to_string(),
        ];
        let table = assign_aliases(&distros, &["wsl-debian".to_string()]);
        assert_eq!(
            table,
            vec![("wsl-ubuntu".to_string(), "Ubuntu".to_string())]
        );
    }

    #[test]
    fn a_command_runs_the_joined_words_under_sh_like_ssh_would() {
        assert_eq!(
            exec_args("Ubuntu", &["bash", "-lc", "'tmux ls'"]),
            vec![
                "--distribution",
                "Ubuntu",
                "--cd",
                "~",
                "--exec",
                "sh",
                "-c",
                "bash -lc 'tmux ls'"
            ]
        );
        let argv = attach_argv("Ubuntu", "exec tmux attach");
        assert!(argv[0].ends_with("wsl.exe"), "{argv:?}");
        assert_eq!(
            argv[1..],
            [
                "--distribution",
                "Ubuntu",
                "--cd",
                "~",
                "--exec",
                "bash",
                "-lc",
                "exec tmux attach"
            ]
        );
    }

    #[test]
    fn lookups_answer_only_for_found_distributions() {
        let _table = TEST_TABLE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_for_tests(vec![("wsl-ubuntu".into(), "Ubuntu".into())]);
        assert_eq!(distro_for("wsl-ubuntu").as_deref(), Some("Ubuntu"));
        assert!(is_wsl_host("wsl-ubuntu"));
        assert!(!is_wsl_host("wsl-debian"));
        assert!(!is_wsl_host("mefistos"));
        set_for_tests(Vec::new());
        assert!(!is_wsl_host("wsl-ubuntu"));
    }
}
