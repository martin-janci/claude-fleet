//! Add account (Orbit Fleet M15 step G2.9): a new login profile on a host
//! (`~/.claude-profiles/<name>`, docs/accounts.md), signed in to a Claude
//! subscription, holding a `claude setup-token` token, or holding an
//! Anthropic API key.
//!
//! **Subscription.** `start_login` opens a login pane on the host: a tmux
//! session of fleet's own, `fleet-login--<name>` (left out of every session
//! list, like a shell terminal), running `claude /login` with
//! `CLAUDE_CONFIG_DIR` on the new profile. `login_status` reads the pane
//! back (its last lines and the sign-in link it printed) and the host's
//! profiles, so the dialog shows the link and turns Done on once the host
//! reports the login. `login_key` presses one of the CLI's own keys (a
//! numbered choice, Enter, an arrow) and `login_code` pastes the code the
//! sign-in page hands out; `end_login` closes the pane. Whatever the CLI's
//! flow becomes, the "run it there" command
//! (`CLAUDE_CONFIG_DIR=~/.claude-profiles/<name> claude /login`) stays the
//! fallback.
//!
//! **API key.** `api_key` asks Anthropic whether the key works (`GET
//! /v1/models`): a refusal is the provider's own 401 / 403 on the field.
//! The key then goes to the host on **stdin**, into
//! `~/.claude-profiles/<name>/.fleet-api-key` (mode 600, under `umask
//! 077`), beside `.fleet-account.json`, the account the profile is listed
//! as (`apikey-` + 16 hex of the key's SHA-256). A session under the profile
//! exports it as `ANTHROPIC_API_KEY` (`tmux::PROFILE_API_KEY`). The key is
//! never in an argv, a log, an audit line, the store or a reply; fleet keeps
//! no copy, so changing it means adding it again.
//!
//! **Setup token.** `setup_token` takes the long-lived token `claude
//! setup-token` prints (`sk-ant-oat…`, a year's sign-in to a subscription,
//! no browser on the host). Anthropic is asked whether it works (its
//! profile when the token may read it, else `GET /v1/models`); it then goes
//! to the host on stdin into `~/.claude-profiles/<name>/.fleet-oauth-token`
//! (mode 600), and a session under the profile exports it as
//! `CLAUDE_CODE_OAUTH_TOKEN` (`tmux::PROFILE_API_KEY`), which outranks the
//! profile's own `/login`. The profile is listed as the token's account when
//! Anthropic names it, else the account the person says it belongs to (or
//! the profile's existing `/login`), else an account of its own
//! (`setuptoken-` + 16 hex of the token's SHA-256). The same call on a
//! profile that already holds one replaces it, and on a `/login` profile
//! (signed out, say) puts the token in front of that login. Never a
//! profile holding an API key. Secret like the key: never in an argv, a
//! log, an audit line, the store or a reply.
//!
//! **Daily limit.** An API-key account may carry a daily spend limit in
//! USD (`accounts.daily_limits`, a JSON map by account). Spend is the same
//! per-account roll-up the Accounts page shows (`usage_daily_account`), so
//! the limit is checked when a session starts: a start under a login whose
//! account spent its limit today (UTC) is refused, and automation leaves it
//! alone (`account_limits`). A session already running is not stopped.

use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::ipc_error::{codes, IpcError};
use crate::net::https::{HttpTransport, Request, TransportError};
use crate::shell::quote;
use crate::ssh::SshExec;
use crate::store::Store;
use crate::tmux::TmuxExec;
use rmcp::schemars;

/// The key's file in the profile dir, read by `tmux::PROFILE_API_KEY`.
pub const API_KEY_FILE: &str = ".fleet-api-key";
/// The account an API-key profile is listed as (`tmux::profiles_script`).
pub const API_ACCOUNT_FILE: &str = ".fleet-account.json";
/// Every API-key account's uuid starts with this; no `/login` account does
/// (theirs are UUIDs).
pub const API_KEY_ACCOUNT_PREFIX: &str = "apikey-";
/// A setup token's file in the profile dir, read by `tmux::PROFILE_API_KEY`.
pub const SETUP_TOKEN_FILE: &str = ".fleet-oauth-token";
/// The account of a setup token nobody named starts with this.
pub const SETUP_TOKEN_ACCOUNT_PREFIX: &str = "setuptoken-";
/// `fleetCredential` / `HostProfileRow::credential` of a setup-token profile.
pub const SETUP_TOKEN_CREDENTIAL: &str = "setup_token";
/// The internal setting holding the daily limits: `{account_uuid: usd}`.
pub const DAILY_LIMITS_KEY: &str = "accounts.daily_limits";
/// The highest daily limit accepted, in USD.
pub const MAX_DAILY_LIMIT_USD: f64 = 100_000.0;

const MODELS_URL: &str = "https://api.anthropic.com/v1/models?limit=1";
/// Who an OAuth token belongs to, when its scopes let it ask.
const PROFILE_URL: &str = "https://api.anthropic.com/api/oauth/profile";
/// The beta header an OAuth bearer token needs on Anthropic's API.
const OAUTH_BETA: &str = "oauth-2025-04-20";
const ANTHROPIC_HOST: &str = "api.anthropic.com";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const WRITE_WALL_CLOCK: Duration = Duration::from_secs(30);
/// Lines of the login pane `login_status` hands back.
const PANE_LINES: usize = 24;
const OK_MARK: &str = "__fleet_ok__";
const LOGIN_PROFILE_MARK: &str = "__fleet_login_profile__";
const API_KEY_PROFILE_MARK: &str = "__fleet_api_key_profile__";

/// Whether `uuid` names an API-key account (no usage windows to poll).
pub fn is_api_key_account(uuid: &str) -> bool {
    uuid.starts_with(API_KEY_ACCOUNT_PREFIX)
}

/// Whether `uuid` names an account fleet made up for a setup token nobody
/// named (no usage windows fleet can read).
pub fn is_setup_token_account(uuid: &str) -> bool {
    uuid.starts_with(SETUP_TOKEN_ACCOUNT_PREFIX)
}

/// `HostProfileRow::credential` of a probed profile: `setup_token` when its
/// `.fleet-account.json` says fleet wrote a setup token there; anything
/// else is a `/login` or an API key (`None`).
pub fn credential_of(a: &crate::service::hosts::OauthAccount) -> Option<String> {
    (a.fleet_credential.as_deref() == Some(SETUP_TOKEN_CREDENTIAL))
        .then(|| SETUP_TOKEN_CREDENTIAL.to_string())
}

/// The tool's and the desktop command's arguments. `api_key`,
/// `setup_token` and `code` are secrets: `Debug` prints none of them, and
/// no caller logs them.
#[derive(Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars", rename = "AddAccountParams")]
pub struct AddAccountArgs {
    /// start_login, login_status, login_key, login_code, end_login, api_key,
    /// setup_token, daily_limit.
    pub action: String,
    /// Host.
    pub host_alias: String,
    /// Profile.
    pub profile: String,
    /// 1-9, Enter, Up, Down, Escape, Tab.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// sk-ant-… key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// sk-ant-oat… token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup_token: Option<String>,
    /// Its account's uuid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// Nickname.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nickname: Option<String>,
    /// USD a day; 0 clears. Left out, `api_key` keeps the account's limit
    /// (the same key on another host is the same account).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily_limit_usd: Option<f64>,
    /// Sign an existing login in again; profile "" is the host's own.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub again: bool,
}

