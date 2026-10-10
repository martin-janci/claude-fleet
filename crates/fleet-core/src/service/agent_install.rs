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
    /// The host; reachable over SSH now.
    pub alias: String,
    /// The URL the agent dials (default: this hub's public URL).
    #[serde(default)]
    pub hub_url: Option<String>,
    /// The release (default: this hub's version).
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

/// The release tarball's target triple for `uname -s`/`uname -m`: the
/// first line that reads as one, since `bash -lc` prints whatever a chatty
/// login profile says around the answer.
pub fn target_for(uname: &str) -> Option<&'static str> {
    uname.lines().find_map(|line| {
        let mut it = line.split_whitespace();
        let (os, arch) = (it.next()?, it.next()?);
        if os != "Linux" || it.next().is_some() {
            return None;
        }
        match arch {
            "x86_64" | "amd64" => Some("x86_64-unknown-linux-gnu"),
            "aarch64" | "arm64" => Some("aarch64-unknown-linux-gnu"),
            _ => None,
        }
    })
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
           old=$(cat \"$cfg/agent.pid\" 2>/dev/null || true)\n\
           case \"$old\" in ''|*[!0-9]*) ;; *) \
             case \"$(tr '\\0' ' ' < \"/proc/$old/cmdline\" 2>/dev/null)\" in \
               *fleet-agent*) kill \"$old\" 2>/dev/null || true ;; esac ;; esac\n\
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

/// Make sure the host has tmux, which every fleet session runs in (review
/// r09/r18): present, it says `tmux=present`; absent, it installs it with
/// the first package manager it finds that can run without a prompt —
/// Homebrew as the user, else apt-get, dnf, yum, zypper or apk as root or
/// through `sudo -n` (never a password prompt) — and says `tmux=installed`.
/// When none can, it prints `error=<why, naming tmux>` and exits 4.
pub fn tmux_script() -> String {
    r#"if command -v tmux >/dev/null 2>&1; then printf 'tmux=present\n'; exit 0; fi
if [ "$(id -u)" = 0 ]; then S=''; elif command -v sudo >/dev/null 2>&1 && sudo -n true >/dev/null 2>&1; then S='sudo -n'; else S=none; fi
ok=1
if command -v brew >/dev/null 2>&1; then brew install tmux </dev/null >/dev/null 2>&1 && ok=0
fi
if [ "$ok" != 0 ] && [ "$S" != none ]; then
  if command -v apt-get >/dev/null 2>&1; then
    { $S env DEBIAN_FRONTEND=noninteractive apt-get install -y -q tmux || { $S apt-get update -q && $S env DEBIAN_FRONTEND=noninteractive apt-get install -y -q tmux; }; } </dev/null >/dev/null 2>&1 && ok=0
  elif command -v dnf >/dev/null 2>&1; then $S dnf install -y -q tmux </dev/null >/dev/null 2>&1 && ok=0
  elif command -v yum >/dev/null 2>&1; then $S yum install -y -q tmux </dev/null >/dev/null 2>&1 && ok=0
  elif command -v zypper >/dev/null 2>&1; then $S zypper --non-interactive install tmux </dev/null >/dev/null 2>&1 && ok=0
  elif command -v apk >/dev/null 2>&1; then $S apk add --no-progress tmux </dev/null >/dev/null 2>&1 && ok=0
  fi
fi
if [ "$ok" = 0 ] && command -v tmux >/dev/null 2>&1; then printf 'tmux=installed\n'; exit 0; fi
if [ "$S" = none ]; then
  printf 'error=tmux is not installed on this host, and it cannot be installed without a password (no root, no passwordless sudo, no Homebrew): install tmux there, then run the install again\n'
else
  printf 'error=tmux is not installed on this host, and no package manager here installed it: install tmux there, then run the install again\n'
fi
exit 4
"#
    .to_string()
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

#[cfg(test)]
fn plan(store: &Mutex<Store>, args: &InstallAgentArgs) -> Result<Plan, IpcError> {
    plan_in(&*lock(store)?, args)
}

/// Plan the job and write its `running` row under ONE store guard, so the
/// "already running" check and the insert are atomic: two concurrent
/// installs on a host cannot both pass the check (r18).
fn plan_and_claim(
    store: &Mutex<Store>,
    args: &InstallAgentArgs,
) -> Result<(Plan, AgentInstallRow), IpcError> {
    let s = lock(store)?;
    let plan = plan_in(&s, args)?;
    let row = s.insert_agent_install(&plan.alias, &plan.version)?;
    Ok((plan, row))
}

fn plan_in(s: &Store, args: &InstallAgentArgs) -> Result<Plan, IpcError> {
    crate::validate::host_alias(&args.alias)?;
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
    crate::service::trackers::admin::refuse_agent_transport_on_tracker_host(s, &host.alias)?;
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
        None => {
            // A loopback default would point the agent at its own machine:
            // it reaches the hub only through a provisioning tunnel, which
            // the hub stops keeping once the host is an agent host (r18-A1).
            let base = crate::service::hub::HubBase::read(s)?;
            if !base.public {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    "an agent dials the hub, so the hub needs a public URL: set hub.public_url, or pass hub_url",
                ));
            }
            base.url
        }
    };
    // A readonly token is refused at the agent's upgrade; minting over it
    // would also cut off the hooks already provisioned with it (r18-A2).
    if let Some(t) = s.get_host_token(&host.alias)? {
        if t.mode != "full" {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!(
                    "{}'s control-API token is {}; an agent needs a full one, so set its mode to full first",
                    host.alias, t.mode
                ),
            ));
        }
    }
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
    let (plan, row) = plan_and_claim(&store, &args)?;
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

