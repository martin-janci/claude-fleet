//! Moving the tmux servers the agent starts out of the agent's own cgroup.
//!
//! The hub starts tmux through the agent (`tmux new-session` inside the
//! `bash -c` scripts `exec` runs), and a tmux server stays in the cgroup of
//! whoever started it: `fleet-agent.service`. `KillMode=process` keeps it
//! alive across a stop or restart, but systemd-oomd kills a whole cgroup
//! regardless of `KillMode`. Every Claude session on the host then died with
//! the agent whenever one of them built something large (mercury, 2026-10-14:
//! one 22.5 GiB unit, seven sessions, a failed task).
//!
//! So, under a `systemd --user` manager, the agent sweeps its own cgroup every
//! [`EVERY`] and asks the manager (D-Bus, through `busctl`) to move:
//!
//! - each tmux server into a scope of its own, `fleet-tmux-<server>.scope`,
//!   next to `fleet-agent.service` in the same slice;
//! - each pane's process tree (the pane's shell or `claude` and everything it
//!   started) into `fleet-tmux-<server>-pane-<root>.scope`.
//!
//! oomd then picks the one pane that is actually growing; the agent, the tmux
//! server and the other sessions stay. Panes a server forks after its move
//! start in the server's scope, so those scopes are swept too. Processes that
//! fork mid-move are caught by the next sweep and attached to the pane's
//! existing scope.
//!
//! Anywhere else — no cgroup v2, a system unit, not under systemd, no
//! `busctl`, or `FLEET_AGENT_TMUX_SCOPES=0` — nothing moves and tmux stays
//! where it was started, as before.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How often the sweep runs: the longest a new session shares the agent's
/// fate.
pub const EVERY: Duration = Duration::from_secs(5);

/// What `/proc/<pid>/comm` holds for a tmux server (tmux sets it with
/// `prctl(PR_SET_NAME)`; a client is `tmux: client`).
const TMUX_SERVER_COMM: &str = "tmux: server";

/// The longest ancestor chain walked from a process to its tmux server.
const MAX_DEPTH: usize = 64;

/// Consecutive failed manager calls after which the sweep gives up.
const MAX_FAILURES: u32 = 3;

/// One process, as the planner needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proc {
    pub ppid: u32,
    pub comm: String,
}

/// Which swept cgroup a process was found in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// `fleet-agent.service` itself.
    Agent,
    /// A `fleet-tmux-<server>.scope` this module made earlier.
    Server,
}

/// What one sweep found: the processes in the swept cgroups, and enough of
/// every process's ancestry to tell which tmux server (if any) it hangs off.
#[derive(Debug, Default)]
pub struct Snapshot {
    pub agent_pid: u32,
    pub swept: Vec<(u32, Source)>,
    pub procs: HashMap<u32, Proc>,
}

/// One call to the manager.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Move {
    pub unit: String,
    pub pids: Vec<u32>,
    /// `StartTransientUnit` (the unit's root process is among `pids`), or
    /// `AttachProcessesToUnit` (the root moved earlier; these are its
    /// descendants that forked in the meantime).
    pub create: bool,
}

/// The scope a tmux server goes to.
pub fn server_unit(server: u32) -> String {
    format!("fleet-tmux-{server}.scope")
}

/// The scope a pane's process tree goes to.
pub fn pane_unit(server: u32, root: u32) -> String {
    format!("fleet-tmux-{server}-pane-{root}.scope")
}