impl std::fmt::Debug for AddAccountArgs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AddAccountArgs")
            .field("action", &self.action)
            .field("host_alias", &self.host_alias)
            .field("profile", &self.profile)
            .field("key", &self.key)
            .field("code", &self.code.as_ref().map(|_| "<redacted>"))
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field(
                "setup_token",
                &self.setup_token.as_ref().map(|_| "<redacted>"),
            )
            .field("account", &self.account)
            .field("nickname", &self.nickname)
            .field("daily_limit_usd", &self.daily_limit_usd)
            .field("again", &self.again)
            .finish()
    }
}

impl AddAccountArgs {
    /// The login directory the action is about: the profile's, or `None`
    /// for the host's own login (`again` with an empty profile).
    pub fn login_dir(&self) -> Option<&str> {
        (!(self.again && self.profile.is_empty())).then_some(self.profile.as_str())
    }

    /// The name the login pane is known by: the profile, or [`OWN_LOGIN`].
    pub fn pane_id(&self) -> &str {
        self.login_dir().unwrap_or(OWN_LOGIN)
    }

    /// The audit line: identifying fields only, never the key or the code.
    pub fn audit_detail(&self) -> String {
        format!(
            "action={} host={} profile={} key={:?} limit={:?}{}",
            self.action.escape_debug(),
            self.host_alias.escape_debug(),
            self.profile.escape_debug(),
            self.key.as_deref().map(str::escape_debug),
            self.daily_limit_usd,
            if self.again { " again" } else { "" },
        )
    }
}

/// What `login_status` saw.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoginStatus {
    /// The host reports the profile logged in to an account.
    pub logged_in: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_uuid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// The login pane's last lines, while not logged in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<String>,
    /// The sign-in link the pane printed, when one is on screen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sign_in_url: Option<String>,
    /// The "run it there" fallback, for a terminal on the host.
    pub command: String,
}

/// What `api_key` added.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiKeyAdded {
    pub host_alias: String,
    pub profile: String,
    pub account_uuid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily_limit_usd: Option<f64>,
}

/// What `setup_token` added or replaced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetupTokenAdded {
    pub host_alias: String,
    pub profile: String,
    pub account_uuid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// The profile held a setup token already, now replaced.
    pub replaced: bool,
}

/// [`run`] in production: tmux and SSH through the shared client, Anthropic
/// through [`anthropic_transport`].
pub async fn run_with_client(
    args: &AddAccountArgs,
    store: &Mutex<Store>,
    ssh: &std::sync::Arc<crate::ssh::SshClient>,
) -> Result<serde_json::Value, IpcError> {
    let tmux = crate::service::sessions::exec_for(&args.host_alias, ssh);
    let http = anthropic_transport();
    run(
        args,
        store,
        ssh.as_ref(),
        tmux.as_ref(),
        &http,
        crate::store::now_unix(),
    )
    .await
}

/// Run one action. `tmux` and `ssh` reach `args.host_alias`; `http`
/// reaches Anthropic.
pub async fn run(
    args: &AddAccountArgs,
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    tmux: &dyn TmuxExec,
    http: &dyn HttpTransport,
    now: i64,
) -> Result<serde_json::Value, IpcError> {
    crate::validate::host_alias(&args.host_alias)?;
    if args.login_dir().is_some() {
        crate::validate::claude_profile(&args.profile)?;
    }
    if args.again && !LOGIN_ACTIONS.contains(&args.action.as_str()) {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("again applies to {} only", LOGIN_ACTIONS.join(", ")),
        ));
    }
    {
        let s = lock(store)?;
        if s.get_host_row(&args.host_alias)?.is_none() {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("host {} not found", args.host_alias),
            ));
        }
    }
    let out = match args.action.as_str() {
        "start_login" => {
            start_login(args, store, tmux).await?;
            serde_json::json!({ "session": login_session_name(args.pane_id()) })
        }
        "login_status" => {
            let status = login_status(args, store, tmux, now).await?;
            if status.logged_in && !args.again {
                // Review 2026-11-03 F3: a new login's first session must not
                // miss the host's MCP servers (fleet's own among them). Best
                // effort, and a no-op once the profile has them all.
                if let Err(e) = crate::service::provision::share_mcp_servers_into(
                    ssh,
                    &args.host_alias,
                    &args.profile,
                )
                .await
                {
                    tracing::warn!(
                        "[add-account] sharing MCP servers into {} on {} failed: {}",
                        args.profile,
                        args.host_alias,
                        e.message
                    );
                }
            }
            to_value(status)
        }
        "login_key" => {
            login_key(args, tmux).await?;
            serde_json::json!({ "sent": true })
        }
        "login_code" => {
            login_code(args, ssh).await?;
            serde_json::json!({ "sent": true })
        }
        "end_login" => {
            end_login(args, tmux).await?;
            serde_json::json!({ "closed": true })
        }
        "api_key" => to_value(add_api_key(args, store, ssh, http, now).await?),
        "setup_token" => {
            let added = add_setup_token(args, store, ssh, http, now).await?;
            // Like a fresh `/login` (review 2026-11-03 F3): the first
            // session under the token must not miss the host's MCP servers.
            if let Err(e) = crate::service::provision::share_mcp_servers_into(
                ssh,
                &args.host_alias,
                &args.profile,
            )
            .await
            {
                tracing::warn!(
                    "[add-account] sharing MCP servers into {} on {} failed: {}",
                    args.profile,
                    args.host_alias,
                    e.message
                );
            }
            to_value(added)
        }
        "daily_limit" => {
            let uuid = profile_account(store, &args.host_alias, &args.profile)?
                .filter(|u| is_api_key_account(u))
                .ok_or_else(|| {
                    IpcError::new(
                        codes::E_INVALID,
                        format!(
                            "profile {} on {} is not an API-key account; a daily limit applies to those",
                            args.profile, args.host_alias
                        ),
                    )
                })?;
            let limit = checked_limit(args.daily_limit_usd)?;
            set_daily_limit(&*lock(store)?, &uuid, limit)?;
            serde_json::json!({ "account_uuid": uuid, "daily_limit_usd": limit })
        }
        other => {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!(
                    "unknown action {:?}: start_login, login_status, login_key, login_code, end_login, api_key, setup_token or daily_limit",
                    other.escape_debug().to_string()
                ),
            ))
        }
    };
    Ok(out)
}

fn to_value<T: Serialize>(v: T) -> serde_json::Value {
    serde_json::to_value(v).unwrap_or(serde_json::Value::Null)
}

fn lock(store: &Mutex<Store>) -> Result<std::sync::MutexGuard<'_, Store>, IpcError> {
    store
        .lock()
        .map_err(|_| IpcError::new(codes::E_LOCK, "store lock poisoned"))
}

