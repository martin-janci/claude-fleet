//! Shell terminals (redesign step 5.3): a session's terminals 0..N beside
//! its agent. Each is a tmux session of its own, `<name>--sh<N>`
//! (`tmux::shell_terminal_name`), never a window or pane of the agent's
//! session, so `exact_pane(name)` — the target every prompt, capture and
//! respawn uses — always lands on the agent and a terminal can never take
//! keys meant for it. Every session list leaves them out
//! (`tmux::parse_sessions_checked`), so reconcile and discover never see a
//! terminal as a session or a ghost of one.
//!
//! Closing a terminal never stops the session; killing the session closes
//! its terminals, and renaming it renames them (`lifecycle.rs`).

use super::*;
use crate::ipc_error::{codes, lock};
use crate::tmux::{
    close_shell_terminal_script, open_shell_terminal_script, shell_terminal_commands_in,
    shell_terminal_name, LIST_SESSION_COMMANDS_SCRIPT, MAX_SHELL_TERMINALS,
    SHELL_TERMINAL_NO_SESSION,
};

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, rmcp::schemars::JsonSchema,
)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "snake_case")]
pub enum ShellTerminalAction {
    // Plain comments, not docs: a documented variant turns the served
    // schema into a `oneOf` that costs every client ~500 bytes.
    // The session's open terminals.
    #[default]
    List,
    // Open terminal `n`, or the lowest free one when `n` is not given.
    Open,
    // Close terminal `n`.
    Close,
}

/// Where `open` starts a new terminal, on the session's own host (the
/// strip's "New terminal opens on" picker).
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, rmcp::schemars::JsonSchema,
)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "snake_case")]
pub enum ShellTerminalStart {
    // The agent pane's current directory: the session's worktree.
    #[default]
    Worktree,
    // The login user's home directory on the session's host.
    Home,
}

