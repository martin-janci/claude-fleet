//! A warm build for a mission's runs (mission improvements M6, design
//! 2026-10-26 §8): with `orchestrator.build_cache` on, a run's session
//! compiles Rust through `sccache` when its host has it, so a fresh
//! checkout's build reuses what earlier runs compiled. A shared
//! `CARGO_TARGET_DIR` is not used: parallel runs would serialise on cargo's
//! lock. A lane already keeps one warm checkout, so this helps most in
//! `item` execution.
//!
//! The variable goes into the session's environment
//! (`NewSessionArgs::env`, the session-env design 2026-10-11), never into a
//! prompt. A host is asked whether it has `sccache` once an hour; a host
//! that does not answer is taken as not having it.

use crate::shell::quote as shq;
use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a host's answer is kept.
const PROBE_TTL: Duration = Duration::from_secs(3600);

/// The marker the probe prints when the host has `sccache`.
const HAS: &str = "__FLEET_SCCACHE yes";

/// PURE: the probe: prints [`HAS`] when `sccache` is on the host's `PATH`.
pub fn probe_script() -> String {
    format!(
        "if command -v sccache >/dev/null 2>&1; then echo {}; fi\n",
        shq(HAS)
    )
}

/// PURE: the session environment for a host with or without `sccache`.
pub fn env_with(has_sccache: bool) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    if has_sccache {
        env.insert("RUSTC_WRAPPER".to_string(), "sccache".to_string());
    }
    env
}

fn answers() -> &'static Mutex<HashMap<String, (bool, Instant)>> {
    static A: std::sync::OnceLock<Mutex<HashMap<String, (bool, Instant)>>> =
        std::sync::OnceLock::new();
    A.get_or_init(|| Mutex::new(HashMap::new()))
}

/// What a run's session on `host` adds to its environment for a warm
/// build: empty when the host has no `sccache` (or did not answer).
pub async fn env_for(
    ssh: &std::sync::Arc<crate::ssh::SshClient>,
    host: &str,
) -> BTreeMap<String, String> {
    let cached = answers()
        .lock()
        .ok()
        .and_then(|a| a.get(host).copied())
        .filter(|(_, at)| at.elapsed() < PROBE_TTL)
        .map(|(has, _)| has);
    let has = match cached {
        Some(has) => has,
        None => {
            let has =
                crate::service::catalog::inventory::run_host_script(ssh, host, &probe_script())
                    .await
                    .is_ok_and(|out| out.lines().any(|l| l.trim() == HAS));
            if let Ok(mut a) = answers().lock() {
                a.insert(host.to_string(), (has, Instant::now()));
            }
            has
        }
    };
    env_with(has)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_host_with_sccache_gets_the_wrapper() {
        assert_eq!(
            env_with(true),
            BTreeMap::from([("RUSTC_WRAPPER".to_string(), "sccache".to_string())])
        );
        assert!(env_with(false).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn the_probe_names_sccache_and_prints_the_marker_only_when_found() {
        let s = probe_script();
        assert!(s.contains("command -v sccache"), "{s}");
        let out = crate::proc::std_command("bash")
            .arg("-c")
            .arg(format!("PATH=/nonexistent\n{s}"))
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&out.stdout).trim().is_empty());
    }
}
