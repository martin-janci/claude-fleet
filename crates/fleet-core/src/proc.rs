//! Building a child process: the one place that creates a `Command`, so every
//! child the app starts gets the same platform treatment.
//!
//! On Windows the release desktop is a GUI-subsystem process with no console
//! (`windows_subsystem = "windows"`). A console program it spawns — `ssh.exe`
//! for every probe the reconcile tick runs, `git.exe` for the catalog — gets a
//! console window of its own and flashes it on screen, unless it is created
//! with `CREATE_NO_WINDOW`. Pipes are unaffected, so stdin/stdout/stderr work
//! as before. Everywhere else these are plain `Command::new`.
//!
//! `no_eprintln_tests::production_code_spawns_through_proc` keeps production
//! code from calling `Command::new` directly. The interactive terminal is not
//! built here: it runs through portable-pty's ConPTY, which owns its console.

use std::ffi::OsStr;

/// `CREATE_NO_WINDOW` from the Win32 process-creation flags.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A `tokio::process::Command` for `program` that opens no console window.
pub fn command(program: impl AsRef<OsStr>) -> tokio::process::Command {
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut cmd = tokio::process::Command::new(program);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

/// A `std::process::Command` for `program` that opens no console window.
pub fn std_command(program: impl AsRef<OsStr>) -> std::process::Command {
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut cmd = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exit_3() -> (&'static str, &'static [&'static str]) {
        if cfg!(windows) {
            ("cmd", &["/C", "exit 3"])
        } else {
            ("sh", &["-c", "exit 3"])
        }
    }

    #[test]
    fn the_std_builder_spawns_a_working_child() {
        let (program, args) = exit_3();
        let out = std_command(program).args(args).output().unwrap();
        assert_eq!(out.status.code(), Some(3));
    }

    #[tokio::test]
    async fn the_tokio_builder_spawns_a_working_child() {
        let (program, args) = exit_3();
        let out = command(program).args(args).output().await.unwrap();
        assert_eq!(out.status.code(), Some(3));
    }
}
