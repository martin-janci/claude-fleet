use crate::ipc_error::{codes, IpcError};
use crate::shell::quote;
use async_trait::async_trait;
use serde::Serialize;
use std::path::PathBuf;

use crate::ssh::{SshClient, SshExec};
use std::sync::Arc;

/// tmux target for an EXACT session name. A bare `-t NAME` is a *lookup*:
/// tmux tries an exact match, then a unique PREFIX, then an fnmatch pattern —
/// so an operation aimed at a dead `dev-foo` silently lands on
/// `dev-foo--feat-x`. The `=` prefix disables both fallbacks (verified
/// against tmux 3.6a: `has-session -t api` succeeds against `api-review`,
/// `has-session -t '=api'` fails with "can't find session").
pub fn exact_session(name: &str) -> String {
    format!("={name}")
}

/// The same, for commands taking a *pane* target (`capture-pane`,
/// `respawn-pane`). tmux only accepts `=` in the session part of a pane
/// target, so the trailing `:` (current window, active pane) is required:
/// `-t '=NAME'` fails there with "can't find pane" (tmux 3.6a).
pub fn exact_pane(name: &str) -> String {
    format!("={name}:")
}

/// Backend-agnostic tmux operations. Implementations differ only in how
/// the `tmux` binary is invoked: locally or wrapped in `ssh <host>`.
#[async_trait]
pub trait TmuxExec: Send + Sync {
    async fn list_sessions(&self) -> Result<Vec<TmuxSession>, IpcError>;
    /// `pane_cmd` is the shell command tmux runs as the pane's initial
    /// process — `pane_command_for()` for a Claude session, `shell_pane_command()`
    /// for a plain shell session.
    async fn new_session(
        &self,
        name: &str,
        working_dir: &std::path::Path,
        pane_cmd: &str,
    ) -> Result<(), IpcError>;
    async fn kill_session(&self, name: &str) -> Result<(), IpcError>;
    async fn rename_session(&self, old: &str, new: &str) -> Result<(), IpcError>;
    async fn restart_session(&self, name: &str, pane_cmd: &str) -> Result<(), IpcError>;
    /// `restart_session` with an explicit start directory (`respawn-pane -k
    /// -c <cwd>`), for a pane whose cwd was deleted or recreated. The default
    /// ignores `cwd` so test doubles need not implement it.
    async fn respawn_pane_in(
        &self,
        name: &str,
        cwd: &std::path::Path,
        pane_cmd: &str,
    ) -> Result<(), IpcError> {
        let _ = cwd;
        self.restart_session(name, pane_cmd).await
    }
    async fn capture_pane(&self, name: &str) -> Result<String, IpcError>;
    /// Capture the pane plus `lines` rows of scrollback history.
    async fn capture_pane_scrollback(&self, name: &str, lines: u32) -> Result<String, IpcError>;
    /// Run `claude agents --json` on this host and return parsed session info.
    /// Returns an empty vec if claude CLI is not installed or the command fails —
    /// the fleet treats missing Claude agent data as degraded-gracefully.
    async fn list_claude_agents(&self) -> Vec<crate::claude_agents::ClaudeAgentRow>;
    /// `sessionId → transcript mtime (unix s)` for the given ids; ids that are not
    /// valid Claude session ids are skipped. `Some(map)` when the call succeeded
    /// (possibly empty: no transcript found, or no valid id to ask about);
    /// `None` on any failure (spawn error, non-zero exit, timeout), so callers
    /// can tell "unknown" apart from "no transcript".
    async fn transcript_mtimes(
        &self,
        ids: &[String],
    ) -> Option<std::collections::HashMap<String, i64>> {
        let _ = ids;
        Some(std::collections::HashMap::new())
    }
    /// The Claude account this host is currently logged into, read from its
    /// `~/.claude.json` `oauthAccount`. `None` means "could not tell" (ssh
    /// failure, file missing/unparseable, logged out, or an executor that
    /// does not implement it) — reconcile then leaves the host's stored
    /// account link untouched; it never clears it. Only `Some` with a uuid
    /// can relink a host (see `service::hosts::sync_host_account`).
    ///
    /// The default is `None`: `LocalTmux` keeps it, because `local` is
    /// synced by `service::hosts::sync_local_account` (an injectable-home
    /// file read, exercised by its own tests) — implementing it here too
    /// would probe `local` twice per pass. `RemoteTmux` overrides it.
    async fn read_oauth_account(&self) -> Option<crate::service::hosts::OauthAccount> {
        None
    }
    /// This host's boot identity (kernel boot id + tmux server pid), read
    /// once per reconcile probe. `None` means "could not tell" (ssh failure,
    /// transport timeout, unparseable output, or an executor that does not
    /// implement it) — it must NEVER be read as "no tmux server", because a
    /// later pass treats `Some(HostIdentity { tmux_server_pid: None, .. })`
    /// as exactly that and marks every session on the host lost. The default
    /// is `None`; `LocalTmux` and `RemoteTmux` both override it.
    async fn host_identity(&self) -> Option<HostIdentity> {
        None
    }
}

/// Shell script printing `<sessionId>\t<mtime>` for the first
/// `$HOME/.claude/projects/*/<sessionId>.jsonl` found per id. `date -r <file>
/// +%s` behaves the same on GNU and BSD. Every id is validated as a Claude
/// session id and shell-quoted; `None` when no id survives validation.
pub fn transcript_mtimes_script(ids: &[String]) -> Option<String> {
    let valid: Vec<String> = ids
        .iter()
        .filter(|id| crate::validate::claude_session_id(id).is_ok())
        .map(|id| quote(id))
        .collect();
    if valid.is_empty() {
        return None;
    }
    Some(format!(
        "for id in {}; do for f in \"$HOME\"/.claude/projects/*/\"$id\".jsonl; do \
         if [ -f \"$f\" ]; then printf '%s\\t%s\\n' \"$id\" \"$(date -r \"$f\" +%s)\"; break; fi; \
         done; done",
        valid.join(" ")
    ))
}

/// Parse [`transcript_mtimes_script`] output. Lines that are not exactly
/// `<valid session id>\t<i64>` (login-shell noise, a failed `date`) are
/// ignored.
pub fn parse_mtimes(stdout: &str) -> std::collections::HashMap<String, i64> {
    stdout
        .lines()
        .filter_map(|line| {
            let (id, mtime) = line.split_once('\t')?;
            if mtime.contains('\t') || crate::validate::claude_session_id(id).is_err() {
                return None;
            }
            Some((id.to_string(), mtime.trim().parse::<i64>().ok()?))
        })
        .collect()
}