impl ShellTerminalStart {
    fn is_worktree(&self) -> bool {
        *self == ShellTerminalStart::Worktree
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellTerminalsArgs {
    pub session_id: i64,
    #[serde(default)]
    pub action: ShellTerminalAction,
    #[serde(default)]
    pub n: Option<u32>,
    /// `open` only: where the new terminal starts. Left off the wire when it
    /// is the default, so a call that does not pick says exactly what it
    /// said before the picker existed.
    #[serde(default, skip_serializing_if = "ShellTerminalStart::is_worktree")]
    pub at: ShellTerminalStart,
}

/// One open terminal: its number, the tmux session a terminal pane
/// attaches to, and what runs in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellTerminal {
    pub n: u32,
    pub tmux_name: String,
    /// What runs in the front of the terminal (tmux's
    /// `pane_current_command`): its shell when it is idle, `node` while
    /// `pnpm dev` runs. Absent from an older hub, or when tmux said nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellTerminalsResult {
    pub session_id: i64,
    pub host_alias: String,
    /// The session's open terminals after the action, by number.
    pub terminals: Vec<ShellTerminal>,
    /// The terminal an `open` opened (or found already open).
    #[serde(default)]
    pub opened: Option<u32>,
}

/// List, open or close a session's shell terminals.
pub async fn shell_terminals(
    args: ShellTerminalsArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<ShellTerminalsResult, IpcError> {
    let row = lock(store)?
        .get_session_by_id(args.session_id)?
        .ok_or_else(|| {
            IpcError::new(
                codes::E_NOTFOUND,
                format!("session {} not found", args.session_id),
            )
        })?;
    let tmux = exec_for(&row.host_alias, ssh);
    shell_terminals_with(args, &row, &*tmux).await
}

/// [`shell_terminals`] for a row already read, with its tmux executor as a
/// parameter.
pub(crate) async fn shell_terminals_with(
    args: ShellTerminalsArgs,
    row: &SessionRow,
    tmux: &dyn TmuxExec,
) -> Result<ShellTerminalsResult, IpcError> {
    if crate::store::has_no_pane(&row.kind) {
        return Err(IpcError::new(
            codes::E_VALIDATE,
            format!(
                "session {} has no tmux pane to open a terminal beside",
                row.id
            ),
        ));
    }
    let name = row.tmux_name.as_str();
    let mut opened = None;
    match args.action {
        ShellTerminalAction::List => {}
        ShellTerminalAction::Open => {
            if matches!(row.status.as_str(), "dead" | "stopped") {
                return Err(IpcError::new(
                    codes::E_INVALID_STATE,
                    format!("session {} is not running", row.id),
                ));
            }
            let n = match args.n {
                Some(n) => valid_n(n)?,
                None => {
                    let open: Vec<u32> = list(tmux, name)
                        .await?
                        .into_iter()
                        .map(|(n, _)| n)
                        .collect();
                    next_free(&open).ok_or_else(|| {
                        IpcError::new(
                            codes::E_VALIDATE,
                            format!(
                                "session {} already has {MAX_SHELL_TERMINALS} terminals open",
                                row.id
                            ),
                        )
                    })?
                }
            };
            let out = tmux
                .run_script(&open_shell_terminal_script(
                    name,
                    n,
                    args.at == ShellTerminalStart::Home,
                ))
                .await?;
            if out.status.code() == Some(SHELL_TERMINAL_NO_SESSION) {
                return Err(IpcError::new(
                    codes::E_INVALID_STATE,
                    format!(
                        "session {} has no live tmux session on {}",
                        row.id, row.host_alias
                    ),
                ));
            }
            check(&out)?;
            opened = Some(n);
        }
        ShellTerminalAction::Close => {
            let n = valid_n(args.n.ok_or_else(|| {
                IpcError::new(codes::E_VALIDATE, "close needs the terminal's number, n")
            })?)?;
            check(
                &tmux
                    .run_script(&close_shell_terminal_script(name, n))
                    .await?,
            )?;
        }
    }
    let terminals = list(tmux, name)
        .await?
        .into_iter()
        .map(|(n, command)| ShellTerminal {
            n,
            tmux_name: shell_terminal_name(name, n),
            command,
        })
        .collect();
    Ok(ShellTerminalsResult {
        session_id: row.id,
        host_alias: row.host_alias.clone(),
        terminals,
        opened,
    })
}

fn valid_n(n: u32) -> Result<u32, IpcError> {
    if (1..=MAX_SHELL_TERMINALS).contains(&n) {
        Ok(n)
    } else {
        Err(IpcError::new(
            codes::E_VALIDATE,
            format!("a terminal's number is 1 to {MAX_SHELL_TERMINALS}, not {n}"),
        ))
    }
}

/// The lowest terminal number not in `open`. PURE.
pub fn next_free(open: &[u32]) -> Option<u32> {
    (1..=MAX_SHELL_TERMINALS).find(|n| !open.contains(n))
}

/// The session's open terminals by number, each with what runs in it.
async fn list(tmux: &dyn TmuxExec, session: &str) -> Result<Vec<(u32, Option<String>)>, IpcError> {
    let out = tmux.run_script(LIST_SESSION_COMMANDS_SCRIPT).await?;
    check(&out)?;
    Ok(shell_terminal_commands_in(
        session,
        &String::from_utf8_lossy(&out.stdout),
    ))
}

fn check(out: &std::process::Output) -> Result<(), IpcError> {
    if out.status.success() {
        Ok(())
    } else {
        Err(IpcError::new(
            codes::E_TMUX,
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ))
    }
}

/// Best-effort: close every terminal of a session that was just killed.
pub(super) async fn close_all_after_kill(tmux: &dyn TmuxExec, session: &str) {
    let script = crate::tmux::close_all_shell_terminals_script(session);
    if let Err(e) = tmux.run_script(&script).await {
        tracing::debug!(session, error = %e, "[terminals] closing a killed session's terminals failed");
    }
}

/// Best-effort: carry a renamed session's terminals over to its new name.
pub(super) async fn rename_after_rename(tmux: &dyn TmuxExec, old: &str, new: &str) {
    let script = crate::tmux::rename_shell_terminals_script(old, new);
    if let Err(e) = tmux.run_script(&script).await {
        tracing::debug!(old, new, error = %e, "[terminals] renaming a session's terminals failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tmux::{
        close_all_shell_terminals_script, is_shell_terminal_name, parse_shell_terminal_name,
        rename_shell_terminals_script, shell_terminals_in, LIST_SESSION_NAMES_SCRIPT,
    };

    #[test]
    fn a_terminal_name_round_trips_and_nothing_else_parses() {
        assert_eq!(shell_terminal_name("api", 3), "api--sh3");
        assert_eq!(parse_shell_terminal_name("api--sh3"), Some(("api", 3)));
        assert_eq!(parse_shell_terminal_name("a--b--sh12"), Some(("a--b", 12)));
        for not in [
            "api",
            "--sh1",
            "api--sh",
            "api--sh0",
            "api--sh01",
            "api--sh123",
            "api--shx",
            "api-sh1",
        ] {
            assert!(!is_shell_terminal_name(not), "{not}");
        }
    }

    #[test]
    fn next_free_takes_the_lowest_gap_and_stops_at_the_cap() {
        assert_eq!(next_free(&[]), Some(1));
        assert_eq!(next_free(&[1, 2, 4]), Some(3));
        assert_eq!(
            next_free(&(1..=MAX_SHELL_TERMINALS).collect::<Vec<_>>()),
            None
        );
    }

    #[test]
    fn terminals_are_picked_out_by_their_own_session_only() {
        let names = "api\napi--sh2\napi--sh1\napi-2--sh1\nweb--sh1\napi--sh1\n";
        assert_eq!(shell_terminals_in("api", names), vec![1, 2]);
        assert_eq!(shell_terminals_in("api-2", names), vec![1]);
        assert!(shell_terminals_in("nope", names).is_empty());
    }

    /// What runs in each shell (M15 G4.4): the command comes first, so a
    /// `|` in either half still finds the terminal, and another session's
    /// terminals (or the agent's own row) never leak in.
    #[test]
    fn each_terminal_carries_what_runs_in_it() {
        let lines = "zsh|api\nnode|api--sh2\nzsh|api--sh1\nvim|api-2--sh1\n\
                     a|b|api--sh3\n|api--sh4\nzsh|a|pi--sh5\nnode|api--sh2\n";
        assert_eq!(
            shell_terminal_commands_in("api", lines),
            vec![
                (1, Some("zsh".into())),
                (2, Some("node".into())),
                (3, Some("a|b".into())),
                (4, None),
            ]
        );
        assert_eq!(
            shell_terminal_commands_in("a|pi", lines),
            vec![(5, Some("zsh".into()))]
        );
        assert_eq!(
            shell_terminal_commands_in("api-2", lines),
            vec![(1, Some("vim".into()))]
        );
        assert!(shell_terminal_commands_in("nope", lines).is_empty());
    }

    /// The list a strip reads, through the service and a fake tmux: each
    /// terminal with its command, and none on the wire when tmux said none.
    #[tokio::test]
    async fn the_list_names_what_runs_in_each_terminal() {
        use crate::ssh_fake::{FakeSsh, Match, Reply};
        let fake = FakeSsh::new();
        fake.on_host(
            "h",
            Match::script_contains("pane_current_command"),
            Reply::ok("zsh|api\nnode|api--sh1\n|api--sh2\n"),
        );
        let tmux = crate::tmux::RemoteTmux {
            client: fake.clone(),
            host: "h".into(),
        };
        let store = Store::open_in_memory().unwrap();
        store.upsert_host("h").unwrap();
        let id = store
            .upsert_session("api", "h", None, None, 1, 1, "running", None)
            .unwrap();
        let row = store.get_session_by_id(id).unwrap().unwrap();
        let got = shell_terminals_with(
            ShellTerminalsArgs {
                session_id: row.id,
                action: ShellTerminalAction::List,
                n: None,
                at: ShellTerminalStart::Worktree,
            },
            &row,
            &tmux,
        )
        .await
        .unwrap();
        assert_eq!(
            got.terminals,
            vec![
                ShellTerminal {
                    n: 1,
                    tmux_name: "api--sh1".into(),
                    command: Some("node".into()),
                },
                ShellTerminal {
                    n: 2,
                    tmux_name: "api--sh2".into(),
                    command: None,
                },
            ]
        );
        let wire = serde_json::to_value(&got.terminals).unwrap();
        assert_eq!(wire[0]["command"], "node");
        assert!(wire[1].get("command").is_none(), "{wire}");
    }

    #[test]
    fn a_terminal_name_is_refused_as_a_session_name() {
        assert!(crate::validate::tmux_name("api--sh1").is_err());
        assert!(crate::validate::tmux_name("api--shell").is_ok());
    }

    /// The pass that ghosts vanished rows and adopts unknown ones never sees
    /// a terminal: `parse_sessions_checked` (every list's parser) leaves them
    /// out, and a host running nothing BUT terminals still parses as a
    /// healthy, empty one rather than an unreadable answer.
    #[test]
    fn shells_never_become_ghost_rows() {
        let line = |n: &str| format!("{n}|1|1|0|/w|%1");
        let input = [line("api"), line("api--sh1"), line("api--sh2")].join("\n");
        let got = crate::tmux::parse_sessions_for_test(&input).unwrap();
        assert_eq!(
            got.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            ["api"]
        );
        let only = [line("api--sh1")].join("\n");
        assert!(crate::tmux::parse_sessions_for_test(&only)
            .unwrap()
            .is_empty());
    }

    /// The same through a whole reconcile pass over a host whose tmux runs
    /// a session and two of its terminals: one row, the session's, and it
    /// stays one row across passes; no terminal is adopted, ghosted or
    /// listed.
    #[tokio::test]
    async fn reconcile_keeps_one_row_for_a_session_with_terminals() {
        use crate::ssh_fake::{FakeSsh, Match, Reply};
        let store = Mutex::new(Store::open_in_memory().unwrap());
        store.lock().unwrap().upsert_host("h").unwrap();
        let line = |n: &str| format!("{n}|1|1|0|/w|%1");
        let snapshot = format!(
            "---FLEET:identity\nboot=b\ntmuxrc=0\ntmuxout=1\n---FLEET:sessions\nrc=0\n{}\n{}\n{}\n\
             ---FLEET:account\n\n---FLEET:panes\n---FLEET:pane api\n$ \n---FLEET:end\n",
            line("api"),
            line("api--sh1"),
            line("api--sh2"),
        );
        let fake = FakeSsh::new();
        fake.on_host(
            "h",
            Match::script_contains("---FLEET:identity"),
            Reply::ok(&snapshot),
        );
        let exec_fake = fake.clone();
        let deps = Arc::new(ReconcileDeps::fake(
            move |alias| {
                Box::new(crate::tmux::RemoteTmux {
                    client: exec_fake.clone(),
                    host: alias.to_string(),
                })
            },
            std::time::Duration::from_secs(5),
        ));
        for _ in 0..3 {
            reconcile_sessions_with(&store, &deps).await.unwrap();
        }
        let rows = store.lock().unwrap().list_all_sessions().unwrap();
        let on_h: Vec<_> = rows
            .iter()
            .filter(|r| r.host_alias == "h")
            .map(|r| (r.tmux_name.as_str(), r.status.as_str()))
            .collect();
        assert_eq!(on_h.len(), 1, "{on_h:?}");
        assert_eq!(on_h[0].0, "api");
        assert!(
            rows.iter().all(|r| !is_shell_terminal_name(&r.tmux_name)),
            "{rows:?}"
        );
    }

    // ── The scripts, against a real tmux server on a socket of its own ──

    struct Tmux {
        sock: std::path::PathBuf,
    }

    impl Tmux {
        fn new() -> Option<Self> {
            std::process::Command::new("tmux").arg("-V").output().ok()?;
            let sock = std::path::PathBuf::from(format!(
                "/tmp/cf-sh-{}",
                &uuid::Uuid::new_v4().simple().to_string()[..12]
            ));
            std::fs::create_dir_all(&sock).unwrap();
            Some(Self { sock })
        }
        fn sh(&self, script: &str) -> std::process::Output {
            std::process::Command::new("bash")
                .arg("-c")
                .arg(script)
                .env("TMUX_TMPDIR", &self.sock)
                .env("SHELL", "/bin/sh")
                .env_remove("TMUX")
                .output()
                .unwrap()
        }
        fn names(&self) -> Vec<String> {
            let out = self.sh(LIST_SESSION_NAMES_SCRIPT);
            let mut v: Vec<String> = String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(str::to_string)
                .collect();
            v.sort();
            v
        }
    }

    impl Drop for Tmux {
        fn drop(&mut self) {
            self.sh("tmux kill-server 2>/dev/null");
            let _ = std::fs::remove_dir_all(&self.sock);
        }
    }

    #[test]
    fn open_close_kill_and_rename_scripts_drive_a_real_tmux() {
        let Some(t) = Tmux::new() else { return };
        let dir = tempfile::tempdir().unwrap();
        let d = quote(dir.path().to_str().unwrap());
        // `x--sh1` belongs to another session and must survive everything.
        assert!(t
            .sh(&format!(
                "tmux new-session -d -s 'my api' -c {d} 'sleep 60' && tmux new-session -d -s x--sh1 'sleep 60'"
            ))
            .status
            .success());

        // No agent session: refused with its own exit code, nothing opened.
        let gone = t.sh(&open_shell_terminal_script("nope", 1, false));
        assert_eq!(gone.status.code(), Some(SHELL_TERMINAL_NO_SESSION));

        let open = t.sh(&open_shell_terminal_script("my api", 1, false));
        assert!(
            open.status.success(),
            "{}",
            String::from_utf8_lossy(&open.stderr)
        );
        // Opening an open terminal is a no-op, not a "duplicate session".
        assert!(t
            .sh(&open_shell_terminal_script("my api", 1, false))
            .status
            .success());
        assert!(t
            .sh(&open_shell_terminal_script("my api", 2, false))
            .status
            .success());
        assert_eq!(
            t.names(),
            ["my api", "my api--sh1", "my api--sh2", "x--sh1"]
        );
        // Started in the agent pane's directory.
        let cwd = t.sh("tmux display-message -p -t '=my api--sh1:' '#{pane_current_path}'");
        assert_eq!(
            std::fs::canonicalize(String::from_utf8_lossy(&cwd.stdout).trim()).unwrap(),
            std::fs::canonicalize(dir.path()).unwrap()
        );
        // "Opens on: Home folder" starts it in $HOME instead.
        let home = tempfile::tempdir().unwrap();
        let in_home = std::process::Command::new("bash")
            .arg("-c")
            .arg(open_shell_terminal_script("my api", 3, true))
            .env("TMUX_TMPDIR", &t.sock)
            .env("SHELL", "/bin/sh")
            .env("HOME", home.path())
            .env_remove("TMUX")
            .output()
            .unwrap();
        assert!(
            in_home.status.success(),
            "{}",
            String::from_utf8_lossy(&in_home.stderr)
        );
        let cwd3 = t.sh("tmux display-message -p -t '=my api--sh3:' '#{pane_current_path}'");
        assert_eq!(
            std::fs::canonicalize(String::from_utf8_lossy(&cwd3.stdout).trim()).unwrap(),
            std::fs::canonicalize(home.path()).unwrap()
        );
        assert!(t
            .sh(&close_shell_terminal_script("my api", 3))
            .status
            .success());

        assert!(t
            .sh(&close_shell_terminal_script("my api", 2))
            .status
            .success());
        assert!(t
            .sh(&close_shell_terminal_script("my api", 2))
            .status
            .success());
        assert_eq!(t.names(), ["my api", "my api--sh1", "x--sh1"]);

        assert!(t
            .sh("tmux rename-session -t '=my api' web")
            .status
            .success());
        assert!(t
            .sh(&rename_shell_terminals_script("my api", "web"))
            .status
            .success());
        assert_eq!(t.names(), ["web", "web--sh1", "x--sh1"]);

        assert!(t
            .sh(&close_all_shell_terminals_script("web"))
            .status
            .success());
        assert_eq!(t.names(), ["web", "x--sh1"]);
    }
}
