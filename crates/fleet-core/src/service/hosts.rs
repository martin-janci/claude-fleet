//! Service layer for SSH host management — transport-agnostic logic over
//! `store/` helpers plus `ssh_config.rs` (discovery) and any `ssh::SshExec`
//! (probing). Called by both the Tauri command wrappers and the MCP server.

use crate::cancel::CancellationRegistry;
use crate::ipc_error::lock;
use crate::ipc_error::{codes, IpcError};
use crate::shell::quote;
use crate::ssh::SshExec;
use crate::ssh_config::{self, SshHost};
use crate::store::{HostRow, Store};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub fn discover_hosts() -> Result<Vec<SshHost>, IpcError> {
    Ok(ssh_config::load_user_config())
}

pub fn list_hosts(store: &Mutex<Store>) -> Result<Vec<HostRow>, IpcError> {
    let s = lock(store)?;
    s.list_hosts().map_err(IpcError::from)
}

/// One agent host, as `agent_status` reports it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AgentHostStatus {
    pub alias: String,
    /// A `fleet-agent` is connected for this host right now.
    pub connected: bool,
    /// Unix seconds the live connection was registered; `None` when offline.
    pub connected_at: Option<i64>,
    pub agent_version: Option<String>,
    pub host_name: Option<String>,
    pub os: Option<String>,
}

/// What `agent_status` answers.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AgentStatusReport {
    /// This server accepts agent connections (a hub). `false` on the desktop,
    /// which reaches every host over SSH.
    pub enabled: bool,
    /// Every host on the agent transport, connected or not, by alias.
    pub hosts: Vec<AgentHostStatus>,
}

/// Which agent hosts have a `fleet-agent` connected, since when, and which
/// version — every host whose transport is `agent`, so an offline one is
/// listed too.
pub fn agent_status(
    store: &Mutex<Store>,
    registry: Option<&crate::agent::AgentRegistry>,
) -> Result<AgentStatusReport, IpcError> {
    let agent_hosts: Vec<String> = {
        let s = lock(store)?;
        s.list_hosts()?
            .into_iter()
            .filter(|h| h.transport == "agent")
            .map(|h| h.alias)
            .collect()
    };
    let live = registry.map(|r| r.snapshot()).unwrap_or_default();
    let mut hosts: Vec<AgentHostStatus> = agent_hosts
        .into_iter()
        .map(|alias| match live.iter().find(|a| a.alias == alias) {
            Some(a) => AgentHostStatus {
                alias,
                connected: true,
                connected_at: Some(a.connected_at),
                agent_version: Some(a.agent_version.clone()),
                host_name: Some(a.host_name.clone()),
                os: Some(a.os.clone()),
            },
            None => AgentHostStatus {
                alias,
                connected: false,
                connected_at: None,
                agent_version: None,
                host_name: None,
                os: None,
            },
        })
        .collect();
    hosts.sort_by(|a, b| a.alias.cmp(&b.alias));
    Ok(AgentStatusReport {
        enabled: registry.is_some(),
        hosts,
    })
}

pub fn list_accounts(store: &Mutex<Store>) -> Result<Vec<crate::store::AccountRow>, IpcError> {
    let s = lock(store)?;
    s.list_accounts().map_err(IpcError::from)
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars", rename = "AddHostParams")]
pub struct AddHostArgs {
    /// claude-fleet alias to register the host under (must be a safe
    /// identifier — letters, digits, dashes).
    pub alias: String,
    /// SSH config alias used to reach the host (from `~/.ssh/config`).
    pub ssh_alias: String,
    /// `"ssh"` (the default) or `"agent"` — how the host is reached.
    /// Anything else is rejected before the host is persisted. An `"agent"`
    /// host is added without an SSH probe: it is reachable only through a
    /// `fleet-agent` that has yet to dial in.
    #[serde(default)]
    pub transport: Option<String>,
}

pub async fn add_host(
    args: AddHostArgs,
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
) -> Result<HostRow, IpcError> {
    // Reject hostile aliases (e.g. `-oProxyCommand=…`) before they reach ssh.
    crate::validate::host_alias(&args.alias)?;
    // Only ever an argument to `ssh`, never the `local` host itself.
    crate::validate::host_alias_syntax(&args.ssh_alias)?;
    // Validate the transport before the probe or any write: `insert_host`
    // autocommits and fires `host_added` immediately (no transaction wraps
    // this whole call), so validating only once we reach
    // `Store::set_host_transport` left a ghost, unreachable-looking row
    // behind — and an emitted event — on a rejected value.
    if let Some(t) = args.transport.as_deref() {
        if !crate::store::HOST_TRANSPORTS.contains(&t) {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("unknown transport {t:?}: must be \"ssh\" or \"agent\""),
            ));
        }
    }
    // A host some tracker runs curl or gh on cannot become an agent host,
    // which can run neither: the trackers' rule, asked from this side too,
    // before the probe and any write.
    if args.transport.as_deref() == Some("agent") {
        let s = lock(store)?;
        crate::service::trackers::admin::refuse_agent_transport_on_tracker_host(&s, &args.alias)?;
    }
    // An agent host is, by definition, one the hub cannot dial — that is the
    // whole reason it needs an agent — so an SSH probe must not be the price
    // of admission. Nor can the agent dial in first: the per-host token it
    // authenticates with is minted against the row this call creates. So the
    // row is persisted unprobed and `reachable=false`, and reachability
    // arrives from the agent registry — through the router, which needs the
    // row to exist — on the first `probe_host` or reconcile pass after the
    // agent connects.
    let (reachable, claude_ver, tmux_ver, account) = if args.transport.as_deref() == Some("agent") {
        (false, None, None, None)
    } else {
        // Probe first; we don't want to persist a host we can't talk to.
        probe(ssh, &args.ssh_alias).await?
    };
    {
        let s = lock(store)?;
        s.insert_host(&args.alias, Some(&args.ssh_alias))?;
        // `insert_host` is an upsert, so this branch also runs on a re-add
        // of an existing alias. Only write the transport when the caller
        // named one: `None` means "unspecified", not "reset to ssh" — a
        // fresh row already gets 'ssh' from the column default, but an
        // existing row (e.g. already "agent") must not be silently
        // downgraded by a re-add that didn't mention transport at all.
        if let Some(t) = args.transport.as_deref() {
            s.set_host_transport(&args.alias, t)?;
        }
        // Link account if probe found one
        if let Some(acc) = account
            .as_ref()
            .and_then(|a| account_row_from(a, now_unix()))
        {
            s.upsert_account(&acc)?;
            s.set_host_account(&args.alias, Some(&acc.uuid))?;
        } else {
            s.set_host_account(&args.alias, None)?;
        }
        s.update_host_probe(
            &args.alias,
            reachable,
            claude_ver.as_deref(),
            tmux_ver.as_deref(),
            now_unix(),
        )?;
    }
    list_one(store, &args.alias)
}

/// Preview-only probe used by AddHostPicker before the user confirms `Add`.
/// Does NOT persist anything; just runs the strict probe and returns versions
/// + the detected account so the picker can show it for confirmation.
#[derive(serde::Serialize, Debug)]
pub struct ProbePreview {
    pub reachable: bool,
    pub claude_version: Option<String>,
    pub tmux_version: Option<String>,
    pub account: Option<OauthAccount>,
}

#[derive(Deserialize)]
pub struct ProbeSshAliasArgs {
    pub ssh_alias: String,
    pub call_id: Option<u64>,
}

