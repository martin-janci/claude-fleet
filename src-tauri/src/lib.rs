mod commands;
mod ipc_error;
mod projects;
mod pty;
mod store;
mod tmux;

use directories::ProjectDirs;
use pty::PtyState;
use std::sync::Mutex;
use store::Store;

fn appdata_db_path() -> std::path::PathBuf {
    let dirs = ProjectDirs::from("sk", "rlt", "claude-fleet")
        .expect("could not resolve platform appdata dir");
    let dir = dirs.data_dir();
    std::fs::create_dir_all(dir).unwrap_or_else(|e| panic!("create appdata dir {dir:?}: {e}"));
    dir.join("state.db")
}

/// Pure: compute a new PATH that appends any of `common_bin_dirs` that are not
/// already in `current` and that `dir_exists` reports as present. Returns
/// `None` if nothing would change.
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

/// Run the user's login shell with `-l -c 'printf $PATH'` and adopt the result
/// as the process PATH. This catches everything the user has added in their
/// shell init files (Homebrew, dotfiles bin/, fnm/nvm/asdf/mise, custom
/// per-user wrappers like `cl`) — paths a Finder-launched GUI app does not
/// inherit by default. Best-effort: returns false if `$SHELL` is unset, the
/// shell exits non-zero, or the captured PATH is empty.
fn import_login_shell_path() -> bool {
    let Ok(shell) = std::env::var("SHELL") else {
        return false;
    };
    let Ok(output) = std::process::Command::new(&shell)
        .args(["-l", "-c", "printf '%s' \"$PATH\""])
        .output()
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let imported = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if imported.is_empty() {
        return false;
    }
    std::env::set_var("PATH", imported);
    true
}

/// When launched from Finder (Spotlight, Dock, double-click), macOS hands the
/// app a minimal PATH that does NOT include Homebrew (`/opt/homebrew/bin` on
/// Apple Silicon, `/usr/local/bin` on Intel). Without this fix, every shelled
/// command — `tmux`, `git` — fails with "binary not found on PATH".
///
/// Backfill the common locations once at startup. We append (not prepend) so
/// anything the user has explicitly set wins for ambiguous cases.
fn backfill_path_for_gui_launch() {
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Layered PATH recovery for Finder-launched apps:
    //   1. Try to import the user's full login-shell PATH (Homebrew, dotfiles
    //      bin/, fnm, mise, etc.). This is where `cl` lives.
    //   2. Belt-and-suspenders backfill ensures the standard macOS bin dirs
    //      are present even if step 1 failed or returned a stunted PATH.
    import_login_shell_path();
    backfill_path_for_gui_launch();

    let db_path = appdata_db_path();
    let store = Store::open(&db_path).expect("open store");

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(Mutex::new(store))
        .manage(Mutex::new(PtyState::new()))
        .invoke_handler(tauri::generate_handler![
            commands::health::health_check,
            commands::projects::list_projects,
            commands::projects::refresh_projects,
            commands::sessions::list_sessions,
            commands::sessions::new_session,
            commands::sessions::kill_session,
            pty::pty_open,
            pty::pty_write,
            pty::pty_resize,
            pty::pty_close,
            pty::pty_drain,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
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
}
