//! The fleet-agent install job (Orbit Fleet 4.9): a hub installs its agent
//! on a host it reaches over SSH today, then moves the host onto the agent.
//! What `docs/hub.md` → *Set it up* tells an operator to do by hand, as one
//! job a person starts from the wizard or the API and watches step by step.
//!
//! The steps, each a row update on `agent_installs`:
//!
//! 1. `target`: `uname` picks the release tarball (x86_64 or aarch64 Linux).
//! 2. `download`: the host fetches the tarball and `SHA256SUMS` itself,
//!    checks the sum and installs the binary as `~/.local/bin/fleet-agent`.
//! 3. `start`: the host's token goes in on **stdin** — never argv, never a
//!    file the script names in its text — to `fleet-agent install --user`
//!    where systemd runs, else to a token file (mode 0600) and
//!    `fleet-agent run` under `nohup` (no supervisor: it will not come back
//!    after a reboot, and the job says so). `FLEET_AGENT_NO_SYSTEMD=1` in
//!    the host's login environment takes the second path on a systemd host
//!    too (hub-e2e sets it, so a CI runner never gets a user unit).
//! 4. `connect`: the host's transport becomes `agent`, and the job waits for
//!    the agent's hello. No hello in [`CONNECT_WAIT`]: the transport goes
//!    back to `ssh`, so the host is never left unreachable by a failed job.
//!
//! Only a hub accepts agents; the desktop refuses the job up front.
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::agent::AgentRegistry;
use crate::ipc_error::{codes, IpcError};
use crate::shell::quote as shq;
use crate::ssh::SshExec;
use crate::store::{AgentInstallRow, Store};

/// Where release tarballs come from, `{version}` filled in. `FLEET_AGENT_DIST`
/// replaces the whole base (a mirror, or a `file://` directory in tests).
pub const DIST_ENV: &str = "FLEET_AGENT_DIST";
const RELEASES: &str = "https://github.com/martin-janci/claude-fleet/releases/download/v";

/// How long the job waits for the agent's hello once the host is moved
/// onto it. The agent's reconnect back-off tops out at 60 s.
pub const CONNECT_WAIT: Duration = Duration::from_secs(120);

/// A job still `running` this long after it started has lost the process
/// that ran it.
pub const STALE_AFTER_SECS: i64 = 30 * 60;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const DOWNLOAD_WALL: Duration = Duration::from_secs(300);
const STEP_WALL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars", rename = "InstallAgentParams")]
pub struct InstallAgentArgs {
    /// The host to install on; it must be reachable over SSH now.
    pub alias: String,
    /// The URL the agent dials. Default: this hub's public URL.
    #[serde(default)]
    pub hub_url: Option<String>,
    /// The release to install. Default: this hub's own version.
    #[serde(default)]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars", rename = "AgentInstallsParams")]
pub struct AgentInstallsArgs {
    /// Only this host's jobs.
    #[serde(default)]
    pub alias: Option<String>,
}

/// The release tarball's target triple for `uname -s`/`uname -m`.
pub fn target_for(uname: &str) -> Option<&'static str> {
    let mut it = uname.split_whitespace();
    let (os, arch) = (it.next()?, it.next()?);
    if os != "Linux" {
        return None;
    }
    match arch {
        "x86_64" | "amd64" => Some("x86_64-unknown-linux-gnu"),
        "aarch64" | "arm64" => Some("aarch64-unknown-linux-gnu"),
        _ => None,
    }
}

/// The release base URL for `version`.
pub fn dist_base(version: &str) -> String {
    match std::env::var(DIST_ENV) {
        Ok(v) if !v.trim().is_empty() => v.trim().trim_end_matches('/').to_string(),
        _ => format!("{RELEASES}{version}"),
    }
}