/// The actions `again` applies to: the login pane's.
const LOGIN_ACTIONS: [&str; 5] = [
    "start_login",
    "login_status",
    "login_key",
    "login_code",
    "end_login",
];

/// The "run it there" command for the login `dir` (`None` = the host's own).
pub fn login_command(dir: Option<&str>) -> String {
    match dir {
        Some(profile) => format!("CLAUDE_CONFIG_DIR=~/.claude-profiles/{profile} claude /login"),
        None => "claude /login".to_string(),
    }
}

/// The stored host row's entry for `profile`, if any.
fn profile_row(
    store: &Mutex<Store>,
    host: &str,
    profile: &str,
) -> Result<Option<crate::store::HostProfileRow>, IpcError> {
    let s = lock(store)?;
    Ok(s.get_host_row(host)?
        .and_then(|h| h.claude_profiles)
        .unwrap_or_default()
        .into_iter()
        .find(|p| p.name == profile))
}

/// The account the stored host row lists `profile` on, if any.
fn profile_account(
    store: &Mutex<Store>,
    host: &str,
    profile: &str,
) -> Result<Option<String>, IpcError> {
    let s = lock(store)?;
    Ok(s.get_host_row(host)?
        .and_then(|h| h.claude_profiles)
        .unwrap_or_default()
        .into_iter()
        .find(|p| p.name == profile)
        .and_then(|p| p.account_uuid))
}

/// The login pane's id for the host's own login (`again` with no profile).
/// Not a profile name (those start with a letter or digit), so it never
/// meets one.
pub const OWN_LOGIN: &str = "_own";

/// What `claude /login` prints once a sign-in went through.
const LOGIN_SUCCESS: &str = "login successful";

/// The login pane's tmux session for `profile` (a validated name, or
/// [`OWN_LOGIN`]). Every
/// session list leaves it out (`tmux::is_login_session_name`).
pub fn login_session_name(profile: &str) -> String {
    format!("{}{profile}", crate::tmux::LOGIN_SESSION_PREFIX)
}

async fn run_tmux(tmux: &dyn TmuxExec, script: &str, what: &str) -> Result<String, IpcError> {
    let out = tmux.run_script(script).await?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(IpcError::new(
            codes::E_TMUX,
            format!("could not {what}: {}", first_line(&out.stderr)),
        ))
    }
}

/// PURE: the script opening the login pane, unless one is open. A fresh
/// profile gets a `.claude.json` that marks onboarding done, so the pane
/// opens on the login choice rather than the theme picker; the pane holds
/// after `claude` exits, so its last words stay readable.
fn start_login_script(profile: &str) -> String {
    let name = login_session_name(profile);
    let pane = format!(
        "export CLAUDE_CONFIG_DIR=\"$HOME/.claude-profiles/\"{p}; /bin/sh -c {links} 2>/dev/null; \
         claude /login; echo; echo 'claude exited. Close this from fleet.'; exec sleep 3600",
        p = quote(profile),
        links = quote(crate::tmux::PROFILE_LINKS),
    );
    start_pane_script(&name, &pane, Some(profile))
}

/// PURE: the script opening the pane signing the host's own login in again
/// (no `CLAUDE_CONFIG_DIR`: `~/.claude`), unless one is open.
fn start_own_login_script() -> String {
    let pane = "claude /login; echo; echo 'claude exited. Close this from fleet.'; exec sleep 3600";
    start_pane_script(&login_session_name(OWN_LOGIN), pane, None)
}

/// PURE: open the login pane `name` running `pane`; for a profile, make its
/// directory first (marked as this flow's, [`NEW_LOGIN_MARK`]) with
/// onboarding done.
fn start_pane_script(name: &str, pane: &str, profile: Option<&str>) -> String {
    let dir = match profile {
        Some(p) => format!(
            "umask 077; d=\"$HOME/.claude-profiles/\"{p}; \
             [ -d \"$d\" ] || {{ mkdir -p \"$d\" || exit 1; : > \"$d/{NEW_LOGIN_MARK}\"; }}; \
             [ -e \"$d/.claude.json\" ] || printf '%s\\n' '{{\"hasCompletedOnboarding\":true}}' > \"$d/.claude.json\"; ",
            p = quote(p),
        ),
        None => String::new(),
    };
    format!(
        "tmux has-session -t {exact} 2>/dev/null && exit 0; {dir}\
         tmux new-session -d -s {name} -x 120 -y 40 -c \"$HOME\" -e COLORTERM=truecolor \
         -e TERM=xterm-256color -e \"PATH=$PATH\" {pane}",
        exact = quote(&crate::tmux::exact_session(name)),
        name = quote(name),
        pane = quote(pane),
    )
}

async fn start_login(
    args: &AddAccountArgs,
    store: &Mutex<Store>,
    tmux: &dyn TmuxExec,
) -> Result<(), IpcError> {
    if args.again {
        return start_login_again(args, store, tmux).await;
    }
    if let Some(uuid) = profile_account(store, &args.host_alias, &args.profile)? {
        let what = if is_api_key_account(&uuid) {
            "an API key"
        } else if is_setup_token_account(&uuid) {
            "a setup token"
        } else {
            "a login"
        };
        let problem = format!(
            "{} already has a profile {} with {what}; pick another name",
            args.host_alias, args.profile
        );
        return Err(IpcError::new(codes::E_EXISTS, problem.clone())
            .with_details(problems("profile", &problem)));
    }
    run_tmux(
        tmux,
        &start_login_script(&args.profile),
        "open the login pane",
    )
    .await?;
    Ok(())
}

/// `start_login { again }`: open the login pane over a login that exists,
/// so a person signs it in again. A profile that holds an API key or a
/// setup token has no `/login` to renew: refused, naming what renews it.
async fn start_login_again(
    args: &AddAccountArgs,
    store: &Mutex<Store>,
    tmux: &dyn TmuxExec,
) -> Result<(), IpcError> {
    let Some(profile) = args.login_dir() else {
        run_tmux(tmux, &start_own_login_script(), "open the login pane").await?;
        return Ok(());
    };
    let row = profile_row(store, &args.host_alias, profile)?;
    let token = row
        .as_ref()
        .is_some_and(|r| r.credential.as_deref() == Some(SETUP_TOKEN_CREDENTIAL));
    let key = row
        .as_ref()
        .and_then(|r| r.account_uuid.as_deref())
        .is_some_and(is_api_key_account);
    if token || key {
        let problem = if token {
            format!(
                "{profile} on {} signs in with a setup token: add a new token to it (Setup token)",
                args.host_alias
            )
        } else {
            format!(
                "{profile} on {} uses an API key: add a new key to it",
                args.host_alias
            )
        };
        return Err(IpcError::new(codes::E_INVALID, problem.clone())
            .with_details(problems("profile", &problem)));
    }
    run_tmux(tmux, &start_login_script(profile), "open the login pane").await?;
    Ok(())
}