/// A host's boot identity, read once per reconcile probe.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HostIdentity {
    /// Kernel boot id — `/proc/sys/kernel/random/boot_id` (the HOST's, even
    /// inside a container), else `sysctl -n kern.bootsessionuuid` (macOS): a
    /// UUID stable for the whole boot. `None` when neither produced anything.
    /// Deliberately NOT `kern.boottime` or `uptime -s`: both render their
    /// timestamp in the local timezone, so the same boot prints a different
    /// string under a different `TZ` (or across a DST change, or after NTP
    /// steps the clock) — a later task reads any change in this string as a
    /// reboot and marks every session on the host lost, so a
    /// timezone-flavored "identity" is worse than none. Do not reintroduce
    /// them.
    pub boot_id: Option<String>,
    /// Pid of the tmux server. `None` ⇒ no tmux server is running.
    pub tmux_server_pid: Option<i64>,
}

/// Prints `boot=<id>`, then `tmuxrc=<tmux's own exit code>` and
/// `tmuxout=<first line of tmux's combined stdout+stderr>`.
///
/// This deliberately does NOT decide "no server" in shell: every tmux
/// failure looks the same to a naive `2>/dev/null` — missing binary (exit
/// 127, e.g. a login profile edit or a `brew`/`apt upgrade tmux` mid-relink
/// dropping tmux off `PATH`), a client/server protocol mismatch after an
/// upgrade (the new client can't talk to the still-running old server),
/// socket permission errors, or an actually-dead server — and only the last
/// one means the server (and every session on it) is gone. So the script
/// only *surfaces* tmux's raw exit code and output; `parse_host_identity`
/// classifies it in Rust, via the same [`is_no_server_running`] the session
/// lister already trusts, instead of guessing here.
///
/// `tmux list-sessions -F '#{pid}'` (not `display-message`) because it
/// needs no attached client, matching what `list_local_sessions` /
/// `RemoteTmux::list_sessions` already run.
///
/// The boot id source is deliberately just `/proc/sys/kernel/random/boot_id`
/// (Linux) falling back to `sysctl -n kern.bootsessionuuid` (macOS) — both
/// are per-boot identifiers with no wall-clock rendering. `kern.boottime`
/// and `uptime -s` were rejected: both print a local-time timestamp, so the
/// same boot yields a different "boot id" under a different `TZ`, across a
/// DST change, or after NTP steps the clock, and a spurious change is read
/// downstream as a reboot that marks every session on the host lost.
pub const HOST_IDENTITY_SCRIPT: &str = "printf 'boot=%s\\n' \"$(cat /proc/sys/kernel/random/boot_id 2>/dev/null || sysctl -n kern.bootsessionuuid 2>/dev/null)\"; out=$(tmux list-sessions -F '#{pid}' 2>&1); rc=$?; printf 'tmuxrc=%s\\n' \"$rc\"; printf 'tmuxout=%s\\n' \"$(printf '%s' \"$out\" | head -n 1)\"";

/// Parse [`HOST_IDENTITY_SCRIPT`] output. Requires BOTH a `tmuxrc=` line
/// (tmux's exit code) and a `tmuxout=` line (the first line of its combined
/// stdout+stderr) — missing either, or an unparseable `tmuxrc`, is output we
/// cannot trust → outer `None`. Then:
/// - `tmuxrc == 0`: the output must parse as the server pid. Anything else
///   (empty — e.g. a live server holding zero sessions — or non-numeric) means
///   we cannot tell the pid, so outer `None`, never a guess.
/// - `tmuxrc != 0` and [`is_no_server_running`] matches the output: the ONLY
///   path to `tmux_server_pid: None` — the server is confirmed gone.
/// - `tmuxrc != 0` and anything else (missing binary, protocol mismatch,
///   permission error, …): untrustworthy, so outer `None`. An untrusted
///   "no server" would mark every session on the host lost.
pub fn parse_host_identity(stdout: &str) -> Option<HostIdentity> {
    let mut boot_id = None;
    let mut rc_line: Option<&str> = None;
    let mut out_line: Option<&str> = None;
    for line in stdout.lines() {
        if let Some(v) = line.strip_prefix("boot=") {
            let v = v.trim();
            if !v.is_empty() {
                boot_id = Some(v.to_string());
            }
        } else if let Some(v) = line.strip_prefix("tmuxrc=") {
            rc_line = Some(v.trim());
        } else if let Some(v) = line.strip_prefix("tmuxout=") {
            out_line = Some(v.trim());
        }
    }
    let rc: i32 = rc_line?.parse().ok()?;
    let out = out_line?;
    let tmux_server_pid = if rc == 0 {
        Some(out.parse::<i64>().ok()?)
    } else if is_no_server_running(out) {
        None
    } else {
        return None;
    };
    Some(HostIdentity {
        boot_id,
        tmux_server_pid,
    })
}

/// tmux (and `claude` / `bash`) on this machine, the `local` host. Every
/// method refuses first when this is a hub with `hub.local_host=false`: the
/// fallible ones return `E_NOTFOUND`, the infallible ones their "nothing
/// known" value, so no construction site can spawn on a disabled `local`.
pub struct LocalTmux;

fn local_allowed() -> Result<(), IpcError> {
    crate::service::hub::ensure_local_allowed(crate::service::projects::LOCAL_HOST)
}

