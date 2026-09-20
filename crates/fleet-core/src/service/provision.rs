//! Provision a host's Claude with the fleet-control skill + MCP server entry.

use crate::ipc_error::lock;
use crate::ipc_error::{codes, IpcError};
use crate::service::hub::HubBase;
use crate::service::tunnel::TunnelSupervisor;
use crate::shell::quote;
use crate::ssh::SshExec;
use crate::store::Store;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const PROVISION_TIMEOUT: Duration = Duration::from_secs(15);

const FLEET_SKILL: &str = include_str!("../../../../skills/claude-fleet-control/SKILL.md");
const SKILL_DIR: &str = "~/.claude/skills/claude-fleet-control";
const SKILL_PATH: &str = "~/.claude/skills/claude-fleet-control/SKILL.md";
const FRIENDLY_NAME_SKILL: &str = include_str!("../../../../skills/fleet-friendly-name/SKILL.md");
const FRIENDLY_NAME_SKILL_DIR: &str = "~/.claude/skills/fleet-friendly-name";
const FRIENDLY_NAME_SKILL_PATH: &str = "~/.claude/skills/fleet-friendly-name/SKILL.md";
const CLAUDE_JSON: &str = "~/.claude.json";
const CLAUDE_DIR: &str = "~/.claude";
const CLAUDE_MD_PATH: &str = "~/.claude/CLAUDE.md";
const TMUX_CONF: &str = "~/.tmux.conf";

/// Sentinel-delimited block claude-fleet maintains in each host's
/// `~/.claude/CLAUDE.md`. Idempotent: appended if absent, refreshed in place
/// if the body changed, left alone if up to date. Anything outside the
/// sentinels is the user's own content and untouched.
const CLAUDE_MD_BEGIN: &str = "<!-- BEGIN claude-fleet managed (do not edit between sentinels) -->";
const CLAUDE_MD_END: &str = "<!-- END claude-fleet managed -->";
const CLAUDE_MD_BODY: &str = "## claude-fleet

This host is managed by claude-fleet (Claude Code sessions in tmux across machines).
Use the **claude-fleet-control** skill to operate sessions over the fleet MCP server.
If you run inside a fleet tmux session, use the **fleet-friendly-name** skill to
label this session; it defines when to fire and how to look up your `host_alias`.";

/// Install the skill + merge the MCP entry on one host. `base.mcp_url()` is the MCP
/// endpoint that host should use; `token` is that host's own bearer token
/// (see [`resolve_host_token`]). Reads `~/.claude.json`, merges (preserving
/// siblings), backs it up, writes it back. Parse errors abort BEFORE any
/// write. Files that carry the token are written with `umask 077` and
/// `chmod 600` (SEC-2).
///
/// `base.hook_url()` is also installed as fleet's `type: "http"` hooks in
/// `~/.claude/settings.json` so the host's Claude Code can notify fleet —
/// without this, safe-kill on remote hosts never finalizes (the marker check
/// is gated on the Stop hook firing). For a loopback hub,
/// `127.0.0.1:<port>` on a remote host IS the reverse tunnel's loopback end
/// (see `tunnel_argv`), so the same URL works on every host; a public hub's
/// URL is reached directly.
pub async fn provision_one(
    ssh: &dyn SshExec,
    host: &str,
    base: &HubBase,
    token: &str,
) -> Result<(), IpcError> {
    // 1. Skills (live-discovered, no restart). Both ship from the repo so
    //    every fleet host gets the same shared copy.
    write_host_file(ssh, host, SKILL_DIR, SKILL_PATH, FLEET_SKILL).await?;
    write_host_file(
        ssh,
        host,
        FRIENDLY_NAME_SKILL_DIR,
        FRIENDLY_NAME_SKILL_PATH,
        FRIENDLY_NAME_SKILL,
    )
    .await?;
    // 1b. Global ~/.claude/CLAUDE.md — keep a managed block in sync so the
    //     fleet-friendly-name skill is invoked on every task start without
    //     the user editing CLAUDE.md by hand.
    provision_claude_md(ssh, host).await?;
    // 2. MCP entry: read → merge (preserve siblings) → back up → write.
    let existing = read_host_file(ssh, host, CLAUDE_JSON).await?;
    let merged = merge_mcp_entry(&existing, &base.mcp_url(), token)?; // errors before any write
    if !existing.trim().is_empty() {
        // The backup carries the previous token too — same mode.
        write_host_file_secret(
            ssh,
            host,
            CLAUDE_DIR,
            &format!("{CLAUDE_JSON}.fleet-bak"),
            &existing,
        )
        .await?;
    }
    write_host_file_secret(ssh, host, CLAUDE_DIR, CLAUDE_JSON, &merged).await?;
    // 3. Ensure tmux clipboard passthrough for OSC 52.
    provision_tmux_clipboard(ssh, host).await?;
    // 4. Stop / WorktreeCreate hooks. Required for safe-kill finalization on
    //    any host that runs Claude Code.
    provision_hook(ssh, host, &base.hook_url(), token).await?;
    Ok(())
}

const SETTINGS_JSON: &str = "~/.claude/settings.json";

/// Merge fleet's hook block (see [`super::hooks_install::FLEET_HOOK_EVENTS`])
/// into the host's `~/.claude/settings.json`, and write the SessionStart
/// command hook's bearer-token headers file. Idempotent — re-running
/// replaces fleet's entries (whatever base URL they pointed at) and leaves
/// the user's own hooks alone. Both files carry (or, for settings.json,
/// reference) the host's bearer token, so both are written 0600.
pub async fn provision_hook(
    ssh: &dyn SshExec,
    host: &str,
    hook_url: &str,
    token: &str,
) -> Result<(), IpcError> {
    let existing = read_host_file(ssh, host, SETTINGS_JSON).await?;
    // Errors (malformed JSON → E_PROVISION) fire BEFORE any write.
    let merged = super::hooks_install::merge_hook_into_settings_json(&existing, hook_url, token)?;
    // The SessionStart command hook reads its bearer token from this file
    // (`curl -H @file`) rather than argv or the command string (SEC-3).
    // Written BEFORE settings.json: a hook installed ahead of its headers
    // file would post without a token until the next provision.
    let headers_path = format!("{CLAUDE_DIR}/{}", super::hooks_install::HOOK_HEADERS_FILE);
    write_host_file_secret(
        ssh,
        host,
        CLAUDE_DIR,
        &headers_path,
        &super::hooks_install::hook_headers_content(token),
    )
    .await?;
    if !existing.trim().is_empty() {
        // The file carries the user's permissions/env/hooks: back it up
        // first, like ~/.claude.json.
        write_host_file_secret(
            ssh,
            host,
            CLAUDE_DIR,
            &format!("{SETTINGS_JSON}.fleet-bak"),
            &existing,
        )
        .await?;
    }
    write_host_file_secret(ssh, host, CLAUDE_DIR, SETTINGS_JSON, &merged).await
}

/// The token a host should be provisioned with: its existing row unless
/// `rotate` (or none yet), in which case a fresh one is minted. The mint is
/// NOT persisted here — [`commit_host_token`] runs after the host's files
/// were written successfully, so a failed provision never strands a host on
/// a token it never received.
pub fn resolve_host_token(
    store: &Mutex<Store>,
    host: &str,
    rotate: bool,
) -> Result<(String, bool), IpcError> {
    let s = lock(store)?;
    match s.get_host_token(host)? {
        Some(row) if !rotate => Ok((row.token, false)),
        _ => Ok((crate::mcp::generate_token(), true)),
    }
}

/// Persist a freshly minted host token (no-op when `minted` is false).
pub fn commit_host_token(
    store: &Mutex<Store>,
    host: &str,
    token: &str,
    minted: bool,
) -> Result<(), IpcError> {
    if !minted {
        return Ok(());
    }
    let s = lock(store)?;
    s.upsert_host_token(host, token)
}

/// An agent host's token, for the operator to hand to the host out of band.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentHostToken {
    pub token: String,
    /// `full` or `readonly`. A readonly token is refused at `/agent`.
    pub mode: String,
    /// Freshly minted by this call (the host had none, or `rotate`).
    pub minted: bool,
}

/// The token an agent host's `fleet-agent` must be installed with — what
/// `fleet-hub agent-token` prints. The existing one unless `rotate` or there
/// is none, in which case a fresh one is minted and COMMITTED before this
/// returns: committing is what revokes the old token, and with it any agent
/// still connected on it. It is the only way an agent host's token reaches
/// the host — never over the agent connection (see
/// [`provision_host_with_token`]).
///
/// Refused for a host that is not an agent host: an SSH host's token is
/// written by provisioning, over SSH.
pub fn agent_host_token(
    store: &Mutex<Store>,
    host: &str,
    rotate: bool,
) -> Result<AgentHostToken, IpcError> {
    let s = lock(store)?;
    let row = s
        .list_hosts()?
        .into_iter()
        .find(|h| h.alias == host)
        .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("no host named {host}")))?;
    if row.transport != "agent" {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!(
                "{host} is not an agent host (transport {}): its token is written by \
                 provisioning, over SSH",
                row.transport
            ),
        ));
    }
    match s.get_host_token(host)? {
        Some(existing) if !rotate => Ok(AgentHostToken {
            token: existing.token,
            mode: existing.mode,
            minted: false,
        }),
        _ => {
            let token = crate::mcp::generate_token();
            s.upsert_host_token(host, &token)?;
            let mode = s
                .get_host_token(host)?
                .map(|r| r.mode)
                .unwrap_or_else(|| "full".into());
            Ok(AgentHostToken {
                token,
                mode,
                minted: true,
            })
        }
    }
}

/// Is `host` reached through a `fleet-agent`?
fn routes_to_agent(store: &Mutex<Store>, host: &str) -> Result<bool, IpcError> {
    let s = lock(store)?;
    Ok(s.agent_host_alias(host)?.as_deref() == Some(host))
}

