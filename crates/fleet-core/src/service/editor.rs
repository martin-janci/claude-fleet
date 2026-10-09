//! Open in VS Code (Orbit Fleet redesign step 5.5): open a session's
//! worktree in VS Code on THIS machine, locally for a `local` session and
//! through VS Code's Remote - SSH (`ssh-remote+<alias>`) for any other host.
//!
//! The folder is the one the session's pane is in, raised to its git
//! toplevel (the worktree), asked of the host over this machine's own SSH
//! from the alias and tmux name passed in, so nothing here reads `state.db`:
//! the same story as `pty_open`. Which program and arguments open it is a
//! pure function of the host, the folder and the OS ([`editor_command`]),
//! so every OS's answer is tested on any of them.

use crate::ipc_error::{codes, IpcError};
use crate::service::local_sync::open::{self, OpenApp, Os, CMD_META};
use crate::service::projects::LOCAL_HOST;
use crate::shell::quote;
use crate::ssh::SshExec;
use std::time::Duration;

/// How long the folder lookup may take; it is one `tmux` and one `git`.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(15);

/// The exit code the lookup script uses when the pane's folder is gone.
const NO_FOLDER_EXIT: i32 = 3;

/// The script that prints the session's folder: the pane's directory,
/// raised to its git toplevel when it is inside a repository.
pub fn folder_script(tmux_name: &str) -> String {
    format!(
        "p=\"$(tmux display-message -t {pane} -p '#{{pane_current_path}}')\" || exit 1\n\
         [ -d \"$p\" ] || exit {NO_FOLDER_EXIT}\n\
         root=\"$(git -C \"$p\" rev-parse --show-toplevel 2>/dev/null)\" || root=\"$p\"\n\
         [ -n \"$root\" ] || root=\"$p\"\n\
         printf '%s' \"$root\"",
        pane = quote(&crate::tmux::exact_pane(tmux_name)),
    )
}

/// The session's folder on its host.
pub async fn session_folder(
    ssh: &dyn SshExec,
    host_alias: &str,
    tmux_name: &str,
) -> Result<String, IpcError> {
    let out =
        crate::ssh::run_shell(ssh, host_alias, &folder_script(tmux_name), LOOKUP_TIMEOUT).await?;
    if out.status.code() == Some(NO_FOLDER_EXIT) {
        return Err(IpcError::new(
            codes::E_NOTFOUND,
            format!("{tmux_name}'s folder on {host_alias} is not there any more"),
        ));
    }
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(IpcError::new(
            codes::E_SSH,
            format!(
                "could not find {tmux_name}'s folder on {host_alias}: {}",
                err.trim()
            ),
        ));
    }
    let folder = String::from_utf8_lossy(&out.stdout).trim_end().to_string();
    crate::validate::remote_abs_path("the session's folder", &folder)?;
    Ok(folder)
}

/// Percent-encode a path for a `vscode://` URI, keeping its `/`.
fn uri_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for b in path.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The program and its arguments that open `folder` on `host_alias` in
/// VS Code on `os`. `wsl_distro` is the host's WSL distro when it is one
/// (`wsl::distro_for`): a `wsl-*` alias is no `~/.ssh/config` entry, so
/// Remote-SSH cannot reach it and VS Code's WSL authority opens it instead.
pub(crate) fn editor_command(
    host_alias: &str,
    folder: &str,
    os: Os,
    wsl_distro: Option<&str>,
) -> Result<(String, Vec<String>), IpcError> {
    if host_alias == LOCAL_HOST {
        return open::command_for(OpenApp::Vscode, folder, os);
    }
    crate::validate::host_alias_syntax(host_alias)?;
    crate::validate::remote_abs_path("the session's folder", folder)?;
    let authority = match wsl_distro {
        Some(d) => {
            if d.is_empty()
                || !d
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
            {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    format!("not a WSL distro name: {d:?}"),
                ));
            }
            format!("wsl+{d}")
        }
        None => format!("ssh-remote+{host_alias}"),
    };
    Ok(match os {
        // A Finder-launched app has no shell PATH, so no `code`: VS Code's
        // own URI handler opens the remote folder instead.
        Os::Mac => (
            "open".into(),
            vec![format!(
                "vscode://vscode-remote/{authority}{}",
                uri_path(folder)
            )],
        ),
        Os::Linux => (
            "code".into(),
            vec!["--remote".into(), authority, folder.into()],
        ),
        // `code` is a .cmd script on Windows, which only cmd runs.
        Os::Windows => {
            if folder.contains(CMD_META) {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    format!("{folder} has a character Windows' cmd would misread"),
                ));
            }
            (
                "cmd".into(),
                vec![
                    "/C".into(),
                    "code".into(),
                    "--remote".into(),
                    authority,
                    folder.into(),
                ],
            )
        }
    })
}