#[async_trait]
impl TmuxExec for LocalTmux {
    async fn list_sessions(&self) -> Result<Vec<TmuxSession>, IpcError> {
        local_allowed()?;
        list_local_sessions().await
    }
    async fn new_session(
        &self,
        name: &str,
        cwd: &std::path::Path,
        pane_cmd: &str,
    ) -> Result<(), IpcError> {
        local_allowed()?;
        new_session(name, cwd, pane_cmd).await
    }
    async fn kill_session(&self, name: &str) -> Result<(), IpcError> {
        local_allowed()?;
        kill_session(name).await
    }
    async fn rename_session(&self, old: &str, new: &str) -> Result<(), IpcError> {
        local_allowed()?;
        rename_session(old, new).await
    }
    async fn restart_session(&self, name: &str, pane_cmd: &str) -> Result<(), IpcError> {
        local_allowed()?;
        restart_session(name, pane_cmd).await
    }
    async fn respawn_pane_in(
        &self,
        name: &str,
        cwd: &std::path::Path,
        pane_cmd: &str,
    ) -> Result<(), IpcError> {
        local_allowed()?;
        respawn_pane_in(name, cwd, pane_cmd).await
    }
    async fn capture_pane(&self, name: &str) -> Result<String, IpcError> {
        local_allowed()?;
        let output = tokio::process::Command::new("tmux")
            .args(["capture-pane", "-t", &exact_pane(name), "-p"])
            .output()
            .await
            .map_err(|e| IpcError::new(codes::E_TMUX, format!("spawn tmux failed: {e}")))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            // Non-existent pane: return empty so the poller keeps waiting.
            Ok(String::new())
        }
    }
    async fn capture_pane_scrollback(&self, name: &str, lines: u32) -> Result<String, IpcError> {
        local_allowed()?;
        let start = scrollback_start(lines);
        // Bounded like the remote path (30 s): the Conversation tab's probe
        // polls this every 2 s, and a wedged local tmux server must not
        // hang it.
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            tokio::process::Command::new("tmux")
                .args(["capture-pane", "-t", &exact_pane(name), "-S", &start, "-p"])
                .output(),
        )
        .await
        .map_err(|_| IpcError::new(codes::E_TIMEOUT, "tmux capture-pane timed out"))?
        .map_err(|e| IpcError::new(codes::E_TMUX, format!("spawn tmux failed: {e}")))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            Ok(String::new())
        }
    }
    async fn list_claude_agents(&self) -> Vec<crate::claude_agents::ClaudeAgentRow> {
        if local_allowed().is_err() {
            return Vec::new();
        }
        let output = tokio::process::Command::new("claude")
            .args(["agents", "--json"])
            .output()
            .await
            .ok();
        let json = output
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_else(|| "[]".to_string());
        crate::claude_agents::parse_claude_agents_json(&json)
    }
    async fn transcript_mtimes(
        &self,
        ids: &[String],
    ) -> Option<std::collections::HashMap<String, i64>> {
        local_allowed().ok()?;
        let Some(script) = transcript_mtimes_script(ids) else {
            return Some(std::collections::HashMap::new());
        };
        // No login shell: the script needs only `$HOME`, which is inherited.
        match tokio::process::Command::new("bash")
            .args(["-c", &script])
            .output()
            .await
        {
            Ok(o) if o.status.success() => Some(parse_mtimes(&String::from_utf8_lossy(&o.stdout))),
            _ => None,
        }
    }
    async fn host_identity(&self) -> Option<HostIdentity> {
        local_allowed().ok()?;
        let out = tokio::process::Command::new("bash")
            .args(["-c", HOST_IDENTITY_SCRIPT])
            .output()
            .await
            .ok()
            .filter(|o| o.status.success())?;
        parse_host_identity(&String::from_utf8_lossy(&out.stdout))
    }
}

/// tmux over an `SshExec`. Generic (defaulting to the production client) so
/// the exact scripts it builds can be exercised against a scripted fake or
/// the local `bash -c` executor without a real host; every construction site
/// still just writes `RemoteTmux { client: Arc::clone(ssh), host }`.
pub struct RemoteTmux<C: SshExec = Arc<SshClient>> {
    pub client: C,
    pub host: String,
}

impl<C: SshExec> RemoteTmux<C> {
    /// We always wrap remote tmux invocations in `bash -lc '…'` so the
    /// remote user's login env (PATH, LANG, etc.) is sourced. sshd may have
    /// `AcceptEnv` disabled which would silently drop SendEnv vars; the
    /// login shell route is portable.
    ///
    /// CRITICAL: `ssh <host> bash -lc <script>` joins ALL trailing argv with
    /// spaces before sending to the remote sshd. The remote shell then re-
    /// tokenizes, so any spaces in `<script>` would break `bash -c` (it
    /// would get just the first token as the script and everything else as
    /// positional args). We therefore single-quote the WHOLE script via
    /// `quote` so it crosses the ssh boundary as one shell word.
    /// `quote` already escapes the embedded `'` characters used by
    /// per-arg quoting inside `script`.
    ///
    /// The 10s here is ssh's `ConnectTimeout` only. `SshClient::run` bounds
    /// the whole command by `default_wall_clock(10s)` = 30s on top, so a tmux
    /// command that hangs after connect (wedged ControlMaster) surfaces as
    /// `E_SSH_TIMEOUT` instead of blocking the caller forever.
    async fn remote_bash(&self, script: &str) -> Result<std::process::Output, IpcError> {
        let quoted = quote(script);
        self.client
            .run(
                &self.host,
                &["bash", "-lc", &quoted],
                std::time::Duration::from_secs(10),
            )
            .await
    }
}

#[async_trait]
impl<C: SshExec> TmuxExec for RemoteTmux<C> {
    async fn list_sessions(&self) -> Result<Vec<TmuxSession>, IpcError> {
        let script = "tmux list-sessions -F '#{session_name}|#{session_created}|#{session_activity}|#{session_attached}|#{pane_current_path}' 2>&1";
        let output = self.remote_bash(script).await?;
        let combined = String::from_utf8_lossy(&output.stdout).into_owned();
        if output.status.success() {
            return parse_sessions_checked(&combined);
        }
        if is_no_server_running(&combined) {
            return Ok(Vec::new());
        }
        Err(IpcError::new(codes::E_TMUX, combined.trim()))
    }

    async fn new_session(
        &self,
        name: &str,
        cwd: &std::path::Path,
        pane_cmd: &str,
    ) -> Result<(), IpcError> {
        // Build the `tmux new-session` command identically to LocalTmux but
        // shell-escape arguments since we're sending a single script string.
        let mut script = String::from("tmux new-session -d");
        script.push_str(&format!(" -s {}", quote(name)));
        script.push_str(&format!(" -c {}", quote(&cwd.to_string_lossy())));
        // Forward env explicitly — remote sshd typically doesn't pass LANG.
        script.push_str(" -e COLORTERM=truecolor -e TERM=xterm-256color");
        script.push_str(&format!(
            " -e LANG={}",
            quote(&std::env::var("LANG").unwrap_or_else(|_| "en_US.UTF-8".into()))
        ));
        script.push(' ');
        script.push_str(&quote(pane_cmd));
        let output = self.remote_bash(&script).await?;
        if output.status.success() {
            Ok(())
        } else {
            Err(IpcError::new(
                codes::E_TMUX,
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ))
        }
    }

    async fn kill_session(&self, name: &str) -> Result<(), IpcError> {
        let script = format!("tmux kill-session -t {}", quote(&exact_session(name)));
        let output = self.remote_bash(&script).await?;
        if output.status.success() {
            Ok(())
        } else {
            Err(IpcError::new(
                codes::E_TMUX,
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ))
        }
    }

    async fn rename_session(&self, old: &str, new: &str) -> Result<(), IpcError> {
        let trimmed = new.trim();
        if trimmed.is_empty() {
            return Err(IpcError::new(
                codes::E_TMUX,
                "new session name must not be empty",
            ));
        }
        if trimmed.contains(|c: char| c.is_whitespace() || c == '.' || c == ':') {
            return Err(IpcError::new(
                codes::E_TMUX,
                "tmux session name must not contain whitespace, `.`, or `:`",
            ));
        }
        if trimmed == old {
            return Ok(());
        }
        let script = format!(
            "tmux rename-session -t {} {}",
            quote(&exact_session(old)),
            quote(trimmed)
        );
        let output = self.remote_bash(&script).await?;
        if output.status.success() {
            Ok(())
        } else {
            Err(IpcError::new(
                codes::E_TMUX,
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ))
        }
    }

