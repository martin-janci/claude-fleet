//! Environment recovery for Finder-launched (GUI) starts, and the app data
//! directory.

use directories::ProjectDirs;

/// The platform app data directory (`state.db`, `logs/`), created if missing,
/// and a line for the log when an older build's data was moved into it (or
/// could not be). Panics on failure, so it is called once, at startup in
/// [`crate::run`], before logging exists; IPC handlers read the managed
/// `commands::diagnostics::AppDataDir` instead.
///
/// On Windows that is the **Local** profile (`%LOCALAPPDATA%`): the database
/// holds tracker credentials and per-host tokens, and a Roaming profile is
/// copied to a server at sign-out on a domain machine. Builds before this one
/// used Roaming; their files are moved over once ([`move_data_dir`]).
/// Everywhere else it is `data_dir()`, as it always was.
pub(crate) fn appdata_dir() -> (std::path::PathBuf, Option<String>) {
    let dirs = ProjectDirs::from("sk", "rlt", "claude-fleet")
        .expect("could not resolve platform appdata dir");
    let (dir, note) = if cfg!(windows) {
        let local = dirs.data_local_dir();
        match move_data_dir(dirs.data_dir(), local) {
            Ok(0) => (local.to_path_buf(), None),
            Ok(n) => (
                local.to_path_buf(),
                Some(format!(
                    "moved {n} entries of the app data from {} to {}",
                    dirs.data_dir().display(),
                    local.display()
                )),
            ),
            // Nothing was moved (a failure puts back what had been): run on
            // the old directory rather than a new, empty database.
            Err(e) => (
                dirs.data_dir().to_path_buf(),
                Some(format!(
                    "app data not moved to {} ({e}); using {}",
                    local.display(),
                    dirs.data_dir().display()
                )),
            ),
        }
    } else {
        (dirs.data_dir().to_path_buf(), None)
    };
    std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("create appdata dir {dir:?}: {e}"));
    (dir, note)
}

/// The database: its presence decides whether a directory holds app data.
const DB_FILE: &str = "state.db";

/// Move everything in `old` into `new`, once: only when `old` holds a
/// database and `new` does not. The database and its `-wal` / `-shm` go
/// first, then every other entry (logs, the legacy hub token file) that
/// `new` does not already have. A rename that fails puts back every one
/// already made and reports the error, so the data is never split between
/// the two. Returns how many entries moved (0: nothing to do).
fn move_data_dir(old: &std::path::Path, new: &std::path::Path) -> Result<usize, String> {
    move_data_dir_with(old, new, |from, to| std::fs::rename(from, to))
}

/// [`move_data_dir`] with the rename injected (a test makes one fail).
fn move_data_dir_with(
    old: &std::path::Path,
    new: &std::path::Path,
    rename: impl Fn(&std::path::Path, &std::path::Path) -> std::io::Result<()>,
) -> Result<usize, String> {
    if old == new || new.join(DB_FILE).exists() || !old.join(DB_FILE).is_file() {
        return Ok(0);
    }
    std::fs::create_dir_all(new).map_err(|e| format!("create {}: {e}", new.display()))?;
    let first = [
        DB_FILE.to_string(),
        format!("{DB_FILE}-wal"),
        format!("{DB_FILE}-shm"),
    ];
    let mut names: Vec<std::ffi::OsString> = first
        .iter()
        .map(std::ffi::OsString::from)
        .filter(|n| old.join(n).exists())
        .collect();
    let mut rest: Vec<std::ffi::OsString> = std::fs::read_dir(old)
        .map_err(|e| format!("read {}: {e}", old.display()))?
        .filter_map(|e| e.ok().map(|e| e.file_name()))
        .filter(|n| !names.contains(n))
        .collect();
    rest.sort();
    names.extend(rest);
    let mut moved: Vec<&std::ffi::OsString> = Vec::new();
    for name in &names {
        let to = new.join(name);
        if to.exists() {
            continue;
        }
        if let Err(e) = rename(&old.join(name), &to) {
            for back in moved.iter().rev() {
                let _ = rename(&new.join(back), &old.join(back));
            }
            return Err(format!("move {}: {e}", name.to_string_lossy()));
        }
        moved.push(name);
    }
    Ok(moved.len())
}

