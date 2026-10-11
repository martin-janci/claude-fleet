//! Finding `scrcpy` and `adb` on THIS machine. A GUI app on macOS does not
//! inherit the login shell's PATH, so the usual Homebrew and Android SDK
//! folders are searched too; `FLEET_SCRCPY` / `FLEET_ADB` override.

use crate::ipc_error::{codes, IpcError};
use std::path::{Path, PathBuf};

/// The folders a GUI app does not have on its PATH but Homebrew installs into.
pub const SYSTEM_DIRS: [&str; 2] = ["/opt/homebrew/bin", "/usr/local/bin"];

#[derive(Debug, Clone)]
pub struct Tools {
    pub scrcpy: PathBuf,
    pub adb: PathBuf,
}

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// Where to look for `name`, in order: the env override, PATH, `system_dirs`,
/// then (for adb) the Android SDK folders.
pub fn candidates(
    name: &str,
    env: &dyn Fn(&str) -> Option<String>,
    home: &Path,
    system_dirs: &[PathBuf],
) -> Vec<PathBuf> {
    let file = exe(name);
    let mut out = Vec::new();
    if let Some(p) = env(&format!("FLEET_{}", name.to_uppercase())).filter(|p| !p.is_empty()) {
        out.push(PathBuf::from(p));
    }
    if let Some(path) = env("PATH") {
        out.extend(std::env::split_paths(&path).map(|d| d.join(&file)));
    }
    out.extend(system_dirs.iter().map(|d| d.join(&file)));
    if name == "adb" {
        for k in ["ANDROID_HOME", "ANDROID_SDK_ROOT"] {
            if let Some(d) = env(k).filter(|d| !d.is_empty()) {
                out.push(Path::new(&d).join("platform-tools").join(&file));
            }
        }
        for d in ["Library/Android/sdk", "Android/Sdk", "Android/sdk"] {
            out.push(home.join(d).join("platform-tools").join(&file));
        }
    }
    out
}

fn first_file(c: Vec<PathBuf>) -> Option<PathBuf> {
    c.into_iter().find(|p| p.is_file())
}

/// `find` with the search folders named, so a test is independent of what
/// the machine it runs on has installed.
pub fn find_in(
    env: &dyn Fn(&str) -> Option<String>,
    home: &Path,
    system_dirs: &[PathBuf],
) -> Result<Tools, IpcError> {
    let scrcpy = first_file(candidates("scrcpy", env, home, system_dirs)).ok_or_else(|| {
        IpcError::new(
            codes::E_TOOL_MISSING,
            "scrcpy is not installed on this machine: `brew install scrcpy` (macOS) or your package manager; \
             or set FLEET_SCRCPY to its path",
        )
    })?;
    let adb = first_file(candidates("adb", env, home, system_dirs)).ok_or_else(|| {
        IpcError::new(
            codes::E_TOOL_MISSING,
            "adb is not installed on this machine: install Android platform-tools \
             (`brew install android-platform-tools`), or set FLEET_ADB to its path",
        )
    })?;
    Ok(Tools { scrcpy, adb })
}

pub fn find(env: &dyn Fn(&str) -> Option<String>, home: &Path) -> Result<Tools, IpcError> {
    let dirs: Vec<PathBuf> = SYSTEM_DIRS.iter().map(PathBuf::from).collect();
    find_in(env, home, &dirs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    fn system_dirs() -> Vec<PathBuf> {
        SYSTEM_DIRS.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn candidates_cover_path_homebrew_and_the_sdk() {
        let home = Path::new("/home/u");
        let env = |k: &str| match k {
            "PATH" => Some("/a:/b".to_string()),
            "ANDROID_HOME" => Some("/sdk".to_string()),
            _ => None,
        };
        let adb = candidates("adb", &env, home, &system_dirs());
        for want in [
            "/a/adb",
            "/b/adb",
            "/opt/homebrew/bin/adb",
            "/usr/local/bin/adb",
            "/sdk/platform-tools/adb",
            "/home/u/Library/Android/sdk/platform-tools/adb",
            "/home/u/Android/Sdk/platform-tools/adb",
        ] {
            assert!(
                adb.iter().any(|p| p == Path::new(want)),
                "missing {want}: {adb:?}"
            );
        }
        let scrcpy = candidates("scrcpy", &env, home, &system_dirs());
        assert!(scrcpy
            .iter()
            .all(|p| !p.to_string_lossy().contains("platform-tools")));
    }

    #[test]
    fn an_env_override_comes_first() {
        let env = |k: &str| (k == "FLEET_SCRCPY").then(|| "/opt/s/scrcpy".to_string());
        assert_eq!(
            candidates("scrcpy", &env, Path::new("/h"), &[])[0],
            Path::new("/opt/s/scrcpy")
        );
    }

    #[test]
    fn a_missing_scrcpy_says_how_to_install_it() {
        let dir = tempfile::tempdir().unwrap();
        let e = find_in(&no_env, dir.path(), &[]).unwrap_err();
        assert_eq!(e.code, crate::ipc_error::codes::E_TOOL_MISSING);
        assert!(
            e.message.contains("scrcpy") && e.message.contains("brew install scrcpy"),
            "{}",
            e.message
        );
    }
}