    async fn restart_session(&self, name: &str, pane_cmd: &str) -> Result<(), IpcError> {
        let script = format!(
            "tmux respawn-pane -k -t {} {}",
            quote(&exact_pane(name)),
            quote(pane_cmd)
        );
        let output = self.remote_bash(&script).await?;
        if output.status.success() {
            Ok(())
        } else {
            Err(IpcError::new(
                codes::E_TMUX,
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ))
        }
    }

    async fn respawn_pane_in(
        &self,
        name: &str,
        cwd: &std::path::Path,
        pane_cmd: &str,
    ) -> Result<(), IpcError> {
        let script = respawn_pane_in_script(name, cwd, pane_cmd);
        let output = self.remote_bash(&script).await?;
        if output.status.success() {
            Ok(())
        } else {
            Err(IpcError::new(
                codes::E_TMUX,
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ))
        }
    }

    async fn capture_pane(&self, name: &str) -> Result<String, IpcError> {
        let script = format!("tmux capture-pane -t {} -p", quote(&exact_pane(name)));
        let output = self.remote_bash(&script).await?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            // Non-existent pane: return empty so the poller keeps waiting.
            Ok(String::new())
        }
    }
    async fn capture_pane_scrollback(&self, name: &str, lines: u32) -> Result<String, IpcError> {
        let start = scrollback_start(lines);
        let script = format!(
            "tmux capture-pane -t {} -S {} -p",
            quote(&exact_pane(name)),
            quote(&start),
        );
        let output = self.remote_bash(&script).await?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            // Non-existent pane: return empty so the poller keeps waiting.
            Ok(String::new())
        }
    }
    async fn list_claude_agents(&self) -> Vec<crate::claude_agents::ClaudeAgentRow> {
        let script = "claude agents --json 2>/dev/null || echo '[]'";
        let output = self.remote_bash(script).await.ok();
        let json = output
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_else(|| "[]".to_string());
        crate::claude_agents::parse_claude_agents_json(&json)
    }
    async fn transcript_mtimes(
        &self,
        ids: &[String],
    ) -> Option<std::collections::HashMap<String, i64>> {
        let Some(script) = transcript_mtimes_script(ids) else {
            return Some(std::collections::HashMap::new());
        };
        // `remote_bash` bounds the call (its timeout surfaces as `Err`); an
        // unreachable host is ssh exiting 255 — both are failures, not "no
        // transcript".
        match self.remote_bash(&script).await {
            Ok(o) if o.status.success() => Some(parse_mtimes(&String::from_utf8_lossy(&o.stdout))),
            _ => None,
        }
    }
    async fn read_oauth_account(&self) -> Option<crate::service::hosts::OauthAccount> {
        // Same script section `add_host`'s probe runs, on its own. The
        // script itself never fails (`|| true`), so a non-zero exit is ssh
        // (unreachable / timeout) — "could not tell", not "logged out".
        let output = self
            .remote_bash(crate::service::hosts::OAUTH_ACCOUNT_SCRIPT)
            .await
            .ok()
            .filter(|o| o.status.success())?;
        crate::service::hosts::parse_oauth_account(&String::from_utf8_lossy(&output.stdout))
    }
    async fn host_identity(&self) -> Option<HostIdentity> {
        let out = self
            .remote_bash(HOST_IDENTITY_SCRIPT)
            .await
            .ok()
            .filter(|o| o.status.success())?;
        parse_host_identity(&String::from_utf8_lossy(&out.stdout))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TmuxSession {
    pub name: String,
    pub created: i64,
    pub last_activity: i64,
    pub attached: bool,
    pub path: PathBuf,
}

/// Lists tmux sessions on the local host. Returns an empty Vec (not an error)
/// when the tmux server isn't running.
pub async fn list_local_sessions() -> Result<Vec<TmuxSession>, IpcError> {
    let output = tokio::process::Command::new("tmux")
        .args([
            "list-sessions",
            "-F",
            "#{session_name}|#{session_created}|#{session_activity}|#{session_attached}|#{pane_current_path}",
        ])
        .output()
        .await;
    match output {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout).into_owned();
            parse_sessions_checked(&stdout)
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr).to_string();
            if is_no_server_running(&stderr) {
                Ok(Vec::new())
            } else {
                Err(IpcError::new(codes::E_TMUX, stderr.trim()))
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(IpcError::new(
            codes::E_TMUX,
            "tmux binary not found on PATH",
        )),
        Err(e) => Err(IpcError::new(
            codes::E_TMUX,
            format!("spawn tmux failed: {e}"),
        )),
    }
}

/// True for any tmux stderr that means "no server is running on this socket"
/// — i.e. an empty `list_local_sessions()` return rather than an error.
///
/// Variants observed:
/// - "no server running on /tmp/tmux-501/default"  (server was started then exited)
/// - "error connecting to /private/tmp/tmux-501/default (No such file or directory)"
///   (no server has ever been started — the socket file doesn't exist)
fn is_no_server_running(stderr: &str) -> bool {
    let s = stderr.to_lowercase();
    s.contains("no server running")
        || (s.contains("error connecting to") && s.contains("no such file or directory"))
}

fn parse_sessions(input: &str) -> Vec<TmuxSession> {
    input
        .lines()
        .filter_map(|line| {
            // Destructure the fixed 5-field format off the split iterator —
            // no per-line `Vec` allocation. A 6th field means the line is
            // malformed (a `|` inside a session name); reject it.
            let mut it = line.split('|');
            let name = it.next()?;
            let created = it.next()?.parse::<i64>().ok()?;
            let last_activity = it.next()?.parse::<i64>().ok()?;
            let attached_int = it.next()?.parse::<i64>().ok()?;
            let path = it.next()?;
            if it.next().is_some() {
                return None;
            }
            Some(TmuxSession {
                name: name.to_string(),
                created,
                last_activity,
                attached: attached_int > 0,
                path: PathBuf::from(path),
            })
        })
        .collect()
}

/// `parse_sessions` for a SUCCESSFUL `list-sessions`, refusing output that
/// has content but not one parseable line (a tmux wrapper/alias, a login
/// banner, a format mismatch). Treating that as "no sessions" would ghost,
/// then delete, every row on the host; an `E_TMUX` makes the reconcile count
/// the host unreachable for this pass instead. Blank output and "no server
/// running" still mean zero sessions.
fn parse_sessions_checked(input: &str) -> Result<Vec<TmuxSession>, IpcError> {
    let sessions = parse_sessions(input);
    let mut lines = input.lines().map(str::trim).filter(|l| !l.is_empty());
    let Some(first) = lines.next() else {
        return Ok(sessions);
    };
    if !sessions.is_empty() || is_no_server_running(input) {
        return Ok(sessions);
    }
    let sample: String = first.chars().take(80).collect();
    Err(IpcError::new(
        codes::E_TMUX,
        format!("unparseable tmux list-sessions output: {sample:?}"),
    ))
}