/// The server pid in a server scope's name; `None` for anything else,
/// pane scopes included.
pub fn parse_server_unit(name: &str) -> Option<u32> {
    name.strip_prefix("fleet-tmux-")?
        .strip_suffix(".scope")?
        .parse()
        .ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Target {
    Server(u32),
    Pane { server: u32, root: u32 },
}

/// The moves that put every tmux server and pane tree in `snap` into its own
/// scope. Pure: the sweep reads the snapshot and runs the moves.
pub fn plan(snap: &Snapshot) -> Vec<Move> {
    let is_server = |pid: u32| {
        snap.procs
            .get(&pid)
            .is_some_and(|p| p.comm == TMUX_SERVER_COMM)
    };
    let mut groups: BTreeMap<Target, Vec<u32>> = BTreeMap::new();
    for &(pid, source) in &snap.swept {
        if pid == snap.agent_pid {
            continue;
        }
        if is_server(pid) {
            // A server in its own scope is where it belongs.
            if source == Source::Agent {
                groups.entry(Target::Server(pid)).or_default().push(pid);
            }
            continue;
        }
        // Up the chain to the server; `cur` ends as the pane's root, the
        // server's direct child. A chain that reaches the agent is one of
        // `exec`'s own children, which stay.
        let mut cur = pid;
        let mut target = None;
        for _ in 0..MAX_DEPTH {
            let Some(parent) = snap.procs.get(&cur).map(|p| p.ppid) else {
                break;
            };
            if parent <= 1 || parent == snap.agent_pid {
                break;
            }
            if is_server(parent) {
                target = Some(Target::Pane {
                    server: parent,
                    root: cur,
                });
                break;
            }
            cur = parent;
        }
        if let Some(target) = target {
            groups.entry(target).or_default().push(pid);
        }
    }
    groups
        .into_iter()
        .map(|(target, mut pids)| {
            pids.sort_unstable();
            pids.dedup();
            let (unit, root) = match target {
                Target::Server(s) => (server_unit(s), s),
                Target::Pane { server, root } => (pane_unit(server, root), root),
            };
            Move {
                create: pids.contains(&root),
                unit,
                pids,
            }
        })
        .collect()
}

/// The `busctl` argv for `mv`. `slice` is the slice the agent itself runs
/// in, so a new scope lands next to `fleet-agent.service` where the next
/// sweep finds it.
pub fn busctl_args(mv: &Move, slice: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = [
        "--user",
        "--quiet",
        "call",
        "org.freedesktop.systemd1",
        "/org/freedesktop/systemd1",
        "org.freedesktop.systemd1.Manager",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let pids = |args: &mut Vec<String>| {
        args.push(mv.pids.len().to_string());
        args.extend(mv.pids.iter().map(u32::to_string));
    };
    if mv.create {
        args.extend(
            ["StartTransientUnit", "ssa(sv)a(sa(sv))", &mv.unit, "fail"]
                .iter()
                .map(|s| s.to_string()),
        );
        let props = 4 + usize::from(slice.is_some());
        args.push(props.to_string());
        args.extend(["PIDs", "au"].iter().map(|s| s.to_string()));
        pids(&mut args);
        if let Some(slice) = slice {
            args.extend(["Slice", "s", slice].iter().map(|s| s.to_string()));
        }
        // Without it the manager refuses `AttachProcessesToUnit` ("Process
        // migration not available on non-delegated units"), and a straggler
        // could never follow its pane.
        args.extend(["Delegate", "b", "true"].iter().map(|s| s.to_string()));
        // Gone as soon as its last process is, failed or not.
        args.extend(
            ["CollectMode", "s", "inactive-or-failed"]
                .iter()
                .map(|s| s.to_string()),
        );
        args.extend(
            [
                "Description",
                "s",
                "claude-fleet tmux, moved out of fleet-agent.service",
            ]
            .iter()
            .map(|s| s.to_string()),
        );
        // No auxiliary units.
        args.push("0".to_string());
    } else {
        args.extend(
            ["AttachProcessesToUnit", "ssau", &mv.unit, ""]
                .iter()
                .map(|s| s.to_string()),
        );
        pids(&mut args);
    }
    args
}

/// Where the agent runs, when it runs somewhere the sweep can act.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Home {
    /// The agent's cgroup directory under `/sys/fs/cgroup`.
    pub cgroup: PathBuf,
    /// The slice that cgroup sits in (`app.slice`), when it is one.
    pub slice: Option<String>,
}

/// The agent's home from its `/proc/self/cgroup`: a cgroup v2 `.service`
/// under a user manager (`/user@<uid>.service/`), else `None`.
pub fn home_from(proc_self_cgroup: &str, root: &Path) -> Option<Home> {
    let path = proc_self_cgroup
        .lines()
        .find_map(|l| l.strip_prefix("0::"))?
        .trim();
    let unit = path.rsplit('/').next()?;
    if !unit.ends_with(".service") || !path.contains("/user@") {
        return None;
    }
    let parent = path.rsplit_once('/')?.0;
    let slice = parent
        .rsplit('/')
        .next()
        .filter(|s| s.ends_with(".slice"))
        .map(str::to_string);
    Some(Home {
        cgroup: root.join(path.trim_start_matches('/')),
        slice,
    })
}

/// `/proc/<pid>/stat`'s parent pid and command name. The name sits in
/// parentheses and may hold either, so the fields after it are found from
/// the last `)`.
pub fn parse_stat(stat: &str) -> Option<Proc> {
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    let comm = stat.get(open + 1..close)?.to_string();
    let mut rest = stat.get(close + 1..)?.split_whitespace();
    let _state = rest.next()?;
    let ppid = rest.next()?.parse().ok()?;
    Some(Proc { ppid, comm })
}

fn read_pids(dir: &Path) -> Vec<u32> {
    std::fs::read_to_string(dir.join("cgroup.procs"))
        .map(|t| t.lines().filter_map(|l| l.trim().parse().ok()).collect())
        .unwrap_or_default()
}

/// Read the swept cgroups and every swept process's ancestry.
fn snapshot(home: &Home) -> Snapshot {
    let mut snap = Snapshot {
        agent_pid: std::process::id(),
        ..Snapshot::default()
    };
    snap.swept.extend(
        read_pids(&home.cgroup)
            .into_iter()
            .map(|p| (p, Source::Agent)),
    );
    if let Some(Ok(entries)) = home.cgroup.parent().map(std::fs::read_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            if name.to_str().and_then(parse_server_unit).is_some() {
                snap.swept.extend(
                    read_pids(&entry.path())
                        .into_iter()
                        .map(|p| (p, Source::Server)),
                );
            }
        }
    }
    let mut todo: Vec<u32> = snap.swept.iter().map(|&(p, _)| p).collect();
    while let Some(pid) = todo.pop() {
        if pid <= 1 || snap.procs.contains_key(&pid) || snap.procs.len() > 100_000 {
            continue;
        }
        let Some(proc) = std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .as_deref()
            .and_then(parse_stat)
        else {
            continue;
        };
        todo.push(proc.ppid);
        snap.procs.insert(pid, proc);
    }
    snap
}