/// Open the session `tmux_name` on `host_alias` in VS Code on this machine.
/// Does not wait for the editor.
pub async fn open_session_in_editor(
    ssh: &dyn SshExec,
    host_alias: &str,
    tmux_name: &str,
) -> Result<(), IpcError> {
    crate::validate::host_alias(host_alias)?;
    crate::validate::tmux_name_addressable(tmux_name)?;
    let folder = session_folder(ssh, host_alias, tmux_name).await?;
    let os = Os::current();
    let distro = crate::wsl::distro_for(host_alias);
    let (program, args) = editor_command(host_alias, &folder, os, distro.as_deref())?;
    let cwd = (host_alias == LOCAL_HOST).then_some(folder.as_str());
    open::spawn_detached(&program, &args, cwd, OpenApp::Vscode, os)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// r18-W2: a WSL host opens through VS Code's WSL authority.
    #[test]
    fn a_wsl_host_opens_through_the_wsl_authority() {
        let (p, a) =
            editor_command("wsl-ubuntu", "/home/u/r", Os::Windows, Some("Ubuntu-22.04")).unwrap();
        assert_eq!(p, "cmd");
        assert_eq!(
            a,
            ["/C", "code", "--remote", "wsl+Ubuntu-22.04", "/home/u/r"]
        );
        assert!(editor_command("wsl-x", "/home/u/r", Os::Windows, Some("a&b")).is_err());
    }

    #[test]
    fn a_local_session_opens_its_folder_directly() {
        let (p, a) = editor_command("local", "/Users/u/repo/.worktrees/x", Os::Mac, None).unwrap();
        assert_eq!(p, "open");
        assert_eq!(
            a,
            ["-a", "Visual Studio Code", "/Users/u/repo/.worktrees/x"]
        );
        let (p, a) = editor_command("local", "/home/u/repo", Os::Linux, None).unwrap();
        assert_eq!((p.as_str(), a), ("code", vec!["/home/u/repo".to_string()]));
    }

    #[test]
    fn an_ssh_session_opens_through_remote_ssh_on_every_os() {
        let (p, a) = editor_command("mercury", "/home/m/repo", Os::Linux, None).unwrap();
        assert_eq!(p, "code");
        assert_eq!(a, ["--remote", "ssh-remote+mercury", "/home/m/repo"]);
        let (p, a) = editor_command("mercury", "/home/m/repo", Os::Windows, None).unwrap();
        assert_eq!(p, "cmd");
        assert_eq!(
            a,
            [
                "/C",
                "code",
                "--remote",
                "ssh-remote+mercury",
                "/home/m/repo"
            ]
        );
        let (p, a) = editor_command("mercury", "/home/m/my repo", Os::Mac, None).unwrap();
        assert_eq!(p, "open");
        assert_eq!(
            a,
            ["vscode://vscode-remote/ssh-remote+mercury/home/m/my%20repo"]
        );
    }

    #[test]
    fn a_bad_alias_or_folder_never_reaches_the_editor() {
        assert!(editor_command("-oProxyCommand=x", "/home/m", Os::Linux, None).is_err());
        assert!(editor_command("mercury", "relative/path", Os::Linux, None).is_err());
        assert!(editor_command("mercury", "/home/m/a\nb", Os::Mac, None).is_err());
        let e = editor_command("mercury", "/home/m/a&calc", Os::Windows, None).unwrap_err();
        assert_eq!(e.code, codes::E_INVALID);
        assert!(editor_command("mercury", "/home/m/a&b", Os::Linux, None).is_ok());
    }

    #[test]
    fn the_folder_script_asks_the_exact_pane_and_falls_back_to_it() {
        let s = folder_script("dev-foo");
        assert!(
            s.contains(&quote(&crate::tmux::exact_pane("dev-foo"))),
            "{s}"
        );
        assert!(s.contains("rev-parse --show-toplevel"), "{s}");
        assert!(s.contains("|| root=\"$p\""), "{s}");
    }

    #[test]
    fn a_uri_path_keeps_slashes_and_escapes_the_rest() {
        assert_eq!(uri_path("/a b/c#d/é"), "/a%20b/c%23d/%C3%A9");
    }
}