/// The keys `login_key` presses: the CLI's numbered choices and the few
/// keys its screens take, as tmux names. Nothing typed as text.
const LOGIN_KEYS: [&str; 14] = [
    "1", "2", "3", "4", "5", "6", "7", "8", "9", "Enter", "Up", "Down", "Escape", "Tab",
];

async fn login_key(args: &AddAccountArgs, tmux: &dyn TmuxExec) -> Result<(), IpcError> {
    let key = args.key.as_deref().unwrap_or("");
    let Some(key) = LOGIN_KEYS.iter().find(|k| **k == key) else {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("key must be one of {}", LOGIN_KEYS.join(", ")),
        ));
    };
    let target = quote(&crate::tmux::exact_pane(&login_session_name(
        args.pane_id(),
    )));
    run_tmux(
        tmux,
        &format!("tmux send-keys -t {target} {key}"),
        "press the key",
    )
    .await?;
    Ok(())
}

/// Marks a profile directory [`start_login_script`] made itself, so
/// [`end_login_script`] may take it away again when its login never
/// completed. A directory that was there before is never marked.
const NEW_LOGIN_MARK: &str = ".fleet-new-login";

/// PURE: close the login pane; then, for a directory this flow made, keep
/// it once it holds a login (a `.credentials.json`, or an `oauthAccount`
/// in its `.claude.json` where the token is in the Keychain) and only drop
/// the mark, else remove it, so a cancelled + Add account leaves no "not
/// logged in" profile behind in every login picker (review 2026-11-03 D11).
fn end_login_script(profile: &str) -> String {
    let exact = quote(&crate::tmux::exact_session(&login_session_name(profile)));
    format!(
        "tmux kill-session -t {exact} 2>/dev/null; \
         d=\"$HOME/.claude-profiles/\"{p}; \
         if [ -e \"$d/{NEW_LOGIN_MARK}\" ]; then \
           if [ -e \"$d/.credentials.json\" ] || grep -q oauthAccount \"$d/.claude.json\" 2>/dev/null; \
           then rm -f \"$d/{NEW_LOGIN_MARK}\"; else rm -rf -- \"$d\"; fi; \
         fi; true",
        p = quote(profile),
    )
}

async fn end_login(args: &AddAccountArgs, tmux: &dyn TmuxExec) -> Result<(), IpcError> {
    let script = match args.login_dir() {
        Some(profile) => end_login_script(profile),
        None => format!(
            "tmux kill-session -t {} 2>/dev/null; true",
            quote(&crate::tmux::exact_session(&login_session_name(OWN_LOGIN)))
        ),
    };
    run_tmux(tmux, &script, "close the login pane").await?;
    Ok(())
}

async fn login_status(
    args: &AddAccountArgs,
    store: &Mutex<Store>,
    tmux: &dyn TmuxExec,
    now: i64,
) -> Result<LoginStatus, IpcError> {
    if args.again {
        return login_status_again(args, store, tmux, now).await;
    }
    let profiles = tmux.read_profiles().await;
    let account_uuid = {
        let s = lock(store)?;
        crate::service::hosts::sync_host_profiles(&s, &args.host_alias, profiles.as_deref())?
            .remove(&args.profile)
            .flatten()
    };
    let command = login_command(Some(&args.profile));
    if let Some(uuid) = account_uuid {
        let email = lock(store)?
            .get_account_by_uuid(&uuid)?
            .and_then(|a| a.email);
        return Ok(LoginStatus {
            logged_in: true,
            account_uuid: Some(uuid),
            email,
            pane: None,
            sign_in_url: None,
            command,
        });
    }
    // No pane (closed, or never opened): the dialog offers Start again and
    // the command to run there.
    let (pane, sign_in_url) = match tmux.capture_pane(&login_session_name(&args.profile)).await {
        Ok(text) => (Some(pane_tail(&text)), sign_in_url(&text)),
        Err(_) => (None, None),
    };
    Ok(LoginStatus {
        logged_in: false,
        account_uuid: None,
        email: None,
        pane,
        sign_in_url,
        command,
    })
}

/// `login_status { again }`: the login exists already, so its account
/// says nothing; the pane saying the sign-in went through does. Then the
/// login's signed-out note goes (`login_health`), so its card clears and
/// placement takes it again without waiting for a usage read.
async fn login_status_again(
    args: &AddAccountArgs,
    store: &Mutex<Store>,
    tmux: &dyn TmuxExec,
    now: i64,
) -> Result<LoginStatus, IpcError> {
    let dir = args.login_dir();
    let command = login_command(dir);
    let text = tmux
        .capture_pane(&login_session_name(args.pane_id()))
        .await
        .ok();
    let done = text.as_deref().is_some_and(signed_in_again);
    if !done {
        return Ok(LoginStatus {
            logged_in: false,
            account_uuid: None,
            email: None,
            pane: text.as_deref().map(pane_tail),
            sign_in_url: text.as_deref().and_then(sign_in_url),
            command,
        });
    }
    let s = lock(store)?;
    crate::service::login_health::record(
        &s,
        &[crate::service::login_health::Observation {
            host_alias: args.host_alias.clone(),
            profile: dir.map(str::to_string),
            signed_out: false,
        }],
        now,
    )?;
    let host = s.get_host_row(&args.host_alias)?;
    let account_uuid = match dir {
        None => host.and_then(|h| h.account_uuid),
        Some(p) => host
            .and_then(|h| h.claude_profiles)
            .unwrap_or_default()
            .into_iter()
            .find(|x| x.name == p)
            .and_then(|x| x.account_uuid),
    };
    let email = match account_uuid.as_deref() {
        Some(u) => s.get_account_by_uuid(u)?.and_then(|a| a.email),
        None => None,
    };
    Ok(LoginStatus {
        logged_in: true,
        account_uuid,
        email,
        pane: None,
        sign_in_url: None,
        command,
    })
}

/// PURE: whether the login pane says the sign-in went through.
pub fn signed_in_again(pane: &str) -> bool {
    pane.to_lowercase().contains(LOGIN_SUCCESS)
}

/// PURE: the pane's last [`PANE_LINES`] lines, trailing blanks dropped.
pub fn pane_tail(text: &str) -> String {
    let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
    let end = lines
        .iter()
        .rposition(|l| !l.trim().is_empty())
        .map_or(0, |i| i + 1);
    let start = end.saturating_sub(PANE_LINES);
    lines[start..end].join("\n")
}

/// PURE: the first sign-in link on screen. The CLI prints it wider than the
/// pane, so a link is followed onto the next lines while they are URL text
/// alone. Only an Anthropic host (`claude.ai`, `claude.com`,
/// `anthropic.com` and their subdomains) over https counts.
pub fn sign_in_url(text: &str) -> Option<String> {
    let url_char = |c: char| c.is_ascii_alphanumeric() || "-._~:/?#[]@!$&'()*+,;=%".contains(c);
    let lines: Vec<&str> = text.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let Some(at) = line.find("https://") else {
            continue;
        };
        let mut url: String = line[at..].chars().take_while(|c| url_char(*c)).collect();
        let ran_to_edge = line[at..].trim_end().chars().all(url_char);
        if ran_to_edge {
            for next in &lines[i + 1..] {
                let t = next.trim();
                if t.is_empty() || !t.chars().all(url_char) {
                    break;
                }
                url.push_str(t);
            }
        }
        if anthropic_https(&url) {
            return Some(url);
        }
    }
    None
}

