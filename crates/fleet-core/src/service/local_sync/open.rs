//! "Open in IDE" (Phase 2): the program that opens a link's folder in a file
//! manager, VS Code, IntelliJ IDEA or a terminal on this machine. Which
//! program and arguments is a pure function of the app and the OS, so every
//! OS's answer is tested on any of them; [`open`] spawns it.

use crate::ipc_error::{codes, IpcError};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// What to open the folder with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OpenApp {
    /// The OS file manager.
    Folder,
    Vscode,
    Intellij,
    Terminal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Os {
    Mac,
    Linux,
    Windows,
}

impl Os {
    pub(crate) fn current() -> Os {
        if cfg!(target_os = "macos") {
            Os::Mac
        } else if cfg!(windows) {
            Os::Windows
        } else {
            Os::Linux
        }
    }
}

/// What `cmd /C` would read as more than a path.
pub(crate) const CMD_META: &[char] = &['&', '|', '<', '>', '^', '%', '"', '!'];

/// The program and its arguments; the child also starts in the folder.
pub(crate) fn command_for(
    app: OpenApp,
    path: &str,
    os: Os,
) -> Result<(String, Vec<String>), IpcError> {
    let p = path.to_string();
    let via_cmd = |tool: &str| -> Result<(String, Vec<String>), IpcError> {
        // `code` / `idea` are .cmd scripts on Windows, which only cmd runs.
        if path.contains(CMD_META) {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("{path} has a character Windows' cmd would misread; use Open folder"),
            ));
        }
        Ok(("cmd".into(), vec!["/C".into(), tool.into(), p.clone()]))
    };
    Ok(match (app, os) {
        (OpenApp::Folder, Os::Mac) => ("open".into(), vec![p]),
        (OpenApp::Folder, Os::Linux) => ("xdg-open".into(), vec![p]),
        (OpenApp::Folder, Os::Windows) => ("explorer".into(), vec![p]),
        // A Finder-launched app has no shell PATH, so the bundles by name.
        (OpenApp::Vscode, Os::Mac) => (
            "open".into(),
            vec!["-a".into(), "Visual Studio Code".into(), p],
        ),
        (OpenApp::Vscode, Os::Linux) => ("code".into(), vec![p]),
        (OpenApp::Vscode, Os::Windows) => via_cmd("code")?,
        (OpenApp::Intellij, Os::Mac) => {
            ("open".into(), vec!["-a".into(), "IntelliJ IDEA".into(), p])
        }
        (OpenApp::Intellij, Os::Linux) => ("idea".into(), vec![p]),
        (OpenApp::Intellij, Os::Windows) => via_cmd("idea")?,
        (OpenApp::Terminal, Os::Mac) => ("open".into(), vec!["-a".into(), "Terminal".into(), p]),
        (OpenApp::Terminal, Os::Linux) => ("x-terminal-emulator".into(), vec![]),
        (OpenApp::Terminal, Os::Windows) => ("wt".into(), vec!["-d".into(), p]),
    })
}

/// How to get the launcher a missing program is.
fn install_hint(app: OpenApp, os: Os) -> &'static str {
    match (app, os) {
        (OpenApp::Vscode, Os::Mac) => "is Visual Studio Code installed in Applications?",
        (OpenApp::Vscode, _) => {
            "install VS Code's `code` command (Command Palette: Shell Command: Install 'code' command in PATH)"
        }
        (OpenApp::Intellij, Os::Mac) => "is IntelliJ IDEA installed in Applications?",
        (OpenApp::Intellij, _) => {
            "enable IntelliJ's `idea` launcher (JetBrains Toolbox: Settings, Tools, Generate shell scripts)"
        }
        (OpenApp::Terminal, Os::Windows) => "install Windows Terminal",
        (OpenApp::Terminal, _) => "no terminal emulator was found",
        (OpenApp::Folder, _) => "no file manager was found",
    }
}

/// Open `path` (a link's folder) with `app`. Does not wait for it.
pub fn open(app: OpenApp, path: &str) -> Result<(), IpcError> {
    if !Path::new(path).is_dir() {
        return Err(IpcError::new(
            codes::E_NOTFOUND,
            format!("{path} is not there any more"),
        ));
    }
    let os = Os::current();
    let (program, args) = command_for(app, path, os)?;
    spawn_detached(&program, &args, Some(path), app, os)
}

/// Start `program` without waiting for it, in `cwd` when given; a missing
/// program names how to get it ([`install_hint`]).
pub(crate) fn spawn_detached(
    program: &str,
    args: &[String],
    cwd: Option<&str>,
    app: OpenApp,
    os: Os,
) -> Result<(), IpcError> {
    let mut cmd = crate::proc::std_command(program);
    cmd.args(args);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let child = cmd
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                IpcError::new(
                    codes::E_NOTFOUND,
                    format!("{program} was not found: {}", install_hint(app, os)),
                )
            } else {
                IpcError::new(codes::E_IO, format!("could not start {program}: {e}"))
            }
        })?;
    // Reap it whenever it exits (launchers return at once; a terminal may not).
    std::thread::spawn(move || {
        let mut child = child;
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_app_has_a_program_on_every_os() {
        for os in [Os::Mac, Os::Linux, Os::Windows] {
            for app in [
                OpenApp::Folder,
                OpenApp::Vscode,
                OpenApp::Intellij,
                OpenApp::Terminal,
            ] {
                let (program, _) = command_for(app, "/home/u/fleet/app", os).unwrap();
                assert!(!program.is_empty(), "{app:?} on {os:?}");
            }
        }
    }

    #[test]
    fn the_folder_is_passed_as_one_argument() {
        let (p, a) = command_for(OpenApp::Vscode, "/Users/u/my app", Os::Mac).unwrap();
        assert_eq!(p, "open");
        assert_eq!(a, ["-a", "Visual Studio Code", "/Users/u/my app"]);
        let (p, a) = command_for(OpenApp::Intellij, "/home/u/x", Os::Linux).unwrap();
        assert_eq!((p.as_str(), a), ("idea", vec!["/home/u/x".to_string()]));
        let (p, a) = command_for(OpenApp::Vscode, r"C:\work\app", Os::Windows).unwrap();
        assert_eq!(p, "cmd");
        assert_eq!(a, ["/C", "code", r"C:\work\app"]);
    }

    #[test]
    fn a_path_cmd_would_misread_is_refused_on_windows_only() {
        let bad = r"C:\work\a&calc";
        let e = command_for(OpenApp::Vscode, bad, Os::Windows).unwrap_err();
        assert_eq!(e.code, codes::E_INVALID);
        assert!(command_for(OpenApp::Folder, bad, Os::Windows).is_ok());
        assert!(command_for(OpenApp::Vscode, "/home/u/a&b", Os::Linux).is_ok());
    }

    #[test]
    fn a_missing_folder_is_not_opened() {
        let e = open(OpenApp::Folder, "/nonexistent/fleet/folder").unwrap_err();
        assert_eq!(e.code, codes::E_NOTFOUND);
    }
}