/// Step 2's script: fetch, check the sum, install. Every value is quoted.
pub fn download_script(base: &str, version: &str, target: &str) -> String {
    format!(
        "set -e\n\
         v={v}; t={t}; base={b}\n\
         f=\"fleet-agent-$v-$t.tar.gz\"\n\
         command -v curl >/dev/null 2>&1 || {{ echo 'error=curl is not installed'; exit 4; }}\n\
         d=$(mktemp -d); trap 'rm -rf \"$d\"' EXIT; cd \"$d\"\n\
         curl -fsSL -o \"$f\" \"$base/$f\" || {{ echo \"error=could not download $f\"; exit 4; }}\n\
         curl -fsSL -o SHA256SUMS \"$base/SHA256SUMS\" || {{ echo 'error=could not download SHA256SUMS'; exit 4; }}\n\
         want=$(awk -v f=\"$f\" '$2==f || $2==\"*\"f {{print $1}}' SHA256SUMS)\n\
         got=$(sha256sum \"$f\" | cut -d' ' -f1)\n\
         [ -n \"$want\" ] && [ \"$want\" = \"$got\" ] || {{ echo \"error=checksum of $f does not match SHA256SUMS\"; exit 4; }}\n\
         tar xzf \"$f\"\n\
         mkdir -p \"$HOME/.local/bin\"\n\
         install -m 0755 \"fleet-agent-$v-$t/fleet-agent\" \"$HOME/.local/bin/fleet-agent\"\n\
         printf 'installed=%s\\n' \"$(\"$HOME/.local/bin/fleet-agent\" --version 2>/dev/null)\"\n",
        v = shq(version),
        t = shq(target),
        b = shq(base),
    )
}

/// Step 3's script. The token is read from stdin; `insecure` only for a
/// plain-http hub on loopback (the agent refuses it anywhere else).
pub fn start_script(hub_url: &str, insecure: bool) -> String {
    let ins = if insecure { " --insecure" } else { "" };
    format!(
        "set -e\n\
         umask 077\n\
         bin=\"$HOME/.local/bin/fleet-agent\"; hub={h}\n\
         if [ -z \"${{FLEET_AGENT_NO_SYSTEMD:-}}\" ] && [ -d /run/systemd/system ] \
            && command -v systemctl >/dev/null 2>&1; then\n\
           \"$bin\" install --user --hub \"$hub\"{ins} --token-file - >/dev/null\n\
           loginctl enable-linger \"$(id -un)\" >/dev/null 2>&1 || true\n\
           echo started=systemd\n\
         else\n\
           cfg=\"${{XDG_CONFIG_HOME:-$HOME/.config}}/fleet-agent\"; mkdir -p \"$cfg\"\n\
           cat > \"$cfg/token\"\n\
           if [ -f \"$cfg/agent.pid\" ]; then kill \"$(cat \"$cfg/agent.pid\")\" 2>/dev/null || true; fi\n\
           nohup \"$bin\" run --hub \"$hub\"{ins} --token-file \"$cfg/token\" >\"$cfg/agent.log\" 2>&1 </dev/null &\n\
           echo $! > \"$cfg/agent.pid\"\n\
           echo started=nohup\n\
         fi\n",
        h = shq(hub_url),
    )
}

/// `true` for a plain-http URL on loopback; `Err` for plain http anywhere
/// else, which the agent would refuse (the token would cross in clear).
pub fn needs_insecure(hub_url: &str) -> Result<bool, IpcError> {
    let Some(rest) = hub_url
        .strip_prefix("http://")
        .or_else(|| hub_url.strip_prefix("ws://"))
    else {
        return Ok(false);
    };
    let authority = rest.split('/').next().unwrap_or("");
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(""),
        None => authority.rsplit_once(':').map_or(authority, |(h, _)| h),
    };
    if host == "localhost" || host == "::1" || host.starts_with("127.") {
        Ok(true)
    } else {
        Err(IpcError::new(
            codes::E_INVALID,
            format!(
                "{hub_url} is plain http on a host that is not loopback: the agent refuses it, \
                 because its token would cross the network in clear. Give the hub an https URL"
            ),
        ))
    }
}