fn anthropic_https(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    ["claude.ai", "claude.com", "anthropic.com"]
        .iter()
        .any(|d| host == *d || host.ends_with(&format!(".{d}")))
}

/// PURE: a sign-in code as the page hands it out: printable URL-safe text,
/// no spaces, at most 512 characters.
fn checked_code(code: Option<&str>) -> Result<&str, IpcError> {
    let code = code.map(str::trim).unwrap_or("");
    let ok = !code.is_empty()
        && code.len() <= 512
        && code
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-._~#".contains(c));
    if ok {
        Ok(code)
    } else {
        Err(IpcError::new(
            codes::E_INVALID,
            "that is not a sign-in code: paste the code the page shows",
        )
        .with_details(problems("code", "Paste the code the sign-in page shows.")))
    }
}

async fn login_code(args: &AddAccountArgs, ssh: &dyn SshExec) -> Result<(), IpcError> {
    let code = checked_code(args.code.as_deref())?;
    let name = login_session_name(args.pane_id());
    // The code goes on stdin into a tmux buffer, pasted without bracketing
    // (the login prompt is a plain text field), then Enter.
    let target = quote(&crate::tmux::exact_pane(&name));
    let script = format!(
        "b=fleet-login-$$; tmux load-buffer -b \"$b\" - && tmux paste-buffer -d -b \"$b\" -t {target} \
         && sleep 0.2 && tmux send-keys -t {target} Enter || {{ tmux delete-buffer -b \"$b\" 2>/dev/null; exit 1; }}"
    );
    let quoted = quote(&script);
    let out = ssh
        .run_with_stdin(
            &args.host_alias,
            &["bash", "-lc", quoted.as_str()],
            code.as_bytes().to_vec(),
            CONNECT_TIMEOUT,
            WRITE_WALL_CLOCK,
            64 * 1024,
        )
        .await
        .map_err(|e| {
            unsupported_hint(
                e,
                "run the command there and paste the code in that terminal",
            )
        })?;
    if !out.status.success() {
        return Err(IpcError::new(
            codes::E_TMUX,
            format!(
                "could not type the code into the login pane {name}: {}",
                first_line(&out.stderr)
            ),
        ));
    }
    Ok(())
}

/// PURE: an API key's shape: `sk-ant-`, then URL-safe text, 20 to 300
/// characters in all. Checked before anything leaves this process.
fn checked_key(key: Option<&str>) -> Result<&str, IpcError> {
    let key = key.map(str::trim).unwrap_or("");
    let ok = key.starts_with("sk-ant-")
        && (20..=300).contains(&key.len())
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        Ok(key)
    } else {
        Err(IpcError::new(
            codes::E_INVALID,
            "that is not an Anthropic API key (sk-ant-…)",
        )
        .with_details(problems(
            "api_key",
            "An Anthropic API key starts with sk-ant-.",
        )))
    }
}

/// PURE: a daily limit: `None` or 0 clears it; else finite, positive and at
/// most [`MAX_DAILY_LIMIT_USD`].
fn checked_limit(limit: Option<f64>) -> Result<Option<f64>, IpcError> {
    match limit {
        None => Ok(None),
        Some(0.0) => Ok(None),
        Some(l) if l.is_finite() && l > 0.0 && l <= MAX_DAILY_LIMIT_USD => {
            Ok(Some((l * 100.0).round() / 100.0))
        }
        Some(_) => Err(IpcError::new(
            codes::E_INVALID,
            format!("daily limit must be between 0 and {MAX_DAILY_LIMIT_USD} USD"),
        )
        .with_details(problems(
            "daily_limit",
            &format!("A daily limit is 0 (none) to {MAX_DAILY_LIMIT_USD} USD."),
        ))),
    }
}

fn problems(field: &str, problem: &str) -> serde_json::Value {
    serde_json::json!({ "problems": [{ "field": field, "problem": problem }] })
}

/// The account uuid an API key is listed under: `apikey-` and the first 16
/// hex of its SHA-256, so the same key on two hosts is one account. The
/// hash of a key this long reveals nothing usable about it.
pub fn api_key_account_uuid(key: &str) -> String {
    let sha = crate::mcp::auth::sha256_hex(key);
    format!("{API_KEY_ACCOUNT_PREFIX}{}", &sha[..16])
}

