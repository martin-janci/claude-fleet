//! Single-instance guard: terminate other running copies of the app.

/// One entry of the process list, as `kill_other_instances` sees it.
#[derive(Debug, Clone, Copy)]
struct ProcEntry<'a> {
    pid: u32,
    /// Executable file name (falls back to the process name).
    name: &'a str,
    /// On Linux sysinfo lists every thread (`/proc/<pid>/task/<tid>`) as a
    /// process of its own, carrying the same exe name as its process. A thread
    /// is never an instance: its process is listed separately.
    is_thread: bool,
}

/// Pure: given the running processes, our own pid, the thread ids of our own
/// process and our own exe file name, return the pids of *other* instances of
/// this app — same executable file name, a real process (not a thread), and
/// not us. Lifted out of `kill_other_instances` so the decision is
/// unit-testable without touching real processes.
///
/// Before threads were skipped, the Linux build SIGTERMed its own threads and
/// died silently within 0.2 s of starting (v0.6.4 `.deb`).
fn instances_to_kill(
    procs: &[ProcEntry<'_>],
    my_pid: u32,
    my_tasks: &[u32],
    my_name: &str,
) -> Vec<u32> {
    procs
        .iter()
        .filter(|p| {
            p.pid != my_pid && !p.is_thread && !my_tasks.contains(&p.pid) && p.name == my_name
        })
        .map(|p| p.pid)
        .collect()
}

/// Terminate every other running instance of this app before we open the DB or
/// bind the MCP port. Matches by executable file name, so it catches *all*
/// builds (dev `target/debug/claude-fleet`, release bundle, other worktrees).
/// SIGTERM first so the other instance can release its SSH ControlMasters and
/// flush SQLite, then SIGKILL any straggler after a short grace window.
pub(crate) fn kill_other_instances() {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, Signal, System};

    let my_pid = std::process::id();
    let my_name = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
    let Some(my_name) = my_name else {
        tracing::warn!("could not resolve own exe name; skipping instance reaper");
        return;
    };

    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());

    // Collect (pid, exe file name, is-thread) for every process sysinfo can see.
    let procs: Vec<(u32, String, bool)> = sys
        .processes()
        .iter()
        .map(|(pid, proc_)| {
            let name = proc_
                .exe()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| proc_.name().to_string_lossy().into_owned());
            (pid.as_u32(), name, proc_.thread_kind().is_some())
        })
        .collect();
    // Our own threads, in case sysinfo ever lists one without a thread kind.
    let my_tasks: Vec<u32> = sys
        .process(Pid::from_u32(my_pid))
        .and_then(|p| p.tasks())
        .map(|t| t.iter().map(|pid| pid.as_u32()).collect())
        .unwrap_or_default();

    let entries: Vec<ProcEntry<'_>> = procs
        .iter()
        .map(|(pid, name, is_thread)| ProcEntry {
            pid: *pid,
            name: name.as_str(),
            is_thread: *is_thread,
        })
        .collect();
    let targets = instances_to_kill(&entries, my_pid, &my_tasks, &my_name);
    if targets.is_empty() {
        return;
    }

    for pid in &targets {
        if let Some(proc_) = sys.process(Pid::from_u32(*pid)) {
            proc_.kill_with(Signal::Term);
            tracing::info!("sent SIGTERM to prior instance pid {pid}");
        }
    }

    // Poll up to ~500ms for graceful exit.
    for _ in 0..10 {
        std::thread::sleep(std::time::Duration::from_millis(50));
        sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing(),
        );
        if targets
            .iter()
            .all(|pid| sys.process(Pid::from_u32(*pid)).is_none())
        {
            return;
        }
    }

    // SIGKILL whatever is left.
    for pid in &targets {
        if let Some(proc_) = sys.process(Pid::from_u32(*pid)) {
            proc_.kill();
            tracing::warn!("SIGKILLed unresponsive prior instance pid {pid}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, name: &str) -> ProcEntry<'_> {
        ProcEntry {
            pid,
            name,
            is_thread: false,
        }
    }

    fn thread(pid: u32, name: &str) -> ProcEntry<'_> {
        ProcEntry {
            pid,
            name,
            is_thread: true,
        }
    }

    #[test]
    fn instances_to_kill_excludes_self_even_with_matching_name() {
        let procs = [p(100, "claude-fleet")];
        assert!(instances_to_kill(&procs, 100, &[], "claude-fleet").is_empty());
    }

    #[test]
    fn instances_to_kill_picks_other_same_named_process() {
        let procs = [p(100, "claude-fleet"), p(200, "claude-fleet")];
        assert_eq!(
            instances_to_kill(&procs, 100, &[], "claude-fleet"),
            vec![200]
        );
    }

    #[test]
    fn instances_to_kill_ignores_other_names() {
        let procs = [p(100, "claude-fleet"), p(200, "node"), p(300, "tmux")];
        assert!(instances_to_kill(&procs, 100, &[], "claude-fleet").is_empty());
    }

    #[test]
    fn instances_to_kill_handles_empty_list() {
        let procs: [ProcEntry<'_>; 0] = [];
        assert!(instances_to_kill(&procs, 100, &[], "claude-fleet").is_empty());
    }

    /// The Linux self-kill: sysinfo lists our own threads as processes with
    /// our exe name. None of them may be signalled.
    #[test]
    fn instances_to_kill_never_targets_our_own_threads() {
        let procs = [
            p(100, "claude-fleet"),
            thread(101, "claude-fleet"),
            thread(102, "claude-fleet"),
        ];
        assert!(instances_to_kill(&procs, 100, &[100, 101, 102], "claude-fleet").is_empty());
    }

    /// Belt and braces: a task of ours listed without a thread kind is still ours.
    #[test]
    fn instances_to_kill_skips_our_tasks_even_unmarked() {
        let procs = [p(100, "claude-fleet"), p(101, "claude-fleet")];
        assert!(instances_to_kill(&procs, 100, &[100, 101], "claude-fleet").is_empty());
    }

    /// Another instance is still found, and its threads are left out: the
    /// signal goes to its process, never once per thread.
    #[test]
    fn instances_to_kill_targets_other_process_not_its_threads() {
        let procs = [
            p(100, "claude-fleet"),
            thread(101, "claude-fleet"),
            p(200, "claude-fleet"),
            thread(201, "claude-fleet"),
        ];
        assert_eq!(
            instances_to_kill(&procs, 100, &[100, 101], "claude-fleet"),
            vec![200]
        );
    }
}
