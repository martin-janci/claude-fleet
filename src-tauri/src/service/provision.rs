//! Provision a host's Claude with the fleet-control skill + MCP server entry.

use crate::ipc_error::IpcError;
use crate::service::tunnel::TunnelSupervisor;
use crate::shell::quote;
use crate::ssh::SshExec;
use crate::store::Store;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const PROVISION_TIMEOUT: Duration = Duration::from_secs(15);

const FLEET_SKILL: &str = include_str!("../../../skills/claude-fleet-control/SKILL.md");
const SKILL_DIR: &str = "~/.claude/skills/claude-fleet-control";
const SKILL_PATH: &str = "~/.claude/skills/claude-fleet-control/SKILL.md";
const FRIENDLY_NAME_SKILL: &str = include_str!("../../../skills/fleet-friendly-name/SKILL.md");
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

/// Install the skill + merge the MCP entry on one host. `url` is the MCP
/// endpoint that host should use; `token` is that host's own bearer token
/// (see [`resolve_host_token`]). Reads `~/.claude.json`, merges (preserving
/// siblings), backs it up, writes it back. Parse errors abort BEFORE any
/// write. Files that carry the token are written with `umask 077` and
/// `chmod 600` (SEC-2).
///
/// `mcp_port` is also installed as a Stop / PostToolUse(WorktreeCreate)
/// `type: "http"` hook in `~/.claude/settings.json` so the host's Claude Code
/// can notify fleet over the reverse tunnel — without this, safe-kill on
/// remote hosts never finalizes (the marker check is gated on the Stop hook
/// firing). On a remote host `127.0.0.1:<mcp_port>` IS the tunnel's loopback
/// end (see `tunnel_argv`), so the same URL works on every host.
pub async fn provision_one(
    ssh: &dyn SshExec,
    host: &str,
    url: &str,
    token: &str,
    mcp_port: u16,
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
    let merged = merge_mcp_entry(&existing, url, token)?; // errors before any write
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
    provision_hook(ssh, host, mcp_port, token).await?;
    Ok(())
}

const SETTINGS_JSON: &str = "~/.claude/settings.json";

/// Merge fleet's Stop + PostToolUse(WorktreeCreate) http hooks into the
/// host's `~/.claude/settings.json`. Idempotent — re-running replaces fleet
/// entries pointing at the same `mcp_port` and leaves the user's own hooks
/// alone. The block carries the host's bearer token, so the file is written
/// 0600.
pub async fn provision_hook(
    ssh: &dyn SshExec,
    host: &str,
    mcp_port: u16,
    token: &str,
) -> Result<(), IpcError> {
    let existing = read_host_file(ssh, host, SETTINGS_JSON).await?;
    // Errors (malformed JSON → E_PROVISION) fire BEFORE any write.
    let merged = crate::commands::mcp::merge_hook_into_settings_json(&existing, mcp_port, token)?;
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
    let s = store.lock().map_err(|_| IpcError::lock())?;
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
    let s = store.lock().map_err(|_| IpcError::lock())?;
    s.upsert_host_token(host, token)
}