/// Pure: compute a new PATH that appends any of `common_bin_dirs` that are not
/// already in `current` and that `dir_exists` reports as present. Returns
/// `None` if nothing would change.
// Unix-only in the app (see `import_login_shell_env`); the tests are portable.
#[cfg_attr(not(unix), allow(dead_code))]
fn compute_backfilled_path(
    current: &str,
    common_bin_dirs: &[&str],
    dir_exists: impl Fn(&str) -> bool,
) -> Option<String> {
    let parts: Vec<&str> = current.split(':').filter(|p| !p.is_empty()).collect();
    let additions: Vec<&str> = common_bin_dirs
        .iter()
        .copied()
        .filter(|d| !parts.contains(d) && dir_exists(d))
        .collect();
    if additions.is_empty() {
        return None;
    }
    let mut new_parts: Vec<String> = parts.iter().map(|s| s.to_string()).collect();
    new_parts.extend(additions.iter().map(|s| s.to_string()));
    Some(new_parts.join(":"))
}

/// Run the user's login shell once and adopt several env vars that a
/// Finder-launched GUI app does not inherit by default:
///
///   - PATH    — catches Homebrew, dotfiles bin/, fnm/nvm/asdf/mise, and
///     custom per-user wrappers like `cl`.
///   - LANG / LC_ALL / LC_CTYPE — locale. Without these, claude and other
///     TUIs detect a non-UTF-8 terminal and render ASCII fallbacks
///     (`_` instead of `└` / `↑` / `█` etc.).
///
/// One shell invocation prints a sentinel, then each value `\x1e`-terminated.
/// We parse and call set_var on each non-empty value. Best-effort: any failure
/// leaves the var unchanged.
#[cfg(unix)]
pub(crate) fn import_login_shell_env() -> bool {
    let Ok(shell) = std::env::var("SHELL") else {
        return false;
    };
    // Order MUST match the positional parsing in `parse_login_env`.
    const VARS: &[&str] = &["PATH", "LANG", "LC_ALL", "LC_CTYPE"];
    // Print a sentinel first so any banner/chatter the rc files emit to stdout
    // is discarded, then each value `\x1e`-terminated.
    let mut script = format!("printf '%s' '{ENV_DUMP_SENTINEL}'");
    for v in VARS {
        script.push_str(&format!("; printf '%s\\x1e' \"${v}\""));
    }
    // INTERACTIVE login shell (`-i -l`). Users put PATH additions, version-
    // manager shims, and wrappers like `cl` in `.zshrc`/`.bashrc`, which are
    // sourced ONLY for interactive shells. A non-interactive login shell
    // (`-l -c`) sources just `.zprofile`/`.zlogin` and misses them — so a
    // Finder-launched GUI app could not find `cl`, and every tmux pane failed
    // with "cl: command not found". `-c` still runs our script and exits (no
    // interactive prompt loop), so output stays clean.
    let Ok(output) = fleet_core::proc::std_command(&shell)
        .args(["-i", "-l", "-c", &script])
        .output()
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut any_set = false;
    for (var, val) in parse_login_env(&stdout, VARS) {
        std::env::set_var(var, val);
        any_set = true;
    }
    any_set
}

/// Marker printed by [`import_login_shell_env`] immediately before the env
/// values, so interactive-shell startup chatter (greetings, version-manager
/// banners) printed to stdout is dropped during parsing.
// Unix-only in the app (see `import_login_shell_env`); the tests are portable.
#[cfg_attr(not(unix), allow(dead_code))]
const ENV_DUMP_SENTINEL: &str = "__FLEET_ENV_BEGIN__";

/// Parse the `printf` dump from [`import_login_shell_env`]: discard everything
/// up to and including the sentinel, then read the `\x1e`-delimited values
/// positionally against `vars`. Empty/whitespace values are skipped. If the
/// sentinel is absent (degenerate shell), the whole output is parsed as a
/// fallback.
// Unix-only in the app (see `import_login_shell_env`); the tests are portable.
#[cfg_attr(not(unix), allow(dead_code))]
fn parse_login_env<'a>(stdout: &str, vars: &[&'a str]) -> Vec<(&'a str, String)> {
    let body = stdout
        .rsplit_once(ENV_DUMP_SENTINEL)
        .map(|(_, after)| after)
        .unwrap_or(stdout);
    let mut parts = body.split('\x1e');
    let mut out = Vec::new();
    for var in vars {
        let Some(val) = parts.next() else { break };
        let trimmed = val.trim();
        if !trimmed.is_empty() {
            out.push((*var, trimmed.to_string()));
        }
    }
    out
}

