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
/// Skills fleet provisions on every host. The catalog treats them as fleet
/// internals: they are never offered for import.
pub const FLEET_SKILL_NAMES: &[&str] = &["claude-fleet-control", "fleet-friendly-name"];
/// The `mcpServers` key fleet provisions (it carries the host's token).
pub const FLEET_MCP_SERVER: &str = "claude-fleet";
/// Written into each managed skill dir: the fingerprint that put it there.
/// Says "fleet owns this directory" to a human and to a dotfiles sync
/// (hosts F2 — both skills were tracked and dirty in `~/dotfiles`).
const MANAGED_MARKER: &str = ".fleet-managed";
const SKILLS_ROOT: &str = "~/.claude/skills";
pub(crate) const CLAUDE_JSON: &str = "~/.claude.json";
pub(crate) const CLAUDE_DIR: &str = "~/.claude";
const CLAUDE_MD_PATH: &str = "~/.claude/CLAUDE.md";
const TMUX_CONF: &str = "~/.tmux.conf";

/// fleet's `ag` launcher (tools/ag, F2), shipped to every host. Path
/// relative to [`AG_STAGE_DIR`] → content. `every_ag_file_is_compiled_in`
/// keeps this list in step with the directory.
pub(crate) const AG_FILES: &[(&str, &str)] = &[
    ("ag", include_str!("../../../../tools/ag/ag")),
    (
        "install.sh",
        include_str!("../../../../tools/ag/install.sh"),
    ),
    (
        "drivers/claude.sh",
        include_str!("../../../../tools/ag/drivers/claude.sh"),
    ),
    (
        "drivers/codex.sh",
        include_str!("../../../../tools/ag/drivers/codex.sh"),
    ),
    (
        "lib/args.sh",
        include_str!("../../../../tools/ag/lib/args.sh"),
    ),
    (
        "lib/config.sh",
        include_str!("../../../../tools/ag/lib/config.sh"),
    ),
    (
        "lib/doctor.sh",
        include_str!("../../../../tools/ag/lib/doctor.sh"),
    ),
    (
        "lib/harness.sh",
        include_str!("../../../../tools/ag/lib/harness.sh"),
    ),
    (
        "lib/launch.sh",
        include_str!("../../../../tools/ag/lib/launch.sh"),
    ),
    (
        "lib/shims.sh",
        include_str!("../../../../tools/ag/lib/shims.sh"),
    ),
    (
        "lib/util.sh",
        include_str!("../../../../tools/ag/lib/util.sh"),
    ),
];
/// The `/voice` recorder stand-in (docs/voice.md). In the fingerprint, so a
/// change re-provisions stale hosts.
pub const VOICE_ARECORD: &str = include_str!("../../../../tools/voice/arecord");
/// Where the stand-in (`bin/arecord`) and its `voice.env` live on a host.
pub const VOICE_DIR: &str = "~/.claude-fleet/voice";

/// Where provisioning stages [`AG_FILES`] before running their installer.
const AG_STAGE_DIR: &str = "~/.local/share/fleet/ag-src";
/// Where the installer puts the ag tree, passed to it EXPLICITLY.
///
/// `install.sh` honours a caller-set `AG_HOME`/`AG_BIN_DIR` by design, and
/// fleet runs it through `bash -lc`, which sources the host's login profile
/// first — so an `export AG_HOME=...` there would otherwise decide where
/// fleet installs (and, before the installer's guard was hardened, what it
/// deleted). Passing both pins the location to the one
/// `tmux::CL_FALLBACK` reads; `ag_paths_match_the_pane_fallback` ties them.
const AG_HOME_DIR: &str = "~/.local/share/ag";
const AG_BIN_DIR: &str = "~/.local/bin";
/// The alias every provisioned host gets: `cl` = Claude Code without
/// permission prompts, the same launch fleet's panes use.
const AG_CL_ALIAS: &str = "cl=claude --yolo";
/// SSH connect budget for running the installer (it copies a dozen small
/// files and runs `ag shims` + `ag doctor`); the command's wall clock
/// derives from it (`SshClient::default_wall_clock`).
const AG_INSTALL_TIMEOUT: Duration = Duration::from_secs(30);

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

/// SHA-256 over everything provisioning ships that is CONTENT (not a
/// secret, not a URL): both skills, the managed CLAUDE.md body, the
/// hook shape, the ag launcher and the `/voice` stand-in. Stored per host by
/// `set_host_provisioned`; a host whose stored value differs is
/// `provision_stale` (hosts F1: every host ran skills from 15 hub
/// upgrades ago, and nothing compared).
/// **What it does NOT cover: the `~/.claude.json` MCP entry**
/// ([`merge_mcp_entry`]) — so multi-user M1's `X-Fleet-Pane` header arriving
/// in that entry does not make an older provisioning read `provision_stale`,
/// contrary to what T2's "Do." block in
/// `docs/superpowers/plans/2026-09-30-multi-user-m1-private-sessions.md`
/// assumed. Adding the entry's shape here is one line, and it would be WORSE
/// than the gap: `provision_stale` is cleared by
/// [`provision_content_only`] — the unattended sweep
/// ([`spawn_reprovision_stale`]) and the `--content-only` command the plan
/// itself tells the operator to run — which deliberately never rewrites
/// `~/.claude.json` (no token minted, no user file that Claude Code writes
/// concurrently touched). The mark would therefore be raised and cleared
/// within one hub start, with the header still missing and the operator told
/// nothing.
///
/// Telling the two apart needs TWO recorded fingerprints — the content a
/// content-only pass ships, and the full set including the MCP entry — which
/// is a column and a plumbing change, i.e. a decision for the plan's owner,
/// not something to invent here. Until then what the operator is told is:
/// nothing automatic. A host provisioned before the pane header keeps working
/// and simply proves no pane (`Caller::pane` is `None`), so every rule that
/// would have needed the proof refuses — fail-closed — and only a FULL
/// provisioning (`provision_hosts` without `content_only`, `fleet-hub
/// provision --host <alias>`) adds the header. `provision_content_only`'s
/// own doc comment says so from the other side.
pub fn fingerprint() -> &'static str {
    static FP: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    FP.get_or_init(|| {
        let ag: String = AG_FILES
            .iter()
            .map(|(path, body)| format!("{path}\u{0}{body}\u{0}"))
            .collect();
        crate::mcp::auth::sha256_hex(&format!(
            "{FLEET_SKILL}\u{0}{FRIENDLY_NAME_SKILL}\u{0}{CLAUDE_MD_BODY}\u{0}{}\u{0}{ag}\u{0}{VOICE_ARECORD}",
            crate::service::hooks_install::hook_shape()
        ))
    })
}

fn marker_content() -> String {
    format!(
        "claude-fleet manages this directory; provision_hosts overwrites it.\nfingerprint={}\n",
        fingerprint()
    )
}

/// Prefixes the probe's answer, so a login banner (`bash -lc` runs the
/// profile first) is never read as a git toplevel and refused as one.
const GIT_TOP_PREFIX: &str = "fleet-git-top=";

/// The toplevel [`git_tree_probe_script`] reported, or `""`.
fn git_top_of(stdout: &str) -> &str {
    stdout
        .lines()
        .rev()
        .find_map(|l| l.trim_end_matches('\r').strip_prefix(GIT_TOP_PREFIX))
        .unwrap_or("")
        .trim()
}

/// Prints the git toplevel (after [`GIT_TOP_PREFIX`]) when `~/.claude/skills` (following a symlink)
/// sits inside a work tree AND that tree tracks a file in one of fleet's two
/// skill dirs, else nothing. A dotfiles checkout that untracks or ignores
/// them is fine: provisioning then overwrites nothing anyone committed.
fn git_tree_probe_script() -> String {
    let dirs = [SKILL_DIR, FRIENDLY_NAME_SKILL_DIR]
        .iter()
        .map(|d| {
            quote(
                d.strip_prefix(SKILLS_ROOT)
                    .unwrap_or(d)
                    .trim_start_matches('/'),
            )
        })
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "cd {} 2>/dev/null && top=$(git rev-parse --show-toplevel 2>/dev/null) \
         && [ -n \"$(git ls-files -- {dirs} 2>/dev/null)\" ] && echo \"{GIT_TOP_PREFIX}$top\"; true",
        remote_path(SKILLS_ROOT)
    )
}

/// What one `provision_hosts` call covers (host identity & health, task 6).
#[derive(Debug, Clone, Default)]
pub struct ProvisionScope {
    /// Mint fresh tokens (full provisioning only).
    pub rotate: bool,
    /// One host, or every active host.
    pub only_host: Option<String>,
    /// Skills, CLAUDE.md block and hooks only: no token, no
    /// `~/.claude.json` rewrite, no tunnel. Safe to run unattended.
    pub content_only: bool,
}

/// `provision.force_git_tree`: write the skill dirs even inside a git
/// checkout (decision B-2: refuse by default).
fn force_git_tree(store: &Mutex<Store>) -> bool {
    lock(store)
        .ok()
        .and_then(|s| {
            s.get_setting(crate::service::settings::PROVISION_FORCE_GIT_TREE)
                .ok()
                .flatten()
        })
        .is_some_and(|v| v == "true")
}

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
    provision_one_with(ssh, host, base, token, false).await
}

/// [`provision_one`] with the git-tree preflight decided by the caller
/// (`provision.force_git_tree`, read where the store is at hand).
pub async fn provision_one_with(
    ssh: &dyn SshExec,
    host: &str,
    base: &HubBase,
    token: &str,
    force_git_tree: bool,
) -> Result<(), IpcError> {
    // 0. + 1. Skills (live-discovered, no restart). Both ship from the repo
    //    so every fleet host gets the same shared copy.
    provision_skills(ssh, host, force_git_tree).await?;
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
    provision_hook(
        ssh,
        host,
        &base.hook_url(),
        token,
        base.session_start_context,
    )
    .await?;
    // 5. The `/voice` recorder stand-in and the URL it streams from.
    provision_voice(ssh, host, base).await?;
    Ok(())
}