pub async fn probe_ssh_alias(
    args: ProbeSshAliasArgs,
    ssh: &dyn SshExec,
    reg: &Arc<CancellationRegistry>,
) -> Result<ProbePreview, IpcError> {
    crate::validate::host_alias_syntax(&args.ssh_alias)?;
    let (cancel_id, token) = match args.call_id {
        Some(id) => {
            let token = CancellationToken::new();
            reg.bind(id, token.clone());
            (id, token)
        }
        None => reg.register_anonymous(),
    };
    // RAII guard releases the registry slot on every exit path, including a
    // panic — a manual unregister would leak the slot on unwind.
    let _guard = crate::cancel::CancelGuard::new(Arc::clone(reg), cancel_id);

    let result = probe_with_token(ssh, &args.ssh_alias, token).await;

    let (reachable, claude_version, tmux_version, account) = result?;
    Ok(ProbePreview {
        reachable,
        claude_version,
        tmux_version,
        account,
    })
}

#[derive(Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars", rename = "HostAliasParams")]
pub struct HostAliasArgs {
    /// The claude-fleet host alias (e.g. "local", "mefistos").
    pub alias: String,
}

pub async fn probe_host(
    args: HostAliasArgs,
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    reg: &Arc<CancellationRegistry>,
) -> Result<HostRow, IpcError> {
    let ssh_alias = {
        let s = lock(store)?;
        s.list_hosts()?
            .into_iter()
            .find(|h| h.alias == args.alias)
            .and_then(|h| h.ssh_alias)
    };
    let target = ssh_alias.as_deref().unwrap_or(&args.alias);
    // The `local` host has no ssh_alias; probe is best-effort via local shell.
    // For remote hosts we use the lenient probe so a Re-probe of an
    // unreachable host updates `reachable=false` instead of returning an
    // error to the UI.
    let (reachable, claude_ver, tmux_ver, account) = if args.alias == "local" {
        crate::service::hub::ensure_local_allowed(&args.alias)?;
        // probe_local does blocking std::process + fs I/O — keep it off the
        // async runtime worker thread.
        tokio::task::spawn_blocking(probe_local)
            .await
            .unwrap_or_default()
    } else {
        // `target` is the ssh alias (or a non-local alias): syntax only.
        crate::validate::host_alias_syntax(target)?;
        // Anonymous token — probe_host is user-triggered re-probe; we give it
        // a token so it can be cancelled if needed, but no frontend call_id.
        // The CancelGuard releases the slot even if the probe panics.
        let (id, token) = reg.register_anonymous();
        let _guard = crate::cancel::CancelGuard::new(Arc::clone(reg), id);
        probe_lenient_with_token(ssh, target, token).await
    };
    {
        let s = lock(store)?;
        if let Some(acc) = account
            .as_ref()
            .and_then(|a| account_row_from(a, now_unix()))
        {
            s.upsert_account(&acc)?;
            s.set_host_account(&args.alias, Some(&acc.uuid))?;
        } else {
            s.set_host_account(&args.alias, None)?;
        }
        s.update_host_probe(
            &args.alias,
            reachable,
            claude_ver.as_deref(),
            tmux_ver.as_deref(),
            now_unix(),
        )?;
    }
    list_one(store, &args.alias)
}

pub fn remove_host(args: HostAliasArgs, store: &Mutex<Store>) -> Result<HostRow, IpcError> {
    let row = list_one(store, &args.alias)?;
    let s = lock(store)?;
    s.delete_host(&args.alias)?;
    Ok(row)
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars", rename = "HideHostParams")]
pub struct HideHostArgs {
    /// The claude-fleet host alias.
    pub alias: String,
    /// `true` to hide the host (skipped during reconcile), `false` to show it.
    pub hidden: bool,
}

pub fn hide_host(args: HideHostArgs, store: &Mutex<Store>) -> Result<HostRow, IpcError> {
    {
        let s = lock(store)?;
        s.set_host_hidden(&args.alias, args.hidden)?;
    }
    list_one(store, &args.alias)
}

#[derive(Deserialize)]
pub struct SetAccountNicknameArgs {
    pub uuid: String,
    pub nickname: Option<String>,
}

pub fn set_account_nickname(
    args: SetAccountNicknameArgs,
    store: &Mutex<Store>,
) -> Result<crate::store::AccountRow, IpcError> {
    let s = lock(store)?;
    s.set_account_nickname(&args.uuid, args.nickname.as_deref())
}

// --- helpers ---

fn list_one(store: &Mutex<Store>, alias: &str) -> Result<HostRow, IpcError> {
    let s = lock(store)?;
    s.list_hosts()?
        .into_iter()
        .find(|h| h.alias == alias)
        .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("host {alias} not found")))
}

/// Strict probe — returns Err(E_PROBE) if the SSH round trip fails. Used by
/// add_host. Reads tmux + claude versions AND the oauthAccount in a single
/// round trip (sections separated by literal `---`).
async fn probe(
    ssh: &dyn SshExec,
    host: &str,
) -> Result<(bool, Option<String>, Option<String>, Option<OauthAccount>), IpcError> {
    let token = CancellationToken::new();
    probe_with_token(ssh, host, token).await
}

/// The `oauthAccount` section of the probe on its own: prints
/// `~/.claude.json`'s `oauthAccount` as one line of compact JSON (via `jq`,
/// falling back to `python3`), or nothing when the file/tools are missing.
/// Also run by itself every reconcile pass for each remote host
/// (`RemoteTmux::read_oauth_account`) so an account switch on a remote host
/// is picked up without a manual Re-probe. A macro rather than a `const` so
/// `PROBE_SCRIPT` can `concat!` it. MUST be `quote`'d before it is handed to
/// `bash -lc` over ssh.
macro_rules! oauth_account_script {
    () => {
        r#"( cat "$HOME/.claude.json" 2>/dev/null | jq -c .oauthAccount 2>/dev/null \
  || python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); print(json.dumps(d.get("oauthAccount") or {}))' "$HOME/.claude.json" 2>/dev/null \
  || true )"#
    };
}
pub(crate) const OAUTH_ACCOUNT_SCRIPT: &str = oauth_account_script!();

/// The remote probe script: reads tmux + claude versions AND the
/// `oauthAccount` from `~/.claude.json` in one SSH round trip, with the three
/// sections separated by a literal `---`. Each section is independently
/// guarded (`|| true`) so a missing tool/file degrades to an empty section
/// rather than failing the whole probe. MUST be `quote`'d before it's handed to
/// `bash -lc` over ssh (see `probe_with_token`).
const PROBE_SCRIPT: &str = concat!(
    "tmux -V 2>/dev/null || true\necho ---\nclaude --version 2>/dev/null || true\necho ---\n",
    oauth_account_script!()
);