/// tmux `-S` start offset for `lines` rows of scrollback (a negative count).
pub(crate) fn scrollback_start(lines: u32) -> String {
    format!("-{lines}")
}

/// The pane command for a Claude ("work"/"review") session. With a known
/// session id: resume it, else create it under that id, else a bare `cl` — an
/// idempotent create-or-resume. Without an id (legacy rows): today's
/// most-recent-for-cwd behavior. The id is single-quoted; externally-supplied
/// ids should be validated with `validate::claude_session_id` before being
/// passed in (minted ids are safe by construction).
pub fn pane_command_for(claude_session_id: Option<&str>) -> String {
    let tail = "exec ${SHELL:-/bin/zsh} -l";
    match claude_session_id {
        Some(id) => {
            format!("cl --resume '{id}' 2>/dev/null || cl --session-id '{id}' || cl; {tail}")
        }
        None => format!("cl --continue || cl; {tail}"),
    }
}

/// Pane command for a plain shell session (`kind = "shell"`). Runs an
/// interactive login shell in a loop so the pane — and therefore the tmux
/// session — survives the user typing `exit`; a fresh shell respawns instead.
///
/// When `start_command` is given, it runs first (in the session's cwd, with
/// the env tmux injected via `-e`), its exit code is printed, and then the
/// pane drops to the respawning interactive shell — so the output stays
/// visible. `start_command` is the user's raw text; the remote transport
/// layer (`quote`) escapes the whole pane command as one shell word.
pub fn shell_pane_command(start_command: Option<&str>) -> String {
    let respawn = "while :; do ${SHELL:-/bin/zsh} -l; done";
    match start_command {
        Some(cmd) if !cmd.trim().is_empty() => {
            format!("{{ {cmd}; }}; printf '\\n[exit %s]\\n' \"$?\"; {respawn}")
        }
        _ => respawn.to_string(),
    }
}

pub async fn new_session(
    name: &str,
    working_dir: &std::path::Path,
    pane_cmd: &str,
) -> Result<(), IpcError> {
    // Push env into the session explicitly via `-e KEY=VAL`. This matters
    // because the tmux SERVER may already be running with stale env (e.g.
    // started before claude-fleet imported the user's locale from their
    // login shell). `-e` overrides the server env for processes started
    // in this session, so the spawned `cl`/`bash` reliably sees UTF-8.
    let mut cmd = tokio::process::Command::new("tmux");
    cmd.args([
        "new-session",
        "-d",
        "-s",
        name,
        "-c",
        &working_dir.to_string_lossy(),
    ]);
    for var in ["LANG", "LC_ALL", "LC_CTYPE", "PATH"] {
        if let Ok(val) = std::env::var(var) {
            if !val.is_empty() {
                cmd.args(["-e", &format!("{var}={val}")]);
            }
        }
    }
    cmd.args(["-e", "COLORTERM=truecolor", "-e", "TERM=xterm-256color"]);
    cmd.arg(pane_cmd);
    let output = cmd
        .output()
        .await
        .map_err(|e| IpcError::new(codes::E_TMUX, format!("spawn tmux failed: {e}")))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        Err(IpcError::new(codes::E_TMUX, stderr.trim()))
    }
}

/// Rename an existing tmux session. New name must follow tmux's naming rules
/// (no `.`, `:`, or whitespace; non-empty). Validation here keeps the caller
/// from getting a cryptic tmux error.
pub async fn rename_session(old: &str, new: &str) -> Result<(), IpcError> {
    let trimmed = new.trim();
    if trimmed.is_empty() {
        return Err(IpcError::new(
            codes::E_TMUX,
            "new session name must not be empty",
        ));
    }
    if trimmed.contains(|c: char| c.is_whitespace() || c == '.' || c == ':') {
        return Err(IpcError::new(
            codes::E_TMUX,
            "tmux session name must not contain whitespace, `.`, or `:`",
        ));
    }
    if trimmed == old {
        return Ok(()); // no-op
    }
    let output = tokio::process::Command::new("tmux")
        .args(["rename-session", "-t", &exact_session(old), trimmed])
        .output()
        .await
        .map_err(|e| IpcError::new(codes::E_TMUX, format!("spawn tmux failed: {e}")))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        Err(IpcError::new(codes::E_TMUX, stderr.trim()))
    }
}

/// Restart the pane's process by killing claude (or whatever's running) and
/// respawning with the same command the session was created with. Uses
/// `respawn-pane -k` so we don't need to know if claude is currently running
/// or already dropped to shell.
pub async fn restart_session(name: &str, pane_cmd: &str) -> Result<(), IpcError> {
    let output = tokio::process::Command::new("tmux")
        .args(["respawn-pane", "-k", "-t", &exact_pane(name), pane_cmd])
        .output()
        .await
        .map_err(|e| IpcError::new(codes::E_TMUX, format!("spawn tmux failed: {e}")))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        Err(IpcError::new(codes::E_TMUX, stderr.trim()))
    }
}

/// The remote form of [`respawn_pane_in`]: one shell word per argument.
pub(crate) fn respawn_pane_in_script(name: &str, cwd: &std::path::Path, pane_cmd: &str) -> String {
    format!(
        "tmux respawn-pane -k -c {} -t {} {}",
        quote(&cwd.to_string_lossy()),
        quote(&exact_pane(name)),
        quote(pane_cmd)
    )
}

/// `restart_session` with an explicit start directory. Used by the workspace
/// repair path when the pane's cwd was deleted (or just recreated under it —
/// a process keeps the dead inode as its cwd until it is respawned).
pub async fn respawn_pane_in(
    name: &str,
    cwd: &std::path::Path,
    pane_cmd: &str,
) -> Result<(), IpcError> {
    let output = tokio::process::Command::new("tmux")
        .args([
            "respawn-pane",
            "-k",
            "-c",
            &cwd.to_string_lossy(),
            "-t",
            &exact_pane(name),
            pane_cmd,
        ])
        .output()
        .await
        .map_err(|e| IpcError::new(codes::E_TMUX, format!("spawn tmux failed: {e}")))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        Err(IpcError::new(codes::E_TMUX, stderr.trim()))
    }
}