/// Voice relay F1: install fleet's `arecord` stand-in at
/// `~/.claude-fleet/voice/bin/arecord` (executable; the pane command puts
/// that dir first on `claude`'s PATH, `tmux::VOICE_PATH_PREFIX`) and
/// `voice.env` with the hub URL it streams from. The bearer is not here:
/// the stand-in reads the hook headers file `provision_hook` wrote.
async fn provision_voice(ssh: &dyn SshExec, host: &str, base: &HubBase) -> Result<(), IpcError> {
    let bin = format!("{VOICE_DIR}/bin");
    let path = format!("{bin}/arecord");
    write_host_file(ssh, host, &bin, &path, VOICE_ARECORD).await?;
    let out = crate::ssh::run_shell(
        ssh,
        host,
        &format!("chmod 755 {}", remote_path(&path)),
        PROVISION_TIMEOUT,
    )
    .await?;
    if !out.status.success() {
        return Err(IpcError::new(
            codes::E_PROVISION,
            format!(
                "chmod {path} on {host}: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        ));
    }
    let env = format!("FLEET_VOICE_URL={}\n", quote(&base.url));
    write_host_file(
        ssh,
        host,
        VOICE_DIR,
        &format!("{VOICE_DIR}/voice.env"),
        &env,
    )
    .await
}

/// Steps 0 and 1: refuse to write into somebody else's git checkout unless
/// told to (`provision.force_git_tree`, decision B-2), then both skills and
/// their `.fleet-managed` markers (hosts F2).
async fn provision_skills(
    ssh: &dyn SshExec,
    host: &str,
    force_git_tree: bool,
) -> Result<(), IpcError> {
    let toplevel = if host == "local" {
        String::new()
    } else {
        let script = quote(&git_tree_probe_script());
        let out = ssh
            .run(host, &["bash", "-lc", &script], PROVISION_TIMEOUT)
            .await?;
        git_top_of(&String::from_utf8_lossy(&out.stdout)).to_string()
    };
    if !toplevel.is_empty() && !force_git_tree {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!(
                "{host}: the git work tree {toplevel} tracks files in {SKILL_DIR} or \
                 {FRIENDLY_NAME_SKILL_DIR}; fleet would overwrite them. Untrack both dirs there \
                 (git rm -r --cached, then add them to .gitignore), or set \
                 provision.force_git_tree=true to write anyway"
            ),
        )
        .with_details(serde_json::json!({ "skills_dir": SKILLS_ROOT, "git_toplevel": toplevel })));
    }
    for (dir, path, body) in [
        (SKILL_DIR, SKILL_PATH, FLEET_SKILL),
        (
            FRIENDLY_NAME_SKILL_DIR,
            FRIENDLY_NAME_SKILL_PATH,
            FRIENDLY_NAME_SKILL,
        ),
    ] {
        write_host_file(ssh, host, dir, path, body).await?;
        write_host_file(
            ssh,
            host,
            dir,
            &format!("{dir}/{MANAGED_MARKER}"),
            &marker_content(),
        )
        .await?;
    }
    Ok(())
}

/// The non-secret half of provisioning (hosts F1): refresh what the
/// binary ships without touching the token or `~/.claude.json`. Reuses
/// the host's existing token for the hook headers; a host with none is
/// refused (`E_NO_TOKEN`) — it needs a full provisioning first.
///
/// `Ok(Some(warning))` is refreshed but degraded: the `ag` launcher did not
/// install ([`provision_ag`]); the host is still marked provisioned.
///
/// Because it does not rewrite `~/.claude.json`, it cannot add or repair the
/// MCP entry — including multi-user M1's `X-Fleet-Pane` header
/// ([`merge_mcp_entry`]) — yet it clears `provision_stale` for the host.
/// That asymmetry is why [`fingerprint`] does not cover the MCP entry; read
/// its doc comment before changing either.
pub async fn provision_content_only(
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    host: &str,
    base: &HubBase,
) -> Result<Option<String>, IpcError> {
    let token = {
        let s = lock(store)?;
        s.get_host_token(host)?.map(|t| t.token).ok_or_else(|| {
            IpcError::new(
                codes::E_NO_TOKEN,
                format!("{host} has no host token; run a full provision_hosts first"),
            )
        })?
    };
    let base = &base.with_start_context(&*lock(store)?);
    let force = force_git_tree(store);
    provision_skills(ssh, host, force).await?;
    provision_claude_md(ssh, host).await?;
    provision_hook(
        ssh,
        host,
        &base.hook_url(),
        &token,
        base.session_start_context,
    )
    .await?;
    provision_voice(ssh, host, base).await?;
    let warning = if install_ag(store) {
        provision_ag(ssh, host).await
    } else {
        None
    };
    // the ag step is retryable, so a warning from it is owed another pass
    mark_provisioned(store, host, warning.as_deref(), warning.is_some());
    Ok(warning)
}

/// Record a finished provisioning: provisioned either way, with the warning
/// kept so the reason survives the call and reaches `fleet_health` and
/// Attention, and the fingerprint forgotten when the run is owed a RETRY, so
/// it reads `provision_stale` instead of being recorded as delivered.
///
/// `owed_retry` is deliberately not `warning.is_some()` — see
/// `Store::record_host_provision_outcome`.
fn mark_provisioned(store: &Mutex<Store>, host: &str, warning: Option<&str>, owed_retry: bool) {
    if let Ok(s) = store.lock() {
        let _ = s.set_host_provisioned(host, true);
        let _ = s.record_host_provision_outcome(host, warning, owed_retry);
    }
}

/// How often a reprovision started under Pause all looks again.
const PAUSED_RECHECK: Duration = Duration::from_secs(60);

/// Refresh every reachable, non-hidden host whose stored fingerprint is
/// not this build's, `delay` after start (the first reconcile pass has
/// refreshed `reachable` by then). Content only: unattended and secret-free.
pub fn spawn_reprovision_stale(
    store: Arc<Mutex<Store>>,
    ssh: Arc<crate::ssh::SshClient>,
    base: HubBase,
    delay: Duration,
) -> tokio::task::JoinHandle<()> {
    crate::rt::spawn(async move {
        tokio::time::sleep(delay).await;
        // A one-shot: started under Pause all, it waits for the resume
        // rather than giving up until the next restart.
        while !crate::service::loops::gate("reprovision", &store, Some(PAUSED_RECHECK)) {
            tokio::time::sleep(PAUSED_RECHECK).await;
        }
        reprovision_stale_pass(&store, &*ssh, &base).await;
    })
}

/// One refresh of every stale host; the hosts it tried, or `None` when
/// Pause all (redesign 8.1) or an unreadable store stopped it.
async fn reprovision_stale_pass(
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    base: &HubBase,
) -> Option<Vec<String>> {
    if !crate::service::loops::gate("reprovision", store, None) {
        return None;
    }
    let stale: Vec<String> = match store.lock() {
        Ok(s) => crate::service::hosts::active_hosts(
            s.list_hosts().unwrap_or_default(),
            crate::service::hub::local_host_enabled(),
        )
        .into_iter()
        .filter(|h| h.provisioned && h.reachable && h.provision_stale)
        .map(|h| h.alias)
        .collect(),
        Err(_) => return None,
    };
    for host in &stale {
        match provision_content_only(store, ssh, host, base).await {
            Ok(None) => tracing::info!(host, "[provision] refreshed stale content"),
            Ok(Some(w)) => tracing::warn!(
                host,
                warning = %w,
                "[provision] refreshed stale content with a warning"
            ),
            Err(e) => tracing::warn!(
                host,
                code = %e.code,
                error = %e.message,
                "[provision] stale content not refreshed"
            ),
        }
    }
    crate::service::loops::report("reprovision", Ok::<_, String>(()), None);
    Some(stale)
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
    sync_start: bool,
) -> Result<(), IpcError> {
    let existing = read_host_file(ssh, host, SETTINGS_JSON).await?;
    // Errors (malformed JSON → E_PROVISION) fire BEFORE any write.
    let merged = super::hooks_install::merge_hook_into_settings_json_with(
        &existing, hook_url, token, sync_start,
    )?;
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

/// What [`provision_host_with_token`] reports for a WSL distribution whose
/// hooks cannot reach this desktop's `127.0.0.1` (WSL2's default NAT
/// networking): the sessions run, but no hook event ever arrives.
pub const WSL_HOOKS_UNREACHABLE: &str = "provisioned, but hooks can't reach the desktop from this \
     WSL distribution (WSL2 NAT networking): enable WSL mirrored networking \
     (networkingMode=mirrored in %USERPROFILE%\\.wslconfig), run `wsl --shutdown`, then \
     provision again";

/// Printed by [`hook_reach_script`] when the distribution has neither `curl`
/// nor `wget` to ask with: the check cannot tell, so it says nothing.
const HOOK_REACH_NO_CLIENT: &str = "FLEET-HOOKREACH-NOCLIENT";

/// The script, run inside a WSL distribution, that asks this desktop's
/// `/healthz` on `127.0.0.1:<port>` — the address its hooks post to. It
/// prints the answer (the listener's own body) or nothing.
pub fn hook_reach_script(port: u16) -> String {
    let url = quote(&format!("http://127.0.0.1:{port}/healthz"));
    format!(
        "if command -v curl >/dev/null 2>&1; then curl -s -m 5 {url}; \
         elif command -v wget >/dev/null 2>&1; then wget -q -T 5 -O - {url}; \
         else echo {HOOK_REACH_NO_CLIENT}; fi"
    )
}

/// Read [`hook_reach_script`]'s output: `Some(true)` when this desktop's
/// listener answered, `Some(false)` when nothing did (or something else did),
/// `None` when the distribution had no client to ask with.
pub fn hook_reach_verdict(stdout: &str) -> Option<bool> {
    if stdout.contains(HOOK_REACH_NO_CLIENT) {
        return None;
    }
    Some(stdout.contains(crate::mcp::HEALTHZ_BODY.trim_end()))
}

/// For a WSL host of a loopback desktop: can the distribution's hooks reach
/// `127.0.0.1:<port>`? `Some(`[`WSL_HOOKS_UNREACHABLE`]`)` when they cannot;
/// `None` when they can, or when the check itself could not run (a failed
/// `wsl.exe` is the provisioning's error to report, not this one's).
pub async fn wsl_hooks_warning(ssh: &dyn SshExec, host: &str, port: u16) -> Option<String> {
    let script = quote(&hook_reach_script(port));
    let out = ssh
        .run(host, &["sh", "-c", &script], PROVISION_TIMEOUT)
        .await
        .ok()?;
    match hook_reach_verdict(&String::from_utf8_lossy(&out.stdout)) {
        Some(false) => {
            tracing::warn!(
                host = %host,
                port,
                "[provision] hooks cannot reach 127.0.0.1 from this WSL distribution; \
                 WSL mirrored networking is needed"
            );
            Some(WSL_HOOKS_UNREACHABLE.to_string())
        }
        _ => None,
    }
}

/// Provision ONE host end to end with its own token: resolve/mint → write
/// files → persist the token → ensure the tunnel (remote host, loopback hub) → mark
/// provisioned. Shared by [`provision_hosts`] and the per-host Rotate action.
///
/// `Ok(Some(warning))` is provisioned but degraded: a WSL distribution whose
/// hooks cannot reach this desktop ([`wsl_hooks_warning`]), the `ag`
/// launcher not installing ([`provision_ag`]), or both (joined with `; `).
pub async fn provision_host_with_token(
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    tunnels: &Arc<TunnelSupervisor>,
    host: &str,
    base: &HubBase,
    rotate: bool,
) -> Result<Option<String>, IpcError> {
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
    // `work.session_start_context` (M4.5) decides how SessionStart installs.
    let base = &base.with_start_context(&*lock(store)?);
    provision_one_with(ssh, host, base, &token, force_git_tree(store)).await?;
    commit_host_token(store, host, &token, minted)?;
    // A public hub is reached directly; only a loopback hub needs the
    // reverse tunnel so the host's 127.0.0.1:<port> lands on this machine.
    // An agent host is never dialed over SSH at all, so it has no use for
    // one either — it reaches the hub over its own outbound connection.
    // A WSL distribution is not dialed over SSH either: `ssh -R` cannot
    // reach it, and where its hooks can reach this machine at all (WSL1,
    // WSL2 mirrored networking) they do so on 127.0.0.1 directly.
    //
    // No tunnel also means nothing fleet can repair: whether the hooks get
    // here is WSL's networking mode. So it is checked, from inside the
    // distribution, and said at once rather than found out from a Stop
    // that never arrives.
    let wsl = crate::wsl::is_wsl_host(host);
    if host != "local" && !base.public && !routes_to_agent(store, host)? && !wsl {
        tunnels.ensure(host, base.port, base.port);
    }
    let wsl_warning = if wsl && !base.public {
        wsl_hooks_warning(ssh, host, base.port).await
    } else {
        None
    };
    let ag_warning = if install_ag(store) {
        provision_ag(ssh, host).await
    } else {
        None
    };
    // Only the `ag` outcome marks the run degraded. `wsl_warning` reports a
    // persistent environment condition (WSL mirrored networking is needed),
    // which recurs on every provisioning until the operator changes their WSL
    // config — clearing the fingerprint for it would leave that host forever
    // `provision_stale` and re-provisioned on every tick.
    let owed_retry = ag_warning.is_some();
    let warning = match (wsl_warning, ag_warning) {
        (Some(a), Some(b)) => Some(format!("{a}; {b}")),
        (a, b) => a.or(b),
    };
    // Both warnings are KEPT; only the ag one is owed a retry.
    mark_provisioned(store, host, warning.as_deref(), owed_retry);
    Ok(warning)
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

/// `bash -lc` body that removes an earlier stage, but only one fleet owns
/// (it carries [`MANAGED_MARKER`]), so a file dropped from tools/ag does
/// not linger there and get copied into the install.
fn ag_clear_stage_script() -> String {
    format!(
        "if [ -f {} ]; then rm -r {}; fi",
        remote_path(&format!("{AG_STAGE_DIR}/{MANAGED_MARKER}")),
        remote_path(AG_STAGE_DIR)
    )
}

/// `bash -lc` body that runs the staged installer (see [`provision_ag`]).
///
/// `AG_HOME`/`AG_BIN_DIR` go in the command's own environment so the host's
/// login profile cannot redirect where fleet installs.
fn ag_install_script() -> String {
    let stage = remote_path(AG_STAGE_DIR);
    format!(
        "env AG_HOME={} AG_BIN_DIR={} bash {stage}/install.sh --from {stage} --alias {} </dev/null 2>&1",
        remote_path(AG_HOME_DIR),
        remote_path(AG_BIN_DIR),
        quote(AG_CL_ALIAS)
    )
}

/// `provision.install_ag` (default on).
fn install_ag(store: &Mutex<Store>) -> bool {
    lock(store)
        .map(|s| {
            crate::service::settings::get_bool(&s, crate::service::settings::PROVISION_INSTALL_AG)
        })
        .unwrap_or(false)
}

/// Step 5 (F2): clear fleet's previous stage, then stage fleet's `ag`
/// launcher under [`AG_STAGE_DIR`] (with a
/// `.fleet-managed` marker, which the installer copies along) and run its
/// installer: `~/.local/share/ag`, `~/.local/bin/ag`, a config with the
/// `cl` alias unless the user already has one, and the `cl` shim.
/// Optional by design — `None` when installed, otherwise a warning and
/// provisioning carries on: the pane command's fallback still reaches
/// plain `claude` (see `tmux::CL_FALLBACK`).
pub async fn provision_ag(ssh: &dyn SshExec, host: &str) -> Option<String> {
    match crate::ssh::run_shell(ssh, host, &ag_clear_stage_script(), PROVISION_TIMEOUT).await {
        Ok(out) if out.status.success() => {}
        Ok(out) => {
            return Some(format!(
                "ag launcher not installed: cannot clear the old stage: {}",
                String::from_utf8_lossy(&out.stderr)
                    .trim()
                    .chars()
                    .take(300)
                    .collect::<String>()
            ))
        }
        Err(e) => return Some(format!("ag launcher not installed: {}", e.message)),
    }
    let files = AG_FILES.iter().map(|(rel, body)| {
        let dir = match rel.rsplit_once('/') {
            Some((d, _)) => format!("{AG_STAGE_DIR}/{d}"),
            None => AG_STAGE_DIR.to_string(),
        };
        (dir, format!("{AG_STAGE_DIR}/{rel}"), body.to_string())
    });
    let marker = (
        AG_STAGE_DIR.to_string(),
        format!("{AG_STAGE_DIR}/{MANAGED_MARKER}"),
        marker_content(),
    );
    for (dir, path, body) in files.chain(std::iter::once(marker)) {
        if let Err(e) = write_host_file(ssh, host, &dir, &path, &body).await {
            return Some(format!("ag launcher not installed: {}", e.message));
        }
    }
    match crate::ssh::run_shell(ssh, host, &ag_install_script(), AG_INSTALL_TIMEOUT).await {
        Ok(out) if out.status.success() => None,
        Ok(out) => {
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            let tail: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
            let tail = tail[tail.len().saturating_sub(3)..].join(" | ");
            Some(format!(
                "ag launcher not installed (install.sh exit {}): {}",
                out.status
                    .code()
                    .map_or_else(|| "signal".to_string(), |c| c.to_string()),
                tail.chars().take(300).collect::<String>()
            ))
        }
        Err(e) => Some(format!("ag launcher not installed: {}", e.message)),
    }
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
    scope: ProvisionScope,
) -> Result<Vec<HostProvisionResult>, IpcError> {
    let hosts = {
        let s = lock(store)?;
        // One hidden/local rule for every host loop (hub-ops F6).
        crate::service::hosts::active_hosts(
            s.list_hosts()?,
            crate::service::hub::local_host_enabled(),
        )
    };
    let mut results = Vec::new();
    for h in hosts {
        if scope
            .only_host
            .as_deref()
            .is_some_and(|only| only != h.alias)
        {
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
        let outcome = if scope.content_only {
            provision_content_only(store, ssh, &h.alias, base).await
        } else {
            provision_host_with_token(store, ssh, tunnels, &h.alias, base, scope.rotate).await
        };
        match outcome {
            Ok(warning) => {
                // A warning is appended: the host still needs (or not) its
                // Claude restart, whatever else went wrong.
                let done = if scope.content_only {
                    "skills, CLAUDE.md block and hooks refreshed (no restart needed)"
                } else {
                    "restart Claude on this host to load the MCP server"
                };
                results.push(HostProvisionResult {
                    host: h.alias,
                    status: "provisioned".into(),
                    detail: Some(match warning {
                        Some(w) => format!("{done}; {w}"),
                        None => done.to_string(),
                    }),
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
        if h.provisioned
            && h.alias != "local"
            && !h.hidden
            && h.transport != "agent"
            && !crate::wsl::is_wsl_host(&h.alias)
        {
            tunnels.ensure(&h.alias, base.port, base.port);
        }
    }
    Ok(())
}

/// Read a file from a host. `local` → `std::fs`; remote → `cat` over SSH.
/// Missing file → `Ok(String::new())` (caller treats as empty config).
///
/// ONLY a missing file is empty. Every caller merges into what this returns
/// and writes the result back over the file, so a read that failed — ssh
/// dropped (exit 255), a root-owned 0600 `~/.claude.json` left by `sudo
/// claude` — must not read as "empty": that rewrote the user's whole Claude
/// config as `{mcpServers:{…}}`, and skipped its backup because there was
/// nothing to back up. Bytes that are not UTF-8 are refused for the same
/// reason: a lossy decode wrote U+FFFD back over them.
pub async fn read_host_file(ssh: &dyn SshExec, host: &str, path: &str) -> Result<String, IpcError> {
    if host == "local" {
        crate::service::hub::ensure_local_allowed(host)?;
        let expanded = expand_home_local(path)?;
        return match std::fs::read(&expanded) {
            Ok(bytes) => host_text(host, path, bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
            Err(e) => Err(IpcError::new(
                codes::E_PROVISION,
                format!("read {path} on {host}: {e}"),
            )),
        };
    }
    // Outer `quote` makes the whole script cross the SSH boundary as ONE shell
    // word — ssh space-joins argv, so an unquoted multi-word script would be
    // re-split by the remote login shell (mirrors claude_cli.rs).
    let script = quote(&remote_read_script(path));
    let out = ssh
        .run(host, &["bash", "-lc", &script], PROVISION_TIMEOUT)
        .await?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(IpcError::new(
            codes::E_PROVISION,
            format!(
                "read {path} on {host} failed ({}): {}",
                out.status,
                crate::logging::redact(stderr.trim())
            ),
        ));
    }
    host_text(host, path, read_payload(&out.stdout).to_vec())
}

/// A read file's bytes as text, or an error naming it when they are not UTF-8.
fn host_text(host: &str, path: &str, bytes: Vec<u8>) -> Result<String, IpcError> {
    String::from_utf8(bytes).map_err(|_| {
        IpcError::new(
            codes::E_PROVISION,
            format!("{path} on {host} is not UTF-8 text; fleet will not rewrite it"),
        )
    })
}

/// The file's bytes from [`remote_read_script`]'s stdout: what follows the
/// marker line. `bash -lc` runs the login profile first, and whatever it
/// prints (a banner, an MOTD) lands ahead of the file; taken whole, it was
/// written back into `~/.tmux.conf` / `~/.claude/CLAUDE.md` and broke the
/// JSON parse of `~/.claude.json`. Without a marker (a host that answered
/// some other way) the stdout is taken as it is.
fn read_payload(stdout: &[u8]) -> &[u8] {
    crate::service::move_session::carry::payload(stdout).unwrap_or(stdout)
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
    let mut f = open_private(path)?;
    f.write_all(content.as_bytes())?;
    f.flush()?;
    Ok(())
}

/// Replace `path` with `content`, 0600, so a reader (or a crash) sees the old
/// file or the new one and never a truncated one: written to a tmp file
/// beside it, then renamed over it ([`place_private_file`], with its copy
/// fallback for a bind-mounted target). [`write_private_file`] truncates in
/// place, which is fine for a file nobody else reads; `~/.claude/settings.json`
/// is read by every Claude Code start, and a crash mid-write left it cut off.
///
/// A symlinked `path` (settings kept in a dotfiles repo) is replaced at its
/// target, so the link survives.
pub fn replace_private_file(path: &std::path::Path, content: &str) -> std::io::Result<()> {
    let target = match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => {
            std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
        }
        _ => path.to_path_buf(),
    };
    let mut tmp = target.as_os_str().to_owned();
    tmp.push(format!(".fleet-tmp-{}", std::process::id()));
    let tmp = std::path::PathBuf::from(tmp);
    if let Err(e) = write_private_file(&tmp, content) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    place_private_file(&tmp, &target, |f, t| std::fs::rename(f, t))
}

/// Open `path` for writing, truncated, at mode 0600 BEFORE a byte is
/// written. `mode()` only applies when the file is created, so a
/// pre-existing 0644 file (an older build's, a restored backup) is tightened
/// on the open descriptor first; tightening it after the write left the new
/// secret world-readable in between.
fn open_private(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let f = opts.open(path)?;
    #[cfg(unix)]
    if let Err(e) = f.set_permissions(
        <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o600),
    ) {
        // As `set_private_mode`: a filesystem that refuses chmod (a drvfs
        // mount) still gets the write; say so rather than fail it.
        tracing::warn!(
            path = %path.display(),
            error = %e,
            "[provision] chmod 600 failed; the file may be readable by other users"
        );
    }
    Ok(f)
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
    format!(
        "printf '\\n%s\\n' {}; if [ -e {p} ]; then cat {p}; fi",
        crate::service::move_session::carry::OUT_MARKER,
        p = remote_path(path)
    )
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
    #[cfg(unix)]
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

/// The current content of a host's `~/.claude.json` as the JSON object
/// Claude Code writes: a missing (empty) file is an empty object, and
/// anything else that is not an object is refused with `E_PROVISION` so no
/// merge ever writes over a file it did not understand. Shared by every
/// read-merge-write on that file ([`merge_mcp_entry`],
/// `operator::pre_trust_claude_json`).
pub(crate) fn parse_claude_json(existing: &str) -> Result<serde_json::Value, IpcError> {
    let root: serde_json::Value = if existing.trim().is_empty() {
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
    Ok(root)
}

/// Merge the claude-fleet HTTP MCP server entry into a host's `~/.claude.json`
/// content, preserving every existing key. Returns the new JSON (pretty).
/// Errors if `existing` is non-empty and not valid JSON.
///
/// Two headers travel with every tool call the host's Claude makes:
///
/// - `Authorization`, the host's own bearer token; and
/// - `X-Fleet-Pane`, the calling tmux pane — multi-user M1's pane proof
///   (plan revision 6, R6-i). Because it is on the CONNECTION rather than an
///   argument on a handful of tools, `mcp::authorize` can stamp
///   `Caller::pane` and every tool can then ask "is this the one row whose
///   pane the caller can prove it is in".
///
/// The pane header is written in the **braced** `${TMUX_PANE:-}` form, not
/// the bare `$TMUX_PANE` the hooks entry uses
/// (`service::hooks_install::hook_entry`). The two are not interchangeable:
/// the hooks entry works only because it also carries
/// `"allowedEnvVars": ["TMUX_PANE"]`, which is a HOOKS-only mechanism with no
/// equivalent for an MCP server entry. Claude Code expands `${VAR}` and
/// `${VAR:-default}` inside an MCP entry's `headers` with no allow-list key,
/// and the `:-` default makes a host outside tmux send an empty value rather
/// than an unexpanded literal — both of which `mcp::hooks::pane_header`
/// refuses, so either way the caller simply proves no pane.
pub fn merge_mcp_entry(existing: &str, url: &str, token: &str) -> Result<String, IpcError> {
    let mut root = parse_claude_json(existing)?;
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
            "headers": {
                "Authorization": format!("Bearer {token}"),
                "X-Fleet-Pane": "${TMUX_PANE:-}"
            }
        }),
    );
    serde_json::to_string_pretty(&root)
        .map_err(|e| IpcError::new(codes::E_PROVISION, format!("serialize: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A WSL2 distribution under NAT networking reaches nothing on
    /// `127.0.0.1:<port>`: the check says so; the listener's own answer, or
    /// a distribution with no client to ask with, says nothing.
    #[tokio::test]
    async fn wsl_hooks_warning_reads_the_healthz_answer_from_inside_the_distribution() {
        use crate::ssh_fake::{FakeSsh, Match, Reply};
        let script = hook_reach_script(4180);
        assert!(
            script.contains("'http://127.0.0.1:4180/healthz'"),
            "{script}"
        );
        assert!(script.contains("curl -s -m 5"), "{script}");
        assert!(script.contains("wget -q -T 5 -O -"), "{script}");

        let ssh = FakeSsh::new();
        ssh.on_host("wsl-nat", Match::contains("healthz"), Reply::fail(7, ""));
        ssh.on_host(
            "wsl-mirrored",
            Match::contains("healthz"),
            Reply::ok(crate::mcp::HEALTHZ_BODY),
        );
        ssh.on_host(
            "wsl-bare",
            Match::contains("healthz"),
            Reply::ok(&format!("{HOOK_REACH_NO_CLIENT}\n")),
        );
        ssh.on_host(
            "wsl-other",
            Match::contains("healthz"),
            Reply::ok("<html>not fleet</html>"),
        );
        assert_eq!(
            wsl_hooks_warning(&ssh, "wsl-nat", 4180).await.as_deref(),
            Some(WSL_HOOKS_UNREACHABLE)
        );
        assert_eq!(
            wsl_hooks_warning(&ssh, "wsl-other", 4180).await.as_deref(),
            Some(WSL_HOOKS_UNREACHABLE)
        );
        assert_eq!(wsl_hooks_warning(&ssh, "wsl-mirrored", 4180).await, None);
        assert_eq!(wsl_hooks_warning(&ssh, "wsl-bare", 4180).await, None);
        let call = &ssh.calls_for("wsl-nat")[0];
        assert_eq!(call.args[..2], ["sh", "-c"]);
        assert!(WSL_HOOKS_UNREACHABLE.contains("mirrored networking"));
    }

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
        // Multi-user M1's pane proof, in the braced form — the bare
        // `$TMUX_PANE` the hooks entry uses would arrive unexpanded here,
        // since `allowedEnvVars` is a hooks-only mechanism.
        assert_eq!(
            v["mcpServers"]["claude-fleet"]["headers"]["X-Fleet-Pane"],
            "${TMUX_PANE:-}"
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
        // A re-merge over an existing file keeps the pane header — a host
        // re-provisioned for a rotated token must not lose its pane proof.
        assert_eq!(
            v2["mcpServers"]["claude-fleet"]["headers"]["X-Fleet-Pane"],
            "${TMUX_PANE:-}"
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
            "printf '\\n%s\\n' __CF_OUT__; if [ -e \"$HOME\"/'.claude.json' ]; \
             then cat \"$HOME\"/'.claude.json'; fi"
        );
    }

    /// Absent is empty and exits 0; present but unreadable exits non-zero, so
    /// [`read_host_file`] can tell the two apart.
    #[cfg(unix)]
    #[test]
    fn the_read_script_fails_on_a_file_it_cannot_read() {
        let dir = tempfile::tempdir().unwrap();
        let run = |p: &std::path::Path| {
            std::process::Command::new("bash")
                .args(["-c", &remote_read_script(&p.to_string_lossy())])
                .output()
                .unwrap()
        };
        let absent = run(&dir.path().join("absent"));
        assert!(absent.status.success());
        assert_eq!(read_payload(&absent.stdout), b"");
        // A directory where the file should be: `cat` fails as it would on
        // EACCES, and that holds for uid 0 too.
        let dir_in_the_way = dir.path().join("d");
        std::fs::create_dir(&dir_in_the_way).unwrap();
        assert!(!run(&dir_in_the_way).status.success());
    }

    /// A read that failed is an error, never an empty file: every caller
    /// merges into the answer and writes it back over the user's config.
    #[tokio::test]
    async fn a_failed_read_is_not_an_empty_file() {
        let fake = fresh_host();
        fake.unreachable("h1");
        let err = read_host_file(&fake, "h1", CLAUDE_JSON).await.unwrap_err();
        assert_eq!(err.code, codes::E_PROVISION);

        let fake = fresh_host();
        fake.on(
            Match::script(&remote_read_script(CLAUDE_JSON)),
            Reply::fail(1, "cat: /home/u/.claude.json: Permission denied"),
        );
        let err = read_host_file(&fake, "h1", CLAUDE_JSON).await.unwrap_err();
        assert!(err.message.contains("Permission denied"), "{}", err.message);

        let fake = fresh_host();
        fake.on(
            Match::script(&remote_read_script(CLAUDE_JSON)),
            Reply::ok("\n__CF_OUT__\n"),
        );
        assert_eq!(read_host_file(&fake, "h1", CLAUDE_JSON).await.unwrap(), "");
    }

    #[test]
    fn bytes_that_are_not_utf8_are_refused_not_mangled() {
        let err = host_text("h1", CLAUDE_MD_PATH, vec![b'a', 0xff, b'b']).unwrap_err();
        assert_eq!(err.code, codes::E_PROVISION);
        assert_eq!(
            host_text("h1", CLAUDE_MD_PATH, b"ok".to_vec()).unwrap(),
            "ok"
        );
    }

    #[test]
    fn a_login_banner_is_not_part_of_a_read_file() {
        assert_eq!(
            read_payload(b"Welcome\n\n__CF_OUT__\n{\"a\":1}"),
            b"{\"a\":1}"
        );
        assert_eq!(read_payload(b"\n__CF_OUT__\n"), b"");
        assert_eq!(read_payload(b"{\"a\":1}"), b"{\"a\":1}");
    }

    #[cfg(unix)]
    #[test]
    fn the_read_script_prints_the_marker_then_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("f");
        std::fs::write(&f, "x\ny\n").unwrap();
        let out = std::process::Command::new("bash")
            .args([
                "-c",
                &format!("echo banner; {}", remote_read_script(&f.to_string_lossy())),
            ])
            .output()
            .unwrap();
        assert_eq!(read_payload(&out.stdout), b"x\ny\n");
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

    #[cfg(unix)]
    #[test]
    fn a_pre_existing_loose_file_is_private_before_the_secret_is_written() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("token");
        std::fs::write(&p, "old").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        let f = open_private(&p).unwrap();
        assert_eq!(f.metadata().unwrap().permissions().mode() & 0o777, 0o600);
        drop(f);
        write_private_file(&p, "new").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "new");
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    /// The local twin of the remote fallback: when the rename fails the way
    /// a bind-mounted target makes it fail, the content must still land, at
    /// 0600 even though the target was world-readable, with no tmp left.
    #[cfg(unix)]
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
        if crate::service::move_session::carry::tests::skip_as_root(
            "mode 0500 does not stop uid 0 writing into the directory",
        ) {
            return;
        }
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
            // 0. the git-tree preflight (task 6): nothing on a fresh host
            Script(git_tree_probe_script()),
            // 1. skills, each followed by its ownership marker
            Script(remote_write_script(SKILL_DIR, SKILL_PATH, FLEET_SKILL)),
            Script(remote_write_script(
                SKILL_DIR,
                &format!("{SKILL_DIR}/{MANAGED_MARKER}"),
                &marker_content(),
            )),
            Script(remote_write_script(
                FRIENDLY_NAME_SKILL_DIR,
                FRIENDLY_NAME_SKILL_PATH,
                FRIENDLY_NAME_SKILL,
            )),
            Script(remote_write_script(
                FRIENDLY_NAME_SKILL_DIR,
                &format!("{FRIENDLY_NAME_SKILL_DIR}/{MANAGED_MARKER}"),
                &marker_content(),
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
        // 5. the `/voice` recorder stand-in and its config (voice relay F1)
        steps.extend(voice_steps());
        steps
    }

    /// What [`provision_voice`] issues on a remote host for [`base`].
    fn voice_steps() -> Vec<Step> {
        use Step::*;
        vec![
            Script(remote_write_script(
                "~/.claude-fleet/voice/bin",
                "~/.claude-fleet/voice/bin/arecord",
                VOICE_ARECORD,
            )),
            Script("chmod 755 \"$HOME\"/'.claude-fleet/voice/bin/arecord'".into()),
            Script(remote_write_script(
                "~/.claude-fleet/voice",
                "~/.claude-fleet/voice/voice.env",
                "FLEET_VOICE_URL='http://127.0.0.1:4180'\n",
            )),
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
                    // Paths only: a written file's payload (the voice
                    // stand-in names `$HOME/.claude/fleet-hook.headers`) is
                    // data, not a path the remote shell resolves.
                    let paths = match (body.find(" printf '%s' "), body.rfind("' > ")) {
                        (Some(start), Some(end)) if start < end => {
                            format!("{}{}", &body[..start], &body[end + 1..])
                        }
                        _ => body.clone(),
                    };
                    for path in [
                        ".claude/skills",
                        ".claude/CLAUDE.md",
                        ".claude.json",
                        ".tmux.conf",
                        ".claude/settings.json",
                        ".claude/fleet-hook.headers",
                    ] {
                        if paths.contains(path) {
                            assert!(
                                paths.contains(&format!("\"$HOME\"/'{path}")),
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

    /// hosts F1: the content fingerprint that `provision_stale` compares.
    ///
    /// Pins exactly WHICH inputs it hashes, in both directions: the four it
    /// covers, and — stated as an assertion rather than a sentence — that the
    /// `~/.claude.json` MCP entry is not among them, so nobody reads
    /// multi-user M1's pane header into a staleness signal it does not
    /// produce. [`fingerprint`]'s doc comment says why that gap is where it
    /// is and what would have to change to close it.
    #[test]
    fn fingerprint_is_stable_and_covers_skills_claude_md_the_hook_shape_and_ag() {
        let fp = fingerprint();
        assert_eq!(fp.len(), 64, "sha256 hex");
        assert_eq!(fp, fingerprint(), "computed once, same every call");
        let ag: String = AG_FILES
            .iter()
            .map(|(p, b)| format!("{p}\u{0}{b}\u{0}"))
            .collect();
        let expected = crate::mcp::auth::sha256_hex(&format!(
            "{FLEET_SKILL}\u{0}{FRIENDLY_NAME_SKILL}\u{0}{CLAUDE_MD_BODY}\u{0}{}\u{0}{ag}\u{0}{VOICE_ARECORD}",
            crate::service::hooks_install::hook_shape()
        ));
        assert_eq!(fp, expected);
        // The hook shape names every event, its matcher and kind, and the
        // SessionStart command template — a new hook event changes it.
        let shape = crate::service::hooks_install::hook_shape();
        assert!(shape.contains("Stop||http"));
        assert!(shape.contains("SessionStart||command"));
        assert!(shape.contains("fleet-hook.headers"));
        // And an ag change moves the value too — not just carried along inert.
        let without_ag = crate::mcp::auth::sha256_hex(&format!(
            "{FLEET_SKILL}\u{0}{FRIENDLY_NAME_SKILL}\u{0}{CLAUDE_MD_BODY}\u{0}{}",
            crate::service::hooks_install::hook_shape()
        ));
        assert_ne!(
            fp, without_ag,
            "an ag change must make hosts provision_stale"
        );
        // And the gap, pinned: the MCP entry is NOT an input. Changing the
        // entry — as multi-user M1 did, adding `X-Fleet-Pane` — leaves the
        // fingerprint of a host provisioned before it identical, so that host
        // does not read `provision_stale` and the operator is not told.
        // `merge_mcp_entry`'s own output is the witness: with the url and
        // token held fixed, it is not reachable from `expected` above.
        let entry = merge_mcp_entry("", "http://127.0.0.1:4180/mcp", "tok").unwrap();
        assert!(
            entry.contains("X-Fleet-Pane"),
            "the entry carries the pane header"
        );
        // `AG_FILES` joined the fingerprint on `main`, so the launcher tree
        // is an input too: the pin has to ask about every input, never a
        // list that was complete when it was written.
        let ag_bodies: Vec<&str> = AG_FILES.iter().map(|(_, b)| *b).collect();
        for covered in [
            FLEET_SKILL,
            FRIENDLY_NAME_SKILL,
            CLAUDE_MD_BODY,
            &crate::service::hooks_install::hook_shape(),
        ]
        .into_iter()
        .chain(ag_bodies)
        {
            assert!(
                !covered.contains("\"X-Fleet-Pane\": \"${TMUX_PANE:-}\""),
                "if an input ever carries the MCP entry's pane header, this \
                 test is stale and so is `fingerprint`'s doc comment"
            );
        }
    }

    /// F2: every file of tools/ag (except README.md), at any depth, is
    /// compiled in, so a new lib/driver file cannot silently stay off the
    /// hosts.
    #[test]
    fn every_ag_file_is_compiled_in() {
        fn walk(root: &std::path::Path, rel: &str, out: &mut Vec<String>) {
            for e in std::fs::read_dir(root.join(rel)).expect("read tools/ag") {
                let e = e.unwrap();
                let name = e.file_name().to_string_lossy().into_owned();
                let path = if rel.is_empty() {
                    name
                } else {
                    format!("{rel}/{name}")
                };
                let ty = e.file_type().unwrap();
                if ty.is_dir() {
                    walk(root, &path, out);
                } else if path != "README.md" {
                    out.push(path);
                }
            }
        }
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tools/ag");
        let mut on_disk = Vec::new();
        walk(&root, "", &mut on_disk);
        on_disk.sort();
        let mut listed: Vec<String> = AG_FILES.iter().map(|(p, _)| p.to_string()).collect();
        listed.sort();
        assert_eq!(on_disk, listed, "list every tools/ag file in AG_FILES");
    }

    /// C1: every file a workspace crate embeds from outside `crates/`
    /// (`include_str!` / `include_bytes!` reaching `../../../../…`) must be in
    /// the hub image's build context, or the tagged image build breaks while
    /// every other build — which compiles from the full checkout — stays
    /// green. It checked only tools/ag until v0.4.10's hub image failed on
    /// `tools/voice/arecord`, which no `COPY` carried.
    #[test]
    fn the_hub_image_build_context_ships_every_embedded_file() {
        use std::path::{Component, Path, PathBuf};
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let dockerfile_path = repo.join("crates/fleet-hub/Dockerfile");
        let dockerfile =
            std::fs::read_to_string(&dockerfile_path).expect("read the hub Dockerfile");
        // The source of every build-stage `COPY <src> <dst>` (not `--from=`).
        let copied: Vec<String> = dockerfile
            .lines()
            .filter_map(|l| l.trim_start().strip_prefix("COPY "))
            .filter(|rest| !rest.trim_start().starts_with("--"))
            .filter_map(|rest| rest.split_whitespace().next())
            .map(|s| s.trim_end_matches('/').to_string())
            .collect();

        fn normalize(p: &Path) -> PathBuf {
            let mut out = PathBuf::new();
            for c in p.components() {
                match c {
                    Component::ParentDir => {
                        out.pop();
                    }
                    Component::CurDir => {}
                    other => out.push(other),
                }
            }
            out
        }
        fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    // Integration tests are never compiled into the image.
                    if p.file_name().is_some_and(|n| n != "tests" && n != "target") {
                        rs_files(&p, out);
                    }
                } else if p.extension().is_some_and(|x| x == "rs") {
                    out.push(p);
                }
            }
        }
        let mut files = Vec::new();
        rs_files(&repo.join("crates"), &mut files);
        let mut missing = Vec::new();
        let mut seen = 0;
        for file in files {
            let name = file.file_name().unwrap().to_string_lossy().to_string();
            // Unit-test files are compiled only under cfg(test), never in the image.
            if name == "tests.rs" || name.starts_with("tests_") {
                continue;
            }
            let src = std::fs::read_to_string(&file).unwrap();
            for mac in ["include_str!(\"", "include_bytes!(\""] {
                for (i, _) in src.match_indices(mac) {
                    let rest = &src[i + mac.len()..];
                    let Some(end) = rest.find('"') else { continue };
                    let target = normalize(&file.parent().unwrap().join(&rest[..end]));
                    let Ok(rel) = target.strip_prefix(normalize(&repo)) else {
                        continue;
                    };
                    let rel = rel.to_string_lossy().replace('\\', "/");
                    if rel.starts_with("crates/") {
                        continue;
                    }
                    seen += 1;
                    let covered = copied
                        .iter()
                        .any(|c| rel == *c || rel.starts_with(&format!("{c}/")));
                    if !covered {
                        missing.push(format!("{rel} (embedded by {})", file.display()));
                    }
                }
            }
        }
        assert!(
            seen > 0,
            "found no out-of-crate embeds — the scan is broken"
        );
        assert!(
            missing.is_empty(),
            "{} has no `COPY` for these embedded files:\n  {}",
            dockerfile_path.display(),
            missing.join("\n  ")
        );
    }

    /// The exact script / calls [`provision_ag`] issues on a fresh host:
    /// fleet's old stage cleared, every [`AG_FILES`] entry staged, then its
    /// marker, then the installer.
    fn ag_stage_steps() -> Vec<Step> {
        let mut steps = vec![Step::Script(ag_clear_stage_script())];
        for (rel, body) in AG_FILES {
            let path = format!("{AG_STAGE_DIR}/{rel}");
            let dir = match rel.rsplit_once('/') {
                Some((d, _)) => format!("{AG_STAGE_DIR}/{d}"),
                None => AG_STAGE_DIR.to_string(),
            };
            steps.push(Step::Script(remote_write_script(&dir, &path, body)));
        }
        steps.push(Step::Script(remote_write_script(
            AG_STAGE_DIR,
            &format!("{AG_STAGE_DIR}/{MANAGED_MARKER}"),
            &marker_content(),
        )));
        steps.push(Step::Script(ag_install_script()));
        steps
    }

    #[tokio::test]
    async fn provision_ag_stages_the_tree_then_runs_its_installer() {
        let fake = fresh_host();
        assert_eq!(provision_ag(&fake, "h1").await, None);
        let calls = fake.calls();
        let steps: Vec<Step> = calls.iter().map(step_of).collect();
        assert_eq!(steps, ag_stage_steps());
        assert_quoting_invariants(&calls);
    }

    #[test]
    fn ag_clear_stage_script_removes_only_a_fleet_managed_stage() {
        assert_eq!(
            ag_clear_stage_script(),
            "if [ -f \"$HOME\"/'.local/share/fleet/ag-src/.fleet-managed' ]; then rm -r \"$HOME\"/'.local/share/fleet/ag-src'; fi"
        );
    }

    #[test]
    fn ag_install_script_is_quoted_and_adds_the_cl_alias() {
        assert_eq!(
            ag_install_script(),
            "env AG_HOME=\"$HOME\"/'.local/share/ag' AG_BIN_DIR=\"$HOME\"/'.local/bin' bash \"$HOME\"/'.local/share/fleet/ag-src'/install.sh --from \"$HOME\"/'.local/share/fleet/ag-src' --alias 'cl=claude --yolo' </dev/null 2>&1"
        );
    }

    /// The installer is told where to install, and it is the one place the
    /// pane's own fallback looks. Two unrelated literals here would mean a
    /// provisioned host whose panes still fall through to plain `claude`.
    #[test]
    fn ag_paths_match_the_pane_fallback() {
        let home = AG_HOME_DIR.strip_prefix('~').expect("~-relative");
        assert!(
            crate::tmux::CL_FALLBACK.contains(&format!("$HOME{home}/ag")),
            "CL_FALLBACK must probe {AG_HOME_DIR}/ag; it reads: {}",
            crate::tmux::CL_FALLBACK
        );
        // and the installer is told that same path, not left to the profile
        assert!(ag_install_script().contains(&remote_path(AG_HOME_DIR)));
        assert!(ag_install_script().contains(&remote_path(AG_BIN_DIR)));
    }

    /// A failed install is a warning, never an error: the pane command's
    /// fallback still reaches plain `claude`.
    #[tokio::test]
    async fn provision_ag_failure_is_a_warning() {
        let fake = fresh_host();
        fake.on(
            Match::script(&ag_install_script()),
            Reply::fail(
                5,
                "install.sh: /home/fake/.local/bin/ag exists and is not ours",
            ),
        );
        let warning = provision_ag(&fake, "h1").await.expect("a warning");
        assert!(
            warning.starts_with("ag launcher not installed"),
            "{warning}"
        );
        assert!(warning.contains("exit 5"), "{warning}");
    }

    /// Redesign 8.1: Pause all stops the stale-host refresh before it
    /// touches a host; it runs once the pause lifts.
    #[tokio::test]
    async fn pause_all_stops_the_stale_refresh() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        {
            let s = store.lock().unwrap();
            s.insert_host("h1", Some("h1")).unwrap();
            s.update_host_probe("h1", true, None, None, 1).unwrap();
            s.upsert_host_token("h1", TOKEN).unwrap();
            s.set_host_provisioned("h1", true).unwrap();
            s.conn_for_test()
                .execute(
                    "UPDATE hosts SET provision_fingerprint='old' WHERE alias='h1'",
                    [],
                )
                .unwrap();
            crate::service::settings::set(&s, crate::service::settings::AUTOMATION_PAUSED, "true")
                .unwrap();
        }
        let fake = fresh_host();
        assert_eq!(reprovision_stale_pass(&store, &fake, &base()).await, None);
        assert!(fake.calls().is_empty(), "no host touched while paused");
        assert!(
            store
                .lock()
                .unwrap()
                .get_host_row("h1")
                .unwrap()
                .unwrap()
                .provision_stale
        );
        crate::service::settings::set(
            &store.lock().unwrap(),
            crate::service::settings::AUTOMATION_PAUSED,
            "false",
        )
        .unwrap();
        assert_eq!(
            reprovision_stale_pass(&store, &fake, &base()).await,
            Some(vec!["h1".to_string()])
        );
    }

    /// hosts F1: the unattended refresh writes skills and hooks with the
    /// host's existing token, never `~/.claude.json`, and clears the stale
    /// mark.
    #[tokio::test]
    async fn provision_content_only_refreshes_content_without_touching_claude_json() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        {
            let s = store.lock().unwrap();
            s.insert_host("h1", Some("h1")).unwrap();
            s.update_host_probe("h1", true, None, None, 1).unwrap();
            s.upsert_host_token("h1", TOKEN).unwrap();
            s.set_host_provisioned("h1", true).unwrap();
            s.conn_for_test()
                .execute(
                    "UPDATE hosts SET provision_fingerprint='old' WHERE alias='h1'",
                    [],
                )
                .unwrap();
            assert!(s.get_host_row("h1").unwrap().unwrap().provision_stale);
        }
        let fake = fresh_host();
        assert_eq!(
            provision_content_only(&store, &fake, "h1", &base())
                .await
                .unwrap(),
            None
        );
        let steps: Vec<Step> = fake.calls().iter().map(step_of).collect();
        assert!(steps.contains(&Step::Script(remote_write_script(
            SKILL_DIR,
            SKILL_PATH,
            FLEET_SKILL
        ))));
        assert!(steps.contains(&Step::Script(remote_read_script(SETTINGS_JSON))));
        for step in voice_steps() {
            assert!(
                steps.contains(&step),
                "content-only refreshes voice: {step:?}"
            );
        }
        assert!(
            !steps.contains(&Step::Script(remote_read_script(CLAUDE_JSON))),
            "content-only never reads or rewrites ~/.claude.json"
        );
        assert!(!fake
            .calls()
            .iter()
            .any(|c| c.command().contains(".claude.json.fleet-tmp")));
        let row = store.lock().unwrap().get_host_row("h1").unwrap().unwrap();
        assert!(!row.provision_stale);

        // A host with no token needs a full provisioning first.
        store.lock().unwrap().insert_host("h2", Some("h2")).unwrap();
        let err = provision_content_only(&store, &fresh_host(), "h2", &base())
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::E_NO_TOKEN);
    }

    /// hosts F2: each managed skill dir says who owns it.
    #[tokio::test]
    async fn provision_one_writes_a_managed_marker_into_each_skill_dir() {
        let fake = fresh_host();
        provision_one(&fake, "h1", &base(), TOKEN).await.unwrap();
        let steps: Vec<Step> = fake.calls().iter().map(step_of).collect();
        for dir in [SKILL_DIR, FRIENDLY_NAME_SKILL_DIR] {
            let marker = format!("{dir}/{MANAGED_MARKER}");
            assert!(
                steps.contains(&Step::Script(remote_write_script(
                    dir,
                    &marker,
                    &marker_content()
                ))),
                "{marker} written"
            );
        }
    }

    #[test]
    fn a_login_banner_is_not_a_git_toplevel() {
        assert_eq!(git_top_of("Welcome\n"), "");
        assert_eq!(git_top_of("Welcome\nfleet-git-top=/home/u\n"), "/home/u");
        assert_eq!(git_top_of(""), "");
    }

    /// hosts F2, decision B-2: a dotfiles checkout that tracks fleet's skill
    /// dirs is refused unless `provision.force_git_tree`.
    #[tokio::test]
    async fn a_skills_dir_inside_a_git_work_tree_is_refused_unless_forced() {
        let fake = fresh_host();
        fake.on(
            Match::script(&git_tree_probe_script()),
            Reply::ok("Welcome to h1\nfleet-git-top=/home/fake/dotfiles\n"),
        );
        let err = provision_one(&fake, "h1", &base(), TOKEN)
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::E_INVALID);
        assert_eq!(
            err.details.as_ref().unwrap()["git_toplevel"],
            "/home/fake/dotfiles"
        );
        assert!(err.message.contains("provision.force_git_tree"));
        assert!(
            !fake.calls().iter().any(|c| c
                .script()
                .is_some_and(|s| s.contains("skills") && s.contains("printf"))),
            "nothing was written"
        );
        let forced = fresh_host();
        forced.on(
            Match::script(&git_tree_probe_script()),
            Reply::ok("Welcome to h1\nfleet-git-top=/home/fake/dotfiles\n"),
        );
        provision_one_with(&forced, "h1", &base(), TOKEN, true)
            .await
            .unwrap();
    }

    /// The probe itself, run by a real bash against a real repo: a skills dir
    /// inside a checkout passes while fleet's dirs are untracked or ignored,
    /// and is reported only once the checkout tracks a file in one of them.
    #[cfg(unix)]
    #[test]
    fn the_git_tree_probe_reports_only_a_checkout_that_tracks_fleets_skill_dirs() {
        let home = tempfile::tempdir().unwrap();
        let skills = home.path().join(".claude/skills");
        let probe = || {
            let out = crate::proc::std_command("bash")
                .args(["-c", &git_tree_probe_script()])
                .env("HOME", home.path())
                .env_remove("GIT_DIR")
                .env_remove("GIT_WORK_TREE")
                .env_remove("GIT_INDEX_FILE")
                .output()
                .unwrap();
            assert!(out.status.success(), "the probe always exits 0");
            git_top_of(&String::from_utf8(out.stdout).unwrap()).to_string()
        };
        let git = |args: &[&str]| {
            let st = crate::proc::std_command("git")
                .args(args)
                .current_dir(home.path())
                .env_remove("GIT_DIR")
                .env_remove("GIT_WORK_TREE")
                .env_remove("GIT_INDEX_FILE")
                .status()
                .unwrap();
            assert!(st.success(), "git {args:?}");
        };

        assert_eq!(probe(), "", "no skills dir at all");
        std::fs::create_dir_all(skills.join("claude-fleet-control")).unwrap();
        std::fs::write(skills.join("claude-fleet-control/SKILL.md"), "x").unwrap();
        std::fs::create_dir_all(skills.join("mine")).unwrap();
        std::fs::write(skills.join("mine/SKILL.md"), "x").unwrap();
        assert_eq!(probe(), "", "not a git work tree");

        git(&["init", "-q"]);
        git(&["add", ".claude/skills/mine/SKILL.md"]);
        assert_eq!(
            probe(),
            "",
            "a checkout tracking only the user's own skills"
        );

        std::fs::write(
            home.path().join(".gitignore"),
            ".claude/skills/claude-fleet-control/\n",
        )
        .unwrap();
        assert_eq!(probe(), "", "fleet's dir ignored");

        git(&["add", "-f", ".claude/skills/claude-fleet-control/SKILL.md"]);
        let top = std::fs::canonicalize(home.path()).unwrap();
        assert_eq!(
            std::fs::canonicalize(probe()).unwrap(),
            top,
            "a tracked file in fleet's dir names the toplevel"
        );
    }

    /// Voice relay F1: the `arecord` stand-in lands executable under
    /// `~/.claude-fleet/voice/bin`, and `voice.env` carries only the hub's
    /// URL (the bearer is the hook headers file's).
    #[tokio::test]
    async fn provision_writes_the_voice_stand_in_and_env() {
        let fake = fresh_host();
        provision_one_with(&fake, "h-a", &HubBase::loopback(4180), "tok", false)
            .await
            .unwrap();
        let steps: Vec<Step> = fake.calls().iter().map(step_of).collect();
        let base = HubBase::loopback(4180);
        let env = format!("FLEET_VOICE_URL={}\n", crate::shell::quote(&base.url));
        assert_eq!(env, "FLEET_VOICE_URL='http://127.0.0.1:4180'\n");
        let bin_write = Step::Script(remote_write_script(
            "~/.claude-fleet/voice/bin",
            "~/.claude-fleet/voice/bin/arecord",
            VOICE_ARECORD,
        ));
        let chmod = Step::Script("chmod 755 \"$HOME\"/'.claude-fleet/voice/bin/arecord'".into());
        let env_write = Step::Script(remote_write_script(
            "~/.claude-fleet/voice",
            "~/.claude-fleet/voice/voice.env",
            &env,
        ));
        let pos = |want: &Step| {
            steps
                .iter()
                .position(|s| s == want)
                .unwrap_or_else(|| panic!("missing {want:?}"))
        };
        assert!(pos(&bin_write) < pos(&chmod), "chmod after the write");
        pos(&env_write);
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
        // Skills are re-shipped (same bytes) with their `.fleet-managed`
        // markers (task 6) — the only unconditional writes. The git-tree
        // preflight reads the dir too but writes nothing.
        let skill_writes = steps
            .iter()
            .filter(
                |s| matches!(s, Step::Script(b) if b.contains(".claude/skills") && b.contains("printf")),
            )
            .count();
        assert_eq!(skill_writes, 4);
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
        // The git-tree preflight (task 6) reads first, then the first
        // write fails and nothing after it runs.
        assert_eq!(fake.calls().len(), 2, "stops at the first failed step");
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
        let results = provision_hosts(&store, &fake, &tunnels, &base(), ProvisionScope::default())
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
        // The fake spawner reports each ssh it is asked to start; the
        // supervisor spawns from a task, so the test waits on this signal
        // instead of guessing how long that task takes to run.
        let (spawned_tx, mut spawned) = tokio::sync::mpsc::unbounded_channel::<String>();
        let tunnels = Arc::new(TunnelSupervisor::with_spawner(
            Arc::new(move |argv: Vec<String>| {
                let _ = spawned_tx.send(argv.last().cloned().unwrap_or_default());
                Box::pin(std::future::pending())
            }),
            Duration::from_secs(3600),
        ));
        let public = HubBase::public("https://fleet.example.com", 4180).unwrap();
        reestablish_tunnels(&store, &tunnels, &public).unwrap();
        // `ensure` registers its task before returning, so an empty snapshot
        // means no supervised task exists — and only a task can spawn ssh.
        assert!(tunnels.snapshot().is_empty(), "no tunnel for a public hub");
        assert!(
            spawned.try_recv().is_err(),
            "a public hub must spawn no ssh at all"
        );
        reestablish_tunnels(&store, &tunnels, &HubBase::loopback(4180)).unwrap();
        assert_eq!(
            tunnels.snapshot().get("mefistos"),
            Some(&true),
            "loopback hub tunnels provisioned hosts"
        );
        assert_eq!(tunnels.snapshot().len(), 1);
        let first = tokio::time::timeout(Duration::from_secs(5), spawned.recv())
            .await
            .expect("the loopback pass spawns ssh within 5s")
            .expect("spawner channel open");
        assert!(first.contains("mefistos"), "{first:?}");
        // And exactly one ssh was spawned: the one task's fake ssh never
        // exits, so it never restarts, and the public pass (run first, on
        // this single-threaded runtime) would have been received ahead of it.
        assert!(spawned.try_recv().is_err(), "exactly one ssh, for mefistos");
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
        // No wait needed: `ensure` registers the host's task before returning,
        // and the snapshot reads only that registry.
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

    /// A WSL distribution is not dialed over SSH, so neither provisioning
    /// nor the app-start pass starts an `ssh -R` for it; on a loopback hub
    /// provisioning reports instead whether its hooks can reach this desktop.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_wsl_host_gets_no_reverse_tunnel() {
        let _table = crate::wsl::TEST_TABLE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        crate::wsl::set_for_tests(vec![("wsl-x".into(), "X".into())]);
        let store = Mutex::new(Store::open_in_memory().unwrap());
        {
            let s = store.lock().unwrap();
            s.upsert_host("wsl-x").unwrap();
            s.set_host_provisioned("wsl-x", true).unwrap();
            s.upsert_host("mefistos").unwrap();
            s.set_host_provisioned("mefistos", true).unwrap();
        }

        let tunnels = quiet_tunnels();
        reestablish_tunnels(&store, &tunnels, &HubBase::loopback(4180)).unwrap();
        assert!(
            !tunnels.snapshot().contains_key("wsl-x"),
            "a WSL host has no ssh -R: {:?}",
            tunnels.snapshot()
        );
        assert_eq!(
            tunnels.snapshot().get("mefistos"),
            Some(&true),
            "an ssh host is still tunneled"
        );
        tunnels.stop_all();

        let tunnels = quiet_tunnels();
        let fake = fresh_host();
        // Nothing answers the healthz probe from inside the distribution
        // (WSL2 NAT): provisioned, with the warning.
        fake.on_host("wsl-x", Match::contains("healthz"), Reply::fail(7, ""));
        let warning = provision_host_with_token(&store, &fake, &tunnels, "wsl-x", &base(), false)
            .await
            .unwrap();
        assert_eq!(warning.as_deref(), Some(WSL_HOOKS_UNREACHABLE));
        assert!(
            tunnels.snapshot().is_empty(),
            "provisioning a WSL host starts no tunnel: {:?}",
            tunnels.snapshot()
        );
        assert!(store
            .lock()
            .unwrap()
            .get_host_token("wsl-x")
            .unwrap()
            .is_some());
        crate::wsl::set_for_tests(Vec::new());
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

    /// F2: `provision.install_ag` defaults on, so an ordinary
    /// `provision_host_with_token` call also stages and installs the ag
    /// launcher, with no warning when it succeeds.
    #[tokio::test]
    async fn provision_host_with_token_installs_ag_by_default() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        store.lock().unwrap().insert_host("h", Some("h")).unwrap();
        let tunnels = quiet_tunnels();
        let fake = fresh_host();
        let warning = provision_host_with_token(&store, &fake, &tunnels, "h", &base(), false)
            .await
            .unwrap();
        assert_eq!(warning, None);
        let script = ag_install_script();
        assert!(fake
            .calls()
            .iter()
            .any(|c| c.script().as_deref() == Some(script.as_str())));
    }

    /// A fake whose ag installer exits 5, like a host with a foreign
    /// `~/.local/bin/ag`.
    fn host_whose_ag_install_fails() -> FakeSsh {
        let fake = fresh_host();
        fake.on(
            Match::script(&ag_install_script()),
            Reply::fail(5, "install.sh: /home/fake/.local/bin/ag exists"),
        );
        fake
    }

    /// M11a: `provision.install_ag=false` leaves ag alone — nothing staged,
    /// nothing installed, nothing under `~/.local` touched.
    #[tokio::test]
    async fn provision_host_with_token_skips_ag_when_install_ag_is_off() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        {
            let s = store.lock().unwrap();
            s.insert_host("h", Some("h")).unwrap();
            s.set_setting(crate::service::settings::PROVISION_INSTALL_AG, "false")
                .unwrap();
        }
        let tunnels = quiet_tunnels();
        let fake = fresh_host();
        let warning = provision_host_with_token(&store, &fake, &tunnels, "h", &base(), false)
            .await
            .unwrap();
        assert_eq!(warning, None);
        let touched: Vec<String> = fake
            .calls()
            .iter()
            .map(Call::command)
            .filter(|c| c.contains(".local") || c.contains("ag-src"))
            .collect();
        assert!(touched.is_empty(), "no ag calls expected: {touched:?}");
        tunnels.stop_all();
    }

    /// M11b: a content-only refresh whose ag install fails is still a
    /// refresh — the warning comes back and the host is provisioned — but it
    /// keeps NO fingerprint, so it reads `provision_stale` and is retried.
    /// Stamping this build's fingerprint here would record a degraded run as
    /// delivered: permanent, invisible, and a no-op to re-enable.
    #[tokio::test]
    async fn provision_content_only_returns_the_ag_warning_and_still_marks_provisioned() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        {
            let s = store.lock().unwrap();
            s.insert_host("h1", Some("h1")).unwrap();
            s.update_host_probe("h1", true, None, None, 1).unwrap();
            s.upsert_host_token("h1", TOKEN).unwrap();
            s.set_host_provisioned("h1", true).unwrap();
            s.conn_for_test()
                .execute(
                    "UPDATE hosts SET provision_fingerprint='old' WHERE alias='h1'",
                    [],
                )
                .unwrap();
        }
        let fake = host_whose_ag_install_fails();
        let warning = provision_content_only(&store, &fake, "h1", &base())
            .await
            .unwrap()
            .expect("an ag warning");
        assert!(
            warning.starts_with("ag launcher not installed"),
            "{warning}"
        );
        let row = store.lock().unwrap().get_host_row("h1").unwrap().unwrap();
        assert!(row.provisioned);
        assert!(
            row.provision_stale,
            "a degraded ag step keeps no fingerprint, so the host is owed a retry"
        );
        assert!(
            row.provisioned_at.is_some(),
            "the content WAS delivered: provisioned_at still stands"
        );
        // and the REASON survives the call that produced it, so it can reach
        // fleet_health and the host's Attention row
        assert_eq!(
            row.provision_warning.as_deref(),
            Some(warning.as_str()),
            "the warning is kept on the host, not just returned"
        );
        let health = crate::service::health::hosts_health(
            std::slice::from_ref(&row),
            &[],
            &crate::service::health::HostHealthThresholds {
                disk_low_pct: 90,
                claude_max_behind: 1,
                hooks_silent_secs: 3600,
            },
            &[],
            "0.0.0",
            crate::store::now_unix(),
        );
        assert_eq!(
            health[0].provision_warning.as_deref(),
            Some(warning.as_str()),
            "fleet_health carries it"
        );

        // A clean re-run clears it: the warning is about the LAST run.
        let ok = fresh_host();
        provision_content_only(&store, &ok, "h1", &base())
            .await
            .unwrap();
        let row = store.lock().unwrap().get_host_row("h1").unwrap().unwrap();
        assert_eq!(
            row.provision_warning, None,
            "a clean run clears the warning"
        );
        assert!(!row.provision_stale, "and re-stamps the fingerprint");
    }

    /// M11d: a full provisioning whose installer exits non-zero is
    /// provisioned with the ag warning.
    #[tokio::test]
    async fn provision_host_with_token_reports_a_failed_ag_install_as_a_warning() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        store.lock().unwrap().insert_host("h", Some("h")).unwrap();
        let tunnels = quiet_tunnels();
        let fake = host_whose_ag_install_fails();
        let warning = provision_host_with_token(&store, &fake, &tunnels, "h", &base(), false)
            .await
            .unwrap()
            .expect("an ag warning");
        assert!(
            warning.starts_with("ag launcher not installed"),
            "{warning}"
        );
        assert!(
            store
                .lock()
                .unwrap()
                .list_hosts()
                .unwrap()
                .iter()
                .find(|h| h.alias == "h")
                .unwrap()
                .provisioned
        );
        tunnels.stop_all();
    }

    /// M11c: a WSL host whose hooks cannot reach the desktop AND whose ag
    /// install fails reports both, joined with `; `.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn wsl_and_ag_warnings_are_joined() {
        let _table = crate::wsl::TEST_TABLE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        crate::wsl::set_for_tests(vec![("wsl-y".into(), "Y".into())]);
        let store = Mutex::new(Store::open_in_memory().unwrap());
        store.lock().unwrap().upsert_host("wsl-y").unwrap();
        let tunnels = quiet_tunnels();
        let fake = host_whose_ag_install_fails();
        fake.on_host("wsl-y", Match::contains("healthz"), Reply::fail(7, ""));
        let warning = provision_host_with_token(&store, &fake, &tunnels, "wsl-y", &base(), false)
            .await
            .unwrap()
            .expect("both warnings");
        crate::wsl::set_for_tests(Vec::new());
        assert!(
            warning.starts_with(&format!(
                "{WSL_HOOKS_UNREACHABLE}; ag launcher not installed"
            )),
            "{warning}"
        );
        tunnels.stop_all();
    }

    /// M8 / I4: a warning is appended to the usual success detail — the
    /// restart hint of a full run is never lost — and a content-only run
    /// surfaces its ag warning too.
    #[tokio::test]
    async fn provision_hosts_appends_the_warning_to_the_success_detail() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        {
            let s = store.lock().unwrap();
            s.insert_host("h", Some("h")).unwrap();
            s.update_host_probe("h", true, None, None, 1).unwrap();
        }
        let tunnels = quiet_tunnels();
        let fake = host_whose_ag_install_fails();
        let detail = |results: Vec<HostProvisionResult>| {
            let r = results.into_iter().find(|r| r.host == "h").unwrap();
            assert_eq!(r.status, "provisioned");
            r.detail.unwrap()
        };

        let full = detail(
            provision_hosts(&store, &fake, &tunnels, &base(), ProvisionScope::default())
                .await
                .unwrap(),
        );
        assert!(
            full.starts_with(
                "restart Claude on this host to load the MCP server; ag launcher not installed"
            ),
            "{full}"
        );

        let content = detail(
            provision_hosts(
                &store,
                &fake,
                &tunnels,
                &base(),
                ProvisionScope {
                    content_only: true,
                    ..ProvisionScope::default()
                },
            )
            .await
            .unwrap(),
        );
        assert!(
            content.starts_with(
                "skills, CLAUDE.md block and hooks refreshed (no restart needed); \
                 ag launcher not installed"
            ),
            "{content}"
        );

        // Clean runs keep the plain success text.
        let clean = fresh_host();
        let plain = detail(
            provision_hosts(&store, &clean, &tunnels, &base(), ProvisionScope::default())
                .await
                .unwrap(),
        );
        assert_eq!(plain, "restart Claude on this host to load the MCP server");
        tunnels.stop_all();
    }
}