fn line_value<'a>(out: &'a str, key: &str) -> Option<&'a str> {
    out.lines()
        .find_map(|l| l.strip_prefix(key).and_then(|r| r.strip_prefix('=')))
        .map(str::trim)
}

fn lock(store: &Mutex<Store>) -> Result<std::sync::MutexGuard<'_, Store>, IpcError> {
    store
        .lock()
        .map_err(|_| IpcError::new(codes::E_INTERNAL, "store lock poisoned"))
}

/// What a started job needs, resolved before anything is written.
struct Plan {
    alias: String,
    ssh_alias: String,
    version: String,
    hub_url: String,
    insecure: bool,
}

fn plan(store: &Mutex<Store>, args: &InstallAgentArgs) -> Result<Plan, IpcError> {
    crate::validate::host_alias(&args.alias)?;
    let s = lock(store)?;
    let host = s
        .list_hosts()?
        .into_iter()
        .find(|h| h.alias == args.alias)
        .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("no host named {}", args.alias)))?;
    if host.alias == crate::service::projects::LOCAL_HOST {
        return Err(IpcError::new(
            codes::E_INVALID,
            "the hub's own machine needs no agent",
        ));
    }
    if host.transport == "agent" {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("{} is already an agent host", host.alias),
        ));
    }
    crate::service::trackers::admin::refuse_agent_transport_on_tracker_host(&s, &host.alias)?;
    if let Some(running) = s
        .agent_installs(Some(&host.alias), 1)?
        .into_iter()
        .find(|j| j.state == "running")
    {
        return Err(IpcError::new(
            codes::E_CONFLICT,
            format!(
                "an install on {} is already running (job {})",
                host.alias, running.id
            ),
        ));
    }
    let hub_url = match args
        .hub_url
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
    {
        Some(u) => u.trim_end_matches('/').to_string(),
        None => crate::service::hub::HubBase::read(&s)?.url,
    };
    let insecure = needs_insecure(&hub_url)?;
    let version = args
        .version
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or(crate::app_version::get())
        .trim_start_matches('v')
        .to_string();
    if !version
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'))
    {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("not a version: {version:?}"),
        ));
    }
    Ok(Plan {
        ssh_alias: host.ssh_alias.unwrap_or_else(|| host.alias.clone()),
        alias: host.alias,
        version,
        hub_url,
        insecure,
    })
}

/// Start a job and return its row; the job runs on in the background.
pub fn start(
    store: Arc<Mutex<Store>>,
    ssh: Arc<dyn SshExec>,
    registry: Option<Arc<AgentRegistry>>,
    args: InstallAgentArgs,
) -> Result<AgentInstallRow, IpcError> {
    let Some(registry) = registry else {
        return Err(IpcError::new(
            codes::E_UNSUPPORTED,
            "only a hub accepts fleet-agent connections; this app reaches its hosts over SSH",
        ));
    };
    let plan = plan(&store, &args)?;
    let row = lock(&store)?.insert_agent_install(&plan.alias, &plan.version)?;
    let id = row.id;
    tokio::spawn(async move {
        let outcome = run(&store, &*ssh, &registry, id, &plan, CONNECT_WAIT).await;
        if let Ok(s) = store.lock() {
            let _ = match outcome {
                Ok(detail) => s.finish_agent_install(id, "done", &detail),
                Err(detail) => s.finish_agent_install(id, "failed", &detail),
            };
        }
    });
    Ok(row)
}

/// The jobs, newest first; one left running by a process that is gone is
/// failed on the way out.
pub fn list(store: &Mutex<Store>, alias: Option<&str>) -> Result<Vec<AgentInstallRow>, IpcError> {
    let s = lock(store)?;
    s.fail_stale_agent_installs(
        crate::store::now_unix() - STALE_AFTER_SECS,
        "interrupted: the hub stopped while it ran; start it again",
    )?;
    Ok(s.agent_installs(alias, 20)?)
}

fn step(store: &Mutex<Store>, id: i64, step: &str, detail: &str) {
    if let Ok(s) = store.lock() {
        let _ = s.set_agent_install_step(id, step, Some(detail));
    }
}