/// When launched from Finder (Spotlight, Dock, double-click), macOS hands the
/// app a minimal PATH that does NOT include Homebrew (`/opt/homebrew/bin` on
/// Apple Silicon, `/usr/local/bin` on Intel). Without this fix, every shelled
/// command — `tmux`, `git` — fails with "binary not found on PATH".
///
/// Backfill the common locations once at startup. We append (not prepend) so
/// anything the user has explicitly set wins for ambiguous cases.
/// Ensure LANG points at a UTF-8 locale so spawned PTYs (and the shell-detection
/// inside claude/tmux) treat the terminal as Unicode-capable. macOS ships
/// en_US.UTF-8; we use C.UTF-8 as a portable fallback. Only writes if no UTF-8
/// locale is already present in any of LC_ALL / LANG / LC_CTYPE — we never
/// override an explicit user choice.
pub(crate) fn backfill_locale_for_gui_launch() {
    let has_utf8 = ["LC_ALL", "LANG", "LC_CTYPE"]
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .any(|v| {
            v.to_ascii_uppercase().contains("UTF-8") || v.to_ascii_uppercase().contains("UTF8")
        });
    if has_utf8 {
        return;
    }
    std::env::set_var("LANG", "en_US.UTF-8");
}

/// True when the process already has a Homebrew bin dir on PATH and a UTF-8
/// locale — the signature of a terminal launch, where the (100-500 ms)
/// login-shell import is redundant. Errs toward `false` (run the import) when
/// unsure: claude-fleet only shells out to `ssh`/`git`/`tmux`, which live in
/// the standard bin dirs `backfill_path_for_gui_launch` guarantees anyway.
#[cfg(unix)]
pub(crate) fn env_looks_complete() -> bool {
    let path = std::env::var("PATH").unwrap_or_default();
    let has_brew = path
        .split(':')
        .any(|p| p == "/opt/homebrew/bin" || p == "/usr/local/bin");
    let has_utf8 = ["LC_ALL", "LANG", "LC_CTYPE"]
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .any(|v| {
            let u = v.to_ascii_uppercase();
            u.contains("UTF-8") || u.contains("UTF8")
        });
    has_brew && has_utf8
}

#[cfg(unix)]
pub(crate) fn backfill_path_for_gui_launch() {
    const COMMON_BIN_DIRS: &[&str] = &[
        "/opt/homebrew/bin", // Apple Silicon Homebrew
        "/usr/local/bin",    // Intel Homebrew
        "/usr/bin",
        "/bin",
    ];
    let current = std::env::var("PATH").unwrap_or_default();
    if let Some(new_path) = compute_backfilled_path(&current, COMMON_BIN_DIRS, |d| {
        std::path::Path::new(d).exists()
    }) {
        std::env::set_var("PATH", new_path);
    }
}

#[cfg(test)]
mod data_dir_tests {
    use super::*;

    #[test]
    fn an_older_builds_data_moves_once_with_the_database_first() {
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join("Roaming");
        let new = root.path().join("Local");
        std::fs::create_dir_all(old.join("logs")).unwrap();
        for f in [
            "state.db",
            "state.db-wal",
            "state.db-shm",
            "hub-client-token",
        ] {
            std::fs::write(old.join(f), f).unwrap();
        }
        std::fs::write(old.join("logs").join("app.log"), "log").unwrap();

        assert_eq!(move_data_dir(&old, &new), Ok(5));
        for f in [
            "state.db",
            "state.db-wal",
            "state.db-shm",
            "hub-client-token",
        ] {
            assert_eq!(std::fs::read_to_string(new.join(f)).unwrap(), f);
            assert!(!old.join(f).exists(), "{f} left behind");
        }
        assert!(new.join("logs").join("app.log").is_file());

        // Once: a database in the new place is never overwritten.
        std::fs::write(old.join("state.db"), "stale").unwrap();
        assert_eq!(move_data_dir(&old, &new), Ok(0));
        assert_eq!(
            std::fs::read_to_string(new.join("state.db")).unwrap(),
            "state.db"
        );
    }

    #[test]
    fn nothing_moves_without_an_old_database() {
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join("Roaming");
        let new = root.path().join("Local");
        assert_eq!(move_data_dir(&old, &new), Ok(0));
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("other"), "x").unwrap();
        assert_eq!(move_data_dir(&old, &new), Ok(0));
        assert!(old.join("other").exists());
        assert_eq!(move_data_dir(&old, &old), Ok(0));
    }

    /// A rename that fails part-way puts back what had moved: the database
    /// is never left in one directory and its WAL in the other.
    #[test]
    fn a_failed_move_puts_back_what_had_moved() {
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join("Roaming");
        let new = root.path().join("Local");
        std::fs::create_dir_all(&old).unwrap();
        for f in ["state.db", "state.db-wal", "state.db-shm"] {
            std::fs::write(old.join(f), f).unwrap();
        }
        let calls = std::cell::Cell::new(0);
        let r = move_data_dir_with(&old, &new, |from, to| {
            calls.set(calls.get() + 1);
            // The third move (`-shm`, held open by another process) fails.
            if calls.get() == 3 {
                return Err(std::io::Error::other("in use"));
            }
            std::fs::rename(from, to)
        });
        assert!(r.unwrap_err().contains("state.db-shm"));
        for f in ["state.db", "state.db-wal", "state.db-shm"] {
            assert!(old.join(f).is_file(), "{f} put back");
            assert!(!new.join(f).exists(), "{f} not split off");
        }
    }
}