fn call(mv: &Move, slice: Option<&str>) -> Result<(), String> {
    let out = std::process::Command::new("busctl")
        .args(busctl_args(mv, slice))
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("could not run busctl: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Is the sweep switched off by `FLEET_AGENT_TMUX_SCOPES`?
pub fn disabled_by(value: Option<&str>) -> bool {
    matches!(value.map(str::trim), Some("0" | "false" | "off" | "no"))
}

/// Sweep forever, or return at once where there is nothing to do (see the
/// module docs). Gives up after [`MAX_FAILURES`] manager calls in a row
/// fail: a manager that refuses is not going to start agreeing.
pub async fn run() {
    if disabled_by(std::env::var("FLEET_AGENT_TMUX_SCOPES").ok().as_deref()) {
        tracing::info!("[agent] tmux scopes off (FLEET_AGENT_TMUX_SCOPES)");
        return;
    }
    let Some(home) = std::fs::read_to_string("/proc/self/cgroup")
        .ok()
        .and_then(|t| home_from(&t, Path::new("/sys/fs/cgroup")))
    else {
        tracing::debug!(
            "[agent] not a systemd --user service on cgroup v2; tmux stays in the agent's cgroup"
        );
        return;
    };
    tracing::info!(
        cgroup = %home.cgroup.display(),
        "[agent] moving the tmux servers it starts into scopes of their own"
    );
    let mut failures = 0u32;
    loop {
        let h = home.clone();
        let failed = tokio::task::spawn_blocking(move || sweep(&h))
            .await
            .unwrap_or(0);
        failures = if failed == 0 { 0 } else { failures + failed };
        if failures >= MAX_FAILURES {
            tracing::warn!(
                "[agent] the user manager refused {failures} scope moves in a row; \
                 tmux stays in the agent's cgroup until the agent restarts"
            );
            return;
        }
        tokio::time::sleep(EVERY).await;
    }
}

/// One sweep; returns how many moves failed. A pid that exited between the
/// read and the call fails its move, so a failure only counts when the unit
/// was being created and its root is still alive.
fn sweep(home: &Home) -> u32 {
    let mut failed = 0;
    for mv in plan(&snapshot(home)) {
        match call(&mv, home.slice.as_deref()) {
            Ok(()) => tracing::info!(
                unit = %mv.unit,
                pids = ?mv.pids,
                created = mv.create,
                "[agent] moved tmux out of the agent's cgroup"
            ),
            Err(e) => {
                let gone = mv
                    .pids
                    .iter()
                    .any(|p| !Path::new(&format!("/proc/{p}")).exists());
                tracing::warn!(unit = %mv.unit, pids = ?mv.pids, error = %e, "[agent] scope move failed");
                if !gone {
                    failed += 1;
                }
            }
        }
    }
    failed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(ppid: u32, comm: &str) -> Proc {
        Proc {
            ppid,
            comm: comm.into(),
        }
    }

    /// The agent (100), its exec child `bash` (101); a tmux server (200,
    /// reparented to the user manager, 50) with two panes: 300 → 301 → 302
    /// and 400.
    fn host() -> HashMap<u32, Proc> {
        HashMap::from([
            (50, proc(1, "systemd")),
            (100, proc(50, "fleet-agent")),
            (101, proc(100, "bash")),
            (200, proc(50, TMUX_SERVER_COMM)),
            (300, proc(200, "claude")),
            (301, proc(300, "cargo")),
            (302, proc(301, "rustc")),
            (400, proc(200, "bash")),
        ])
    }

    #[test]
    fn a_fresh_server_and_each_pane_get_scopes_of_their_own() {
        let snap = Snapshot {
            agent_pid: 100,
            swept: [100, 101, 200, 300, 301, 302, 400]
                .into_iter()
                .map(|p| (p, Source::Agent))
                .collect(),
            procs: host(),
        };
        assert_eq!(
            plan(&snap),
            vec![
                Move {
                    unit: "fleet-tmux-200.scope".into(),
                    pids: vec![200],
                    create: true
                },
                Move {
                    unit: "fleet-tmux-200-pane-300.scope".into(),
                    pids: vec![300, 301, 302],
                    create: true
                },
                Move {
                    unit: "fleet-tmux-200-pane-400.scope".into(),
                    pids: vec![400],
                    create: true
                },
            ]
        );
    }

    /// The agent itself and what `exec` is running stay: a `bash -c` from
    /// the hub is not a session.
    #[test]
    fn the_agent_and_its_exec_children_stay() {
        let snap = Snapshot {
            agent_pid: 100,
            swept: vec![(100, Source::Agent), (101, Source::Agent)],
            procs: host(),
        };
        assert!(plan(&snap).is_empty());
    }

    /// A pane the server forked after its own move starts in the server's
    /// scope; it moves to a pane scope, and the server stays put.
    #[test]
    fn a_pane_born_in_the_server_scope_moves_and_the_server_stays() {
        let snap = Snapshot {
            agent_pid: 100,
            swept: vec![(200, Source::Server), (400, Source::Server)],
            procs: host(),
        };
        assert_eq!(
            plan(&snap),
            vec![Move {
                unit: "fleet-tmux-200-pane-400.scope".into(),
                pids: vec![400],
                create: true
            }]
        );
    }

    /// A child forked while its pane root was moving is left behind; the
    /// next sweep attaches it to the pane's existing scope.
    #[test]
    fn a_straggler_is_attached_to_its_pane_s_existing_scope() {
        let snap = Snapshot {
            agent_pid: 100,
            swept: vec![(302, Source::Agent)],
            procs: host(),
        };
        let moves = plan(&snap);
        assert_eq!(
            moves,
            vec![Move {
                unit: "fleet-tmux-200-pane-300.scope".into(),
                pids: vec![302],
                create: false
            }]
        );
        assert_eq!(
            busctl_args(&moves[0], Some("app.slice"))[6..],
            [
                "AttachProcessesToUnit",
                "ssau",
                "fleet-tmux-200-pane-300.scope",
                "",
                "1",
                "302"
            ]
        );
    }

    /// An orphan (its server gone, reparented to the manager) and a process
    /// whose ancestry could not be read are left alone.
    #[test]
    fn a_process_with_no_server_above_it_stays() {
        let mut procs = host();
        procs.insert(500, proc(50, "sleep"));
        let snap = Snapshot {
            agent_pid: 100,
            swept: vec![(500, Source::Agent), (999, Source::Agent)],
            procs,
        };
        assert!(plan(&snap).is_empty());
    }

    #[test]
    fn a_new_scope_is_started_in_the_agent_s_slice_and_collected_when_empty() {
        let mv = Move {
            unit: "fleet-tmux-200.scope".into(),
            pids: vec![200],
            create: true,
        };
        let args = busctl_args(&mv, Some("app.slice"));
        assert_eq!(
            args[..6],
            [
                "--user",
                "--quiet",
                "call",
                "org.freedesktop.systemd1",
                "/org/freedesktop/systemd1",
                "org.freedesktop.systemd1.Manager"
            ]
        );
        assert_eq!(
            args[6..],
            [
                "StartTransientUnit",
                "ssa(sv)a(sa(sv))",
                "fleet-tmux-200.scope",
                "fail",
                "5",
                "PIDs",
                "au",
                "1",
                "200",
                "Slice",
                "s",
                "app.slice",
                "Delegate",
                "b",
                "true",
                "CollectMode",
                "s",
                "inactive-or-failed",
                "Description",
                "s",
                "claude-fleet tmux, moved out of fleet-agent.service",
                "0"
            ]
        );
        // No slice: the property count drops with the property.
        let args = busctl_args(&mv, None);
        assert_eq!(args[10], "4");
        assert!(!args.contains(&"Slice".to_string()));
    }

    #[test]
    fn only_a_user_service_on_cgroup_v2_is_swept() {
        let root = Path::new("/sys/fs/cgroup");
        let user =
            "0::/user.slice/user-1000.slice/user@1000.service/app.slice/fleet-agent.service\n";
        assert_eq!(
            home_from(user, root),
            Some(Home {
                cgroup: PathBuf::from(
                    "/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service/app.slice/fleet-agent.service"
                ),
                slice: Some("app.slice".into()),
            })
        );
        // Straight under the manager: no slice to name.
        let bare = "0::/user.slice/user-1000.slice/user@1000.service/fleet-agent.service\n";
        assert_eq!(home_from(bare, root).unwrap().slice, None);
        // A system unit (the system manager would need privileges), a shell
        // in a login session, cgroup v1.
        for text in [
            "0::/system.slice/fleet-agent.service\n",
            "0::/user.slice/user-1000.slice/session-4.scope\n",
            "12:pids:/user.slice\n1:name=systemd:/user.slice/fleet-agent.service\n",
            "",
        ] {
            assert_eq!(home_from(text, root), None, "{text:?}");
        }
    }

    #[test]
    fn stat_is_read_from_the_last_parenthesis() {
        assert_eq!(
            parse_stat("200 (tmux: server) S 50 200 200 0 -1"),
            Some(proc(50, TMUX_SERVER_COMM))
        );
        assert_eq!(parse_stat("7 (a) b (c) R 3 7 7"), Some(proc(3, "a) b (c")));
        assert_eq!(parse_stat("garbage"), None);
    }

    #[test]
    fn only_server_scopes_are_swept() {
        assert_eq!(parse_server_unit("fleet-tmux-200.scope"), Some(200));
        assert_eq!(parse_server_unit("fleet-tmux-200-pane-300.scope"), None);
        assert_eq!(parse_server_unit("fleet-agent.service"), None);
        assert_eq!(parse_server_unit(&server_unit(7)), Some(7));
    }

    #[test]
    fn the_switch_turns_it_off_only_when_said_so() {
        for off in ["0", "false", "off", "no", " 0 "] {
            assert!(disabled_by(Some(off)), "{off:?}");
        }
        for on in [None, Some("1"), Some(""), Some("yes")] {
            assert!(!disabled_by(on), "{on:?}");
        }
    }
}