/// At process start: every job still `running` was run by a process that is
/// gone (a job lives in the process that started it), so none of them may
/// hold off a new install for [`STALE_AFTER_SECS`].
///
/// A job cut off in its `connect` step had already put the host on
/// `transport=agent` (review r18 A3), and the revert its wait would have
/// made on a timeout never ran: the host is put back on SSH, the transport
/// it had before (an install refuses a host that is already an agent host).
/// If the agent did come up, installing again moves it over once more.
pub fn fail_interrupted(store: &Store) -> Result<usize, IpcError> {
    let cut_off: Vec<String> = store
        .agent_installs(None, 1000)?
        .into_iter()
        .filter(|j| j.state == "running" && j.step == "connect")
        .map(|j| j.host_alias)
        .collect();
    let n = store.fail_stale_agent_installs(
        crate::store::now_unix() + 1,
        "interrupted: the hub stopped while it ran; start it again",
    )?;
    for alias in cut_off {
        let on_agent = store
            .get_host_row(&alias)?
            .is_some_and(|h| h.transport == "agent");
        if on_agent {
            store.set_host_transport(&alias, "ssh")?;
            tracing::info!(host = %alias, "an agent install the hub stopped during left the host on agent: back on SSH");
        }
    }
    Ok(n)
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
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let why = err
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("no answer");
        return Err(format!("could not reach the host over SSH: {}", why.trim()));
    }
    let uname = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let target = target_for(&uname).ok_or_else(|| {
        format!("no fleet-agent release for {uname:?}: releases are for x86_64 and aarch64 Linux")
    })?;

    // 2. tmux, which every session runs in (review r09/r18)
    step(store, id, "tmux", "checking that the host has tmux");
    let out =
        crate::ssh::run_shell_bounded(ssh, host, &tmux_script(), CONNECT_TIMEOUT, DOWNLOAD_WALL)
            .await
            .map_err(|e| fail("could not check for tmux", e))?;
    if !out.status.success() {
        let text = String::from_utf8_lossy(&out.stdout);
        return Err(line_value(&text, "error")
            .map(str::to_string)
            .unwrap_or_else(|| {
                format!(
                    "tmux is not installed on this host and could not be installed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                )
            }));
    }

    // 3. download
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

    // 4. start, the token on stdin
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

    // 5. connect
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
        // A login profile that prints before the answer.
        assert_eq!(
            target_for("Welcome to mercury\nLast login: today\nLinux x86_64\n"),
            Some("x86_64-unknown-linux-gnu")
        );
    }

    /// A job a previous hub process left `running` no longer blocks a new
    /// install once the hub starts again (not only after 30 min and a list).
    #[tokio::test]
    async fn a_job_left_running_by_a_stopped_hub_does_not_block_a_new_one() {
        let store = store_with_ssh_host();
        let old = {
            let s = store.lock().unwrap();
            s.set_setting(crate::mcp::SETTING_TOKEN, "tok").unwrap();
            s.insert_agent_install("mercury", "0.5.4").unwrap().id
        };
        let args = InstallAgentArgs {
            alias: "mercury".into(),
            hub_url: Some("https://fleet.example.com".into()),
            version: Some("0.5.4".into()),
        };
        assert_eq!(
            plan(&store, &args).err().map(|e| e.code).as_deref(),
            Some(codes::E_CONFLICT)
        );
        assert_eq!(fail_interrupted(&store.lock().unwrap()).unwrap(), 1);
        assert!(plan(&store, &args).is_ok());
        let s = store.lock().unwrap();
        assert_eq!(s.agent_install(old).unwrap().unwrap().state, "failed");
    }

    /// Review r09/r18: the job makes sure the host has tmux, installing it
    /// only where that needs no password, and otherwise fails naming tmux.
    #[cfg(unix)]
    #[test]
    fn the_tmux_step_installs_it_without_a_prompt_or_says_why_not() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let tool = |name: &str, body: &str| {
            use std::os::unix::fs::PermissionsExt;
            let p = bin.join(name);
            std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        let run = || {
            crate::proc::std_command("/bin/bash")
                .args(["-c", &tmux_script()])
                .env("PATH", &bin)
                .output()
                .unwrap()
        };
        // Not root, no sudo, no package manager: it says why, naming tmux.
        tool("id", "echo 1000");
        let out = run();
        assert_eq!(out.status.code(), Some(4));
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        let why = line_value(&text, "error").unwrap();
        assert!(why.contains("tmux") && why.contains("password"), "{why}");
        // Root with apt-get: installed.
        tool("id", "echo 0");
        tool(
            "env",
            "while [ \"${1#*=}\" != \"$1\" ]; do shift; done; exec \"$@\"",
        );
        tool(
            "apt-get",
            &format!(
                "[ \"$1\" = install ] && printf '#!/bin/sh\\n' > {0}/tmux && /bin/chmod +x {0}/tmux",
                bin.display()
            ),
        );
        let out = run();
        assert!(out.status.success(), "{out:?}");
        assert_eq!(
            line_value(&String::from_utf8_lossy(&out.stdout), "tmux"),
            Some("installed")
        );
        // Present: nothing to do.
        let out = run();
        assert_eq!(
            line_value(&String::from_utf8_lossy(&out.stdout), "tmux"),
            Some("present")
        );
    }

    /// Review r18 A3: a hub stopped while a job waited for the agent to
    /// connect left the host on `transport=agent`; the startup sweep puts it
    /// back on SSH. A job stopped earlier never moved it, and a host it did
    /// not touch keeps its transport.
    #[tokio::test]
    async fn a_hub_stopped_mid_connect_puts_the_host_back_on_ssh_at_start() {
        let store = store_with_ssh_host();
        {
            let s = store.lock().unwrap();
            let id = s.insert_agent_install("mercury", "0.5.4").unwrap().id;
            s.set_agent_install_step(id, "connect", Some("waiting"))
                .unwrap();
            s.set_host_transport("mercury", "agent").unwrap();
            s.insert_host("venus", Some("venus")).unwrap();
            s.set_host_transport("venus", "agent").unwrap();
            let early = s.insert_agent_install("venus", "0.5.4").unwrap().id;
            s.set_agent_install_step(early, "download", Some("fetching"))
                .unwrap();
        }
        assert_eq!(fail_interrupted(&store.lock().unwrap()).unwrap(), 2);
        let s = store.lock().unwrap();
        assert_eq!(s.get_host_row("mercury").unwrap().unwrap().transport, "ssh");
        assert_eq!(s.get_host_row("venus").unwrap().unwrap().transport, "agent");
    }

    /// r18: the "already running" check and the insert share one store
    /// guard, so of several installs started at once on one host exactly
    /// one claims it and the rest are refused with a conflict.
    #[test]
    fn concurrent_installs_on_one_host_claim_it_once() {
        let store = store_with_ssh_host();
        store
            .lock()
            .unwrap()
            .set_setting(crate::mcp::SETTING_TOKEN, "tok")
            .unwrap();
        let args = InstallAgentArgs {
            alias: "mercury".into(),
            hub_url: Some("https://fleet.example.com".into()),
            version: Some("0.5.4".into()),
        };
        const N: usize = 8;
        let barrier = Arc::new(std::sync::Barrier::new(N));
        let handles: Vec<_> = (0..N)
            .map(|_| {
                let (store, args, barrier) = (store.clone(), args.clone(), barrier.clone());
                std::thread::spawn(move || {
                    barrier.wait();
                    plan_and_claim(&store, &args).map(|(_, row)| row.id)
                })
            })
            .collect();
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let won = results.iter().filter(|r| r.is_ok()).count();
        assert_eq!(won, 1, "{results:?}");
        assert!(results
            .iter()
            .filter_map(|r| r.as_ref().err())
            .all(|e| e.code == codes::E_CONFLICT));
        let s = store.lock().unwrap();
        let running = s
            .agent_installs(Some("mercury"), 99)
            .unwrap()
            .into_iter()
            .filter(|j| j.state == "running")
            .count();
        assert_eq!(running, 1);
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
        // r18-A4: a stale pid file naming another process (after a reboot
        // the number is reused) is never killed.
        let mut bystander = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let cfg = home.path().join(".config/fleet-agent");
        std::fs::create_dir_all(&cfg).unwrap();
        std::fs::write(cfg.join("agent.pid"), format!("{}\n", bystander.id())).unwrap();
        let out = bash(&script, home.path(), "tok-secret\n");
        assert!(
            bystander.try_wait().unwrap().is_none(),
            "an unrelated process survives the restart"
        );
        let _ = bystander.kill();
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

    /// r18-A1 / A2: no public URL and no hub_url, or a readonly token, is
    /// refused before anything is written or rotated.
    #[tokio::test]
    async fn a_loopback_hub_or_a_readonly_token_is_refused_up_front() {
        let store = store_with_ssh_host();
        {
            let s = store.lock().unwrap();
            s.set_setting(crate::mcp::SETTING_TOKEN, "tok").unwrap();
        }
        let ssh: Arc<dyn SshExec> = Arc::new(crate::ssh_fake::FakeSsh::new());
        let reg = Some(AgentRegistry::new());
        let args = |url: Option<&str>| InstallAgentArgs {
            alias: "mercury".into(),
            hub_url: url.map(str::to_string),
            version: Some("0.5.4".into()),
        };
        let e = start(store.clone(), ssh.clone(), reg.clone(), args(None)).unwrap_err();
        assert_eq!(e.code, codes::E_INVALID);
        assert!(e.message.contains("public URL"), "{}", e.message);

        let token = {
            let s = store.lock().unwrap();
            s.upsert_host_token("mercury", "old-token").unwrap();
            s.set_host_token_mode("mercury", "readonly").unwrap();
            s.set_setting(
                crate::service::hub::SETTING_PUBLIC_URL,
                "https://fleet.example.com",
            )
            .unwrap();
            s.get_host_token("mercury").unwrap().unwrap().token
        };
        let e = start(store.clone(), ssh.clone(), reg.clone(), args(None)).unwrap_err();
        assert_eq!(e.code, codes::E_INVALID);
        assert!(e.message.contains("readonly"), "{}", e.message);
        let s = store.lock().unwrap();
        assert_eq!(s.get_host_token("mercury").unwrap().unwrap().token, token);
        assert!(s.agent_installs(None, 9).unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_unreachable_host_says_so() {
        let store = store_with_ssh_host();
        let fake = crate::ssh_fake::FakeSsh::new();
        fake.set_default(crate::ssh_fake::Reply::fail(
            255,
            "ssh: connect to host box port 22: Connection refused\n",
        ));
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
        let err = run(
            &store,
            &fake,
            &AgentRegistry::new(),
            id,
            &p,
            Duration::from_millis(50),
        )
        .await
        .unwrap_err();
        assert!(
            err.starts_with("could not reach the host over SSH"),
            "{err}"
        );
        assert!(err.contains("Connection refused"), "{err}");
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