#[cfg(test)]
mod path_backfill_tests {
    use super::*;

    #[test]
    fn appends_missing_dirs_at_end() {
        let result =
            compute_backfilled_path("/usr/bin", &["/opt/homebrew/bin", "/usr/bin"], |_| true)
                .unwrap();
        assert_eq!(result, "/usr/bin:/opt/homebrew/bin");
    }

    #[test]
    fn does_not_add_nonexistent_dirs() {
        let result = compute_backfilled_path("/usr/bin", &["/this/does/not/exist"], |_| false);
        assert!(result.is_none());
    }

    #[test]
    fn no_change_when_all_present() {
        let result = compute_backfilled_path(
            "/opt/homebrew/bin:/usr/bin",
            &["/opt/homebrew/bin", "/usr/bin"],
            |_| true,
        );
        assert!(result.is_none());
    }

    #[test]
    fn skips_empty_path_components() {
        let result =
            compute_backfilled_path("/usr/bin::", &["/opt/homebrew/bin"], |_| true).unwrap();
        assert_eq!(result, "/usr/bin:/opt/homebrew/bin");
    }

    #[test]
    fn handles_empty_path() {
        let result =
            compute_backfilled_path("", &["/opt/homebrew/bin", "/usr/bin"], |_| true).unwrap();
        assert_eq!(result, "/opt/homebrew/bin:/usr/bin");
    }

    /// Pure helper for the locale-backfill decision used by
    /// `backfill_locale_for_gui_launch`. Lifted out for testability —
    /// the real fn touches std::env which makes parallel tests racy.
    fn needs_locale_backfill(lc_all: &str, lang: &str, lc_ctype: &str) -> bool {
        ![lc_all, lang, lc_ctype].iter().any(|v| {
            v.to_ascii_uppercase().contains("UTF-8") || v.to_ascii_uppercase().contains("UTF8")
        })
    }

    #[test]
    fn locale_backfill_triggers_when_all_empty() {
        assert!(needs_locale_backfill("", "", ""));
    }

    #[test]
    fn locale_backfill_skipped_when_lang_is_utf8() {
        assert!(!needs_locale_backfill("", "en_US.UTF-8", ""));
        assert!(!needs_locale_backfill("", "C.UTF-8", ""));
        assert!(!needs_locale_backfill("", "sk_SK.utf8", "")); // case-insensitive
    }

    #[test]
    fn locale_backfill_skipped_when_lc_all_is_utf8() {
        assert!(!needs_locale_backfill("en_US.UTF-8", "C", ""));
    }

    #[test]
    fn locale_backfill_triggers_when_only_c() {
        // Plain POSIX C locale isn't UTF-8 — we should still backfill.
        assert!(needs_locale_backfill("C", "C", "C"));
        assert!(needs_locale_backfill("POSIX", "POSIX", ""));
    }

    const VARS: &[&str] = &["PATH", "LANG", "LC_ALL", "LC_CTYPE"];

    #[test]
    fn parse_login_env_reads_values_positionally_and_skips_empty() {
        let dump =
            format!("{ENV_DUMP_SENTINEL}/opt/homebrew/bin:/usr/bin\x1een_US.UTF-8\x1e\x1e\x1e");
        let got = parse_login_env(&dump, VARS);
        assert_eq!(
            got,
            vec![
                ("PATH", "/opt/homebrew/bin:/usr/bin".to_string()),
                ("LANG", "en_US.UTF-8".to_string()),
            ]
        );
    }

    #[test]
    fn parse_login_env_discards_rc_chatter_before_sentinel() {
        // An interactive .zshrc may print a banner / version-manager notice to
        // stdout before our values; the sentinel must isolate the real dump.
        let dump = format!(
            "Welcome!\nfnm: using node v22\n{ENV_DUMP_SENTINEL}/Users/me/bin:/usr/bin\x1e\x1e\x1e\x1e"
        );
        assert_eq!(
            parse_login_env(&dump, VARS),
            vec![("PATH", "/Users/me/bin:/usr/bin".to_string())]
        );
    }

    #[test]
    fn parse_login_env_falls_back_when_sentinel_absent() {
        // Degenerate shell that swallowed the sentinel: parse the whole output.
        let dump = "/usr/bin\x1een_US.UTF-8\x1e\x1e\x1e";
        assert_eq!(
            parse_login_env(dump, VARS),
            vec![
                ("PATH", "/usr/bin".to_string()),
                ("LANG", "en_US.UTF-8".to_string()),
            ]
        );
    }
}