/// Ask Anthropic whether `key` works. A 401 / 403 is the provider's own
/// refusal, on the field.
pub async fn verify_key(http: &dyn HttpTransport, key: &str) -> Result<(), IpcError> {
    let req = Request::get(MODELS_URL)
        .header("x-api-key", key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .with_timeout(Duration::from_secs(20));
    let resp = match http.send(req).await {
        Ok(r) => r,
        Err(TransportError::Timeout) => {
            return Err(IpcError::new(
                codes::E_TIMEOUT,
                "Anthropic did not answer in time; try again",
            ))
        }
        Err(e) => {
            return Err(IpcError::new(
                codes::E_PROBE,
                format!(
                    "could not reach Anthropic to check the key: {}",
                    scrub(&e.to_string(), key)
                ),
            ))
        }
    };
    if resp.is_success() {
        return Ok(());
    }
    let said = provider_message(&resp.text(), key);
    match resp.status {
        401 | 403 => {
            let problem = format!(
                "Anthropic rejected this key ({}{}). Check that it is not revoked.",
                resp.status,
                said.as_deref()
                    .map(|m| format!(": {m}"))
                    .unwrap_or_default()
            );
            Err(IpcError::new(codes::E_INVALID, problem.clone())
                .with_details(problems("api_key", &problem)))
        }
        s => Err(IpcError::new(
            codes::E_PROBE,
            format!(
                "Anthropic answered {s}{} while checking the key; try again",
                said.as_deref()
                    .map(|m| format!(" ({m})"))
                    .unwrap_or_default()
            ),
        )),
    }
}

/// PURE: `error.message` of Anthropic's error body, one short line with no
/// control characters and never the key itself.
fn provider_message(body: &str, key: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let m = v.get("error")?.get("message")?.as_str()?;
    let clean: String = m.chars().filter(|c| !c.is_control()).take(160).collect();
    let clean = scrub(clean.trim(), key);
    (!clean.is_empty()).then_some(clean)
}

fn scrub(text: &str, key: &str) -> String {
    if key.is_empty() {
        text.to_string()
    } else {
        text.replace(key, "<key>")
    }
}

fn first_line(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("no output")
        .chars()
        .take(200)
        .collect()
}

fn unsupported_hint(e: IpcError, instead: &str) -> IpcError {
    if e.code == codes::E_UNSUPPORTED {
        IpcError::new(codes::E_UNSUPPORTED, format!("{}; {instead}", e.message))
    } else {
        e
    }
}

/// The script writing an API-key profile. Reads the key, the account line
/// and the fresh `.claude.json` from stdin, one line each. Refuses a
/// profile that is already a `/login` profile (it has `.claude.json` and no
/// key), so a key never shadows a subscription login.
fn write_script(profile: &str) -> String {
    format!(
        "umask 077; d=\"$HOME/.claude-profiles/\"{p}; \
         if [ -e \"$d/.claude.json\" ] && [ ! -e \"$d/{key}\" ]; then echo {busy}; exit 3; fi; \
         mkdir -p \"$d\" || exit 1; \
         IFS= read -r k || exit 1; IFS= read -r acct || exit 1; IFS= read -r cj || exit 1; \
         printf '%s\\n' \"$k\" > \"$d/{key}.tmp\" && chmod 600 \"$d/{key}.tmp\" && mv -f \"$d/{key}.tmp\" \"$d/{key}\" || exit 1; \
         printf '%s\\n' \"$acct\" > \"$d/{acct}\" || exit 1; \
         if [ ! -e \"$d/.claude.json\" ]; then printf '%s\\n' \"$cj\" > \"$d/.claude.json\" || exit 1; fi; \
         echo {ok}",
        p = quote(profile),
        key = API_KEY_FILE,
        acct = API_ACCOUNT_FILE,
        busy = LOGIN_PROFILE_MARK,
        ok = OK_MARK,
    )
}

/// The stdin [`write_script`] reads: the key, the account, the config.
/// `customApiKeyResponses` approves the key (by its last 20 characters, as
/// Claude Code itself records an approval) so the first session does not
/// stop to ask; `hasCompletedOnboarding` skips the theme and login screens.
fn write_stdin(key: &str, account: &serde_json::Value) -> Vec<u8> {
    let tail: String = key
        .chars()
        .rev()
        .take(20)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let config = serde_json::json!({
        "hasCompletedOnboarding": true,
        "customApiKeyResponses": { "approved": [tail], "rejected": [] },
    });
    format!("{key}\n{account}\n{config}\n").into_bytes()
}

async fn add_api_key(
    args: &AddAccountArgs,
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    http: &dyn HttpTransport,
    now: i64,
) -> Result<ApiKeyAdded, IpcError> {
    let key = checked_key(args.api_key.as_deref())?;
    let limit = checked_limit(args.daily_limit_usd)?;
    let nickname = args
        .nickname
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty());
    if nickname.is_some_and(|n| n.chars().count() > 32) {
        return Err(
            IpcError::new(codes::E_INVALID, "nickname must be 32 characters or fewer")
                .with_details(problems("nickname", "32 characters at most.")),
        );
    }
    if let Some(uuid) = profile_account(store, &args.host_alias, &args.profile)? {
        if !is_api_key_account(&uuid) {
            return Err(login_profile_taken(args));
        }
    }
    verify_key(http, key).await?;

    let uuid = api_key_account_uuid(key);
    let display = nickname
        .map(str::to_string)
        .unwrap_or_else(|| format!("API key · {}", args.profile));
    let account = serde_json::json!({
        "accountUuid": uuid,
        "displayName": display,
        "organizationName": "Anthropic API",
        "seatTier": "api_key",
    });
    let script = quote(&write_script(&args.profile));
    let out = ssh
        .run_with_stdin(
            &args.host_alias,
            &["bash", "-lc", script.as_str()],
            write_stdin(key, &account),
            CONNECT_TIMEOUT,
            WRITE_WALL_CLOCK,
            64 * 1024,
        )
        .await
        .map_err(|e| {
            unsupported_hint(
                e,
                "this host cannot take a key from fleet; use a subscription login there",
            )
        })?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    if stdout.contains(LOGIN_PROFILE_MARK) {
        return Err(login_profile_taken(args));
    }
    if !out.status.success() || !stdout.contains(OK_MARK) {
        return Err(IpcError::new(
            codes::E_HOST_WRITE,
            format!(
                "could not write the key to {}: {}",
                args.host_alias,
                scrub(&first_line(&out.stderr), key)
            ),
        ));
    }

    let s = lock(store)?;
    let row = crate::store::AccountRow {
        uuid: uuid.clone(),
        email: None,
        display_name: Some(display),
        organization_name: Some("Anthropic API".into()),
        organization_uuid: None,
        seat_tier: Some("api_key".into()),
        last_seen_at: Some(now),
        nickname: None,
        has_extra_usage: false,
        forbidden_project_ids: Vec::new(),
        forbidden_org_ids: Vec::new(),
    };
    s.upsert_account(&row)?;
    if nickname.is_some() {
        s.set_account_nickname(&uuid, nickname)?;
    }
    let mut profiles = s
        .get_host_row(&args.host_alias)?
        .and_then(|h| h.claude_profiles)
        .unwrap_or_default();
    profiles.retain(|p| p.name != args.profile);
    profiles.push(crate::store::HostProfileRow {
        name: args.profile.clone(),
        account_uuid: Some(uuid.clone()),
        email: None,
        credential: None,
    });
    profiles.sort_by(|a, b| a.name.cmp(&b.name));
    s.set_host_profiles(&args.host_alias, &profiles)?;
    // Only a limit the request carries is written (0 clears it): the same
    // key added on another host is the same account, and leaving the field
    // empty there must not erase the limit its owner set.
    if args.daily_limit_usd.is_some() {
        set_daily_limit(&s, &uuid, limit)?;
    }
    let limit = daily_limit_usd(&s, &uuid);
    Ok(ApiKeyAdded {
        host_alias: args.host_alias.clone(),
        profile: args.profile.clone(),
        account_uuid: uuid,
        daily_limit_usd: limit,
    })
}

/// PURE: a setup token's shape: `sk-ant-oat`, then URL-safe text, 40 to
/// 400 characters in all. Checked before anything leaves this process.
fn checked_setup_token(token: Option<&str>) -> Result<&str, IpcError> {
    let token = token.map(str::trim).unwrap_or("");
    let ok = token.starts_with("sk-ant-oat")
        && (40..=400).contains(&token.len())
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        Ok(token)
    } else {
        let problem = if token.starts_with("sk-ant-api") {
            "That is an API key; pick API key above, or paste what claude setup-token printed."
        } else {
            "Paste the token claude setup-token printed (sk-ant-oat…)."
        };
        Err(IpcError::new(
            codes::E_INVALID,
            "that is not a Claude setup token (sk-ant-oat…)",
        )
        .with_details(problems("setup_token", problem)))
    }
}

/// Send `req`; a transport failure is a timeout or "could not reach", with
/// `secret` scrubbed from whatever it says.
async fn send_to_anthropic(
    http: &dyn HttpTransport,
    req: Request,
    secret: &str,
    what: &str,
) -> Result<crate::net::https::Response, IpcError> {
    match http.send(req).await {
        Ok(r) => Ok(r),
        Err(TransportError::Timeout) => Err(IpcError::new(
            codes::E_TIMEOUT,
            "Anthropic did not answer in time; try again",
        )),
        Err(e) => Err(IpcError::new(
            codes::E_PROBE,
            format!(
                "could not reach Anthropic to check the {what}: {}",
                scrub(&e.to_string(), secret)
            ),
        )),
    }
}