/// Like `probe` but uses the provided `CancellationToken` so the caller can
/// cancel the SSH round trip.
async fn probe_with_token(
    ssh: &dyn SshExec,
    host: &str,
    token: CancellationToken,
) -> Result<(bool, Option<String>, Option<String>, Option<OauthAccount>), IpcError> {
    // Single-quote the WHOLE script so it crosses the ssh argv-join as one
    // word. ssh concatenates the trailing args with spaces and the remote
    // LOGIN shell (often zsh) re-tokenizes them — without quoting, this
    // multi-line `||`/`(...)` probe splits at the login-shell level: the first
    // line collapses to `bash -lc tmux` (with `-V` swallowed as $0) and the
    // remaining lines run in the login shell, not under `bash -lc`, so the
    // probe came back degraded/partial (the tmux-version section came back
    // empty; depending on the remote login shell other sections can drop too).
    // `quote` also escapes the inner single-quotes of the embedded python3
    // one-liner. Same fix as `ensure_remote_project` in commands/sessions.rs.
    let quoted = quote(PROBE_SCRIPT);
    let out = ssh
        .run_cancellable(
            host,
            &["bash", "-lc", &quoted],
            Duration::from_secs(5),
            token,
        )
        .await
        .map_err(|e| IpcError::new(codes::E_PROBE, format!("ssh {host}: {}", e.message)))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let failure = crate::ssh_diag::classify(host, out.status.code(), &stderr);
        let summary = failure
            .as_ref()
            .map(|f| format!("{}; ", f.kind.summary()))
            .unwrap_or_default();
        let err = IpcError::new(
            codes::E_PROBE,
            format!(
                "ssh {host}: {summary}exited {:?}: {}",
                out.status.code(),
                stderr.trim()
            ),
        );
        return Err(match failure {
            Some(f) => err.with_details(serde_json::json!({ "ssh_failure": f })),
            None => err,
        });
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut parts = stdout.split("---");
    let tmux_line = parts.next().unwrap_or("").trim().to_string();
    let claude_line = parts.next().unwrap_or("").trim().to_string();
    let oauth_line = parts.next().unwrap_or("").trim().to_string();
    Ok((
        true,
        parse_claude_version(&claude_line),
        parse_tmux_version(&tmux_line),
        parse_oauth_account(&oauth_line),
    ))
}

/// Lenient probe with an explicit cancellation token. Used by `probe_host`.
/// An SSH failure collapses to "unreachable, nothing known" rather than an
/// error, so a Re-probe of a down host updates `reachable=false` in the UI.
async fn probe_lenient_with_token(
    ssh: &dyn SshExec,
    host: &str,
    token: CancellationToken,
) -> (bool, Option<String>, Option<String>, Option<OauthAccount>) {
    probe_with_token(ssh, host, token).await.unwrap_or_default()
}

/// The real local home directory (`$HOME`), used by production callers of
/// [`probe_local`]/[`sync_local_account`]. Falls back to the system temp dir
/// (almost certainly without a `.claude.json`) on the exotic case where
/// `HOME` is unset, so the account simply comes back `None` instead of
/// accidentally reading a relative `.claude.json` from the current dir.
pub(crate) fn local_home_dir() -> std::path::PathBuf {
    std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

/// Thin wrapper around [`probe_local_in`] that resolves the real `$HOME`.
/// Kept zero-arg so `tokio::task::spawn_blocking(probe_local)` (blocking
/// std::process + fs I/O, off the async runtime worker thread) needs no
/// closure allocation at its one production call site.
fn probe_local() -> (bool, Option<String>, Option<String>, Option<OauthAccount>) {
    probe_local_in(&local_home_dir())
}

/// Probe the local machine's tmux/claude versions and `oauthAccount`, reading
/// `<home>/.claude.json` for the account instead of `$HOME` directly — so
/// tests can drive this with a temp directory and never touch the real
/// `~/.claude.json`.
pub(crate) fn probe_local_in(
    home: &std::path::Path,
) -> (bool, Option<String>, Option<String>, Option<OauthAccount>) {
    let tmux = std::process::Command::new("tmux")
        .arg("-V")
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        });
    let claude = std::process::Command::new("claude")
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        });
    // Read local ~/.claude.json directly — no subprocess needed.
    let account = match probe_local_account_in(home) {
        LocalAccountProbe::LoggedIn(a) => Some(a),
        LocalAccountProbe::LoggedOut | LocalAccountProbe::Unavailable => None,
    };
    (
        true,
        parse_claude_version(claude.as_deref().unwrap_or("")),
        parse_tmux_version(tmux.as_deref().unwrap_or("")),
        account,
    )
}

/// Outcome of reading `<home>/.claude.json`'s `oauthAccount`, distinguishing
/// a real state (logged in / explicitly logged out) from a failed read. See
/// `sync_local_account`, the only caller that cares about the distinction —
/// `probe_local_in` collapses `LoggedOut` and `Unavailable` to `None` since
/// its caller (a manual "Re-probe") already treats "no account" uniformly.
enum LocalAccountProbe {
    /// The file parsed and has an `oauthAccount` with a uuid.
    LoggedIn(OauthAccount),
    /// The file parsed but has no `oauthAccount` (or one without a uuid) —
    /// an explicit logout.
    LoggedOut,
    /// The file is missing, unreadable, or not valid JSON — a failed read,
    /// not evidence of anything about the account.
    Unavailable,
}

fn probe_local_account_in(home: &std::path::Path) -> LocalAccountProbe {
    let Ok(contents) = std::fs::read_to_string(home.join(".claude.json")) else {
        return LocalAccountProbe::Unavailable;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&contents) else {
        return LocalAccountProbe::Unavailable;
    };
    match v.get("oauthAccount") {
        None => LocalAccountProbe::LoggedOut,
        Some(oa) => match serde_json::from_value::<OauthAccount>(oa.clone()) {
            Ok(a) if a.uuid.is_some() => LocalAccountProbe::LoggedIn(a),
            // Present but malformed/uuid-less: same as absent — nothing to
            // link to.
            _ => LocalAccountProbe::LoggedOut,
        },
    }
}

/// Keep `local`'s linked Claude account in sync with `<home>/.claude.json`,
/// distinguishing a real account change from a failed read so a transient
/// glitch never clears or changes an established link:
///
///   - readable, parses, `oauthAccount.accountUuid` DIFFERS from the stored
///     `account_uuid` (including "nothing stored yet") ⇒ the user switched
///     (or first linked) accounts: upsert the account row and relink.
///   - readable, parses, SAME uuid ⇒ no-op on the link, but the account row
///     is still upserted so `email`/`display_name`/`organization_name`/
///     `seat_tier` stay current.
///   - unreadable / unparseable / no `oauthAccount` / no uuid ⇒ leave the
///     existing link untouched. This deliberately treats an EXPLICIT logout
///     (file parses fine, `oauthAccount` just isn't there) the same as a
///     failed read: we can't tell "the user logged out" apart from "a
///     partial write of ~/.claude.json (claude was mid-rewrite when we
///     read it)" from this file alone, and flapping the link off on every
///     partial-write race would be worse than briefly showing usage under
///     an account the user has since left. See PR discussion: showing
///     stale usage beats flapping.
///
/// This is `local`'s ONLY automatic account-discovery path. A remote host
/// gets its account captured when the user runs `add_host` (see `add_host`
/// above) and then re-read every reconcile pass over ssh
/// (`TmuxExec::read_oauth_account` → [`sync_host_account`]) — but `local`
/// never goes through either flow: it is auto-created by
/// `reconcile_sessions_with`'s `Store::upsert_host("local")` (see
/// `service::sessions::reconcile`), which only ever touches `reachable`.
/// Before this function existed, NOTHING ever probed `local`'s account
/// automatically — the ONLY way to populate (or update) it was the user
/// manually clicking the small "Re-probe" icon on the `local` row in
/// Settings ⇒ Hosts (`probe_host` below, wired to `HostsTable.svelte`'s
/// `onProbe`). Called once per reconcile pass (see `ReconcileDeps::local_home`),
/// so a login, logout-then-login-elsewhere, or account switch is picked up
/// within one tick without any user action.
pub(crate) async fn sync_local_account(
    store: &Mutex<Store>,
    home: std::path::PathBuf,
) -> Result<(), IpcError> {
    // Off the async worker thread: probe_local_account_in does a blocking fs
    // read.
    let probe = tokio::task::spawn_blocking(move || probe_local_account_in(&home))
        .await
        .unwrap_or(LocalAccountProbe::Unavailable);
    let account = match probe {
        LocalAccountProbe::LoggedIn(a) => Some(a),
        // Failed read or explicit logout: never touch an existing link (see
        // the doc comment above).
        LocalAccountProbe::LoggedOut | LocalAccountProbe::Unavailable => None,
    };
    let s = lock(store)?;
    sync_host_account(&s, "local", account.as_ref()).map(|_| ())
}

