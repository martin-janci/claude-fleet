//! This machine's home directory, and the per-user cache under it: the one
//! place that decides where they are, so Windows (which sets no `HOME`) gets
//! the same answer everywhere.

use std::path::PathBuf;

/// The user's home directory. `$HOME` on Unix, exactly as before this
/// helper existed (tests point it at a temp dir). On Windows it is the
/// profile folder (`%USERPROFILE%`, through the Known Folders API), which is
/// also where Windows' own `ssh` looks for `.ssh\config`; `HOME` there is
/// usually unset and, when Git Bash sets it, not necessarily the same place.
pub fn home_dir() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        std::env::var_os("HOME").map(PathBuf::from)
    }
    #[cfg(not(unix))]
    {
        directories::BaseDirs::new().map(|b| b.home_dir().to_path_buf())
    }
}

/// `claude-fleet`'s own cache dir on this machine: `~/.cache/claude-fleet`
/// on Unix (the ssh ControlPath sockets live there), `%LOCALAPPDATA%\
/// claude-fleet` on Windows. The system temp dir when neither is known.
pub fn cache_dir() -> PathBuf {
    #[cfg(unix)]
    let base = home_dir().map(|h| h.join(".cache"));
    #[cfg(not(unix))]
    let base = directories::BaseDirs::new().map(|b| b.cache_dir().to_path_buf());
    base.unwrap_or_else(std::env::temp_dir).join("claude-fleet")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cache_dir_is_claude_fleets_own() {
        assert!(cache_dir().ends_with("claude-fleet"));
    }

    #[cfg(unix)]
    #[test]
    fn unix_reads_home_and_caches_under_dot_cache() {
        let Some(home) = std::env::var_os("HOME") else {
            return;
        };
        assert_eq!(home_dir(), Some(PathBuf::from(&home)));
        assert_eq!(
            cache_dir(),
            PathBuf::from(home).join(".cache").join("claude-fleet")
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_uses_the_profile_and_local_app_data() {
        let profile = std::env::var_os("USERPROFILE").expect("USERPROFILE on Windows");
        assert_eq!(home_dir(), Some(PathBuf::from(profile)));
        let local = std::env::var_os("LOCALAPPDATA").expect("LOCALAPPDATA on Windows");
        assert_eq!(cache_dir(), PathBuf::from(local).join("claude-fleet"));
    }
}