/// PURE: the account Anthropic's `/api/oauth/profile` names, as the
/// `.fleet-account.json` line carries it. `None` without an account uuid.
fn profile_identity(body: &str) -> Option<crate::service::hosts::OauthAccount> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let text = |v: Option<&serde_json::Value>| {
        v.and_then(|x| x.as_str())
            .map(|x| {
                x.chars()
                    .filter(|c| !c.is_control())
                    .take(200)
                    .collect::<String>()
            })
            .filter(|x| !x.trim().is_empty())
    };
    let account = v.get("account")?;
    let org = v.get("organization");
    Some(crate::service::hosts::OauthAccount {
        uuid: Some(text(account.get("uuid"))?),
        email: text(account.get("email")),
        display_name: text(account.get("display_name")).or_else(|| text(account.get("full_name"))),
        organization_name: text(org.and_then(|o| o.get("name"))),
        organization_uuid: text(org.and_then(|o| o.get("uuid"))),
        ..Default::default()
    })
}

/// Ask Anthropic whether a setup token works, and whose it is when the
/// token may say (`Ok(Some)`): its profile first, then, when the token's
/// scopes do not reach the profile (a `setup-token` token is usually
/// inference only), `GET /v1/models`. A 401 / 403 there is the provider's
/// own refusal, on the field. The token goes to Anthropic only, in the
/// `Authorization` header.
pub async fn verify_setup_token(
    http: &dyn HttpTransport,
    token: &str,
) -> Result<Option<crate::service::hosts::OauthAccount>, IpcError> {
    let bearer = format!("Bearer {token}");
    let profile = Request::get(PROFILE_URL)
        .header("authorization", bearer.as_str())
        .header("anthropic-beta", OAUTH_BETA)
        .with_timeout(Duration::from_secs(20));
    let resp = send_to_anthropic(http, profile, token, "token").await?;
    if resp.is_success() {
        if let Some(who) = profile_identity(&resp.text()) {
            return Ok(Some(who));
        }
    } else if resp.status == 401 {
        return Err(token_rejected(resp.status, &resp.text(), token));
    }
    let models = Request::get(MODELS_URL)
        .header("authorization", bearer.as_str())
        .header("anthropic-beta", OAUTH_BETA)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .with_timeout(Duration::from_secs(20));
    let resp = send_to_anthropic(http, models, token, "token").await?;
    match resp.status {
        s if (200..300).contains(&s) => Ok(None),
        401 | 403 => Err(token_rejected(resp.status, &resp.text(), token)),
        s => Err(IpcError::new(
            codes::E_PROBE,
            format!(
                "Anthropic answered {s}{} while checking the token; try again",
                provider_message(&resp.text(), token)
                    .map(|m| format!(" ({m})"))
                    .unwrap_or_default()
            ),
        )),
    }
}

fn token_rejected(status: u16, body: &str, token: &str) -> IpcError {
    let problem = format!(
        "Anthropic rejected this token ({status}{}). It may be revoked or past its year; run claude setup-token again.",
        provider_message(body, token)
            .map(|m| format!(": {m}"))
            .unwrap_or_default()
    );
    IpcError::new(codes::E_INVALID, problem.clone()).with_details(problems("setup_token", &problem))
}

/// The account a setup token nobody named is listed under:
/// `setuptoken-` and the first 16 hex of its SHA-256, so the same token on
/// two hosts is one account.
pub fn setup_token_account_uuid(token: &str) -> String {
    let sha = crate::mcp::auth::sha256_hex(token);
    format!("{SETUP_TOKEN_ACCOUNT_PREFIX}{}", &sha[..16])
}

/// The script writing a setup-token profile. Reads the token, the account
/// line and the fresh `.claude.json` from stdin, one line each. Refuses a
/// profile holding an API key (exit 3); a `/login` profile keeps its own
/// `.claude.json` and login, which the token now outranks. Prints
/// `replaced` when a token was there before.
fn token_write_script(profile: &str) -> String {
    format!(
        "umask 077; d=\"$HOME/.claude-profiles/\"{p}; \
         if [ -e \"$d/{api}\" ]; then echo {busy}; exit 3; fi; \
         mkdir -p \"$d\" || exit 1; \
         IFS= read -r k || exit 1; IFS= read -r acct || exit 1; IFS= read -r cj || exit 1; \
         [ -e \"$d/{tok}\" ] && echo replaced; \
         printf '%s\\n' \"$k\" > \"$d/{tok}.tmp\" && chmod 600 \"$d/{tok}.tmp\" && mv -f \"$d/{tok}.tmp\" \"$d/{tok}\" || exit 1; \
         printf '%s\\n' \"$acct\" > \"$d/{acct}.tmp\" && mv -f \"$d/{acct}.tmp\" \"$d/{acct}\" || exit 1; \
         if [ ! -e \"$d/.claude.json\" ]; then printf '%s\\n' \"$cj\" > \"$d/.claude.json\" || exit 1; fi; \
         rm -f \"$d/{new}\"; \
         echo {ok}",
        p = quote(profile),
        api = API_KEY_FILE,
        tok = SETUP_TOKEN_FILE,
        acct = API_ACCOUNT_FILE,
        new = NEW_LOGIN_MARK,
        busy = API_KEY_PROFILE_MARK,
        ok = OK_MARK,
    )
}

/// The stdin [`token_write_script`] reads: the token, the account, the
/// config (`hasCompletedOnboarding` skips the theme and login screens).
fn token_write_stdin(token: &str, account: &crate::service::hosts::OauthAccount) -> Vec<u8> {
    let account = serde_json::to_string(account).unwrap_or_else(|_| "{}".into());
    let config = serde_json::json!({ "hasCompletedOnboarding": true });
    format!("{token}\n{account}\n{config}\n").into_bytes()
}

fn api_key_profile_taken(args: &AddAccountArgs) -> IpcError {
    let problem = format!(
        "{} already has a profile {} holding an API key; pick another name",
        args.host_alias, args.profile
    );
    IpcError::new(codes::E_EXISTS, problem.clone()).with_details(problems("profile", &problem))
}