/// Provision ONE host end to end with its own token: resolve/mint → write
/// files → persist the token → ensure the tunnel (remote host, loopback hub) → mark
/// provisioned. Shared by [`provision_hosts`] and the per-host Rotate action.
pub async fn provision_host_with_token(
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    tunnels: &Arc<TunnelSupervisor>,
    host: &str,
    base: &HubBase,
    rotate: bool,
) -> Result<(), IpcError> {
    let (token, minted) = resolve_host_token(store, host, rotate)?;
    if minted && routes_to_agent(store, host)? {
        // An agent host's new token cannot go the usual way. The usual way
        // writes it to the host FIRST and commits after, so a failed write
        // never strands a host on a token it never received — but for an
        // agent host "writing it to the host" means sending it over the agent
        // connection, which authenticated with the token being replaced. A
        // rotation is how an operator answers a stolen token, so that
        // connection may be the thief's.
        //
        // So: commit first, which revokes the old token and with it the live
        // connection (`HostRouter::agent_alias` drops it before anything else
        // is sent; the endpoint drops it on its next beat), and send nothing.
        // The operator hands the new token to the host out of band.
        commit_host_token(store, host, &token, true)?;
        return Err(IpcError::new(
            codes::E_AGENT_REINSTALL,
            format!(
                "{host} is an agent host: its new token was saved but NOT sent over the agent \
                 connection, which authenticated with the old one and has been cut off. Print it \
                 on the hub with `fleet-hub agent-token {host}`, install it on the host with \
                 `fleet-agent install --token-file -`, then provision {host} again to rewrite \
                 its hooks"
            ),
        ));
    }
    provision_one(ssh, host, base, &token).await?;
    commit_host_token(store, host, &token, minted)?;
    // A public hub is reached directly; only a loopback hub needs the
    // reverse tunnel so the host's 127.0.0.1:<port> lands on this machine.
    // An agent host is never dialed over SSH at all, so it has no use for
    // one either — it reaches the hub over its own outbound connection.
    if host != "local" && !base.public && !routes_to_agent(store, host)? {
        tunnels.ensure(host, base.port, base.port);
    }
    if let Ok(s) = store.lock() {
        let _ = s.set_host_provisioned(host, true);
    }
    Ok(())
}

/// Ensure `~/.tmux.conf` has `set -g set-clipboard on` for OSC 52 passthrough.
/// Appends the setting if not already present; creates the file if missing.
pub async fn provision_tmux_clipboard(ssh: &dyn SshExec, host: &str) -> Result<(), IpcError> {
    let existing = read_host_file(ssh, host, TMUX_CONF).await?;
    if has_tmux_clipboard_setting(&existing) {
        return Ok(());
    }
    let home_dir = if host == "local" {
        std::env::var("HOME").unwrap_or_default()
    } else {
        "~".to_string()
    };
    let dir = &home_dir;
    let addition = "\n# Enable OSC 52 clipboard (added by claude-fleet)\nset -g set-clipboard on\n";
    let merged = format!("{}{}", existing.trim_end(), addition);
    write_host_file(ssh, host, dir, TMUX_CONF, &merged).await
}

/// Ensure `~/.claude/CLAUDE.md` on `host` contains the claude-fleet managed
/// block (sentinel-delimited). Idempotent: writes only when the block is
/// missing or its body drifted. Everything outside the sentinels is the
/// user's own content and is preserved verbatim.
pub async fn provision_claude_md(ssh: &dyn SshExec, host: &str) -> Result<(), IpcError> {
    let existing = read_host_file(ssh, host, CLAUDE_MD_PATH).await?;
    let Some(merged) = merge_claude_md(&existing, CLAUDE_MD_BEGIN, CLAUDE_MD_END, CLAUDE_MD_BODY)
    else {
        return Ok(()); // already up to date
    };
    write_host_file(ssh, host, CLAUDE_DIR, CLAUDE_MD_PATH, &merged).await
}

/// Pure: merge the sentinel-delimited block into `existing`. Returns `None`
/// when the block is already present and the body matches (no write needed);
/// `Some(new)` with the updated content otherwise. New content is appended at
/// EOF when the block is missing entirely.
fn merge_claude_md(existing: &str, begin: &str, end: &str, body: &str) -> Option<String> {
    let block = format!("{begin}\n{body}\n{end}");
    if let Some(b) = existing.find(begin) {
        if let Some(e_rel) = existing[b..].find(end) {
            let e = b + e_rel + end.len();
            let current = &existing[b..e];
            if current == block {
                return None;
            }
            let mut out = String::with_capacity(existing.len() + body.len());
            out.push_str(&existing[..b]);
            out.push_str(&block);
            out.push_str(&existing[e..]);
            return Some(out);
        }
        // Begin sentinel present but end missing — user damaged the block.
        // Treat as "needs refresh" and append a fresh one rather than guess
        // where the corrupted block ends.
    }
    let sep = if existing.is_empty() || existing.ends_with("\n\n") {
        ""
    } else if existing.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };
    Some(format!("{existing}{sep}{block}\n"))
}

/// Check if tmux.conf already has a set-clipboard directive.
fn has_tmux_clipboard_setting(content: &str) -> bool {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        // Match: set -g set-clipboard, set-option -g set-clipboard, etc.
        if trimmed.contains("set-clipboard") {
            return true;
        }
    }
    false
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostProvisionResult {
    pub host: String,
    /// "provisioned" | "skipped" | "failed"
    pub status: String,
    pub detail: Option<String>,
}

/// Provision every non-hidden host, each with its OWN bearer token (reused
/// unless `rotate`). Every host is pointed at `base`; for a loopback hub the
/// remote hosts also get the reverse tunnel that makes its localhost URL
/// work there (`local` never needs one, nor does any host of a public hub).
/// Per-host failures never abort the others.
pub async fn provision_hosts(
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    tunnels: &Arc<TunnelSupervisor>,
    base: &HubBase,
    rotate: bool,
) -> Result<Vec<HostProvisionResult>, IpcError> {
    let hosts = {
        let s = lock(store)?;
        s.list_hosts()?
    };
    let mut results = Vec::new();
    for h in hosts {
        if h.hidden {
            continue;
        }
        if h.alias != "local" && !h.reachable {
            results.push(HostProvisionResult {
                host: h.alias,
                status: "skipped".into(),
                detail: Some("unreachable".into()),
            });
            continue;
        }
        match provision_host_with_token(store, ssh, tunnels, &h.alias, base, rotate).await {
            Ok(()) => {
                results.push(HostProvisionResult {
                    host: h.alias,
                    status: "provisioned".into(),
                    detail: Some("restart Claude on this host to load the MCP server".into()),
                });
            }
            Err(e) => results.push(HostProvisionResult {
                host: h.alias,
                status: "failed".into(),
                detail: Some(e.message),
            }),
        }
    }
    Ok(results)
}

/// Re-establish tunnels for already-provisioned remote hosts (app start / MCP
/// re-enable). Does NOT re-write config. A no-op for a public hub, whose
/// hosts reach it directly.
pub fn reestablish_tunnels(
    store: &Mutex<Store>,
    tunnels: &Arc<TunnelSupervisor>,
    base: &HubBase,
) -> Result<(), IpcError> {
    if base.public {
        return Ok(());
    }
    let hosts = { lock(store)?.list_hosts()? };
    for h in hosts {
        if h.provisioned && h.alias != "local" && !h.hidden && h.transport != "agent" {
            tunnels.ensure(&h.alias, base.port, base.port);
        }
    }
    Ok(())
}

