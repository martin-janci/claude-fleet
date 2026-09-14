//! Single-instance guard: terminate other running copies of the app.

/// Pure: given the running processes as `(pid, exe_file_name)` pairs, our own
/// pid and our own exe file name, return the pids of *other* instances of this
/// app — same executable file name, different pid. Lifted out of
/// `kill_other_instances` so the decision is unit-testable without touching
/// real processes.
fn instances_to_kill(procs: &[(u32, &str)], my_pid: u32, my_name: &str) -> Vec<u32> {
    procs
        .iter()
        .filter(|(pid, name)| *pid != my_pid && *name == my_name)
        .map(|(pid, _)| *pid)
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

    // Collect (pid, exe file name) for every process sysinfo can see.
    let procs: Vec<(u32, String)> = sys
        .processes()
        .iter()
        .map(|(pid, proc_)| {
            let name = proc_
                .exe()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| proc_.name().to_string_lossy().into_owned());
            (pid.as_u32(), name)
        })
        .collect();

    let proc_refs: Vec<(u32, &str)> = procs
        .iter()
        .map(|(pid, name)| (*pid, name.as_str()))
        .collect();
    let targets = instances_to_kill(&proc_refs, my_pid, &my_name);
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

    #[test]
    fn instances_to_kill_excludes_self_even_with_matching_name() {
        let procs = [(100u32, "claude-fleet")];
        assert!(instances_to_kill(&procs, 100, "claude-fleet").is_empty());
    }

    #[test]
    fn instances_to_kill_picks_other_same_named_process() {
        let procs = [(100u32, "claude-fleet"), (200u32, "claude-fleet")];
        assert_eq!(instances_to_kill(&procs, 100, "claude-fleet"), vec![200]);
    }

    #[test]
    fn instances_to_kill_ignores_other_names() {
        let procs = [(100u32, "claude-fleet"), (200u32, "node"), (300u32, "tmux")];
        assert!(instances_to_kill(&procs, 100, "claude-fleet").is_empty());
    }

    #[test]
    fn instances_to_kill_handles_empty_list() {
        let procs: [(u32, &str); 0] = [];
        assert!(instances_to_kill(&procs, 100, "claude-fleet").is_empty());
    }
}
