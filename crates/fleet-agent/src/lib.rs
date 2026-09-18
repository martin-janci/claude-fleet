//! `fleet-agent` — the outbound transport agent.
//!
//! A host the hub cannot reach runs this: it dials `wss://<hub>/agent`, keeps
//! one authenticated connection open, and executes the frames the hub sends
//! (`fleet_proto`). Design:
//! `docs/superpowers/specs/2026-09-18-host-agent-design.md`.
//!
//! A library as well as a binary, so an end-to-end test can link the real
//! agent against the real hub in one process. It still depends on
//! `fleet-proto` and never on `fleet-core`.

pub mod cli;
pub mod config;
pub mod conn;
pub mod exec;
pub mod install;

/// Helpers the unit tests share.
#[cfg(test)]
pub(crate) mod test_util {
    use std::path::Path;
    use std::time::{Duration, Instant};

    /// Far longer than anything waited for here takes; only a hang reaches it.
    const PATIENCE: Duration = Duration::from_secs(60);

    /// The pid a child wrote to `path`, once it has.
    pub async fn wait_for_pid(path: &Path) -> i32 {
        let deadline = Instant::now() + PATIENCE;
        loop {
            if let Ok(text) = std::fs::read_to_string(path) {
                if let Ok(pid) = text.trim().parse() {
                    return pid;
                }
            }
            assert!(Instant::now() < deadline, "no pid in {}", path.display());
            tokio::task::yield_now().await;
        }
    }

    /// Running, and not a zombie waiting for a reaper that may never come in
    /// a container. `kill(pid, 0)` answers for a zombie too, so on Linux its
    /// state is read as well.
    pub fn alive(pid: i32) -> bool {
        // SAFETY: signal 0 only checks that the process exists.
        if unsafe { libc::kill(pid, 0) } != 0 {
            return false;
        }
        match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(stat) => {
                let state = stat.rsplit(')').next().unwrap_or("").trim_start();
                !state.starts_with('Z') && !state.starts_with('X')
            }
            Err(_) => true,
        }
    }
}
