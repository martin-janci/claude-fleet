//! Service layer for SSH host management — transport-agnostic logic over
//! `store.rs` helpers plus `ssh_config.rs` (discovery) and `ssh::SshClient`
//! (probing). Called by both the Tauri command wrappers and the MCP server.

use crate::cancel::CancellationRegistry;
use crate::ipc_error::IpcError;
use crate::shell::quote as shq;
use crate::ssh::SshClient;
use crate::ssh_config::{self, SshHost};
use crate::store::{HostRow, Store};
use serde::Deserialize;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub fn discover_hosts() -> Result<Vec<SshHost>, IpcError> {
    Ok(ssh_config::load_user_config())
}

pub fn list_hosts(store: &Mutex<Store>) -> Result<Vec<HostRow>, IpcError> {
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    s.list_hosts().map_err(IpcError::from)
}

pub fn list_accounts(store: &Mutex<Store>) -> Result<Vec<crate::store::AccountRow>, IpcError> {
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    s.list_accounts().map_err(IpcError::from)
}

#[derive(Deserialize)]
pub struct AddHostArgs {
    pub alias: String,
    pub ssh_alias: String,
}

pub async fn add_host(
    args: AddHostArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<HostRow, IpcError> {
    // Reject hostile aliases (e.g. `-oProxyCommand=…`) before they reach ssh.
    crate::validate::host_alias(&args.alias)?;
    crate::validate::host_alias(&args.ssh_alias)?;
    // Probe first; we don't want to persist a host we can't talk to.
    let (reachable, claude_ver, tmux_ver, account) = probe(ssh, &args.ssh_alias).await?;
    {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        s.insert_host(&args.alias, Some(&args.ssh_alias))?;
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
#[derive(serde::Serialize)]
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
    ssh: &Arc<SshClient>,
    reg: &Arc<CancellationRegistry>,
) -> Result<ProbePreview, IpcError> {
    crate::validate::host_alias(&args.ssh_alias)?;
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

#[derive(Deserialize)]
pub struct HostAliasArgs {
    pub alias: String,
}

pub async fn probe_host(
    args: HostAliasArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    reg: &Arc<CancellationRegistry>,
) -> Result<HostRow, IpcError> {
    let ssh_alias = {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
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
        // probe_local does blocking std::process + fs I/O — keep it off the
        // async runtime worker thread.
        tokio::task::spawn_blocking(probe_local)
            .await
            .unwrap_or_default()
    } else {
        crate::validate::host_alias(target)?;
        // Anonymous token — probe_host is user-triggered re-probe; we give it
        // a token so it can be cancelled if needed, but no frontend call_id.
        // The CancelGuard releases the slot even if the probe panics.
        let (id, token) = reg.register_anonymous();
        let _guard = crate::cancel::CancelGuard::new(Arc::clone(reg), id);
        probe_lenient_with_token(ssh, target, token).await
    };
    {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
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
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    s.delete_host(&args.alias).map_err(IpcError::from)?;
    Ok(row)
}

#[derive(Deserialize)]
pub struct HideHostArgs {
    pub alias: String,
    pub hidden: bool,
}

pub fn hide_host(args: HideHostArgs, store: &Mutex<Store>) -> Result<HostRow, IpcError> {
    {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        s.set_host_hidden(&args.alias, args.hidden)
            .map_err(IpcError::from)?;
    }
    list_one(store, &args.alias)
}

// --- helpers ---

fn list_one(store: &Mutex<Store>, alias: &str) -> Result<HostRow, IpcError> {
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    s.list_hosts()?
        .into_iter()
        .find(|h| h.alias == alias)
        .ok_or_else(|| IpcError::new("E_NOTFOUND", format!("host {alias} not found")))
}

/// Strict probe — returns Err(E_PROBE) if the SSH round trip fails. Used by
/// add_host. Reads tmux + claude versions AND the oauthAccount in a single
/// round trip (sections separated by literal `---`).
async fn probe(
    ssh: &Arc<SshClient>,
    host: &str,
) -> Result<(bool, Option<String>, Option<String>, Option<OauthAccount>), IpcError> {
    let token = CancellationToken::new();
    probe_with_token(ssh, host, token).await
}

/// The remote probe script: reads tmux + claude versions AND the
/// `oauthAccount` from `~/.claude.json` in one SSH round trip, with the three
/// sections separated by a literal `---`. Each section is independently
/// guarded (`|| true`) so a missing tool/file degrades to an empty section
/// rather than failing the whole probe. MUST be `shq`'d before it's handed to
/// `bash -lc` over ssh (see `probe_with_token`).
const PROBE_SCRIPT: &str = r#"tmux -V 2>/dev/null || true
echo ---
claude --version 2>/dev/null || true
echo ---
( cat "$HOME/.claude.json" 2>/dev/null | jq -c .oauthAccount 2>/dev/null \
  || python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); print(json.dumps(d.get("oauthAccount") or {}))' "$HOME/.claude.json" 2>/dev/null \
  || true )"#;

/// Like `probe` but uses the provided `CancellationToken` so the caller can
/// cancel the SSH round trip.
async fn probe_with_token(
    ssh: &Arc<SshClient>,
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
    // `shq` also escapes the inner single-quotes of the embedded python3
    // one-liner. Same fix as `ensure_remote_project` in commands/sessions.rs.
    let quoted = shq(PROBE_SCRIPT);
    let out = ssh
        .run_cancellable(
            host,
            &["bash", "-lc", &quoted],
            Duration::from_secs(5),
            token,
        )
        .await
        .map_err(|e| IpcError::new("E_PROBE", format!("ssh {host}: {}", e.message)))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(IpcError::new(
            "E_PROBE",
            format!(
                "ssh {host} exited {:?}: {}",
                out.status.code(),
                stderr.trim()
            ),
        ));
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
    ssh: &Arc<SshClient>,
    host: &str,
    token: CancellationToken,
) -> (bool, Option<String>, Option<String>, Option<OauthAccount>) {
    probe_with_token(ssh, host, token).await.unwrap_or_default()
}

fn probe_local() -> (bool, Option<String>, Option<String>, Option<OauthAccount>) {
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
    let account = std::env::var("HOME").ok().and_then(|home| {
        let path = std::path::Path::new(&home).join(".claude.json");
        let contents = std::fs::read_to_string(path).ok()?;
        let v: serde_json::Value = serde_json::from_str(&contents).ok()?;
        let oa = v.get("oauthAccount")?;
        serde_json::from_value::<OauthAccount>(oa.clone())
            .ok()
            .filter(|a| a.uuid.is_some())
    });
    (
        true,
        parse_claude_version(claude.as_deref().unwrap_or("")),
        parse_tmux_version(tmux.as_deref().unwrap_or("")),
        account,
    )
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
}

/// Parse the third probe section. Empty / "null" / "{}" → None.
/// Treats account-without-uuid as None (we use uuid as PK).
fn parse_oauth_account(line: &str) -> Option<OauthAccount> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed == "{}" || trimmed == "null" {
        return None;
    }
    serde_json::from_str::<OauthAccount>(trimmed)
        .ok()
        .filter(|a| a.uuid.is_some())
}

/// Convert a probed `OauthAccount` into a storable `AccountRow`, dropping
/// records without a uuid (can't be primary-keyed).
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
            .args(["-c", &format!("bash -lc {}", shq(PROBE_SCRIPT))])
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
        // Why `probe_with_token` must `shq` the script: a RAW multi-line
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
            .args(["-c", &format!("bash -lc {}", shq(script))])
            .output()
            .expect("run sh quoted");
        assert!(
            section1(&quoted.stdout).contains("VER_MARKER"),
            "quoted script must PRESERVE section-1 content, got: {:?} stderr: {:?}",
            String::from_utf8_lossy(&quoted.stdout),
            String::from_utf8_lossy(&quoted.stderr)
        );
    }

    #[tokio::test]
    #[ignore = "requires network + a reachable 'mefistos' ssh host with claude logged in"]
    async fn probe_mefistos_end_to_end() {
        // Exercises the REAL probe path (probe → ssh `bash -lc` → mefistos).
        // Before the shq fix the multi-line script was re-tokenized by the
        // remote login shell, so the first line collapsed to `bash -lc tmux`
        // (with `-V` swallowed as $0) and the tmux version came back empty —
        // a degraded/partial probe. After the fix the whole script runs as one
        // bash program and every section is populated. Run with:
        //   cargo test -- --ignored probe_mefistos_end_to_end --nocapture
        let ssh = Arc::new(SshClient::new());
        let (reachable, claude_v, tmux_v, account) =
            probe(&ssh, "mefistos").await.expect("probe mefistos");
        eprintln!("reachable={reachable} claude={claude_v:?} tmux={tmux_v:?} account={account:?}");
        assert!(reachable, "mefistos must be reachable");
        // The tmux version is the field the re-tokenization bug dropped — it
        // must be present once the script is `shq`'d.
        assert!(
            tmux_v.is_some(),
            "tmux version must parse (was empty before the shq fix)"
        );
        assert!(claude_v.is_some(), "claude version must parse");
        let acct = account.expect("oauthAccount must parse");
        assert!(acct.uuid.is_some(), "parsed account must carry a uuid");
    }
}
