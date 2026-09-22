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
pub mod report;

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

    /// A source file's production code: everything before its test module.
    pub fn production(src: &str) -> &str {
        match src.find("#[cfg(test)]\nmod tests") {
            Some(i) => &src[..i],
            None => src,
        }
    }

    /// The body of `fn name(`, braces included, found by brace matching.
    /// Panics if there is no such function: a gate must not pass because it
    /// looked at nothing.
    pub fn fn_body<'a>(src: &'a str, name: &str) -> &'a str {
        let start = src
            .find(&format!("fn {name}("))
            .unwrap_or_else(|| panic!("no fn {name} in the scanned source"));
        let open = start + src[start..].find('{').expect("a body");
        let mut depth = 0usize;
        for (i, c) in src[open..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return &src[open..=open + i];
                    }
                }
                _ => {}
            }
        }
        panic!("unbalanced braces in fn {name}");
    }
}
