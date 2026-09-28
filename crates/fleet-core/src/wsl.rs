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

use std::sync::RwLock;

/// Host aliases this module owns start with this.
pub const ALIAS_PREFIX: &str = "wsl-";

/// The distributions found by the last [`refresh`]: `(alias, distribution)`.
static DISTROS: RwLock<Vec<(String, String)>> = RwLock::new(Vec::new());

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
    let looks_utf16 = raw.starts_with(&[0xFF, 0xFE])
        || (raw.len() >= 2
            && raw.len().is_multiple_of(2)
            && raw.iter().skip(1).step_by(2).all(|&b| b == 0));
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
pub fn refresh(taken: &[String], timeout: std::time::Duration) {
    let Some(found) = detect(timeout) else {
        return;
    };
    let table = assign_aliases(&found, taken);
    *DISTROS
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = table;
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
/// run, failed, or overran (a WSL service still starting can take seconds);
/// `Some(empty)` when WSL is there with no distribution.
#[cfg(windows)]
fn detect(timeout: std::time::Duration) -> Option<Vec<String>> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let out = crate::proc::std_command(wsl_binary())
            .args(["--list", "--quiet"])
            .stdin(std::process::Stdio::null())
            .output();
        let _ = tx.send(out);
    });
    let out = rx.recv_timeout(timeout).ok()?.ok()?;
    if !out.status.success() {
        // "no installed distributions" exits non-zero: there are none.
        return Some(Vec::new());
    }
    Some(parse_list(&out.stdout))
}

#[cfg(not(windows))]
fn detect(_timeout: std::time::Duration) -> Option<Vec<String>> {
    None
}

/// The `wsl.exe` arguments that run `args` in `distro` the way `ssh` would
/// run them on a host: the words joined by spaces, read by a shell.
pub fn exec_args(distro: &str, args: &[&str]) -> Vec<String> {
    vec![
        "--distribution".into(),
        distro.into(),
        "--exec".into(),
        "sh".into(),
        "-c".into(),
        args.join(" "),
    ]
}

/// A `tokio` command running `args` in `distro` (see [`exec_args`]).
pub fn command(distro: &str, args: &[&str]) -> tokio::process::Command {
    let mut cmd = crate::proc::command(wsl_binary());
    cmd.args(exec_args(distro, args));
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