pub async fn kill_session(name: &str) -> Result<(), IpcError> {
    let output = tokio::process::Command::new("tmux")
        .args(["kill-session", "-t", &exact_session(name)])
        .output()
        .await
        .map_err(|e| IpcError::new(codes::E_TMUX, format!("spawn tmux failed: {e}")))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        Err(IpcError::new(codes::E_TMUX, stderr.trim()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrollback_start_is_negative_lines() {
        assert_eq!(scrollback_start(120), "-120");
        assert_eq!(scrollback_start(0), "-0");
    }

    #[test]
    fn parse_two_sessions() {
        let input = "dev-foo|1716000000|1716100000|1|/repos/foo\ndev-bar|1716000100|1716200000|0|/repos/bar\n";
        let sessions = parse_sessions(input);
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].name, "dev-foo");
        assert!(sessions[0].attached);
        assert_eq!(sessions[0].path, PathBuf::from("/repos/foo"));
        assert_eq!(sessions[1].name, "dev-bar");
        assert!(!sessions[1].attached);
    }

    #[test]
    fn parse_skips_malformed_lines() {
        let input = "good|1716000000|1716100000|1|/x\nbad-line-without-pipes\nempty||||\n";
        let sessions = parse_sessions(input);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].name, "good");
    }

    #[test]
    fn parse_empty_input() {
        assert!(parse_sessions("").is_empty());
    }

    #[test]
    fn checked_parse_accepts_blank_and_no_server_as_zero_sessions() {
        for blank in ["", "\n", "  \n\n"] {
            assert!(
                parse_sessions_checked(blank).unwrap().is_empty(),
                "{blank:?}"
            );
        }
        assert!(
            parse_sessions_checked("no server running on /tmp/tmux-1000/default\n")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn checked_parse_keeps_good_lines_among_noise() {
        let out = parse_sessions_checked("warning: x\ngood|1|2|0|/x\n").unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "good");
    }

    #[test]
    fn checked_parse_rejects_content_with_no_parseable_line() {
        let err = parse_sessions_checked("Welcome to delta!\nsession list unavailable: ???\n")
            .unwrap_err();
        assert_eq!(err.code, "E_TMUX");
        assert!(err.message.contains("Welcome to delta!"), "{}", err.message);
        let long = "x".repeat(500);
        let err = parse_sessions_checked(&long).unwrap_err();
        assert!(err.message.len() < 200, "sample is truncated");
    }

    #[test]
    fn detects_no_server_running_classic() {
        assert!(is_no_server_running(
            "no server running on /tmp/tmux-501/default\n"
        ));
    }

    #[test]
    fn detects_socket_file_missing_macos() {
        // What `tmux list-sessions` actually prints on macOS when no server
        // was ever started — the user-reported bug.
        assert!(is_no_server_running(
            "error connecting to /private/tmp/tmux-501/default (No such file or directory)\n"
        ));
    }

    #[test]
    fn detects_socket_file_missing_case_insensitive() {
        assert!(is_no_server_running(
            "Error connecting to /tmp/tmux-501/default (No such file or directory)"
        ));
    }

    #[test]
    fn does_not_swallow_unrelated_errors() {
        assert!(!is_no_server_running("can't find session: dev-foo"));
        assert!(!is_no_server_running("ambiguous option"));
        assert!(!is_no_server_running(""));
    }

    #[test]
    fn pane_command_for_none_falls_back_to_shell_after_claude_exits() {
        let cmd = pane_command_for(None);
        // The semicolon (NOT `||`) after the second `cl` is the whole point:
        // it makes the shell always continue to the exec regardless of `cl`'s
        // exit status. Regression test that the next person who edits this
        // doesn't accidentally use `||` and resurrect the "session dies on
        // /exit" bug.
        assert!(cmd.contains("cl --continue || cl;"), "got: {cmd}");
        assert!(cmd.contains("exec ${SHELL:-/bin/zsh}"), "got: {cmd}");
    }

    #[test]
    fn shell_pane_command_respawns_shell_so_session_survives_exit() {
        let cmd = shell_pane_command(None);
        // The loop is the point: a shell session must NOT die when the user
        // types `exit` — a fresh login shell respawns instead.
        assert!(cmd.contains("while :;"), "got: {cmd}");
        assert!(cmd.contains("${SHELL:-/bin/zsh}"), "got: {cmd}");
    }

    #[test]
    fn shell_pane_command_runs_start_command_then_keeps_pane_alive() {
        let cmd = shell_pane_command(Some("cargo test"));
        // Start command runs, its exit code is printed, then the pane drops
        // to the same respawning shell so the output stays on screen.
        assert!(cmd.contains("{ cargo test; }"), "got: {cmd}");
        assert!(cmd.contains("[exit %s]"), "got: {cmd}");
        assert!(cmd.contains("while :;"), "got: {cmd}");
        // Blank / whitespace-only start command is treated as "no command".
        assert_eq!(shell_pane_command(Some("  ")), shell_pane_command(None));
    }

    #[tokio::test]
    async fn rename_rejects_whitespace_dots_colons_and_empty() {
        // Can't actually run tmux in unit tests; just exercise the validation
        // path. tmux command is never reached.
        assert!(rename_session("a", "").await.is_err());
        assert!(rename_session("a", "   ").await.is_err());
        assert!(rename_session("a", "has space").await.is_err());
        assert!(rename_session("a", "has.dot").await.is_err());
        assert!(rename_session("a", "has:colon").await.is_err());
    }

    #[test]
    fn pane_command_for_resumes_or_creates_with_id() {
        let id = "550e8400-e29b-41d4-a716-446655440000";
        let cmd = pane_command_for(Some(id));
        assert!(cmd.contains(&format!("cl --resume '{id}'")), "got: {cmd}");
        assert!(
            cmd.contains(&format!("cl --session-id '{id}'")),
            "got: {cmd}"
        );
        assert!(cmd.contains("|| cl;"), "bare fallback missing: {cmd}");
        assert!(cmd.contains("exec ${SHELL"), "got: {cmd}");
    }

    #[test]
    fn respawn_pane_in_script_quotes_cwd_name_and_command() {
        let s = respawn_pane_in_script(
            "dev-x",
            std::path::Path::new("/re po/it's"),
            "cl; exec $SHELL",
        );
        assert_eq!(
            s,
            "tmux respawn-pane -k -c '/re po/it'\\''s' -t '=dev-x:' 'cl; exec $SHELL'"
        );
    }

    #[test]
    fn exact_targets_carry_the_no_lookup_prefix() {
        // Verified against tmux 3.6a: a SESSION target takes `=NAME`, a PANE
        // target needs the trailing `:` — `-t '=NAME'` fails there with
        // "can't find pane".
        assert_eq!(exact_session("dev-foo"), "=dev-foo");
        assert_eq!(exact_pane("dev-foo"), "=dev-foo:");
    }

    #[test]
    fn pane_command_for_none_uses_continue() {
        let cmd = pane_command_for(None);
        assert!(cmd.contains("cl --continue || cl;"), "got: {cmd}");
        assert!(!cmd.contains("--session-id"), "got: {cmd}");
    }

    #[test]
    fn mtimes_script_quotes_ids_and_skips_invalid() {
        let ids = vec![
            "44366faf-ae97-426a-91cd-beaf3c74f1d7".to_string(),
            "'; rm -rf / #".to_string(),
        ];
        let s = crate::tmux::transcript_mtimes_script(&ids).unwrap();
        assert!(s.contains("'44366faf-ae97-426a-91cd-beaf3c74f1d7'"));
        assert!(!s.contains("rm -rf"));
        assert!(s.contains("date -r"));
        assert!(crate::tmux::transcript_mtimes_script(&["bad".into()]).is_none());
    }

    #[test]
    fn mtimes_script_runs_against_a_real_projects_tree() {
        // The script itself, through `bash -c` with a throwaway $HOME: finds a
        // transcript in any project dir and skips ids with no transcript.
        let home = tempfile::tempdir().unwrap();
        let proj = home.path().join(".claude/projects/-Users-u-proj");
        std::fs::create_dir_all(&proj).unwrap();
        let found = "44366faf-ae97-426a-91cd-beaf3c74f1d7";
        let missing = "0b8e2f41-9d3c-4a7e-b1f0-6c5d4e3a2b19";
        std::fs::write(proj.join(format!("{found}.jsonl")), "{}\n").unwrap();
        let script = transcript_mtimes_script(&[found.to_string(), missing.to_string()]).unwrap();
        let out = std::process::Command::new("bash")
            .args(["-c", &script])
            .env("HOME", home.path())
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        let m = parse_mtimes(&String::from_utf8_lossy(&out.stdout));
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert_eq!(m.len(), 1, "{m:?}");
        assert!(
            (now - m[found]).abs() < 120,
            "mtime {} vs now {now}",
            m[found]
        );
    }

    #[test]
    fn parse_host_identity_reads_both_fields() {
        let id = parse_host_identity("boot=abc-123\ntmuxrc=0\ntmuxout=4242\n").unwrap();
        assert_eq!(id.boot_id.as_deref(), Some("abc-123"));
        assert_eq!(id.tmux_server_pid, Some(4242));
    }

    #[test]
    fn a_missing_boot_id_is_just_unknown_boot() {
        let id = parse_host_identity("boot=\ntmuxrc=0\ntmuxout=7\n").unwrap();
        assert_eq!(id.boot_id, None);
        assert_eq!(id.tmux_server_pid, Some(7));
    }

    #[test]
    fn unreadable_output_is_unknown_so_it_can_never_mark_a_host_lost() {
        // No tmuxrc=/tmuxout= lines at all: a login banner, a wrapper, a
        // truncated run.
        assert_eq!(parse_host_identity("Welcome to Ubuntu\n"), None);
        assert_eq!(parse_host_identity(""), None);
        // Only one of the two required lines present.
        assert_eq!(parse_host_identity("boot=a\ntmuxrc=0\n"), None);
        assert_eq!(parse_host_identity("boot=a\ntmuxout=4242\n"), None);
        // An rc that is not a number is garbage, not a real exit code.
        assert_eq!(
            parse_host_identity("boot=a\ntmuxrc=not-a-number\ntmuxout=4242\n"),
            None
        );
    }

    #[test]
    fn a_missing_tmux_binary_is_unknown_not_no_server() {
        // exit 127: `tmux` isn't on PATH at all (a login profile edit, a
        // brew relink mid-upgrade). The server may well be alive and
        // holding every session — this must never read as "no server".
        assert_eq!(
            parse_host_identity("boot=a\ntmuxrc=127\ntmuxout=bash: tmux: command not found\n"),
            None
        );
    }

    #[test]
    fn a_protocol_mismatch_is_unknown_not_no_server() {
        // The realistic trigger: `brew`/`apt upgrade tmux` leaves a client
        // that can't talk to the still-running old server.
        assert_eq!(
            parse_host_identity(
                "boot=a\ntmuxrc=1\ntmuxout=protocol version mismatch (client 8, server 7)\n"
            ),
            None
        );
    }

    #[test]
    fn no_server_running_is_the_only_path_to_tmux_server_pid_none() {
        let id = parse_host_identity(
            "boot=a\ntmuxrc=1\ntmuxout=no server running on /tmp/tmux-501/default\n",
        )
        .unwrap();
        assert_eq!(id.tmux_server_pid, None);
    }

    #[test]
    fn a_successful_but_empty_tmux_output_is_unknown_not_no_server() {
        // rc=0 (tmux ran fine) but no pid printed — e.g. `list-sessions`
        // formatted zero lines. We can't tell the pid, so `None`, not a
        // guess at "no server" (the server may hold zero sessions and still
        // be very much alive).
        assert_eq!(parse_host_identity("boot=a\ntmuxrc=0\ntmuxout=\n"), None);
        assert_eq!(
            parse_host_identity("boot=a\ntmuxrc=0\ntmuxout=not-a-pid-either\n"),
            None
        );
    }

    #[tokio::test]
    async fn the_identity_script_runs_under_local_bash() {
        // Real bash, real `tmux` if installed. The script's *shape*
        // (boot=/tmuxrc=/tmuxout= lines) must hold regardless of whether
        // tmux is on PATH or a server is running here; the *outer*
        // classification is only guaranteed trustworthy when tmux itself
        // resolved (the 127 case is pinned explicitly by
        // `the_script_is_unknown_when_tmux_is_missing_from_path`).
        let out = tokio::process::Command::new("bash")
            .args(["-c", HOST_IDENTITY_SCRIPT])
            .output()
            .await
            .unwrap();
        assert!(out.status.success());
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(stdout.lines().any(|l| l.starts_with("boot=")), "{stdout}");
        assert!(stdout.lines().any(|l| l.starts_with("tmuxrc=")), "{stdout}");
        assert!(
            stdout.lines().any(|l| l.starts_with("tmuxout=")),
            "{stdout}"
        );
        let tmux_on_path = tokio::process::Command::new("bash")
            .args(["-c", "command -v tmux"])
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false);
        if tmux_on_path {
            assert!(
                parse_host_identity(&stdout).is_some(),
                "tmux is on PATH, so its rc/out must classify: {stdout}"
            );
        }
    }

    #[tokio::test]
    async fn the_script_is_unknown_when_tmux_is_missing_from_path() {
        // The 127 case this whole fix exists for: a login profile edit or a
        // brew relink mid-upgrade drops tmux's directory off PATH entirely,
        // while the server it can no longer reach is still holding every
        // session. `/usr/bin:/bin` has no `tmux` on macOS (only
        // `/opt/homebrew/bin/tmux` does) or on a stock Linux box, but keeps
        // `cat`/`head` (both under `/bin` or `/usr/bin`) resolvable, so only
        // the tmux half of the script is hidden — the real regression shape.
        let out = tokio::process::Command::new("bash")
            .args(["-c", HOST_IDENTITY_SCRIPT])
            .env("PATH", "/usr/bin:/bin")
            .output()
            .await
            .unwrap();
        assert!(out.status.success());
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        assert_eq!(parse_host_identity(&stdout), None, "{stdout}");
    }

    #[tokio::test]
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    async fn the_boot_id_does_not_depend_on_the_timezone() {
        let run = |tz: &'static str| async move {
            let out = tokio::process::Command::new("bash")
                .args(["-c", HOST_IDENTITY_SCRIPT])
                .env("TZ", tz)
                .output()
                .await
                .unwrap();
            parse_host_identity(&String::from_utf8_lossy(&out.stdout))
                .unwrap()
                .boot_id
        };
        let utc = run("UTC").await;
        assert!(utc.is_some(), "boot id must be readable on this OS");
        assert_eq!(utc, run("America/Los_Angeles").await);
    }

    #[tokio::test]
    async fn remote_read_oauth_account_parses_json_and_is_none_when_it_cannot_tell() {
        use crate::ssh_fake::{FakeSsh, Match, Reply};
        let tmux = |fake: &FakeSsh| RemoteTmux {
            client: fake.clone(),
            host: "h".to_string(),
        };

        // Logged in: the compact JSON `jq -c .oauthAccount` prints.
        let fake = FakeSsh::new();
        fake.on(
            Match::script_contains(".claude.json"),
            Reply::ok(r#"{"accountUuid":"acc-2","emailAddress":"new@x.com","seatTier":null}"#),
        );
        let acc = tmux(&fake)
            .read_oauth_account()
            .await
            .expect("a logged-in host yields its account");
        assert_eq!(acc.uuid.as_deref(), Some("acc-2"));
        assert_eq!(acc.email.as_deref(), Some("new@x.com"));

        // Logged out / no file: the script prints nothing (or `null`) and
        // still exits 0 — no account, never an error.
        for stdout in ["", "\n", "null\n", "{}\n"] {
            let fake = FakeSsh::new();
            fake.on(Match::Any, Reply::ok(stdout));
            assert!(
                tmux(&fake).read_oauth_account().await.is_none(),
                "stdout {stdout:?} must not yield an account"
            );
        }

        // Unreachable host (ssh exit 255) → None.
        let fake = FakeSsh::new();
        fake.unreachable("h");
        assert!(tmux(&fake).read_oauth_account().await.is_none());

        // Spawn error → None.
        let fake = FakeSsh::new();
        fake.on(
            Match::Any,
            Reply::SpawnError {
                message: "no ssh".into(),
            },
        );
        assert!(tmux(&fake).read_oauth_account().await.is_none());
    }

    #[tokio::test]
    async fn remote_host_identity_is_none_on_ssh_failure_and_some_through_a_login_banner() {
        use crate::ssh_fake::{FakeSsh, Match, Reply};
        let tmux = |fake: &FakeSsh| RemoteTmux {
            client: fake.clone(),
            host: "h".to_string(),
        };

        // A login banner (MOTD, a shell wrapper) ahead of the script's own
        // lines must not stop the tagged lines from being found.
        let fake = FakeSsh::new();
        fake.on(
            Match::script_contains("tmux list-sessions"),
            Reply::ok("Welcome to Ubuntu 24.04 LTS\nboot=abc-123\ntmuxrc=0\ntmuxout=4242\n"),
        );
        let id = tmux(&fake)
            .host_identity()
            .await
            .expect("a login banner ahead of the tagged lines must still parse");
        assert_eq!(id.boot_id.as_deref(), Some("abc-123"));
        assert_eq!(id.tmux_server_pid, Some(4242));

        // Unreachable host (ssh exit 255) → None: the transport failed, not
        // "no tmux server".
        let fake = FakeSsh::new();
        fake.unreachable("h");
        assert!(tmux(&fake).host_identity().await.is_none());

        // A non-zero exit from the `bash -lc` invocation itself (distinct
        // from tmux's own exit code, which the script always captures into
        // `tmuxrc=` rather than propagating) → None.
        let fake = FakeSsh::new();
        fake.on(Match::Any, Reply::fail(1, "bash: some unrelated failure"));
        assert!(tmux(&fake).host_identity().await.is_none());
    }

    #[tokio::test]
    async fn remote_transcript_mtimes_is_none_on_failure_and_some_on_success() {
        use crate::ssh_fake::{FakeSsh, Match, Reply};
        let id = "44366faf-ae97-426a-91cd-beaf3c74f1d7".to_string();
        let ids = std::slice::from_ref(&id);
        let tmux = |fake: &FakeSsh| RemoteTmux {
            client: fake.clone(),
            host: "h".to_string(),
        };

        // Non-zero exit → None (not "no transcript").
        let fake = FakeSsh::new();
        fake.on(Match::script_contains("date -r"), Reply::fail(1, "boom"));
        assert_eq!(tmux(&fake).transcript_mtimes(ids).await, None);

        // Unreachable host (ssh exit 255) → None.
        let fake = FakeSsh::new();
        fake.unreachable("h");
        assert_eq!(tmux(&fake).transcript_mtimes(ids).await, None);

        // Spawn error → None.
        let fake = FakeSsh::new();
        fake.on(
            Match::Any,
            Reply::SpawnError {
                message: "no ssh".into(),
            },
        );
        assert_eq!(tmux(&fake).transcript_mtimes(ids).await, None);

        // Success: a map, possibly empty.
        let fake = FakeSsh::new();
        fake.on(
            Match::script_contains("date -r"),
            Reply::ok(&format!("{id}\t1779999999\n")),
        );
        let got = tmux(&fake).transcript_mtimes(ids).await;
        assert_eq!(got.and_then(|m| m.get(&id).copied()), Some(1_779_999_999));
        let fake = FakeSsh::new();
        fake.on(Match::script_contains("date -r"), Reply::ok(""));
        assert_eq!(
            tmux(&fake).transcript_mtimes(ids).await,
            Some(std::collections::HashMap::new())
        );

        // No valid id: nothing to ask, not a failure.
        let fake = FakeSsh::new();
        assert_eq!(
            tmux(&fake).transcript_mtimes(&["bad".into()]).await,
            Some(std::collections::HashMap::new())
        );
        assert!(fake.calls().is_empty());
    }

    #[test]
    fn parse_mtimes_reads_tab_lines_and_ignores_noise() {
        let m = crate::tmux::parse_mtimes(
            "motd\n44366faf-ae97-426a-91cd-beaf3c74f1d7\t1779999999\nx\tnotanumber\n",
        );
        assert_eq!(m.len(), 1);
        assert_eq!(m["44366faf-ae97-426a-91cd-beaf3c74f1d7"], 1_779_999_999);
    }
}