/// Which account a setup token is listed as: the one Anthropic named, else
/// the one the person named (a known subscription account), else the
/// profile's own `/login`, else an account of its own.
fn token_identity(
    store: &Mutex<Store>,
    verified: Option<crate::service::hosts::OauthAccount>,
    named: Option<&str>,
    existing: Option<&str>,
    token: &str,
    label: &str,
) -> Result<(crate::service::hosts::OauthAccount, bool), IpcError> {
    use crate::service::hosts::OauthAccount;
    if let Some(mut who) = verified {
        who.fleet_credential = Some(SETUP_TOKEN_CREDENTIAL.into());
        return Ok((who, true));
    }
    let s = lock(store)?;
    let known = |uuid: &str| -> Result<Option<OauthAccount>, IpcError> {
        Ok(s.get_account_by_uuid(uuid)?.map(|a| OauthAccount {
            uuid: Some(a.uuid),
            email: a.email,
            display_name: a.display_name,
            organization_name: a.organization_name,
            organization_uuid: a.organization_uuid,
            fleet_credential: Some(SETUP_TOKEN_CREDENTIAL.into()),
            ..Default::default()
        }))
    };
    if let Some(uuid) = named {
        let ok = !is_api_key_account(uuid) && !is_setup_token_account(uuid);
        return match ok.then(|| known(uuid)).transpose()?.flatten() {
            Some(a) => Ok((a, true)),
            None => Err(IpcError::new(
                codes::E_INVALID,
                format!("{uuid:?} is not a Claude subscription account fleet knows"),
            )
            .with_details(problems(
                "account",
                "Pick one of the listed accounts, or none.",
            ))),
        };
    }
    if let Some(uuid) = existing.filter(|u| !is_api_key_account(u)) {
        if let Some(a) = known(uuid)? {
            return Ok((a, true));
        }
    }
    Ok((
        OauthAccount {
            uuid: Some(setup_token_account_uuid(token)),
            display_name: Some(label.to_string()),
            fleet_credential: Some(SETUP_TOKEN_CREDENTIAL.into()),
            ..Default::default()
        },
        false,
    ))
}

async fn add_setup_token(
    args: &AddAccountArgs,
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    http: &dyn HttpTransport,
    now: i64,
) -> Result<SetupTokenAdded, IpcError> {
    let token = checked_setup_token(args.setup_token.as_deref())?;
    let nickname = args
        .nickname
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty());
    if nickname.is_some_and(|n| n.chars().count() > 32) {
        return Err(
            IpcError::new(codes::E_INVALID, "nickname must be 32 characters or fewer")
                .with_details(problems("nickname", "32 characters at most.")),
        );
    }
    let named = args
        .account
        .as_deref()
        .map(str::trim)
        .filter(|a| !a.is_empty());
    if named.is_some_and(|a| a.len() > 128) {
        return Err(IpcError::new(codes::E_INVALID, "account is too long"));
    }
    let existing = profile_account(store, &args.host_alias, &args.profile)?;
    if existing.as_deref().is_some_and(is_api_key_account) {
        return Err(api_key_profile_taken(args));
    }
    let verified = verify_setup_token(http, token).await?;
    let label = nickname
        .map(str::to_string)
        .unwrap_or_else(|| format!("Setup token · {}", args.profile));
    let (account, real) =
        token_identity(store, verified, named, existing.as_deref(), token, &label)?;
    let uuid = account.uuid.clone().unwrap_or_default();

    let script = quote(&token_write_script(&args.profile));
    let out = ssh
        .run_with_stdin(
            &args.host_alias,
            &["bash", "-lc", script.as_str()],
            token_write_stdin(token, &account),
            CONNECT_TIMEOUT,
            WRITE_WALL_CLOCK,
            64 * 1024,
        )
        .await
        .map_err(|e| {
            unsupported_hint(
                e,
                "this host cannot take a token from fleet; run claude /login there",
            )
        })?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    if stdout.contains(API_KEY_PROFILE_MARK) {
        return Err(api_key_profile_taken(args));
    }
    if !out.status.success() || !stdout.contains(OK_MARK) {
        return Err(IpcError::new(
            codes::E_HOST_WRITE,
            format!(
                "could not write the token to {}: {}",
                args.host_alias,
                scrub(&first_line(&out.stderr), token)
            ),
        ));
    }
    let replaced = stdout.lines().any(|l| l.trim() == "replaced");

    let s = lock(store)?;
    if !real || s.get_account_by_uuid(&uuid)?.is_none() {
        s.upsert_account(&crate::store::AccountRow {
            uuid: uuid.clone(),
            email: account.email.clone(),
            display_name: account.display_name.clone(),
            organization_name: account.organization_name.clone(),
            organization_uuid: account.organization_uuid.clone(),
            seat_tier: None,
            last_seen_at: Some(now),
            nickname: None,
            has_extra_usage: false,
            forbidden_project_ids: Vec::new(),
            forbidden_org_ids: Vec::new(),
        })?;
    }
    if nickname.is_some() && !real {
        s.set_account_nickname(&uuid, nickname)?;
    }
    let host = s.get_host_row(&args.host_alias)?;
    let mut profiles = host
        .as_ref()
        .and_then(|h| h.claude_profiles.clone())
        .unwrap_or_default();
    profiles.retain(|p| p.name != args.profile);
    profiles.push(crate::store::HostProfileRow {
        name: args.profile.clone(),
        account_uuid: Some(uuid.clone()),
        email: account.email.clone(),
        credential: Some(SETUP_TOKEN_CREDENTIAL.into()),
    });
    profiles.sort_by(|a, b| a.name.cmp(&b.name));
    s.set_host_profiles(&args.host_alias, &profiles)?;
    // The token signs the profile in: a login health note that it was
    // signed out no longer holds.
    if let Some(h) = host {
        let before = h.signed_out_logins.len();
        let kept: Vec<_> = h
            .signed_out_logins
            .into_iter()
            .filter(|l| l.profile.as_deref() != Some(args.profile.as_str()))
            .collect();
        if kept.len() != before {
            s.set_host_signed_out_logins(&args.host_alias, &kept)?;
        }
    }
    Ok(SetupTokenAdded {
        host_alias: args.host_alias.clone(),
        profile: args.profile.clone(),
        account_uuid: uuid,
        email: account.email,
        replaced,
    })
}

fn login_profile_taken(args: &AddAccountArgs) -> IpcError {
    let problem = format!(
        "{} already has a login profile {}; pick another name",
        args.host_alias, args.profile
    );
    IpcError::new(codes::E_EXISTS, problem.clone()).with_details(problems("profile", &problem))
}

/// Every daily limit, by account.
pub fn daily_limits(s: &Store) -> std::collections::BTreeMap<String, f64> {
    s.get_setting(DAILY_LIMITS_KEY)
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_default()
}

/// The daily limit of `account_uuid`, in USD.
pub fn daily_limit_usd(s: &Store, account_uuid: &str) -> Option<f64> {
    daily_limits(s)
        .get(account_uuid)
        .copied()
        .filter(|l| l.is_finite() && *l > 0.0)
}

/// Set (or with `None`, clear) the daily limit of `account_uuid`.
pub fn set_daily_limit(s: &Store, account_uuid: &str, limit: Option<f64>) -> Result<(), IpcError> {
    let mut all = daily_limits(s);
    match limit {
        Some(l) => all.insert(account_uuid.to_string(), l),
        None => all.remove(account_uuid),
    };
    let json = serde_json::to_string(&all)
        .map_err(|e| IpcError::new(codes::E_SERIALIZE, e.to_string()))?;
    s.set_setting(DAILY_LIMITS_KEY, &json)?;
    Ok(())
}

/// The HTTPS transport `verify_key` uses in production: Anthropic's API
/// host and nothing else.
pub fn anthropic_transport() -> crate::net::https::DirectTransport {
    crate::net::https::DirectTransport::new(std::sync::Arc::new(|h: &str| h == ANTHROPIC_HOST))
}

#[cfg(test)]
#[path = "add_account_tests.rs"]
mod tests;