/// Read a file from a host. `local` → `std::fs`; remote → `cat` over SSH.
/// Missing file → `Ok(String::new())` (caller treats as empty config).
pub async fn read_host_file(ssh: &dyn SshExec, host: &str, path: &str) -> Result<String, IpcError> {
    if host == "local" {
        crate::service::hub::ensure_local_allowed(host)?;
        let expanded = expand_home_local(path)?;
        return Ok(std::fs::read_to_string(&expanded).unwrap_or_default());
    }
    // Outer `quote` makes the whole script cross the SSH boundary as ONE shell
    // word — ssh space-joins argv, so an unquoted multi-word script would be
    // re-split by the remote login shell (mirrors claude_cli.rs).
    let script = quote(&remote_read_script(path));
    let out = ssh
        .run(host, &["bash", "-lc", &script], PROVISION_TIMEOUT)
        .await?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Write a file to a host (creating parent dirs). `local` → fs; remote → a
/// shell that `mkdir -p`s the parent and `printf '%s'`s the (shell-quoted)
/// content to `path`. `dir` is the parent dir; `path` the file.
pub async fn write_host_file(
    ssh: &dyn SshExec,
    host: &str,
    dir: &str,
    path: &str,
    content: &str,
) -> Result<(), IpcError> {
    if host == "local" {
        crate::service::hub::ensure_local_allowed(host)?;
        let edir = expand_home_local(dir)?;
        std::fs::create_dir_all(&edir)
            .map_err(|e| IpcError::new(codes::E_PROVISION, format!("mkdir {edir}: {e}")))?;
        let epath = expand_home_local(path)?;
        std::fs::write(&epath, content)
            .map_err(|e| IpcError::new(codes::E_PROVISION, format!("write {epath}: {e}")))?;
        return Ok(());
    }
    let script = quote(&remote_write_script(dir, path, content));
    run_remote_write(ssh, host, path, &script).await
}

/// Like [`write_host_file`] for a file that carries a secret (the bearer
/// token). The content never appears in a process argv on either side, and
/// the target is never truncated in place — a write is either fully applied
/// or leaves the previous content untouched:
///
/// - remote: an empty `<path>.fleet-tmp` is created under `umask 077` +
///   `chmod 600` (a script with only paths in it), the content is streamed
///   over stdin through `SshExec::upload_file` (`cat > <path>.fleet-tmp`),
///   then `mv -f <path>.fleet-tmp <path>` renames it onto the target
///   atomically. The tmp file is removed (best-effort) if any step fails;
/// - local: the content is written to `<path>.fleet-tmp` with mode 0600
///   ([`write_private_file`]), then renamed onto `path`; the tmp file is
///   removed if either step fails.
pub async fn write_host_file_secret(
    ssh: &dyn SshExec,
    host: &str,
    dir: &str,
    path: &str,
    content: &str,
) -> Result<(), IpcError> {
    let tmp_path = format!("{path}.fleet-tmp");
    if host == "local" {
        crate::service::hub::ensure_local_allowed(host)?;
        let edir = expand_home_local(dir)?;
        std::fs::create_dir_all(&edir)
            .map_err(|e| IpcError::new(codes::E_PROVISION, format!("mkdir {edir}: {e}")))?;
        let epath = expand_home_local(path)?;
        let etmp = expand_home_local(&tmp_path)?;
        let etmp_path = std::path::Path::new(&etmp);
        if let Err(e) = write_private_file(etmp_path, content) {
            let _ = std::fs::remove_file(etmp_path);
            return Err(IpcError::new(
                codes::E_PROVISION,
                format!("write {epath}: {e}"),
            ));
        }
        if let Err(e) = place_private_file(etmp_path, std::path::Path::new(&epath), |f, t| {
            std::fs::rename(f, t)
        }) {
            return Err(IpcError::new(
                codes::E_PROVISION,
                format!("write {epath}: {e}"),
            ));
        }
        return Ok(());
    }
    // 1. Create the (empty) tmp file 0600 — paths only, no secret in argv.
    let script = quote(&remote_touch_private_script(dir, &tmp_path));
    run_remote_write(ssh, host, &tmp_path, &script).await?;
    // 2. Stream the content over stdin to the tmp path. `upload_file` runs
    //    `cat > '<abs>'`; `~` is not expanded in a quoted word, so resolve
    //    $HOME first.
    let abs_tmp = match tmp_path.strip_prefix("~/") {
        Some(rest) => format!("{}/{rest}", ssh.remote_home(host).await?),
        None => tmp_path.clone(),
    };
    let spool = PrivateTempFile::create(content)
        .map_err(|e| IpcError::new(codes::E_PROVISION, format!("spool secret for {path}: {e}")))?;
    if let Err(e) = ssh
        .upload_file(host, spool.path(), &abs_tmp, PROVISION_TIMEOUT)
        .await
    {
        remove_remote_tmp(ssh, host, &tmp_path).await;
        return Err(IpcError::new(
            codes::E_PROVISION,
            format!("write {path} on {host}: {}", e.message),
        ));
    }
    // 3. Rename the tmp file onto the real path — atomic, no window where
    //    `path` is truncated but not yet rewritten.
    let mv_script = quote(&remote_rename_script(&tmp_path, path));
    if let Err(e) = run_remote_write(ssh, host, path, &mv_script).await {
        remove_remote_tmp(ssh, host, &tmp_path).await;
        return Err(e);
    }
    Ok(())
}

/// Best-effort cleanup of a `.fleet-tmp` file left behind by a failed
/// [`write_host_file_secret`] step. Never fails the caller — the write
/// already failed for its own reason, and a stray 0600 tmp file under the
/// user's own `~/.claude` is not a security issue, just clutter.
async fn remove_remote_tmp(ssh: &dyn SshExec, host: &str, tmp_path: &str) {
    let script = quote(&format!("rm -f {}", remote_path(tmp_path)));
    let _ = ssh
        .run(host, &["bash", "-lc", &script], PROVISION_TIMEOUT)
        .await;
}

/// Write `content` to `path`, creating the file with mode 0600 (unix) so it is
/// never world-readable, not even between create and chmod. An existing file
/// is truncated in place and tightened to 0600.
pub fn write_private_file(path: &std::path::Path, content: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    f.write_all(content.as_bytes())?;
    f.flush()?;
    // `mode()` only applies at creation; tighten a pre-existing file too.
    set_private_mode(path);
    Ok(())
}

/// A 0600 temp file holding secret content for the duration of an upload;
/// removed on drop.
struct PrivateTempFile(std::path::PathBuf);

impl PrivateTempFile {
    fn create(content: &str) -> std::io::Result<Self> {
        let name = format!(
            "claude-fleet-{}-{}.secret",
            std::process::id(),
            crate::mcp::generate_token()
        );
        let path = std::env::temp_dir().join(name);
        write_private_file(&path, content)?;
        Ok(Self(path))
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for PrivateTempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// `chmod 600` a local file. Best-effort: a failure is logged, never fatal
/// (the write itself already succeeded). No-op off unix.
pub fn set_private_mode(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "[provision] chmod 600 failed; the file may be readable by other users"
            );
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

async fn run_remote_write(
    ssh: &dyn SshExec,
    host: &str,
    path: &str,
    script: &str,
) -> Result<(), IpcError> {
    let out = ssh
        .run(host, &["bash", "-lc", script], PROVISION_TIMEOUT)
        .await?;
    if !out.status.success() {
        return Err(IpcError::new(
            codes::E_PROVISION,
            format!(
                "write {path} on {host}: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        ));
    }
    Ok(())
}

/// Render a path as a token for a remote `bash -lc` script. A leading `~/` is
/// emitted as `"$HOME"/<rest>` so the home dir expands on the remote — `quote`
/// would single-quote `~` and defeat tilde expansion, creating a literal `~`
/// directory. `$HOME` is double-quoted (literal through the outer `quote`, then
/// expanded by the remote `bash`); the rest of the path is `quote`-quoted inert.
fn remote_path(path: &str) -> String {
    if path == "~" {
        // A bare `~` (the parent dir of `~/.tmux.conf`) must expand too —
        // `quote` would turn it into a literal `'~'` and `mkdir -p` would
        // create a directory named `~` in the remote cwd.
        return "\"$HOME\"".to_string();
    }
    match path.strip_prefix("~/") {
        Some(rest) => format!("\"$HOME\"/{}", quote(rest)),
        None => quote(path),
    }
}

/// Remote `bash -lc` script body that reads `path` (missing file → empty stdout).
fn remote_read_script(path: &str) -> String {
    format!("cat {} 2>/dev/null || true", remote_path(path))
}

/// Remote `bash -lc` script body that creates `dir` then writes `content` to `path`.
fn remote_write_script(dir: &str, path: &str, content: &str) -> String {
    format!(
        "mkdir -p {} && printf '%s' {} > {}",
        remote_path(dir),
        quote(content),
        remote_path(path)
    )
}

/// Remote `bash -lc` script body that makes sure `path` exists with mode
/// 0600 WITHOUT writing any content: `umask 077` so a NEW file is born 0600
/// (no window where it is world-readable), `chmod 600` so an EXISTING file
/// is tightened too. The secret itself follows over stdin
/// (see [`write_host_file_secret`]), so it is never part of this argv.
fn remote_touch_private_script(dir: &str, path: &str) -> String {
    let p = remote_path(path);
    format!(
        "umask 077 && mkdir -p {} && touch {p} && chmod 600 {p}",
        remote_path(dir),
    )
}

/// Move `tmp` onto `target`, falling back to a copy when the rename fails.
///
/// The local twin of [`remote_rename_script`]'s fallback, and for the same
/// reason: a bind-mounted target cannot be replaced by a rename (EBUSY), only
/// written through. `rename` is a parameter so the fallback is testable
/// without a second filesystem or a real mount.
///
/// The copy branch forces mode 0600 — a copy onto an existing file keeps
/// THAT file's mode, and every caller is writing a secret — and leaves `tmp`
/// in place when it fails, so the content is still recoverable.
fn place_private_file(
    tmp: &std::path::Path,
    target: &std::path::Path,
    mut rename: impl FnMut(&std::path::Path, &std::path::Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
    if rename(tmp, target).is_ok() {
        return Ok(());
    }
    std::fs::copy(tmp, target)?;
    std::fs::set_permissions(
        target,
        <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o600),
    )?;
    let _ = std::fs::remove_file(tmp);
    Ok(())
}

/// Remote `bash -lc` script body that renames `tmp` onto `path` — the last
/// step of [`write_host_file_secret`]'s tmp-file dance. The rename is the
/// normal path: `path` is not truncated, so a reader either sees the old
/// content or the new, never a partial write.
///
/// The fallback copies the content in place instead. `mv` fails with EBUSY
/// when the target is a bind mount, which is how `~/.claude.json` is set up
/// in the containerised hosts (`claude-fleet-host`): the mount point cannot
/// be replaced, only written through. Without the fallback provisioning
/// aborts there with "Device or resource busy" and the host keeps whatever
/// hooks and MCP entry it had — the failure this exists for.
///
/// Writing in place is the weaker guarantee (a reader can catch a truncated
/// file, and a failure mid-write leaves it short), so it is only reached
/// after the rename failed. The tmp file is deliberately kept when the copy
/// fails, so the content is still recoverable on the host. `chmod 600`
/// follows the copy because an in-place write keeps the TARGET's mode, and
/// every caller of this script is writing a secret ([`write_host_file_secret`]
/// is the only one) — a pre-existing 0644 file must not keep that mode.
fn remote_rename_script(tmp: &str, path: &str) -> String {
    let (t, p) = (remote_path(tmp), remote_path(path));
    format!("mv -f {t} {p} 2>/dev/null || {{ cat {t} > {p} && chmod 600 {p} && rm -f {t}; }}")
}

/// Expand a leading `~/` — or a bare `~` — against the LOCAL home dir.
/// The bare form is the parent directory of a dotfile that lives directly in
/// `$HOME` (`~/.claude.json`, `~/.tmux.conf`), so it reaches `create_dir_all`
/// on the local path; leaving it literal would create a directory named `~`
/// in the process's cwd (`remote_path` handles the same case remotely).
pub(crate) fn expand_home_local(path: &str) -> Result<String, IpcError> {
    let home =
        || std::env::var("HOME").map_err(|_| IpcError::new(codes::E_PROVISION, "HOME not set"));
    if path == "~" {
        return home();
    }
    if let Some(rest) = path.strip_prefix("~/") {
        Ok(format!("{}/{rest}", home()?))
    } else {
        Ok(path.to_string())
    }
}

/// Merge the claude-fleet HTTP MCP server entry into a host's `~/.claude.json`
/// content, preserving every existing key. Returns the new JSON (pretty).
/// Errors if `existing` is non-empty and not valid JSON.
pub fn merge_mcp_entry(existing: &str, url: &str, token: &str) -> Result<String, IpcError> {
    let mut root: serde_json::Value = if existing.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(existing).map_err(|e| {
            IpcError::new(
                codes::E_PROVISION,
                format!("~/.claude.json is not valid JSON: {e}"),
            )
        })?
    };
    if !root.is_object() {
        return Err(IpcError::new(
            codes::E_PROVISION,
            "~/.claude.json is not a JSON object",
        ));
    }
    let servers = root
        .as_object_mut()
        .unwrap()
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}));
    if !servers.is_object() {
        return Err(IpcError::new(
            codes::E_PROVISION,
            "mcpServers is not a JSON object",
        ));
    }
    servers.as_object_mut().unwrap().insert(
        "claude-fleet".to_string(),
        serde_json::json!({
            "type": "http",
            "url": url,
            "headers": { "Authorization": format!("Bearer {token}") }
        }),
    );
    serde_json::to_string_pretty(&root)
        .map_err(|e| IpcError::new(codes::E_PROVISION, format!("serialize: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_adds_entry_to_empty() {
        let out = merge_mcp_entry("", "http://127.0.0.1:4180/mcp", "tok").unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["mcpServers"]["claude-fleet"]["type"], "http");
        assert_eq!(
            v["mcpServers"]["claude-fleet"]["url"],
            "http://127.0.0.1:4180/mcp"
        );
        assert_eq!(
            v["mcpServers"]["claude-fleet"]["headers"]["Authorization"],
            "Bearer tok"
        );
    }

    #[test]
    fn merge_preserves_siblings_and_is_idempotent() {
        let existing = r#"{"oauthAccount":{"email":"x@y.z"},"mcpServers":{"other":{"type":"http","url":"u"}}}"#;
        let out = merge_mcp_entry(existing, "http://127.0.0.1:4180/mcp", "tok").unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["oauthAccount"]["email"], "x@y.z");
        assert_eq!(v["mcpServers"]["other"]["url"], "u");
        assert_eq!(
            v["mcpServers"]["claude-fleet"]["url"],
            "http://127.0.0.1:4180/mcp"
        );
        let out2 = merge_mcp_entry(&out, "http://127.0.0.1:4180/mcp", "tok2").unwrap();
        let v2: serde_json::Value = serde_json::from_str(&out2).unwrap();
        assert_eq!(
            v2["mcpServers"]["claude-fleet"]["headers"]["Authorization"],
            "Bearer tok2"
        );
        assert_eq!(v2["mcpServers"]["other"]["url"], "u");
    }

    #[test]
    fn merge_rejects_invalid_json() {
        assert!(merge_mcp_entry("not json", "u", "t").is_err());
    }

    #[test]
    fn remote_path_expands_home_not_quotes_tilde() {
        // `~/` must become an expandable `$HOME` token, NOT a single-quoted
        // literal `~` (which the remote would treat as a directory named `~`).
        let t = remote_path("~/.claude/skills/claude-fleet-control");
        assert_eq!(t, "\"$HOME\"/'.claude/skills/claude-fleet-control'");
        assert!(!t.starts_with("'~"));
        // A bare `~` (parent dir of `~/.tmux.conf`) expands too — it used to
        // become `'~'`, and `mkdir -p '~'` created a literal `~` directory.
        assert_eq!(remote_path("~"), "\"$HOME\"");
        // Absolute paths are quoted whole.
        assert_eq!(remote_path("/etc/hosts"), "'/etc/hosts'");
    }

    #[test]
    fn remote_read_script_targets_home() {
        assert_eq!(
            remote_read_script("~/.claude.json"),
            "cat \"$HOME\"/'.claude.json' 2>/dev/null || true"
        );
    }

    #[test]
    fn remote_write_script_mkdirs_and_writes_under_home() {
        let s = remote_write_script("~/.claude", "~/.claude.json", "{\"a\":1}");
        assert_eq!(
            s,
            "mkdir -p \"$HOME\"/'.claude' && printf '%s' '{\"a\":1}' > \"$HOME\"/'.claude.json'"
        );
        // The whole script survives the SSH boundary as one shell word once
        // wrapped — outer quote leaves the inner `"$HOME"` intact for the
        // remote bash to expand.
        let quoted = crate::shell::quote(&s);
        assert!(quoted.starts_with('\'') && quoted.ends_with('\''));
        assert!(quoted.contains("\"$HOME\""));
    }

    #[test]
    fn remote_rename_script_falls_back_to_an_in_place_write() {
        let s = remote_rename_script("~/.claude.json.fleet-tmp", "~/.claude.json");
        // The rename is still the normal path…
        assert!(s.starts_with("mv -f \"$HOME\"/'.claude.json.fleet-tmp' \"$HOME\"/'.claude.json'"));
        // …and the fallback writes THROUGH the target, which is the only way
        // onto a bind-mounted file (EBUSY on rename): `claude-fleet-host`
        // containers mount ~/.claude.json in.
        assert!(
            s.contains("|| { cat \"$HOME\"/'.claude.json.fleet-tmp' > \"$HOME\"/'.claude.json'")
        );
        // The secret must not inherit the old file's mode, and the tmp file
        // is only removed once the copy succeeded.
        assert!(s.contains("&& chmod 600 \"$HOME\"/'.claude.json' && rm -f"));
        let quoted = crate::shell::quote(&s);
        assert!(quoted.starts_with('\'') && quoted.ends_with('\''));
        assert!(quoted.contains("\"$HOME\""));
    }

    /// The local twin of the remote fallback: when the rename fails the way
    /// a bind-mounted target makes it fail, the content must still land, at
    /// 0600 even though the target was world-readable, with no tmp left.
    #[test]
    fn place_private_file_falls_back_to_a_copy_when_the_rename_fails() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let tmp = dir.path().join("secret.json.fleet-tmp");
        let target = dir.path().join("secret.json");
        std::fs::write(&tmp, "s3cret").unwrap();
        std::fs::write(&target, "old").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();

        place_private_file(&tmp, &target, |_, _| {
            Err(std::io::Error::from_raw_os_error(16)) // EBUSY, as on a bind mount
        })
        .expect("the copy fallback");

        assert_eq!(std::fs::read_to_string(&target).unwrap(), "s3cret");
        assert_eq!(
            std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(
            !tmp.exists(),
            "the tmp file is removed once the copy landed"
        );
    }

    /// A rename that works is still the normal path — the fallback must not
    /// run (and so must not need the target to exist).
    #[test]
    fn place_private_file_prefers_the_rename() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = dir.path().join("a.fleet-tmp");
        let target = dir.path().join("a");
        std::fs::write(&tmp, "v").unwrap();
        let mut renamed = false;
        place_private_file(&tmp, &target, |f, t| {
            renamed = true;
            std::fs::rename(f, t)
        })
        .unwrap();
        assert!(renamed);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "v");
        assert!(!tmp.exists());
    }

    #[test]
    fn remote_touch_private_script_sets_umask_and_chmods_without_content() {
        let s = remote_touch_private_script("~/.claude", "~/.claude.json");
        assert_eq!(
            s,
            "umask 077 && mkdir -p \"$HOME\"/'.claude' && touch \"$HOME\"/'.claude.json' \
             && chmod 600 \"$HOME\"/'.claude.json'"
        );
        assert!(s.starts_with("umask 077 && "));
        // Paths only: the secret content is streamed over stdin, never argv.
        assert!(!s.contains("printf"));
    }

    #[cfg(unix)]
    #[test]
    fn write_private_file_creates_0600_and_tightens_existing() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        // Fresh file: 0600 from creation.
        let p = dir.path().join("new.json");
        write_private_file(&p, "{\"a\":1}").unwrap();
        assert_eq!(mode(&p), 0o600);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "{\"a\":1}");
        // Existing world-readable file: truncated, rewritten, tightened.
        let q = dir.path().join("old.json");
        std::fs::write(&q, "old longer content").unwrap();
        std::fs::set_permissions(&q, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_private_file(&q, "new").unwrap();
        assert_eq!(mode(&q), 0o600);
        assert_eq!(std::fs::read_to_string(&q).unwrap(), "new");
        // The upload spool file is 0600 and removed on drop.
        let spool_path = {
            let t = PrivateTempFile::create("s3cret").unwrap();
            assert_eq!(mode(t.path()), 0o600);
            t.path().to_path_buf()
        };
        assert!(!spool_path.exists(), "spool file must be removed on drop");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn write_host_file_secret_local_is_atomic_and_leaves_no_tmp_behind() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret.json");
        // `host == "local"` never touches `ssh`; an unscripted `FakeSsh`
        // would panic if it somehow did.
        let fake = FakeSsh::new();
        write_host_file_secret(
            &fake,
            "local",
            dir.path().to_str().unwrap(),
            path.to_str().unwrap(),
            "s3cret",
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "s3cret");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert!(
            !dir.path().join("secret.json.fleet-tmp").exists(),
            "the .fleet-tmp sibling must not survive a successful write"
        );
        assert!(fake.calls().is_empty(), "local writes never touch ssh");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn write_host_file_secret_local_failure_leaves_the_original_untouched() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret.json");
        std::fs::write(&path, "original").unwrap();
        // Make the directory unwritable so creating `secret.json.fleet-tmp`
        // fails — the failure mode this test simulates.
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
        let fake = FakeSsh::new();
        let result = write_host_file_secret(
            &fake,
            "local",
            dir.path().to_str().unwrap(),
            path.to_str().unwrap(),
            "new-secret",
        )
        .await;
        // Restore write access so the tempdir can clean itself up.
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(result.is_err(), "an unwritable dir must fail the write");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "original",
            "a failed write must never touch the existing file"
        );
        assert!(!dir.path().join("secret.json.fleet-tmp").exists());
    }

    #[test]
    fn resolve_host_token_reuses_unless_rotate_and_commit_persists_only_mints() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        // No row yet → minted.
        let (t1, minted) = resolve_host_token(&store, "mefistos", false).unwrap();
        assert!(minted);
        assert_eq!(t1.len(), 64);
        // Not committed until the host's files were written.
        assert!(store
            .lock()
            .unwrap()
            .get_host_token("mefistos")
            .unwrap()
            .is_none());
        commit_host_token(&store, "mefistos", &t1, minted).unwrap();
        assert_eq!(
            store
                .lock()
                .unwrap()
                .get_host_token("mefistos")
                .unwrap()
                .unwrap()
                .token,
            t1
        );
        // Existing row, no rotate → reused verbatim, nothing to commit.
        let (t2, minted) = resolve_host_token(&store, "mefistos", false).unwrap();
        assert_eq!(t2, t1);
        assert!(!minted);
        commit_host_token(&store, "mefistos", "ignored", minted).unwrap();
        assert_eq!(
            store
                .lock()
                .unwrap()
                .get_host_token("mefistos")
                .unwrap()
                .unwrap()
                .token,
            t1
        );
        // rotate → fresh token.
        let (t3, minted) = resolve_host_token(&store, "mefistos", true).unwrap();
        assert!(minted);
        assert_ne!(t3, t1);
    }

    #[cfg(unix)]
    #[test]
    fn set_private_mode_chmods_600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("secret.json");
        std::fs::write(&p, "{}").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        set_private_mode(&p);
        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn expand_home_local_expands_tilde() {
        // `HOME` is process-wide and this test never previously restored it,
        // so once this test ran, every other test in the same (parallel,
        // multi-threaded) `cargo test` process would see `HOME=/Users/test`
        // for the rest of the run — a nonexistent directory on Linux. That
        // silently corrupted anything spawning a real child process that
        // relies on `$HOME` (e.g. the catalog scan script's `cd "$HOME"`),
        // which used to fail *open* (an empty-but-"successful" snapshot) and
        // so never surfaced here; it now fails *closed* (`E_SCAN`) per the
        // asset-catalog hardening review, which is what actually exposed
        // this pre-existing hazard. Save and restore the real value so this
        // test's mutation cannot leak into any other test.
        //
        // Restoring is not enough while it runs: the catalog tests point
        // `HOME` at a temp dir and scan / write through it, so an unserialised
        // swap mid-test sends them to `/Users/test` (a failed scan) or makes
        // this test restore their temp dir — the intermittent
        // `applies_creates_conflicts_overwrites_removals_and_merges_locally`
        // failure. Take the lock they already hold around `HOME`.
        let _lock = crate::service::catalog::CATALOG_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", "/Users/test");
        assert_eq!(
            super::expand_home_local("~/.claude.json").unwrap(),
            "/Users/test/.claude.json"
        );
        // A bare `~` is the parent dir of `~/.claude.json`; it must expand
        // too, or a local write there would `create_dir_all("~")` in the
        // process's cwd (mirrors `remote_path`).
        assert_eq!(super::expand_home_local("~").unwrap(), "/Users/test");
        assert_eq!(super::expand_home_local("/abs/path").unwrap(), "/abs/path");
        match original_home {
            Some(home) => std::env::set_var("HOME", home),
            None => std::env::remove_var("HOME"),
        }
    }

    #[test]
    fn has_tmux_clipboard_detects_setting() {
        assert!(!super::has_tmux_clipboard_setting(""));
        assert!(!super::has_tmux_clipboard_setting("set -g mouse on\n"));
        assert!(super::has_tmux_clipboard_setting(
            "set -g set-clipboard on\n"
        ));
        assert!(super::has_tmux_clipboard_setting(
            "set-option -g set-clipboard on\n"
        ));
        assert!(super::has_tmux_clipboard_setting(
            "  set -g set-clipboard external\n"
        ));
        // Comments don't count
        assert!(!super::has_tmux_clipboard_setting(
            "# set -g set-clipboard on\n"
        ));
    }

    const B: &str = "<!-- BEGIN -->";
    const E: &str = "<!-- END -->";

    #[test]
    fn merge_claude_md_appends_when_block_missing() {
        let out = super::merge_claude_md("# my notes\n", B, E, "body").unwrap();
        assert!(out.starts_with("# my notes\n"));
        assert!(out.contains(&format!("{B}\nbody\n{E}")));
    }

    #[test]
    fn merge_claude_md_appends_to_empty_file() {
        let out = super::merge_claude_md("", B, E, "body").unwrap();
        assert_eq!(out, format!("{B}\nbody\n{E}\n"));
    }

    #[test]
    fn merge_claude_md_is_idempotent_when_body_matches() {
        let block = format!("{B}\nbody\n{E}");
        let initial = format!("intro\n\n{block}\n");
        assert!(super::merge_claude_md(&initial, B, E, "body").is_none());
    }

    #[test]
    fn merge_claude_md_refreshes_when_body_drifted() {
        let initial = format!("intro\n\n{B}\nold body\n{E}\n");
        let out = super::merge_claude_md(&initial, B, E, "new body").unwrap();
        assert!(out.contains(&format!("{B}\nnew body\n{E}")));
        assert!(!out.contains("old body"));
        assert!(out.starts_with("intro\n"));
    }

    #[test]
    fn merge_claude_md_preserves_content_outside_sentinels() {
        let initial = format!("before\n{B}\nold\n{E}\nafter\n");
        let out = super::merge_claude_md(&initial, B, E, "new").unwrap();
        assert!(out.starts_with("before\n"));
        assert!(out.ends_with("after\n"));
    }

    #[test]
    fn claude_md_body_is_a_short_pointer_to_the_skills() {
        // The managed block used to duplicate the fleet-friendly-name skill
        // body; the skill is the single source of truth, so the block only
        // says what claude-fleet is and which skills to use. Keep it short —
        // it lands in every host's global CLAUDE.md.
        let body = super::CLAUDE_MD_BODY;
        assert!(
            body.contains("claude-fleet-control"),
            "managed CLAUDE.md block must point at the claude-fleet-control skill"
        );
        assert!(
            body.contains("fleet-friendly-name"),
            "managed CLAUDE.md block must point at the fleet-friendly-name skill"
        );
        assert!(
            !body.contains("hostname"),
            "alias lookup rules live in the skill, not the managed block"
        );
        let lines = body.lines().filter(|l| !l.trim().is_empty()).count();
        assert!(
            lines <= 6,
            "managed block must stay short, has {lines} lines"
        );
    }

    // ── end-to-end through the real provision functions over `FakeSsh` ──────

    use crate::ssh_fake::{Call, FakeSsh, Match, Reply};

    const URL: &str = "http://127.0.0.1:4180/mcp";
    const TOKEN: &str = "tok-s3cret-0123456789abcdef";
    const PORT: u16 = 4180;

    fn base() -> HubBase {
        HubBase::loopback(PORT)
    }
    const HOME: &str = "/home/fake";

    /// A host with nothing on it yet: every read comes back empty (the
    /// default reply), `$HOME` resolves.
    fn fresh_host() -> FakeSsh {
        let fake = FakeSsh::new();
        fake.with_home(HOME);
        fake
    }

    /// The exact script / command each step of `provision_one` should issue
    /// on a fresh host. `Script(s)` is a `bash -lc '<s>'` call, `Cmd(c)` a
    /// bare argv (`printenv HOME`), `Upload(path, content)` a `cat > path`
    /// with `content` on stdin.
    #[derive(Debug, PartialEq, Eq)]
    enum Step {
        Script(String),
        Cmd(String),
        Upload(String, String),
    }

    fn step_of(call: &Call) -> Step {
        if let Some(s) = call.script() {
            return Step::Script(s);
        }
        match (call.args.as_slice(), call.stdin_str()) {
            ([cmd], Some(stdin)) if cmd.starts_with("cat > ") => {
                Step::Upload(cmd["cat > ".len()..].to_string(), stdin)
            }
            _ => Step::Cmd(call.command()),
        }
    }

    fn expected_claude_md() -> String {
        merge_claude_md("", CLAUDE_MD_BEGIN, CLAUDE_MD_END, CLAUDE_MD_BODY).unwrap()
    }

    fn expected_tmux_conf() -> String {
        "\n# Enable OSC 52 clipboard (added by claude-fleet)\nset -g set-clipboard on\n".to_string()
    }

    fn expected_settings() -> String {
        crate::service::hooks_install::merge_hook_into_settings_json("", &base().hook_url(), TOKEN)
            .unwrap()
    }

    fn headers_path() -> String {
        format!(
            "{CLAUDE_DIR}/{}",
            crate::service::hooks_install::HOOK_HEADERS_FILE
        )
    }

    fn expected_headers() -> String {
        crate::service::hooks_install::hook_headers_content(TOKEN)
    }

    /// The three (or four, first time) steps [`write_host_file_secret`]
    /// issues for one file: touch the `.fleet-tmp` sibling 0600, optionally
    /// resolve `$HOME` (only the first secret write on a fresh `FakeSsh`,
    /// which caches it), upload the content to the tmp path, then rename it
    /// onto `path`.
    fn secret_write_steps(dir: &str, path: &str, content: &str, home_cached: bool) -> Vec<Step> {
        use Step::*;
        let tmp = format!("{path}.fleet-tmp");
        let mut steps = vec![Script(remote_touch_private_script(dir, &tmp))];
        if !home_cached {
            steps.push(Cmd("printenv HOME".into()));
        }
        let abs_tmp = match tmp.strip_prefix("~/") {
            Some(rest) => format!("{HOME}/{rest}"),
            None => tmp.clone(),
        };
        steps.push(Upload(quote(&abs_tmp), content.to_string()));
        steps.push(Script(remote_rename_script(&tmp, path)));
        steps
    }

    fn fresh_host_sequence() -> Vec<Step> {
        use Step::*;
        let mut steps = vec![
            // 1. skills
            Script(remote_write_script(SKILL_DIR, SKILL_PATH, FLEET_SKILL)),
            Script(remote_write_script(
                FRIENDLY_NAME_SKILL_DIR,
                FRIENDLY_NAME_SKILL_PATH,
                FRIENDLY_NAME_SKILL,
            )),
            // 1b. managed CLAUDE.md block
            Script(remote_read_script(CLAUDE_MD_PATH)),
            Script(remote_write_script(
                CLAUDE_DIR,
                CLAUDE_MD_PATH,
                &expected_claude_md(),
            )),
            // 2. MCP entry — no backup for an absent file; the token travels
            //    over stdin into a 0600 tmp file, never through argv, then
            //    an atomic rename lands it on the real path.
            Script(remote_read_script(CLAUDE_JSON)),
        ];
        steps.extend(secret_write_steps(
            CLAUDE_DIR,
            CLAUDE_JSON,
            &merge_mcp_entry("", URL, TOKEN).unwrap(),
            false,
        ));
        steps.extend([
            // 3. tmux clipboard
            Script(remote_read_script(TMUX_CONF)),
            Script(remote_write_script("~", TMUX_CONF, &expected_tmux_conf())),
            // 4. Stop / WorktreeCreate hooks ($HOME is cached now).
            Script(remote_read_script(SETTINGS_JSON)),
        ]);
        // The SessionStart command hook's bearer-token headers file ($HOME
        // is cached by now), written before the settings that reference it.
        steps.extend(secret_write_steps(
            CLAUDE_DIR,
            &headers_path(),
            &expected_headers(),
            true,
        ));
        steps.extend(secret_write_steps(
            CLAUDE_DIR,
            SETTINGS_JSON,
            &expected_settings(),
            true,
        ));
        steps
    }

    /// Every argument the remote shell sees must be inert: `bash -lc` gets
    /// ONE single-quoted word, every path inside expands under `"$HOME"`
    /// or is single-quoted, and the token appears in no argv at all.
    fn assert_quoting_invariants(calls: &[Call]) {
        for c in calls {
            match c.args.as_slice() {
                [b, l, script] if b == "bash" && l == "-lc" => {
                    assert!(
                        script.starts_with('\'') && script.ends_with('\''),
                        "bash -lc script must be one quoted word: {script}"
                    );
                    let body = c.script().unwrap();
                    // A quoted tilde (`'~'`, `'~/x'`) would be a literal path
                    // named `~` on the remote; the skill body may mention
                    // `~` inside its printf payload, so only the quoted
                    // path shape is illegal.
                    assert!(
                        !body.contains("'~"),
                        "no quoted tilde path may reach the remote: {body}"
                    );
                    for path in [
                        ".claude/skills",
                        ".claude/CLAUDE.md",
                        ".claude.json",
                        ".tmux.conf",
                        ".claude/settings.json",
                        ".claude/fleet-hook.headers",
                    ] {
                        if body.contains(path) {
                            assert!(
                                body.contains(&format!("\"$HOME\"/'{path}")),
                                "{path} must be \"$HOME\"/'…'-quoted in: {body}"
                            );
                        }
                    }
                }
                [cmd] if cmd.starts_with("cat > ") => {
                    let target = &cmd["cat > ".len()..];
                    assert!(
                        target.starts_with('\'') && target.ends_with('\''),
                        "upload target must be quoted: {cmd}"
                    );
                    assert!(target.starts_with(&format!("'{HOME}/")));
                }
                [a, b] if a == "printenv" && b == "HOME" => {}
                other => panic!("unexpected argv shape: {other:?}"),
            }
            assert!(
                !c.command().contains(TOKEN),
                "token must never be in argv (SEC-2): {}",
                c.command()
            );
        }
    }

    #[tokio::test]
    async fn provision_one_fresh_host_issues_the_exact_sequence() {
        let fake = fresh_host();
        provision_one(&fake, "h1", &base(), TOKEN).await.unwrap();
        let calls = fake.calls();
        assert!(calls.iter().all(|c| c.host == "h1"));
        let steps: Vec<Step> = calls.iter().map(step_of).collect();
        let expected = fresh_host_sequence();
        for (i, (got, want)) in steps.iter().zip(expected.iter()).enumerate() {
            assert_eq!(got, want, "step {i} differs");
        }
        assert_eq!(steps.len(), expected.len(), "step count");
        assert_quoting_invariants(&calls);
        // The secret reached the host exactly three times (claude.json, the
        // hook settings, and the SessionStart headers file), all over stdin.
        let uploads = calls.iter().filter(|c| c.stdin.is_some()).count();
        assert_eq!(uploads, 3);
        assert!(calls
            .iter()
            .filter_map(Call::stdin_str)
            .all(|s| s.contains(TOKEN)));
    }

    /// A host already provisioned by `provision_one`: its files read back
    /// exactly what the first run wrote.
    fn provisioned_host() -> FakeSsh {
        let fake = fresh_host();
        fake.on(
            Match::script(&remote_read_script(CLAUDE_MD_PATH)),
            Reply::ok(&expected_claude_md()),
        )
        .on(
            Match::script(&remote_read_script(CLAUDE_JSON)),
            Reply::ok(&merge_mcp_entry("", URL, TOKEN).unwrap()),
        )
        .on(
            Match::script(&remote_read_script(TMUX_CONF)),
            Reply::ok(&expected_tmux_conf()),
        )
        .on(
            Match::script(&remote_read_script(SETTINGS_JSON)),
            Reply::ok(&expected_settings()),
        );
        fake
    }

    #[tokio::test]
    async fn provision_one_second_run_is_idempotent_and_non_destructive() {
        let fake = provisioned_host();
        provision_one(&fake, "h1", &base(), TOKEN).await.unwrap();
        let calls = fake.calls();
        assert_quoting_invariants(&calls);
        let steps: Vec<Step> = calls.iter().map(step_of).collect();

        // Content-gated files are read but NOT rewritten.
        let claude_md_write =
            remote_write_script(CLAUDE_DIR, CLAUDE_MD_PATH, &expected_claude_md());
        let tmux_write = remote_write_script("~", TMUX_CONF, &expected_tmux_conf());
        assert!(!steps.contains(&Step::Script(claude_md_write)));
        assert!(!steps.contains(&Step::Script(tmux_write)));
        assert!(steps.contains(&Step::Script(remote_read_script(CLAUDE_MD_PATH))));
        assert!(steps.contains(&Step::Script(remote_read_script(TMUX_CONF))));

        // Token-bearing files: backed up BEFORE being rewritten, and
        // rewritten with byte-identical content. Each write lands via its
        // own `.fleet-tmp` upload + atomic rename (write_host_file_secret),
        // never a direct upload to the final path.
        let json_tmp = quote(&format!("{HOME}/.claude.json.fleet-tmp"));
        let json_bak_tmp = quote(&format!("{HOME}/.claude.json.fleet-bak.fleet-tmp"));
        let settings_tmp = quote(&format!("{HOME}/.claude/settings.json.fleet-tmp"));
        let settings_bak_tmp = quote(&format!("{HOME}/.claude/settings.json.fleet-bak.fleet-tmp"));
        let pos = |target: &str| {
            steps
                .iter()
                .position(|s| matches!(s, Step::Upload(t, _) if t == target))
                .unwrap_or_else(|| panic!("no upload to {target}"))
        };
        assert!(
            pos(&json_bak_tmp) < pos(&json_tmp),
            "backup precedes the rewrite"
        );
        assert!(pos(&settings_bak_tmp) < pos(&settings_tmp));
        let content = |target: &str| match &steps[pos(target)] {
            Step::Upload(_, c) => c.clone(),
            _ => unreachable!(),
        };
        assert_eq!(content(&json_tmp), merge_mcp_entry("", URL, TOKEN).unwrap());
        assert_eq!(
            content(&json_bak_tmp),
            merge_mcp_entry("", URL, TOKEN).unwrap()
        );
        assert_eq!(content(&settings_tmp), expected_settings());
        assert_eq!(content(&settings_bak_tmp), expected_settings());
        // Each tmp file is renamed onto its real path — the file is never
        // truncated in place.
        assert!(steps.contains(&Step::Script(remote_rename_script(
            &format!("{CLAUDE_JSON}.fleet-tmp"),
            CLAUDE_JSON
        ))));
        assert!(steps.contains(&Step::Script(remote_rename_script(
            &format!("{CLAUDE_JSON}.fleet-bak.fleet-tmp"),
            &format!("{CLAUDE_JSON}.fleet-bak")
        ))));
        assert!(steps.contains(&Step::Script(remote_rename_script(
            &format!("{SETTINGS_JSON}.fleet-tmp"),
            SETTINGS_JSON
        ))));
        assert!(steps.contains(&Step::Script(remote_rename_script(
            &format!("{SETTINGS_JSON}.fleet-bak.fleet-tmp"),
            &format!("{SETTINGS_JSON}.fleet-bak")
        ))));
        // The SessionStart headers file has no backup (it carries only the
        // token, no user content), but is rewritten with byte-identical
        // content via its own tmp-rename dance every run.
        let headers_tmp = quote(&format!("{HOME}/.claude/fleet-hook.headers.fleet-tmp"));
        assert_eq!(content(&headers_tmp), expected_headers());
        assert!(steps.contains(&Step::Script(remote_rename_script(
            &format!("{}.fleet-tmp", headers_path()),
            &headers_path()
        ))));
        // The backups are 0600 too (touch-private before each upload) — on
        // their own `.fleet-tmp` sibling, same as the main files.
        assert!(steps.contains(&Step::Script(remote_touch_private_script(
            CLAUDE_DIR,
            &format!("{CLAUDE_JSON}.fleet-bak.fleet-tmp")
        ))));
        assert!(steps.contains(&Step::Script(remote_touch_private_script(
            CLAUDE_DIR,
            &format!("{SETTINGS_JSON}.fleet-bak.fleet-tmp")
        ))));

        // Nothing destructive beyond the sanctioned tmp-rename dance: no
        // rmdir / unlink / truncate ever, and the only mv/rm that appear are
        // `write_host_file_secret`'s own `.fleet-tmp` rename/cleanup — never
        // an arbitrary path (quoted payloads — the skill markdown — are
        // data, so they are stripped before the scan).
        // POSIX-ish: a `'…'` segment is inert, a `\` outside quotes escapes
        // the next char (that is how `quote` renders an embedded `'`).
        let skeleton = |body: &str| -> String {
            let mut out = String::new();
            let mut chars = body.chars();
            while let Some(ch) = chars.next() {
                match ch {
                    '\'' => {
                        for c in chars.by_ref() {
                            if c == '\'' {
                                break;
                            }
                        }
                        out.push_str("'…'");
                    }
                    '\\' => {
                        chars.next();
                    }
                    c => out.push(c),
                }
            }
            out
        };
        for c in &calls {
            let raw = c.script().unwrap_or_else(|| c.command());
            let body = skeleton(&raw);
            for bad in ["rmdir", "unlink", "truncate"] {
                assert!(!body.contains(bad), "destructive command in {body}");
            }
            if body.contains("mv ") || body.contains("rm ") {
                // The path itself is quoted (redacted to `'…'` by
                // `skeleton`), so check the raw, unredacted command for it —
                // paths are not attacker-controlled data the way skill
                // markdown content is.
                assert!(
                    raw.contains(".fleet-tmp"),
                    "mv/rm outside the write_host_file_secret tmp-rename dance: {raw}"
                );
            }
        }
        // Skills are re-shipped (same bytes) — the only unconditional writes.
        let skill_writes = steps
            .iter()
            .filter(|s| matches!(s, Step::Script(b) if b.contains(".claude/skills")))
            .count();
        assert_eq!(skill_writes, 2);
    }

    #[tokio::test]
    async fn malformed_remote_settings_json_is_e_provision_with_no_write() {
        // Regression for the #45 blocker: a settings.json we cannot parse is
        // never "repaired" — the merge fails BEFORE the backup/touch/upload.
        let fake = provisioned_host();
        fake.on(
            Match::script(&remote_read_script(SETTINGS_JSON)),
            Reply::ok("{ \"hooks\": [ oops"),
        );
        let err = provision_one(&fake, "h1", &base(), TOKEN)
            .await
            .unwrap_err();
        assert_eq!(err.code, "E_PROVISION");
        assert!(err.message.contains("settings.json"), "{}", err.message);
        let calls = fake.calls();
        let read_idx = calls
            .iter()
            .position(|c| c.script().as_deref() == Some(remote_read_script(SETTINGS_JSON).as_str()))
            .expect("settings.json was read");
        assert_eq!(
            read_idx,
            calls.len() - 1,
            "the failed read is the LAST call — no touch, no backup, no upload: {:?}",
            calls[read_idx + 1..]
                .iter()
                .map(Call::command)
                .collect::<Vec<_>>()
        );
        assert!(!calls
            .iter()
            .any(|c| c.command().contains("settings.json") && c.stdin.is_some()));
    }

    #[tokio::test]
    async fn malformed_remote_claude_json_is_e_provision_before_any_secret_write() {
        let fake = fresh_host();
        fake.on(
            Match::script(&remote_read_script(CLAUDE_JSON)),
            Reply::ok("{not json"),
        );
        let err = provision_one(&fake, "h1", &base(), TOKEN)
            .await
            .unwrap_err();
        assert_eq!(err.code, "E_PROVISION");
        assert!(err.message.contains("~/.claude.json"), "{}", err.message);
        let calls = fake.calls();
        assert!(
            calls.iter().all(|c| c.stdin.is_none()),
            "no upload may happen after a parse failure"
        );
        assert!(
            !calls.iter().any(|c| c.command().contains("settings.json")),
            "provisioning stops at the first failure"
        );
        // Skills + CLAUDE.md (steps before the failure) were still written.
        assert!(calls
            .iter()
            .any(|c| c.script().is_some_and(|s| s.contains(".claude/skills"))));
    }

    #[tokio::test]
    async fn a_failed_remote_write_is_e_provision_with_stderr() {
        let fake = fresh_host();
        fake.on(
            Match::script_contains("mkdir -p \"$HOME\"/'.claude/skills/claude-fleet-control'"),
            Reply::fail(1, "mkdir: cannot create directory: Read-only file system"),
        );
        let err = provision_one(&fake, "h1", &base(), TOKEN)
            .await
            .unwrap_err();
        assert_eq!(err.code, "E_PROVISION");
        assert!(
            err.message.contains("Read-only file system"),
            "{}",
            err.message
        );
        assert_eq!(fake.calls().len(), 1, "stops at the first failed step");
    }

    /// Tunnel supervisor whose "ssh" never exits and records nothing — keeps
    /// `provision_host_with_token` from spawning a real `ssh -R`.
    fn quiet_tunnels() -> Arc<TunnelSupervisor> {
        Arc::new(TunnelSupervisor::with_spawner(
            Arc::new(|_argv| Box::pin(std::future::pending())),
            Duration::from_secs(3600),
        ))
    }

    #[tokio::test]
    async fn provision_hosts_skips_unreachable_and_hidden_and_isolates_failures() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        {
            let s = store.lock().unwrap();
            for (alias, reachable) in [
                ("up", true),
                ("broken", true),
                ("down", false),
                ("hid", true),
            ] {
                s.insert_host(alias, Some(alias)).unwrap();
                s.update_host_probe(alias, reachable, None, None, 1)
                    .unwrap();
            }
            s.set_host_hidden("hid", true).unwrap();
        }
        let fake = fresh_host();
        fake.on_host(
            "broken",
            Match::script(&remote_read_script(SETTINGS_JSON)),
            Reply::ok("not json"),
        );
        let tunnels = quiet_tunnels();
        let results = provision_hosts(&store, &fake, &tunnels, &base(), false)
            .await
            .unwrap();
        let status = |h: &str| {
            results
                .iter()
                .find(|r| r.host == h)
                .map(|r| r.status.as_str())
                .unwrap_or("absent")
        };
        assert_eq!(status("up"), "provisioned");
        assert_eq!(status("broken"), "failed");
        assert_eq!(status("down"), "skipped");
        assert_eq!(status("hid"), "absent");
        assert!(fake.calls_for("down").is_empty());
        assert!(fake.calls_for("hid").is_empty());

        // Token committed + tunnel ensured + marked provisioned ONLY for the
        // host whose files were actually written.
        let s = store.lock().unwrap();
        let up_token = s.get_host_token("up").unwrap().expect("token for up");
        assert!(s.get_host_token("broken").unwrap().is_none());
        assert!(s.get_host_token("down").unwrap().is_none());
        let hosts = s.list_hosts().unwrap();
        let provisioned = |a: &str| hosts.iter().find(|h| h.alias == a).unwrap().provisioned;
        assert!(provisioned("up"));
        assert!(!provisioned("broken"));
        assert!(!provisioned("down"));
        let snap = tunnels.snapshot();
        assert_eq!(snap.get("up"), Some(&true));
        assert!(!snap.contains_key("broken"));
        assert!(!snap.contains_key("down"));
        // The token that reached `up` is the one persisted for it.
        let uploaded = fake
            .calls_for("up")
            .into_iter()
            .filter_map(|c| c.stdin_str())
            .collect::<Vec<_>>();
        assert!(uploaded.iter().all(|u| u.contains(&up_token.token)));
        tunnels.stop_all();
    }

    /// An AGENT host's rotation is the answer to a stolen token, and the
    /// thief may be the agent that is connected. So the new token must never
    /// travel over that connection: it is committed FIRST, which revokes the
    /// old one, and nothing is sent to the host at all. The operator hands
    /// the new token to the host out of band, as the error says.
    #[tokio::test]
    async fn rotating_an_agent_host_never_sends_the_new_token_over_its_live_connection() {
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        {
            let s = store.lock().unwrap();
            s.insert_host("laptop", Some("laptop")).unwrap();
            s.set_host_transport("laptop", "agent").unwrap();
            s.upsert_host_token("laptop", "old-token").unwrap();
        }
        let reg = crate::agent::AgentRegistry::new();
        let ssh = crate::ssh::SshClient::with_agents(Arc::clone(&reg), Arc::clone(&store));
        // Connected with the OLD token, answering everything as a host would.
        let agent = crate::agent::fake::FakeAgent::connect_with_token(
            &reg,
            "laptop",
            "old-token",
            crate::agent::fake::answer_with(0, b"", b""),
        );

        let err =
            provision_host_with_token(&store, &ssh, &quiet_tunnels(), "laptop", &base(), true)
                .await
                .unwrap_err();

        let new = store
            .lock()
            .unwrap()
            .get_host_token("laptop")
            .unwrap()
            .unwrap();
        assert_ne!(new.token, "old-token", "the rotation is committed");
        assert_eq!(new.mode, "full");
        assert!(
            agent.sent().is_empty(),
            "nothing may be sent over the connection being revoked: {:?}",
            agent.sent()
        );
        assert_eq!(err.code, codes::E_AGENT_REINSTALL, "{err:?}");
        assert!(
            err.message.contains("fleet-hub agent-token laptop")
                && err.message.contains("fleet-agent install"),
            "it tells the operator how to deliver the token: {}",
            err.message
        );
        assert!(
            !err.message.contains(&new.token),
            "the error is not a place to print a secret"
        );
        // And the old connection is refused from here on.
        let refused = crate::ssh::SshExec::run(&ssh, "laptop", &["true"], Duration::from_secs(5))
            .await
            .unwrap_err();
        assert_eq!(refused.code, codes::E_AGENT_OFFLINE);
        assert!(agent.sent().is_empty());
    }

    fn agent_host_store() -> Mutex<Store> {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("laptop", Some("laptop")).unwrap();
        s.set_host_transport("laptop", "agent").unwrap();
        s.insert_host("mefistos", Some("mefistos")).unwrap();
        Mutex::new(s)
    }

    #[test]
    fn an_agent_host_token_is_minted_once_then_reused_until_rotated() {
        let store = agent_host_store();
        let first = agent_host_token(&store, "laptop", false).unwrap();
        assert!(first.minted);
        assert_eq!(first.mode, "full");
        let stored = store
            .lock()
            .unwrap()
            .get_host_token("laptop")
            .unwrap()
            .unwrap();
        assert_eq!(stored.token, first.token, "committed before it is shown");

        let again = agent_host_token(&store, "laptop", false).unwrap();
        assert_eq!(again.token, first.token);
        assert!(!again.minted);

        let rotated = agent_host_token(&store, "laptop", true).unwrap();
        assert!(rotated.minted);
        assert_ne!(rotated.token, first.token);
        let stored = store
            .lock()
            .unwrap()
            .get_host_token("laptop")
            .unwrap()
            .unwrap();
        assert_eq!(stored.token, rotated.token, "the rotation is committed");
    }

    #[test]
    fn an_agent_host_token_reports_a_readonly_mode() {
        let store = agent_host_store();
        agent_host_token(&store, "laptop", false).unwrap();
        store
            .lock()
            .unwrap()
            .set_host_token_mode("laptop", "readonly")
            .unwrap();
        assert_eq!(
            agent_host_token(&store, "laptop", false).unwrap().mode,
            "readonly"
        );
    }

    #[test]
    fn only_an_agent_host_is_given_a_token_this_way() {
        let store = agent_host_store();
        let err = agent_host_token(&store, "mefistos", false).unwrap_err();
        assert_eq!(err.code, codes::E_INVALID, "{err:?}");
        assert!(err.message.contains("not an agent host"), "{}", err.message);
        assert!(store
            .lock()
            .unwrap()
            .get_host_token("mefistos")
            .unwrap()
            .is_none());
        let err = agent_host_token(&store, "nobody", false).unwrap_err();
        assert_eq!(err.code, codes::E_NOTFOUND, "{err:?}");
    }

    #[tokio::test]
    async fn rotate_mints_a_new_token_only_after_the_host_received_it() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        store.lock().unwrap().insert_host("h", Some("h")).unwrap();
        let tunnels = quiet_tunnels();
        let fake = fresh_host();
        provision_host_with_token(&store, &fake, &tunnels, "h", &base(), false)
            .await
            .unwrap();
        let first = store
            .lock()
            .unwrap()
            .get_host_token("h")
            .unwrap()
            .unwrap()
            .token;

        // Rotation against a host that now fails: the OLD token stays.
        let failing = provisioned_host();
        failing.on(
            Match::script(&remote_read_script(SETTINGS_JSON)),
            Reply::ok("{broken"),
        );
        let err = provision_host_with_token(&store, &failing, &tunnels, "h", &base(), true)
            .await
            .unwrap_err();
        assert_eq!(err.code, "E_PROVISION");
        let after_fail = store
            .lock()
            .unwrap()
            .get_host_token("h")
            .unwrap()
            .unwrap()
            .token;
        assert_eq!(after_fail, first, "a failed rotate never strands the host");

        // Rotation that succeeds persists the new token the host received.
        let ok = provisioned_host();
        provision_host_with_token(&store, &ok, &tunnels, "h", &base(), true)
            .await
            .unwrap();
        let rotated = store
            .lock()
            .unwrap()
            .get_host_token("h")
            .unwrap()
            .unwrap()
            .token;
        assert_ne!(rotated, first);
        assert!(ok
            .calls()
            .iter()
            .filter_map(Call::stdin_str)
            .any(|u| u.contains(&rotated)));
        // The positive half of the public-base test: a LOOPBACK hub has no
        // address the host can reach, so provisioning must leave a reverse
        // tunnel running for it.
        assert_eq!(
            tunnels.snapshot().get("h"),
            Some(&true),
            "a loopback hub tunnels the host it provisioned: {:?}",
            tunnels.snapshot()
        );
        tunnels.stop_all();
    }

    /// An agent host is by definition not dialable over SSH, so a reverse
    /// tunnel to it would just be `ssh -R` restarting forever against an
    /// address that never accepts a connection. `provision_host_with_token`
    /// must skip the tunnel for it even on a loopback hub.
    #[tokio::test]
    async fn provisioning_an_agent_host_starts_no_reverse_tunnel() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        {
            let s = store.lock().unwrap();
            s.insert_host("laptop", Some("laptop")).unwrap();
            s.set_host_transport("laptop", "agent").unwrap();
            // An existing token, so this re-provision does not mint a new
            // one — minting one would instead hit E_AGENT_REINSTALL (an
            // agent host's new token can never travel over the connection
            // it is replacing), which is a different, already-tested path.
            s.upsert_host_token("laptop", "existing-token").unwrap();
        }
        let tunnels = quiet_tunnels();
        let fake = fresh_host();
        provision_host_with_token(&store, &fake, &tunnels, "laptop", &base(), false)
            .await
            .unwrap();
        assert!(
            tunnels.snapshot().is_empty(),
            "an agent host has no way to reach an ssh -R tunnel: {:?}",
            tunnels.snapshot()
        );
    }

    #[tokio::test]
    async fn reestablish_tunnels_is_a_no_op_for_a_public_base() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        {
            let s = store.lock().unwrap();
            s.upsert_host("mefistos").unwrap();
            s.set_host_provisioned("mefistos", true).unwrap();
        }
        let spawned = Arc::new(Mutex::new(Vec::<String>::new()));
        let seen = Arc::clone(&spawned);
        let tunnels = Arc::new(TunnelSupervisor::with_spawner(
            Arc::new(move |argv: Vec<String>| {
                seen.lock()
                    .unwrap()
                    .push(argv.last().cloned().unwrap_or_default());
                Box::pin(std::future::pending())
            }),
            Duration::from_secs(3600),
        ));
        let public = HubBase::public("https://fleet.example.com", 4180).unwrap();
        reestablish_tunnels(&store, &tunnels, &public).unwrap();
        // The supervisor spawns from a task, so let it run before counting.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(tunnels.snapshot().is_empty(), "no tunnel for a public hub");
        assert!(
            spawned.lock().unwrap().is_empty(),
            "a public hub must spawn no ssh at all: {:?}",
            spawned.lock().unwrap()
        );
        reestablish_tunnels(&store, &tunnels, &HubBase::loopback(4180)).unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            tunnels.snapshot().get("mefistos"),
            Some(&true),
            "loopback hub tunnels provisioned hosts"
        );
        assert_eq!(tunnels.snapshot().len(), 1);
        // And exactly one ssh was spawned, for that host.
        let argv = spawned.lock().unwrap().clone();
        assert_eq!(argv.len(), 1, "{argv:?}");
        assert!(argv[0].contains("mefistos"), "{argv:?}");
        tunnels.stop_all();
    }

    /// A provisioned agent host is skipped on the app-start reconcile pass
    /// too, the same as on first provisioning: it has no address for the hub
    /// to dial, so `ssh -R` against it would just restart forever.
    #[tokio::test]
    async fn reestablish_tunnels_skips_agent_transport_hosts() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        {
            let s = store.lock().unwrap();
            s.insert_host("laptop", Some("laptop")).unwrap();
            s.set_host_transport("laptop", "agent").unwrap();
            s.set_host_provisioned("laptop", true).unwrap();
            s.upsert_host("mefistos").unwrap();
            s.set_host_provisioned("mefistos", true).unwrap();
        }
        let tunnels = quiet_tunnels();
        reestablish_tunnels(&store, &tunnels, &HubBase::loopback(4180)).unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            !tunnels.snapshot().contains_key("laptop"),
            "an agent host has no way to reach an ssh -R tunnel: {:?}",
            tunnels.snapshot()
        );
        assert_eq!(
            tunnels.snapshot().get("mefistos"),
            Some(&true),
            "an ssh-transport host is still tunneled"
        );
        tunnels.stop_all();
    }

    #[tokio::test]
    async fn provision_host_with_a_public_base_writes_its_urls_and_starts_no_tunnel() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        store.lock().unwrap().insert_host("h", Some("h")).unwrap();
        let tunnels = quiet_tunnels();
        let fake = fresh_host();
        let public = HubBase::public("https://fleet.example.com", PORT).unwrap();
        provision_host_with_token(&store, &fake, &tunnels, "h", &public, false)
            .await
            .unwrap();
        assert!(
            tunnels.snapshot().is_empty(),
            "a public hub needs no tunnel"
        );
        let uploaded = fake
            .calls()
            .iter()
            .filter_map(Call::stdin_str)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(uploaded.contains("https://fleet.example.com/mcp"));
        assert!(uploaded.contains("https://fleet.example.com/hook"));
        assert!(!uploaded.contains("127.0.0.1"), "{uploaded}");
        let s = store.lock().unwrap();
        let hosts = s.list_hosts().unwrap();
        assert!(hosts.iter().find(|h| h.alias == "h").unwrap().provisioned);
    }
}