/// Provision ONE host end to end with its own token: resolve/mint → write
/// files → persist the token → ensure the tunnel (remote) → mark
/// provisioned. Shared by [`provision_hosts`] and the per-host Rotate action.
pub async fn provision_host_with_token(
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    tunnels: &Arc<TunnelSupervisor>,
    host: &str,
    mcp_port: u16,
    rotate: bool,
) -> Result<(), IpcError> {
    let (token, minted) = resolve_host_token(store, host, rotate)?;
    let url = format!("http://127.0.0.1:{mcp_port}/mcp");
    provision_one(ssh, host, &url, &token, mcp_port).await?;
    commit_host_token(store, host, &token, minted)?;
    if host != "local" {
        tunnels.ensure(host, mcp_port, mcp_port);
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
/// unless `rotate`). `local` gets a direct localhost URL + no tunnel; remote
/// hosts get the reverse tunnel + a localhost:<mcp_port> URL. Per-host
/// failures never abort the others.
pub async fn provision_hosts(
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    tunnels: &Arc<TunnelSupervisor>,
    mcp_port: u16,
    rotate: bool,
) -> Result<Vec<HostProvisionResult>, IpcError> {
    let hosts = {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
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
        match provision_host_with_token(store, ssh, tunnels, &h.alias, mcp_port, rotate).await {
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
/// re-enable). Does NOT re-write config.
pub fn reestablish_tunnels(
    store: &Mutex<Store>,
    tunnels: &Arc<TunnelSupervisor>,
    mcp_port: u16,
) -> Result<(), IpcError> {
    let hosts = {
        store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?
            .list_hosts()?
    };
    for h in hosts {
        if h.provisioned && h.alias != "local" && !h.hidden {
            tunnels.ensure(&h.alias, mcp_port, mcp_port);
        }
    }
    Ok(())
}

/// Read a file from a host. `local` → `std::fs`; remote → `cat` over SSH.
/// Missing file → `Ok(String::new())` (caller treats as empty config).
pub async fn read_host_file(ssh: &dyn SshExec, host: &str, path: &str) -> Result<String, IpcError> {
    if host == "local" {
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
        let edir = expand_home_local(dir)?;
        std::fs::create_dir_all(&edir)
            .map_err(|e| IpcError::new("E_PROVISION", format!("mkdir {edir}: {e}")))?;
        let epath = expand_home_local(path)?;
        std::fs::write(&epath, content)
            .map_err(|e| IpcError::new("E_PROVISION", format!("write {epath}: {e}")))?;
        return Ok(());
    }
    let script = quote(&remote_write_script(dir, path, content));
    run_remote_write(ssh, host, path, &script).await
}

/// Like [`write_host_file`] for a file that carries a secret (the bearer
/// token). The content never appears in a process argv on either side:
///
/// - remote: the file is first created empty under `umask 077` + `chmod 600`
///   (a script with only paths in it), then the content is streamed over
///   stdin through `SshExec::upload_file` (`cat > path`, which keeps the
///   0600 mode when truncating);
/// - local: the file is opened with mode 0600 from creation
///   ([`write_private_file`]).
pub async fn write_host_file_secret(
    ssh: &dyn SshExec,
    host: &str,
    dir: &str,
    path: &str,
    content: &str,
) -> Result<(), IpcError> {
    if host == "local" {
        let edir = expand_home_local(dir)?;
        std::fs::create_dir_all(&edir)
            .map_err(|e| IpcError::new("E_PROVISION", format!("mkdir {edir}: {e}")))?;
        let epath = expand_home_local(path)?;
        return write_private_file(std::path::Path::new(&epath), content)
            .map_err(|e| IpcError::new("E_PROVISION", format!("write {epath}: {e}")));
    }
    // 1. Create the (empty) file 0600 — paths only, no secret in argv.
    let script = quote(&remote_touch_private_script(dir, path));
    run_remote_write(ssh, host, path, &script).await?;
    // 2. Stream the content over stdin. `upload_file` runs `cat > '<abs>'`;
    //    `~` is not expanded in a quoted word, so resolve $HOME first.
    let abs = match path.strip_prefix("~/") {
        Some(rest) => format!("{}/{rest}", ssh.remote_home(host).await?),
        None => path.to_string(),
    };
    let tmp = PrivateTempFile::create(content)
        .map_err(|e| IpcError::new("E_PROVISION", format!("spool secret for {path}: {e}")))?;
    ssh.upload_file(host, tmp.path(), &abs, PROVISION_TIMEOUT)
        .await
        .map_err(|e| {
            IpcError::new(
                "E_PROVISION",
                format!("write {path} on {host}: {}", e.message),
            )
        })
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
            "E_PROVISION",
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

/// Expand a leading `~/` against the LOCAL home dir.
fn expand_home_local(path: &str) -> Result<String, IpcError> {
    if let Some(rest) = path.strip_prefix("~/") {
        let home =
            std::env::var("HOME").map_err(|_| IpcError::new("E_PROVISION", "HOME not set"))?;
        Ok(format!("{home}/{rest}"))
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
                "E_PROVISION",
                format!("~/.claude.json is not valid JSON: {e}"),
            )
        })?
    };
    if !root.is_object() {
        return Err(IpcError::new(
            "E_PROVISION",
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
            "E_PROVISION",
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
        .map_err(|e| IpcError::new("E_PROVISION", format!("serialize: {e}")))
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
        std::env::set_var("HOME", "/Users/test");
        assert_eq!(
            super::expand_home_local("~/.claude.json").unwrap(),
            "/Users/test/.claude.json"
        );
        assert_eq!(super::expand_home_local("/abs/path").unwrap(), "/abs/path");
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
        crate::commands::mcp::merge_hook_into_settings_json("", PORT, TOKEN).unwrap()
    }

    fn fresh_host_sequence() -> Vec<Step> {
        use Step::*;
        vec![
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
            //    over stdin into a 0600 file, never through argv.
            Script(remote_read_script(CLAUDE_JSON)),
            Script(remote_touch_private_script(CLAUDE_DIR, CLAUDE_JSON)),
            Cmd("printenv HOME".into()),
            Upload(
                quote(&format!("{HOME}/.claude.json")),
                merge_mcp_entry("", URL, TOKEN).unwrap(),
            ),
            // 3. tmux clipboard
            Script(remote_read_script(TMUX_CONF)),
            Script(remote_write_script("~", TMUX_CONF, &expected_tmux_conf())),
            // 4. Stop / WorktreeCreate hooks ($HOME is cached now).
            Script(remote_read_script(SETTINGS_JSON)),
            Script(remote_touch_private_script(CLAUDE_DIR, SETTINGS_JSON)),
            Upload(
                quote(&format!("{HOME}/.claude/settings.json")),
                expected_settings(),
            ),
        ]
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
        provision_one(&fake, "h1", URL, TOKEN, PORT).await.unwrap();
        let calls = fake.calls();
        assert!(calls.iter().all(|c| c.host == "h1"));
        let steps: Vec<Step> = calls.iter().map(step_of).collect();
        let expected = fresh_host_sequence();
        for (i, (got, want)) in steps.iter().zip(expected.iter()).enumerate() {
            assert_eq!(got, want, "step {i} differs");
        }
        assert_eq!(steps.len(), expected.len(), "step count");
        assert_quoting_invariants(&calls);
        // The secret reached the host exactly twice, both times over stdin.
        let uploads = calls.iter().filter(|c| c.stdin.is_some()).count();
        assert_eq!(uploads, 2);
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
        provision_one(&fake, "h1", URL, TOKEN, PORT).await.unwrap();
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
        // rewritten with byte-identical content.
        let json = quote(&format!("{HOME}/.claude.json"));
        let json_bak = quote(&format!("{HOME}/.claude.json.fleet-bak"));
        let settings = quote(&format!("{HOME}/.claude/settings.json"));
        let settings_bak = quote(&format!("{HOME}/.claude/settings.json.fleet-bak"));
        let pos = |target: &str| {
            steps
                .iter()
                .position(|s| matches!(s, Step::Upload(t, _) if t == target))
                .unwrap_or_else(|| panic!("no upload to {target}"))
        };
        assert!(pos(&json_bak) < pos(&json), "backup precedes the rewrite");
        assert!(pos(&settings_bak) < pos(&settings));
        let content = |target: &str| match &steps[pos(target)] {
            Step::Upload(_, c) => c.clone(),
            _ => unreachable!(),
        };
        assert_eq!(content(&json), merge_mcp_entry("", URL, TOKEN).unwrap());
        assert_eq!(content(&json_bak), merge_mcp_entry("", URL, TOKEN).unwrap());
        assert_eq!(content(&settings), expected_settings());
        assert_eq!(content(&settings_bak), expected_settings());
        // The backups are 0600 too (touch-private before each upload).
        assert!(steps.contains(&Step::Script(remote_touch_private_script(
            CLAUDE_DIR,
            &format!("{CLAUDE_JSON}.fleet-bak")
        ))));
        assert!(steps.contains(&Step::Script(remote_touch_private_script(
            CLAUDE_DIR,
            &format!("{SETTINGS_JSON}.fleet-bak")
        ))));

        // Nothing destructive, ever: no rm / mv / rmdir in the command
        // skeleton (quoted payloads — the skill markdown — are data, so they
        // are stripped before the scan).
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
            let body = skeleton(&c.script().unwrap_or_else(|| c.command()));
            for bad in ["rm ", "mv ", "rmdir", "unlink", "truncate"] {
                assert!(!body.contains(bad), "destructive command in {body}");
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
        let err = provision_one(&fake, "h1", URL, TOKEN, PORT)
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
        let err = provision_one(&fake, "h1", URL, TOKEN, PORT)
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
        let err = provision_one(&fake, "h1", URL, TOKEN, PORT)
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
        let results = provision_hosts(&store, &fake, &tunnels, PORT, false)
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

    #[tokio::test]
    async fn rotate_mints_a_new_token_only_after_the_host_received_it() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        store.lock().unwrap().insert_host("h", Some("h")).unwrap();
        let tunnels = quiet_tunnels();
        let fake = fresh_host();
        provision_host_with_token(&store, &fake, &tunnels, "h", PORT, false)
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
        let err = provision_host_with_token(&store, &failing, &tunnels, "h", PORT, true)
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
        provision_host_with_token(&store, &ok, &tunnels, "h", PORT, true)
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
        tunnels.stop_all();
    }
}