/// Host-agnostic core of [`sync_local_account`], shared with the reconcile
/// pass for REMOTE hosts (`service::sessions::reconcile::reconcile_write_one_host`,
/// fed by `TmuxExec::read_oauth_account`). Same rules as documented there:
///
///   - `Some(account)` with a uuid that DIFFERS from the host's stored
///     `account_uuid` ⇒ upsert the account row and relink the host.
///   - `Some(account)` with the SAME uuid ⇒ the link is untouched, the
///     account row's fields are refreshed.
///   - `None` (unreadable, logged out, or the executor cannot tell) ⇒ the
///     existing link is left alone. Never clears.
///
/// Returns the host's account uuid AFTER the sync (the fresh one when it
/// relinked, otherwise whatever was stored), so a caller that snapshotted
/// the host row before the probe can attribute newly-discovered sessions to
/// the account the host is logged into NOW rather than the one it left.
/// Takes the store guard the caller already holds — never `.await`s.
pub(crate) fn sync_host_account(
    s: &Store,
    alias: &str,
    account: Option<&OauthAccount>,
) -> Result<Option<String>, IpcError> {
    let stored_uuid = s
        .list_hosts()?
        .into_iter()
        .find(|h| h.alias == alias)
        .and_then(|h| h.account_uuid);
    let Some(row) = account.and_then(|a| account_row_from(a, now_unix())) else {
        return Ok(stored_uuid);
    };
    // Refresh the account row's fields (email/org/seat_tier) whether or not
    // the link itself is changing.
    s.upsert_account(&row)?;
    if stored_uuid.as_deref() != Some(row.uuid.as_str()) {
        s.set_host_account(alias, Some(&row.uuid))?;
    }
    Ok(Some(row.uuid))
}

fn parse_tmux_version(line: &str) -> Option<String> {
    // `tmux 3.6a` → "3.6a"
    line.strip_prefix("tmux ")
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn parse_claude_version(line: &str) -> Option<String> {
    // `2.1.144 (Claude Code)` → "2.1.144"
    line.split_whitespace()
        .next()
        .map(|v| v.to_string())
        .filter(|v| !v.is_empty())
}

/// Subset of `~/.claude.json`'s `oauthAccount` we care about. All fields
/// optional so a partial JSON shape (e.g., older claude versions, missing
/// org fields) still parses cleanly.
#[derive(serde::Deserialize, serde::Serialize, Default, Debug, Clone)]
pub struct OauthAccount {
    #[serde(rename = "accountUuid")]
    pub uuid: Option<String>,
    #[serde(rename = "emailAddress")]
    pub email: Option<String>,
    #[serde(rename = "displayName")]
    pub display_name: Option<String>,
    #[serde(rename = "organizationName")]
    pub organization_name: Option<String>,
    #[serde(rename = "organizationUuid")]
    pub organization_uuid: Option<String>,
    #[serde(rename = "seatTier")]
    pub seat_tier: Option<String>,
    /// Whether hitting a usage limit spends pay-as-you-go money instead of
    /// blocking the account (migration 028).
    #[serde(rename = "hasExtraUsageEnabled")]
    pub has_extra_usage_enabled: Option<bool>,
}

/// Parse the third probe section. Empty / "null" / "{}" → None.
/// Treats account-without-uuid as None (we use uuid as PK).
pub(crate) fn parse_oauth_account(line: &str) -> Option<OauthAccount> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed == "{}" || trimmed == "null" {
        return None;
    }
    serde_json::from_str::<OauthAccount>(trimmed)
        .ok()
        .filter(|a| a.uuid.is_some())
}