/// The job body. `Ok` is the done line, `Err` the failed one.
async fn run(
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    registry: &AgentRegistry,
    id: i64,
    plan: &Plan,
    wait: Duration,
) -> Result<String, String> {
    let host = plan.ssh_alias.as_str();
    let fail = |what: &str, e: IpcError| format!("{what}: {}", e.message);

    // 1. target
    step(store, id, "target", "reading the host's platform");
    let out = crate::ssh::run_shell_bounded(
        ssh,
        host,
        "printf '%s %s\\n' \"$(uname -s)\" \"$(uname -m)\"",
        CONNECT_TIMEOUT,
        STEP_WALL,
    )
    .await
    .map_err(|e| fail("could not reach the host over SSH", e))?;
    let uname = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let target = target_for(&uname).ok_or_else(|| {
        format!("no fleet-agent release for {uname:?}: releases are for x86_64 and aarch64 Linux")
    })?;

    // 2. download
    step(
        store,
        id,
        "download",
        &format!("fetching fleet-agent {} for {target}", plan.version),
    );
    let out = crate::ssh::run_shell_bounded(
        ssh,
        host,
        &download_script(&dist_base(&plan.version), &plan.version, target),
        CONNECT_TIMEOUT,
        DOWNLOAD_WALL,
    )
    .await
    .map_err(|e| fail("download failed", e))?;
    let text = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() {
        return Err(line_value(&text, "error")
            .map(str::to_string)
            .unwrap_or_else(|| {
                format!(
                    "download failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                )
            }));
    }

    // 3. start, the token on stdin
    let token = {
        let s = lock(store).map_err(|e| e.message)?;
        match s.get_host_token(&plan.alias).map_err(|e| e.message)? {
            Some(t) if t.mode == "full" => t.token,
            _ => {
                let token = crate::mcp::generate_token();
                s.upsert_host_token(&plan.alias, &token)
                    .map_err(|e| e.message)?;
                token
            }
        }
    };
    step(store, id, "start", "starting the agent");
    let script = start_script(&plan.hub_url, plan.insecure);
    let quoted = shq(&script);
    let out = ssh
        .run_with_stdin(
            host,
            &["bash", "-lc", &quoted],
            format!("{token}\n").into_bytes(),
            CONNECT_TIMEOUT,
            STEP_WALL,
            64 * 1024,
        )
        .await
        .map_err(|e| fail("could not start the agent", e))?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "could not start the agent: {}",
            err.lines()
                .last()
                .unwrap_or("it exited without saying why")
                .trim()
        ));
    }
    let supervised = line_value(&text, "started") == Some("systemd");

    // 4. connect
    step(store, id, "connect", "waiting for the agent to connect");
    lock(store)
        .and_then(|s| s.set_host_transport(&plan.alias, "agent"))
        .map_err(|e| e.message)?;
    let deadline = tokio::time::Instant::now() + wait;
    while !registry.connected(&plan.alias) {
        if tokio::time::Instant::now() >= deadline {
            // Never leave the host unreachable: back onto SSH.
            let _ = lock(store).and_then(|s| s.set_host_transport(&plan.alias, "ssh"));
            return Err(format!(
                "the agent did not connect within {} s; the host is back on SSH. On the host, \
                 `fleet-agent status --user` (or ~/.config/fleet-agent/agent.log) says why",
                wait.as_secs()
            ));
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let version = registry
        .snapshot()
        .into_iter()
        .find(|a| a.alias == plan.alias)
        .map(|a| a.agent_version)
        .unwrap_or_else(|| plan.version.clone());
    Ok(if supervised {
        format!("connected · fleet-agent {version} · systemd user unit")
    } else {
        format!(
            "connected · fleet-agent {version} · no systemd on this host: it runs under nohup \
             and will not come back after a reboot"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_are_the_two_linux_releases() {
        assert_eq!(target_for("Linux x86_64"), Some("x86_64-unknown-linux-gnu"));
        assert_eq!(
            target_for("Linux aarch64"),
            Some("aarch64-unknown-linux-gnu")
        );
        assert_eq!(target_for("Darwin arm64"), None);
        assert_eq!(target_for("Linux riscv64"), None);
        assert_eq!(target_for(""), None);
    }

    #[test]
    fn plain_http_is_for_loopback_only() {
        assert!(!needs_insecure("https://fleet.example.com").unwrap());
        assert!(needs_insecure("http://127.0.0.1:8080").unwrap());
        assert!(needs_insecure("http://localhost:1/x").unwrap());
        assert!(needs_insecure("http://[::1]:9").unwrap());
        assert!(needs_insecure("http://fleet.lan:8080").is_err());
    }

    /// A release directory the script can fetch from with `file://`, holding
    /// a stand-in `fleet-agent` that records how it was started.
    #[cfg(unix)]
    fn fake_release(dir: &std::path::Path, version: &str, target: &str, good_sum: bool) {
        let name = format!("fleet-agent-{version}-{target}");
        let inner = dir.join(&name);
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::write(
            inner.join("fleet-agent"),
            "#!/bin/sh\n\
             if [ \"$1\" = --version ]; then echo \"fleet-agent 9.9.9\"; exit 0; fi\n\
             echo \"$@\" > \"$HOME/agent.args\"\n\
             if [ \"$1\" = run ]; then while :; do sleep 1; done; fi\n\
             cat > \"$HOME/agent.stdin\"\n",
        )
        .unwrap();
        let tar = format!("{name}.tar.gz");
        let st = std::process::Command::new("tar")
            .args(["czf", &tar, &name])
            .current_dir(dir)
            .status()
            .unwrap();
        assert!(st.success());
        let sum = std::process::Command::new("sha256sum")
            .arg(&tar)
            .current_dir(dir)
            .output()
            .unwrap();
        let mut line = String::from_utf8_lossy(&sum.stdout).to_string();
        if !good_sum {
            line = format!("{}{}", "0".repeat(64), &line[64..]);
        }
        std::fs::write(dir.join("SHA256SUMS"), line).unwrap();
    }

    #[cfg(unix)]
    fn bash(script: &str, home: &std::path::Path, stdin: &str) -> std::process::Output {
        use std::io::Write;
        let mut child = std::process::Command::new("bash")
            .args(["-c", script])
            .env("HOME", home)
            .env_remove("XDG_CONFIG_HOME")
            // Never a user unit (or linger) on the machine running the tests.
            .env("FLEET_AGENT_NO_SYSTEMD", "1")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn the_scripts_download_check_install_and_start_with_the_token_on_stdin() {
        let dist = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let t = "x86_64-unknown-linux-gnu";
        fake_release(dist.path(), "9.9.9", t, true);
        let base = format!("file://{}", dist.path().display());

        let out = bash(&download_script(&base, "9.9.9", t), home.path(), "");
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "{text} {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(line_value(&text, "installed"), Some("fleet-agent 9.9.9"));
        assert!(home.path().join(".local/bin/fleet-agent").exists());

        let script = start_script("http://127.0.0.1:9", true);
        assert!(!script.contains("tok-secret"));
        let out = bash(&script, home.path(), "tok-secret\n");
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "{text} {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(line_value(&text, "started"), Some("nohup"));
        {
            let cfg = home.path().join(".config/fleet-agent");
            assert_eq!(
                std::fs::read_to_string(cfg.join("token")).unwrap(),
                "tok-secret\n"
            );
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(cfg.join("token"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "the token file is private");
            let pid: i32 = std::fs::read_to_string(cfg.join("agent.pid"))
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            // The stand-in records its argv once it runs.
            let args = home.path().join("agent.args");
            for _ in 0..50 {
                if args.exists() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            let argv = std::fs::read_to_string(&args).unwrap();
            assert!(argv.starts_with("run --hub http://127.0.0.1:9 --insecure --token-file "));
            assert!(!argv.contains("tok-secret"), "the token is never in argv");
            let _ = std::process::Command::new("kill")
                .arg(pid.to_string())
                .status();
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_tarball_whose_sum_does_not_match_is_not_installed() {
        let dist = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let t = "aarch64-unknown-linux-gnu";
        fake_release(dist.path(), "1.0.0", t, false);
        let base = format!("file://{}", dist.path().display());
        let out = bash(&download_script(&base, "1.0.0", t), home.path(), "");
        assert!(!out.status.success());
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(line_value(&text, "error").unwrap().contains("checksum"));
        assert!(!home.path().join(".local/bin/fleet-agent").exists());
    }

    fn store_with_ssh_host() -> Arc<Mutex<Store>> {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("mercury", Some("mercury")).unwrap();
        Arc::new(Mutex::new(s))
    }

    #[tokio::test]
    async fn the_desktop_refuses_and_a_bad_request_writes_nothing() {
        let store = store_with_ssh_host();
        let ssh: Arc<dyn SshExec> = Arc::new(crate::ssh_fake::FakeSsh::new());
        let args = |alias: &str, url: Option<&str>| InstallAgentArgs {
            alias: alias.into(),
            hub_url: url.map(str::to_string),
            version: Some("0.5.4".into()),
        };
        let e = start(
            store.clone(),
            ssh.clone(),
            None,
            args("mercury", Some("https://h")),
        )
        .unwrap_err();
        assert_eq!(e.code, codes::E_UNSUPPORTED);
        let reg = Some(AgentRegistry::new());
        let e = start(
            store.clone(),
            ssh.clone(),
            reg.clone(),
            args("venus", Some("https://h")),
        )
        .unwrap_err();
        assert_eq!(e.code, codes::E_NOTFOUND);
        let e = start(
            store.clone(),
            ssh.clone(),
            reg.clone(),
            args("mercury", Some("http://fleet.lan")),
        )
        .unwrap_err();
        assert_eq!(e.code, codes::E_INVALID);
        assert!(store
            .lock()
            .unwrap()
            .agent_installs(None, 9)
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn no_hello_puts_the_host_back_on_ssh() {
        let store = store_with_ssh_host();
        let fake = crate::ssh_fake::FakeSsh::new();
        fake.set_default(crate::ssh_fake::Reply::ok(
            "Linux x86_64\ninstalled=fleet-agent 0.5.4\nstarted=nohup\n",
        ));
        let fake = Arc::new(fake);
        let fake_calls = fake.clone();
        let ssh: Arc<dyn SshExec> = fake;
        let registry = AgentRegistry::new();
        let p = plan(
            &store,
            &InstallAgentArgs {
                alias: "mercury".into(),
                hub_url: Some("https://fleet.example.com".into()),
                version: Some("0.5.4".into()),
            },
        )
        .unwrap();
        let id = store
            .lock()
            .unwrap()
            .insert_agent_install("mercury", "0.5.4")
            .unwrap()
            .id;
        let err = run(&store, &*ssh, &registry, id, &p, Duration::from_millis(50))
            .await
            .unwrap_err();
        assert!(err.contains("back on SSH"), "{err}");
        let s = store.lock().unwrap();
        let host = s
            .list_hosts()
            .unwrap()
            .into_iter()
            .find(|h| h.alias == "mercury")
            .unwrap();
        assert_eq!(host.transport, "ssh");
        assert_eq!(s.agent_install(id).unwrap().unwrap().step, "connect");
        // The token the agent was given is the host's full token.
        assert_eq!(s.get_host_token("mercury").unwrap().unwrap().mode, "full");
        let token = s.get_host_token("mercury").unwrap().unwrap().token;
        drop(s);
        // ...and it went in on stdin, never in a command line.
        let calls = fake_calls.calls();
        assert!(calls.iter().all(|c| !c.command().contains(&token)));
        assert!(calls
            .iter()
            .any(|c| c.stdin_str().is_some_and(|i| i.trim() == token)));
    }
}