/// Convert a probed `OauthAccount` into a storable `AccountRow`, dropping
/// records without a uuid (can't be primary-keyed). `nickname` is always
/// `None` here — it is never read from a probe, and `Store::upsert_account`
/// never writes it from this field anyway (see its doc comment).
fn account_row_from(a: &OauthAccount, now: i64) -> Option<crate::store::AccountRow> {
    let uuid = a.uuid.clone()?;
    Some(crate::store::AccountRow {
        uuid,
        email: a.email.clone(),
        display_name: a.display_name.clone(),
        organization_name: a.organization_name.clone(),
        organization_uuid: a.organization_uuid.clone(),
        seat_tier: a.seat_tier.clone(),
        last_seen_at: Some(now),
        nickname: None,
        has_extra_usage: a.has_extra_usage_enabled.unwrap_or(false),
    })
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn agent_status_lists_every_agent_host_connected_or_not() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        {
            let s = store.lock().unwrap();
            for (alias, transport) in [("laptop", "agent"), ("desk", "agent"), ("mefistos", "ssh")]
            {
                s.insert_host(alias, Some(alias)).unwrap();
                s.set_host_transport(alias, transport).unwrap();
            }
        }
        let reg = crate::agent::AgentRegistry::new();
        let _agent = crate::agent::fake::FakeAgent::connect(
            &reg,
            "laptop",
            crate::agent::fake::answer_exit(0),
        );

        let report = agent_status(&store, Some(&reg)).unwrap();
        assert!(report.enabled);
        let aliases: Vec<_> = report.hosts.iter().map(|h| h.alias.as_str()).collect();
        assert_eq!(aliases, ["desk", "laptop"], "agent hosts only, by alias");
        let laptop = &report.hosts[1];
        assert!(laptop.connected);
        assert!(laptop.connected_at.is_some());
        assert_eq!(laptop.agent_version.as_deref(), Some("9.9.9"));
        assert_eq!(laptop.host_name.as_deref(), Some("fake-host"));
        let desk = &report.hosts[0];
        assert!(!desk.connected);
        assert_eq!(
            (desk.connected_at, desk.agent_version.as_deref()),
            (None, None)
        );

        // The desktop routes nothing: the agent hosts are still listed, all
        // offline, and it says it is not accepting agents at all.
        let desktop = agent_status(&store, None).unwrap();
        assert!(!desktop.enabled);
        assert!(desktop.hosts.iter().all(|h| !h.connected));
        assert_eq!(desktop.hosts.len(), 2);
    }

    #[test]
    fn parse_tmux_version_extracts_version() {
        assert_eq!(parse_tmux_version("tmux 3.6a").as_deref(), Some("3.6a"));
        assert_eq!(parse_tmux_version("tmux 3.5"), Some("3.5".into()));
        assert_eq!(parse_tmux_version(""), None);
        assert_eq!(parse_tmux_version("not a version"), None);
    }

    #[test]
    fn parse_claude_version_extracts_first_token() {
        assert_eq!(
            parse_claude_version("2.1.144 (Claude Code)").as_deref(),
            Some("2.1.144")
        );
        assert_eq!(parse_claude_version("  2.1.12  "), Some("2.1.12".into()));
        assert_eq!(parse_claude_version(""), None);
    }

    #[test]
    fn parse_oauth_account_handles_full_json() {
        let line = r#"{"accountUuid":"abc","emailAddress":"a@b.com","displayName":"A B","organizationName":"32bit","organizationUuid":"org-1","seatTier":"max"}"#;
        let a = parse_oauth_account(line).unwrap();
        assert_eq!(a.uuid.as_deref(), Some("abc"));
        assert_eq!(a.email.as_deref(), Some("a@b.com"));
        assert_eq!(a.display_name.as_deref(), Some("A B"));
        assert_eq!(a.organization_name.as_deref(), Some("32bit"));
        assert_eq!(a.organization_uuid.as_deref(), Some("org-1"));
        assert_eq!(a.seat_tier.as_deref(), Some("max"));
    }

    #[test]
    fn parse_oauth_account_tolerates_missing_optional_fields() {
        let line = r#"{"accountUuid":"abc","emailAddress":"a@b.com"}"#;
        let a = parse_oauth_account(line).unwrap();
        assert_eq!(a.uuid.as_deref(), Some("abc"));
        assert_eq!(a.email.as_deref(), Some("a@b.com"));
        assert!(a.display_name.is_none());
        assert!(a.organization_name.is_none());
        assert!(a.seat_tier.is_none());
    }

    #[test]
    fn parse_oauth_account_parses_has_extra_usage_enabled_true_false_and_absent() {
        let true_line =
            r#"{"accountUuid":"abc","emailAddress":"a@b.com","hasExtraUsageEnabled":true}"#;
        assert_eq!(
            parse_oauth_account(true_line)
                .unwrap()
                .has_extra_usage_enabled,
            Some(true)
        );
        let false_line =
            r#"{"accountUuid":"abc","emailAddress":"a@b.com","hasExtraUsageEnabled":false}"#;
        assert_eq!(
            parse_oauth_account(false_line)
                .unwrap()
                .has_extra_usage_enabled,
            Some(false)
        );
        let absent_line = r#"{"accountUuid":"abc","emailAddress":"a@b.com"}"#;
        assert_eq!(
            parse_oauth_account(absent_line)
                .unwrap()
                .has_extra_usage_enabled,
            None
        );
    }

    #[test]
    fn account_row_from_maps_has_extra_usage_true_false_and_absent_to_false() {
        let mut a = OauthAccount {
            uuid: Some("u1".into()),
            has_extra_usage_enabled: Some(true),
            ..Default::default()
        };
        assert!(account_row_from(&a, 0).unwrap().has_extra_usage);
        a.has_extra_usage_enabled = Some(false);
        assert!(!account_row_from(&a, 0).unwrap().has_extra_usage);
        a.has_extra_usage_enabled = None;
        assert!(
            !account_row_from(&a, 0).unwrap().has_extra_usage,
            "absent defaults to false"
        );
    }

    #[test]
    fn parse_oauth_account_returns_none_for_empty_or_null_or_empty_obj() {
        assert!(parse_oauth_account("").is_none());
        assert!(parse_oauth_account("   ").is_none());
        assert!(parse_oauth_account("{}").is_none());
        assert!(parse_oauth_account("null").is_none());
    }

    #[test]
    fn parse_oauth_account_returns_none_when_uuid_missing() {
        let line = r#"{"emailAddress":"a@b.com","seatTier":"max"}"#;
        assert!(parse_oauth_account(line).is_none());
    }

    #[test]
    fn parse_oauth_account_returns_none_for_malformed_json() {
        assert!(parse_oauth_account("{not-json").is_none());
        assert!(parse_oauth_account("not even an object").is_none());
    }

    // ── set_account_nickname (service layer) ────────────────────────────────

    #[test]
    fn set_account_nickname_service_notfound_and_invalid() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        store
            .lock()
            .unwrap()
            .upsert_account(&crate::store::AccountRow {
                uuid: "u1".into(),
                email: Some("a@b.com".into()),
                display_name: None,
                organization_name: None,
                organization_uuid: None,
                seat_tier: None,
                last_seen_at: None,
                nickname: None,
                has_extra_usage: false,
            })
            .unwrap();

        let err = set_account_nickname(
            SetAccountNicknameArgs {
                uuid: "nope".into(),
                nickname: Some("Home".into()),
            },
            &store,
        )
        .unwrap_err();
        assert_eq!(err.code, "E_NOTFOUND");

        let err = set_account_nickname(
            SetAccountNicknameArgs {
                uuid: "u1".into(),
                nickname: Some("x".repeat(33)),
            },
            &store,
        )
        .unwrap_err();
        assert_eq!(err.code, "E_INVALID");

        let row = set_account_nickname(
            SetAccountNicknameArgs {
                uuid: "u1".into(),
                nickname: Some("  Home  ".into()),
            },
            &store,
        )
        .unwrap();
        assert_eq!(row.nickname.as_deref(), Some("Home"));
    }

    // ── probe_local_in / sync_local_account ─────────────────────────────────

    fn write_claude_json(dir: &std::path::Path, oauth_account_json: &str) {
        std::fs::write(
            dir.join(".claude.json"),
            format!(r#"{{"oauthAccount":{oauth_account_json}}}"#),
        )
        .unwrap();
    }

    /// `.claude.json` with no `oauthAccount` key at all — an explicit logout.
    fn write_logged_out_claude_json(dir: &std::path::Path) {
        std::fs::write(dir.join(".claude.json"), r#"{}"#).unwrap();
    }

    fn seed_existing_account(store: &Mutex<Store>, uuid: &str, email: &str) {
        let s = store.lock().unwrap();
        s.upsert_host("local").unwrap();
        s.upsert_account(&crate::store::AccountRow {
            uuid: uuid.into(),
            email: Some(email.into()),
            display_name: None,
            organization_name: None,
            organization_uuid: None,
            seat_tier: None,
            last_seen_at: None,
            nickname: None,
            has_extra_usage: false,
        })
        .unwrap();
        s.set_host_account("local", Some(uuid)).unwrap();
    }

    #[test]
    fn probe_local_in_reads_oauth_account_from_the_given_home_and_tolerates_null_seat_tier() {
        // Reproduces the evidence gathered from a real `~/.claude.json`:
        // `seatTier` present but JSON `null`. `OauthAccount`'s fields are all
        // `Option<String>`, so this must parse cleanly, not silently drop
        // the whole account.
        let dir = tempfile::tempdir().unwrap();
        write_claude_json(
            dir.path(),
            r#"{"accountUuid":"796436ed-fd1f-436d-bc15-ad1a81f78a71","emailAddress":"mj-janci@users.noreply.github.com","seatTier":null}"#,
        );
        let (reachable, _claude_ver, _tmux_ver, account) = probe_local_in(dir.path());
        assert!(reachable);
        let account = account.expect("oauthAccount must parse despite seatTier: null");
        assert_eq!(
            account.uuid.as_deref(),
            Some("796436ed-fd1f-436d-bc15-ad1a81f78a71")
        );
        assert_eq!(
            account.email.as_deref(),
            Some("mj-janci@users.noreply.github.com")
        );
        assert!(account.seat_tier.is_none());
    }

    #[test]
    fn probe_local_in_returns_no_account_without_a_claude_json() {
        let dir = tempfile::tempdir().unwrap();
        let (reachable, _, _, account) = probe_local_in(dir.path());
        assert!(reachable, "local is always reachable to itself");
        assert!(account.is_none());
    }

    #[tokio::test]
    async fn sync_local_account_links_from_home_when_unknown() {
        // Mirrors exactly what `reconcile_sessions_with` does in production:
        // `local` is auto-created via `Store::upsert_host`, which never
        // probes anything — before `sync_local_account` existed, nothing
        // else ever populated its account either (see its doc comment).
        let store = Mutex::new(Store::open_in_memory().unwrap());
        store.lock().unwrap().upsert_host("local").unwrap();
        let dir = tempfile::tempdir().unwrap();
        write_claude_json(
            dir.path(),
            r#"{"accountUuid":"acc-1","emailAddress":"a@b.c","seatTier":null}"#,
        );

        sync_local_account(&store, dir.path().to_path_buf())
            .await
            .unwrap();

        let row = host_row(&store, "local").expect("local row exists");
        assert_eq!(row.account_uuid.as_deref(), Some("acc-1"));
        let accounts = store.lock().unwrap().list_accounts().unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].uuid, "acc-1");
        assert_eq!(accounts[0].email.as_deref(), Some("a@b.c"));
    }

    #[tokio::test]
    async fn sync_local_account_is_a_noop_without_a_claude_json() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        store.lock().unwrap().upsert_host("local").unwrap();
        let dir = tempfile::tempdir().unwrap(); // no .claude.json inside

        sync_local_account(&store, dir.path().to_path_buf())
            .await
            .unwrap();

        assert!(host_row(&store, "local").unwrap().account_uuid.is_none());
        assert!(store.lock().unwrap().list_accounts().unwrap().is_empty());
    }

    #[tokio::test]
    async fn sync_local_account_relinks_on_a_different_uuid() {
        // The user logged out and into a DIFFERENT account: this must
        // relink (and upsert the new account row), unlike the old
        // fill-blank-only behaviour — otherwise per-account usage on `local`
        // would keep attributing to the account the user left.
        let store = Mutex::new(Store::open_in_memory().unwrap());
        seed_existing_account(&store, "existing-acc", "existing@x.com");
        let dir = tempfile::tempdir().unwrap();
        write_claude_json(
            dir.path(),
            r#"{"accountUuid":"different-acc","emailAddress":"new@x.com"}"#,
        );

        sync_local_account(&store, dir.path().to_path_buf())
            .await
            .unwrap();

        let row = host_row(&store, "local").unwrap();
        assert_eq!(
            row.account_uuid.as_deref(),
            Some("different-acc"),
            "a different uuid in ~/.claude.json must relink local"
        );
        let accounts = store.lock().unwrap().list_accounts().unwrap();
        assert!(accounts.iter().any(|a| a.uuid == "different-acc"));
    }

    #[tokio::test]
    async fn sync_local_account_refreshes_fields_when_the_uuid_is_unchanged() {
        // Same account: the link is a no-op, but the account row's fields
        // (email here) still get refreshed.
        let store = Mutex::new(Store::open_in_memory().unwrap());
        seed_existing_account(&store, "acc-1", "old@x.com");
        let dir = tempfile::tempdir().unwrap();
        write_claude_json(
            dir.path(),
            r#"{"accountUuid":"acc-1","emailAddress":"new@x.com"}"#,
        );

        sync_local_account(&store, dir.path().to_path_buf())
            .await
            .unwrap();

        let row = host_row(&store, "local").unwrap();
        assert_eq!(row.account_uuid.as_deref(), Some("acc-1"));
        let accounts = store.lock().unwrap().list_accounts().unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(
            accounts[0].email.as_deref(),
            Some("new@x.com"),
            "the account row's fields must refresh even when the link doesn't change"
        );
    }

    #[tokio::test]
    async fn sync_local_account_leaves_the_link_intact_when_the_file_is_missing() {
        // A failed read (missing file here; unreadable/unparseable are the
        // same code path via `LocalAccountProbe::Unavailable`) must NEVER
        // clear or change an existing link — a transient glitch reading
        // `~/.claude.json` is not evidence the user logged out.
        let store = Mutex::new(Store::open_in_memory().unwrap());
        seed_existing_account(&store, "existing-acc", "existing@x.com");
        let dir = tempfile::tempdir().unwrap(); // no .claude.json inside

        sync_local_account(&store, dir.path().to_path_buf())
            .await
            .unwrap();

        let row = host_row(&store, "local").unwrap();
        assert_eq!(
            row.account_uuid.as_deref(),
            Some("existing-acc"),
            "an already-linked account must survive a failed read"
        );
    }

    #[tokio::test]
    async fn sync_local_account_leaves_the_link_intact_on_an_explicit_logout() {
        // The file parses fine but has no `oauthAccount` at all: this is the
        // deliberately ambiguous case (real logout vs. a partial write of
        // ~/.claude.json mid-rewrite) — we keep the existing link rather
        // than flap it, per `sync_local_account`'s doc comment.
        let store = Mutex::new(Store::open_in_memory().unwrap());
        seed_existing_account(&store, "existing-acc", "existing@x.com");
        let dir = tempfile::tempdir().unwrap();
        write_logged_out_claude_json(dir.path());

        sync_local_account(&store, dir.path().to_path_buf())
            .await
            .unwrap();

        let row = host_row(&store, "local").unwrap();
        assert_eq!(
            row.account_uuid.as_deref(),
            Some("existing-acc"),
            "an explicit logout must not clear the existing link"
        );
    }

    // ── probe script: ssh argv-join + login-shell re-tokenization ─────────────

    #[test]
    fn probe_script_runs_as_one_program_when_quoted() {
        // The real PROBE_SCRIPT, single-quoted exactly as `probe_with_token`
        // now does, must survive the ssh argv space-join + the remote login
        // shell's re-tokenization and run as ONE `bash -lc` program — exiting 0
        // and emitting its two `---` section separators. The `|| true` guards
        // make this deterministic whether or not tmux / claude / jq /
        // ~/.claude.json are present, so it holds on CI too.
        use std::process::Command;
        let out = Command::new("sh")
            .args(["-c", &format!("bash -lc {}", quote(PROBE_SCRIPT))])
            .output()
            .expect("run sh");
        assert!(
            out.status.success(),
            "quoted probe must exit 0, stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.matches("---").count() >= 2,
            "quoted probe must emit its two section separators, got: {stdout:?}"
        );
    }

    #[test]
    fn probe_script_must_be_quoted_to_survive_login_shell_retokenization() {
        // Why `probe_with_token` must `quote` the script: a RAW multi-line
        // `bash -lc <script>` is re-tokenized by the (remote login) shell —
        // only the first word stays the bash program, its operands are eaten as
        // $0/$1, and later lines run in the login shell, not under `bash -lc`.
        // The probe's leading `tmux -V` degrades exactly this way (it collapses
        // to `bash -lc tmux` with `-V` swallowed, so the version never reaches
        // stdout). We reproduce the re-tokenization locally with `sh -c` and a
        // shell builtin so the test is PATH- and login-profile-independent.
        use std::process::Command;
        let script = "echo VER_MARKER tail\necho ---\necho SECOND";
        let section1 = |stdout: &[u8]| -> String {
            String::from_utf8_lossy(stdout)
                .split("---")
                .next()
                .unwrap_or("")
                .to_string()
        };

        // RAW (the bug): `echo VER_MARKER tail` collapses to `bash -lc echo`
        // (VER_MARKER → $0, tail → $1), so the marker never reaches stdout.
        let raw = Command::new("sh")
            .args(["-c", &format!("bash -lc {script}")])
            .output()
            .expect("run sh raw");
        assert!(
            !section1(&raw.stdout).contains("VER_MARKER"),
            "raw script must LOSE section-1 content, got: {:?}",
            String::from_utf8_lossy(&raw.stdout)
        );

        // QUOTED (the fix): the whole script crosses as one word; bash runs it
        // intact and section 1 keeps its content.
        let quoted = Command::new("sh")
            .args(["-c", &format!("bash -lc {}", quote(script))])
            .output()
            .expect("run sh quoted");
        assert!(
            section1(&quoted.stdout).contains("VER_MARKER"),
            "quoted script must PRESERVE section-1 content, got: {:?} stderr: {:?}",
            String::from_utf8_lossy(&quoted.stdout),
            String::from_utf8_lossy(&quoted.stderr)
        );
    }

    // ── end-to-end through the real service functions over `FakeSsh` ────────

    use crate::ssh_fake::{FakeSsh, Match, Reply};

    /// What a healthy host's probe script prints: the three `---`-separated
    /// sections `probe_with_token` parses.
    const HEALTHY_PROBE: &str = "tmux 3.4\n---\n2.1.144 (Claude Code)\n---\n{\"accountUuid\":\"acc-1\",\"emailAddress\":\"a@b.c\",\"organizationName\":\"32bit\"}\n";

    fn fake_fleet() -> FakeSsh {
        let fake = FakeSsh::new();
        fake.on(Match::script(PROBE_SCRIPT), Reply::ok(HEALTHY_PROBE));
        fake
    }

    fn host_row(store: &Mutex<Store>, alias: &str) -> Option<HostRow> {
        store
            .lock()
            .unwrap()
            .list_hosts()
            .unwrap()
            .into_iter()
            .find(|h| h.alias == alias)
    }

    #[tokio::test]
    async fn add_host_persists_the_parsed_probe_and_links_the_account() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let fake = fake_fleet();
        let row = add_host(
            AddHostArgs {
                alias: "alpha".into(),
                ssh_alias: "alpha.example".into(),
                transport: None,
            },
            &store,
            &fake,
        )
        .await
        .expect("reachable host is added");
        assert!(row.reachable);
        assert_eq!(row.ssh_alias.as_deref(), Some("alpha.example"));
        assert_eq!(row.tmux_version.as_deref(), Some("3.4"));
        assert_eq!(row.claude_version.as_deref(), Some("2.1.144"));
        assert_eq!(row.account_uuid.as_deref(), Some("acc-1"));
        assert_eq!(row.transport, "ssh", "the default transport");
        assert!(row.last_pinged_at.is_some());
        let accounts = store.lock().unwrap().list_accounts().unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].email.as_deref(), Some("a@b.c"));

        // Exactly one round trip, against the ssh alias (not the fleet
        // alias), as `bash -lc` with the WHOLE probe script as one quoted
        // word — the re-tokenisation bug the quoting fixed.
        let calls = fake.calls();
        assert_eq!(calls.len(), 1, "probe is a single round trip: {calls:?}");
        assert_eq!(calls[0].host, "alpha.example");
        assert_eq!(&calls[0].args[..2], ["bash", "-lc"]);
        assert_eq!(calls[0].script().as_deref(), Some(PROBE_SCRIPT));
    }

    /// The feature's headline case: a laptop behind NAT. The hub cannot dial
    /// it — that is *why* it needs an agent — so an SSH probe must not be the
    /// price of admission. Nor can the agent dial in first: the per-host
    /// token it authenticates with is minted against the row this call
    /// creates. So an agent host is persisted unprobed and `reachable=false`,
    /// and its reachability comes from the agent registry (through the
    /// router, which needs the row to exist) on the first `probe_host` or
    /// reconcile pass after the agent connects.
    #[tokio::test]
    async fn add_host_persists_an_agent_host_the_hub_cannot_reach() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let fake = fake_fleet();
        fake.unreachable("gamma.example");
        let row = add_host(
            AddHostArgs {
                alias: "gamma".into(),
                ssh_alias: "gamma.example".into(),
                transport: Some("agent".into()),
            },
            &store,
            &fake,
        )
        .await
        .expect("an agent host is added without an SSH probe");
        assert_eq!(row.transport, "agent");
        assert!(!row.reachable, "no agent has connected yet");
        assert!(
            row.last_pinged_at.is_some(),
            "the add is still a probe stamp"
        );
        assert!(
            fake.calls().is_empty(),
            "an agent host is never SSH-probed: {:?}",
            fake.calls()
        );
        assert!(host_row(&store, "gamma").is_some(), "the row is persisted");
    }

    #[tokio::test]
    async fn add_host_rejects_an_unknown_transport() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let fake = fake_fleet();
        let err = add_host(
            AddHostArgs {
                alias: "delta".into(),
                ssh_alias: "delta.example".into(),
                transport: Some("carrier-pigeon".into()),
            },
            &store,
            &fake,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "E_INVALID");
        // A rejected add_host must persist nothing — the same invariant
        // add_host_unreachable_is_e_probe_and_persists_nothing protects.
        // Before the fix, validation ran after insert_host had already
        // committed and emitted host_added, leaving a ghost row behind.
        assert!(
            host_row(&store, "delta").is_none(),
            "no row for a rejected transport"
        );
        assert!(fake.calls().is_empty(), "validation runs before the probe");
    }

    /// insert_host is an upsert, so re-adding an existing alias is a
    /// supported update path. `transport: None` means "unspecified", not
    /// "reset to ssh" — re-adding an agent host without naming a transport
    /// must leave it on "agent", not silently downgrade it to "ssh".
    #[tokio::test]
    async fn add_host_without_a_transport_does_not_downgrade_an_existing_agent_host() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let fake = fake_fleet();
        add_host(
            AddHostArgs {
                alias: "eps".into(),
                ssh_alias: "eps.example".into(),
                transport: Some("agent".into()),
            },
            &store,
            &fake,
        )
        .await
        .expect("first add");
        let row = add_host(
            AddHostArgs {
                alias: "eps".into(),
                ssh_alias: "eps.example".into(),
                transport: None,
            },
            &store,
            &fake,
        )
        .await
        .expect("re-add without a transport");
        assert_eq!(
            row.transport, "agent",
            "re-adding without a transport must not downgrade an agent host"
        );
    }

    #[tokio::test]
    async fn add_host_unreachable_is_e_probe_and_persists_nothing() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let fake = fake_fleet();
        fake.unreachable("down.example");
        let err = add_host(
            AddHostArgs {
                alias: "down".into(),
                ssh_alias: "down.example".into(),
                transport: None,
            },
            &store,
            &fake,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "E_PROBE");
        assert!(
            err.message.contains("exited Some(255)") && err.message.contains("connect to host"),
            "ssh's own diagnostic must survive: {}",
            err.message
        );
        assert!(
            host_row(&store, "down").is_none(),
            "no row for a host we can't reach"
        );
        assert!(store.lock().unwrap().list_accounts().unwrap().is_empty());
    }

    #[tokio::test]
    async fn add_host_host_key_failure_carries_the_classified_kind() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let fake = fake_fleet();
        fake.on_host(
            "hk.example",
            Match::Any,
            Reply::fail(255, "Host key verification failed.\r\n"),
        );
        let err = add_host(
            AddHostArgs {
                alias: "hk".into(),
                ssh_alias: "hk.example".into(),
                transport: None,
            },
            &store,
            &fake,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "E_PROBE");
        assert!(
            err.message.contains("host key not in known_hosts"),
            "summary in the message: {}",
            err.message
        );
        assert!(
            err.message.contains("Host key verification failed."),
            "ssh's own line survives: {}",
            err.message
        );
        let failure = &err.details.as_ref().expect("details")["ssh_failure"];
        assert_eq!(failure["kind"], "host_key_unknown");
        assert_eq!(failure["ssh_alias"], "hk.example");
    }

    #[tokio::test]
    async fn probe_host_marks_a_now_unreachable_host_and_keeps_its_row() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        {
            let s = store.lock().unwrap();
            s.insert_host("beta", Some("beta.example")).unwrap();
            s.update_host_probe("beta", true, Some("2.1.0"), Some("3.4"), 1)
                .unwrap();
        }
        let fake = fake_fleet();
        fake.unreachable("beta.example");
        let reg = CancellationRegistry::new();
        let row = probe_host(
            HostAliasArgs {
                alias: "beta".into(),
            },
            &store,
            &fake,
            &reg,
        )
        .await
        .expect("lenient probe never errors on an unreachable host");
        assert!(!row.reachable);
        assert_eq!(row.alias, "beta");
        assert_eq!(row.ssh_alias.as_deref(), Some("beta.example"));
        assert!(row.last_pinged_at.unwrap() > 1, "probe time stamped");
        assert!(host_row(&store, "beta").is_some_and(|h| !h.reachable));
        assert_eq!(fake.calls_for("beta.example").len(), 1);
        // The registry slot was released (CancelGuard) — nothing to cancel.
        reg.cancel(0);
    }

    #[tokio::test]
    async fn probe_host_reachable_again_flips_the_row_back() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        store
            .lock()
            .unwrap()
            .insert_host("alpha", Some("alpha.example"))
            .unwrap();
        assert!(!host_row(&store, "alpha").unwrap().reachable);
        let fake = fake_fleet();
        let reg = CancellationRegistry::new();
        let row = probe_host(
            HostAliasArgs {
                alias: "alpha".into(),
            },
            &store,
            &fake,
            &reg,
        )
        .await
        .unwrap();
        assert!(row.reachable);
        assert_eq!(row.tmux_version.as_deref(), Some("3.4"));
        assert_eq!(row.account_uuid.as_deref(), Some("acc-1"));
    }

    #[tokio::test]
    async fn hanging_host_is_bounded_by_the_wall_clock() {
        // The ssh layer's wall clock (E_SSH_TIMEOUT) is the only thing that
        // gets a probe out of a black-holed host. The fake applies the same
        // bound the real client does; shortened here so the test is fast.
        let bound = Duration::from_millis(100);
        let fake = fake_fleet();
        fake.hanging("slow.example").set_wall_clock(bound);
        let reg = CancellationRegistry::new();

        // Strict probe (AddHostPicker preview): E_PROBE wrapping the timeout.
        let start = std::time::Instant::now();
        let err = probe_ssh_alias(
            ProbeSshAliasArgs {
                ssh_alias: "slow.example".into(),
                call_id: Some(7),
            },
            &fake,
            &reg,
        )
        .await
        .unwrap_err();
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "bounded by the wall clock"
        );
        assert_eq!(err.code, "E_PROBE");
        assert!(
            err.message.contains("wall clock"),
            "timeout diagnostic must surface: {}",
            err.message
        );

        // Lenient probe (re-probe of a known host): row marked unreachable.
        let store = Mutex::new(Store::open_in_memory().unwrap());
        {
            let s = store.lock().unwrap();
            s.insert_host("slow", Some("slow.example")).unwrap();
            s.update_host_probe("slow", true, None, None, 1).unwrap();
        }
        let start = std::time::Instant::now();
        let row = probe_host(
            HostAliasArgs {
                alias: "slow".into(),
            },
            &store,
            &fake,
            &reg,
        )
        .await
        .unwrap();
        assert!(start.elapsed() < Duration::from_secs(5));
        assert!(!row.reachable);
    }

    #[tokio::test]
    async fn probe_ssh_alias_is_cancellable_through_the_registry() {
        let fake = fake_fleet();
        fake.hanging("slow.example")
            .set_wall_clock(Duration::from_secs(60));
        let reg = CancellationRegistry::new();
        let reg_for_cancel = Arc::clone(&reg);
        let canceller = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            reg_for_cancel.cancel(42);
        });
        let start = std::time::Instant::now();
        let err = probe_ssh_alias(
            ProbeSshAliasArgs {
                ssh_alias: "slow.example".into(),
                call_id: Some(42),
            },
            &fake,
            &reg,
        )
        .await
        .unwrap_err();
        canceller.await.unwrap();
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "cancel beats the 60s wall clock"
        );
        assert_eq!(err.code, "E_PROBE");
        assert!(err.message.contains("cancelled"), "{}", err.message);
    }

    #[tokio::test]
    async fn probe_ssh_alias_preview_does_not_persist() {
        let fake = fake_fleet();
        let reg = CancellationRegistry::new();
        let preview = probe_ssh_alias(
            ProbeSshAliasArgs {
                ssh_alias: "alpha.example".into(),
                call_id: None,
            },
            &fake,
            &reg,
        )
        .await
        .unwrap();
        assert!(preview.reachable);
        assert_eq!(preview.claude_version.as_deref(), Some("2.1.144"));
        assert_eq!(preview.tmux_version.as_deref(), Some("3.4"));
        assert_eq!(
            preview.account.as_ref().and_then(|a| a.uuid.as_deref()),
            Some("acc-1")
        );
    }

    #[tokio::test]
    async fn probe_tolerates_a_host_without_tmux_claude_or_account() {
        // Each probe section is `|| true`-guarded on the remote, so a bare
        // host yields empty sections — reachable, but nothing known.
        let fake = FakeSsh::new();
        fake.on(Match::script(PROBE_SCRIPT), Reply::ok("\n---\n\n---\n\n"));
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let row = add_host(
            AddHostArgs {
                alias: "bare".into(),
                ssh_alias: "bare.example".into(),
                transport: None,
            },
            &store,
            &fake,
        )
        .await
        .unwrap();
        assert!(row.reachable);
        assert!(row.tmux_version.is_none());
        assert!(row.claude_version.is_none());
        assert!(row.account_uuid.is_none());
    }

    #[tokio::test]
    #[ignore = "requires network + a reachable 'mefistos' ssh host with claude logged in"]
    async fn probe_mefistos_end_to_end() {
        // Exercises the REAL probe path (probe → ssh `bash -lc` → mefistos).
        // Before the quoting fix the multi-line script was re-tokenized by the
        // remote login shell, so the first line collapsed to `bash -lc tmux`
        // (with `-V` swallowed as $0) and the tmux version came back empty —
        // a degraded/partial probe. After the fix the whole script runs as one
        // bash program and every section is populated. Run with:
        //   cargo test -- --ignored probe_mefistos_end_to_end --nocapture
        let ssh = crate::ssh::SshClient::new();
        let (reachable, claude_v, tmux_v, account) =
            probe(&ssh, "mefistos").await.expect("probe mefistos");
        eprintln!("reachable={reachable} claude={claude_v:?} tmux={tmux_v:?} account={account:?}");
        assert!(reachable, "mefistos must be reachable");
        // The tmux version is the field the re-tokenization bug dropped — it
        // must be present once the script is `quote`'d.
        assert!(
            tmux_v.is_some(),
            "tmux version must parse (was empty before the quoting fix)"
        );
        assert!(claude_v.is_some(), "claude version must parse");
        let acct = account.expect("oauthAccount must parse");
        assert!(acct.uuid.is_some(), "parsed account must carry a uuid");
    }
}
