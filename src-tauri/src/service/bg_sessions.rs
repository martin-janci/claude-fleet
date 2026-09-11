//! Service functions for Claude CLI background-session operations.

use crate::claude_cli;
use crate::ipc_error::IpcError;
use crate::ssh::SshClient;
use crate::store::Store;
use crate::validate;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

pub use crate::claude_cli::PurgeReport;

// ─── args / result types ─────────────────────────────────────────────────────
//
// Every arg struct validates the host alias (it becomes an `ssh` operand) and
// every value that becomes a `claude` positional / option value (a leading
// `-` would be read as a flag). `claude_cli` re-checks the same rules when it
// builds the script, so DevTools / MCP callers cannot bypass them.

#[derive(Debug, Deserialize)]
pub struct NewBgSessionArgs {
    pub host_alias: String,
    pub name: String,
    pub prompt: String,
}

impl NewBgSessionArgs {
    pub fn validate(&self) -> Result<(), IpcError> {
        validate::host_alias(&self.host_alias)?;
        validate::not_option_like("session name", &self.name)?;
        if self.name.chars().any(|c| c.is_control()) {
            return Err(IpcError::new(
                "E_INVALID",
                "session name must not contain control characters",
            ));
        }
        // The prompt lands after `--` (see `claude_cli::bg_script`), so a
        // leading `-` is fine — only blank prompts are rejected.
        validate::not_blank("prompt", &self.prompt)?;
        Ok(())
    }
}

#[derive(Debug, Serialize)]
pub struct NewBgSessionResult {
    pub claude_session_id: Option<String>,
    /// Populated when `claude --bg` ran but no session id could be parsed from
    /// its output — the session may still be live, but the fleet can't track
    /// it by id (and thus can't `peek` it). Surfaced so the caller can warn.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
    /// The fleet row (MCP-7): `new_bg_session_tracked` runs a single-host
    /// reconcile right after launch so the `bg:<id>` sentinel exists before
    /// the caller's next tool call. The key is ABSENT from the JSON (not
    /// `null`) when the agent could not be matched yet (it appears on the
    /// next tick) or when the untracked path was used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<crate::store::SessionRow>,
}

#[derive(Debug, Deserialize)]
pub struct PeekSessionArgs {
    pub host_alias: String,
    pub claude_session_id: String,
}

impl PeekSessionArgs {
    pub fn validate(&self) -> Result<(), IpcError> {
        validate::host_alias(&self.host_alias)?;
        validate::claude_session_id(&self.claude_session_id)?;
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
pub struct PurgeProjectArgs {
    /// Every host whose Claude state must go. The fleet row is deleted only
    /// after all of them succeed.
    pub host_aliases: Vec<String>,
    pub project_path: String,
    pub project_id: i64,
}

impl PurgeProjectArgs {
    pub fn validate(&self) -> Result<(), IpcError> {
        if self.host_aliases.is_empty() {
            return Err(IpcError::new(
                "E_INVALID",
                "host_aliases must name at least one host",
            ));
        }
        for host in &self.host_aliases {
            validate::host_alias(host)?;
        }
        validate::not_option_like("project_path", &self.project_path)?;
        // A relative path would resolve against the remote $HOME.
        if !self.project_path.starts_with('/') {
            return Err(IpcError::new("E_INVALID", "project_path must be absolute"));
        }
        if self.project_path.chars().any(|c| c.is_control()) {
            return Err(IpcError::new(
                "E_INVALID",
                "project_path must not contain control characters",
            ));
        }
        Ok(())
    }
}

// ─── service functions ───────────────────────────────────────────────────────

pub async fn new_bg_session(
    args: NewBgSessionArgs,
    ssh: &Arc<SshClient>,
) -> Result<NewBgSessionResult, IpcError> {
    args.validate()?;
    let claude_session_id =
        claude_cli::claude_bg(ssh, &args.host_alias, &args.name, &args.prompt).await?;
    Ok(bg_session_result(claude_session_id))
}

/// Warning surfaced when `claude --bg` succeeded but its output didn't yield a
/// parseable session id.
pub const BG_NO_ID_WARNING: &str = "could not parse session id from claude --bg output";

/// Build the `new_bg_session` result, attaching a warning when no session id
/// was parsed. Pure so the warn-on-null decision is unit-testable without the
/// (SSH/local) `claude --bg` exec.
fn bg_session_result(claude_session_id: Option<String>) -> NewBgSessionResult {
    let warning = if claude_session_id.is_none() {
        Some(BG_NO_ID_WARNING.to_string())
    } else {
        None
    };
    NewBgSessionResult {
        claude_session_id,
        warning,
        session: None,
    }
}

/// `new_bg_session` + immediate registration (MCP-7): after `claude --bg`
/// returns its id, reconcile the host once (as `spawn_review` does) so the
/// synthetic `bg:<id>` row exists, then stamp the row with the launch prompt
/// (`last_prompt`, `started_at`, and a prompt-derived friendly name — PROD-4).
/// Every post-launch step is best-effort: the agent is already running, so a
/// reconcile hiccup degrades to `session: None` rather than an error.
pub async fn new_bg_session_tracked(
    args: NewBgSessionArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<NewBgSessionResult, IpcError> {
    let host_alias = args.host_alias.clone();
    let prompt = args.prompt.clone();
    let mut res = new_bg_session(args, ssh).await?;
    let Some(ref claude_id) = res.claude_session_id else {
        return Ok(res);
    };
    if let Err(e) = crate::service::sessions::reconcile_one_host(store, ssh, &host_alias).await {
        eprintln!("[bg] post-launch reconcile of {host_alias} failed: {e}");
        return Ok(res);
    }
    res.session = stamp_bg_row(store, claude_id, &prompt);
    Ok(res)
}

/// Find the bg row for `claude_id` and record the launch prompt on it.
/// Returns the refreshed row, or `None` when reconcile has not surfaced the
/// agent yet.
fn stamp_bg_row(
    store: &Mutex<Store>,
    claude_id: &str,
    prompt: &str,
) -> Option<crate::store::SessionRow> {
    let s = store.lock().ok()?;
    let row = s.get_session_by_claude_id(claude_id).ok().flatten()?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let _ = s.set_started_at(row.id, now);
    let _ = s.set_last_prompt(row.id, prompt);
    if row.friendly_name.is_none() {
        if let Some(name) = crate::service::sessions::friendly_name_from_prompt(prompt) {
            let _ = s.set_friendly_name(&row.host_alias, &row.tmux_name, Some(&name));
        }
    }
    let _ = s.insert_session_event(
        row.id,
        "prompt_sent",
        Some(&prompt.chars().take(120).collect::<String>()),
    );
    s.get_session_by_id(row.id).ok().flatten()
}

/// Resolve a `peek_session` target (MCP-7) from any of: a fleet `session_id`,
/// or a `claude_session_id` plus its `host_alias` (the id a `new_bg_session`
/// caller already holds, before reconcile has surfaced the row). Returns the
/// `(host_alias, claude_session_id)` pair to peek.
///
/// A fleet row without a Claude id yields `E_INVALID_STATE` so the caller can
/// tell "not tracked yet" apart from "no such session" (`E_NOTFOUND`).
pub fn resolve_peek_target(
    s: &Store,
    session_id: Option<i64>,
    host_alias: Option<&str>,
    claude_session_id: Option<&str>,
) -> Result<(String, String), IpcError> {
    if let Some(id) = session_id {
        let row = s
            .get_session_by_id(id)?
            .ok_or_else(|| IpcError::new("E_NOTFOUND", format!("session {id} not found")))?;
        return match row.claude_session_id {
            Some(cid) => Ok((row.host_alias, cid)),
            None => Err(IpcError::new(
                "E_INVALID_STATE",
                "this session has no Claude session id yet — nothing to peek",
            )),
        };
    }
    match (host_alias, claude_session_id) {
        (Some(host), Some(cid)) => {
            validate::host_alias(host)?;
            validate::claude_session_id(cid)?;
            Ok((host.to_string(), cid.to_string()))
        }
        // A bare Claude id: use the fleet row's host when the agent is
        // already tracked; otherwise the host is genuinely unknown.
        (None, Some(cid)) => {
            validate::claude_session_id(cid)?;
            match s.get_session_by_claude_id(cid)? {
                Some(row) => Ok((row.host_alias, cid.to_string())),
                None => Err(IpcError::new(
                    "E_INVALID",
                    "pass host_alias with claude_session_id (the agent is not tracked yet)",
                )),
            }
        }
        _ => Err(IpcError::new(
            "E_INVALID",
            "pass session_id, or claude_session_id (+ host_alias)",
        )),
    }
}

pub async fn peek_session(args: PeekSessionArgs, ssh: &Arc<SshClient>) -> Result<String, IpcError> {
    args.validate()?;
    claude_cli::claude_logs(ssh, &args.host_alias, &args.claude_session_id).await
}

/// Purge Claude Code state for a project on every host in `host_aliases`,
/// then delete the fleet row. The row (and its session rows) is deleted only
/// when every host succeeded — purged, or held no state. On any failure it is
/// kept, so the purge can be retried and the host list re-derived from the
/// project's sessions.
pub async fn purge_project(
    args: PurgeProjectArgs,
    store: &Arc<Mutex<Store>>,
    ssh: &Arc<SshClient>,
) -> Result<Vec<PurgeReport>, IpcError> {
    purge_project_with(args, store, |host, path| {
        let ssh = Arc::clone(ssh);
        async move { claude_cli::claude_purge_project(&ssh, &host, &path).await }
    })
    .await
}

/// [`purge_project`] with the per-host purge injected, so the
/// all-or-nothing row deletion is testable without ssh or a real `claude`.
async fn purge_project_with<F, Fut>(
    args: PurgeProjectArgs,
    store: &Mutex<Store>,
    purge: F,
) -> Result<Vec<PurgeReport>, IpcError>
where
    F: Fn(String, String) -> Fut,
    Fut: std::future::Future<Output = Result<PurgeReport, IpcError>>,
{
    // Syntax (validate::host_alias) and path checks before anything runs.
    args.validate()?;
    // Syntax is not enough: only registered hosts may be reached over ssh.
    // `local` never goes through ssh and has no guaranteed hosts row.
    {
        let s = store.lock().map_err(|_| IpcError::lock())?;
        for host in args.host_aliases.iter().filter(|h| h.as_str() != "local") {
            if s.get_host_row(host).map_err(IpcError::from)?.is_none() {
                return Err(IpcError::new("E_NOTFOUND", format!("unknown host: {host}")));
            }
        }
    }
    let mut reports = Vec::with_capacity(args.host_aliases.len());
    for host in &args.host_aliases {
        reports.push(purge(host.clone(), args.project_path.clone()).await?);
    }
    let s = store.lock().map_err(|_| IpcError::lock())?;
    s.delete_project(args.project_id)?;
    Ok(reports)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use std::sync::Arc;

    fn make_store() -> Arc<Mutex<Store>> {
        Arc::new(Mutex::new(Store::open_in_memory().unwrap()))
    }

    #[test]
    fn new_bg_session_args_validates_empty_prompt() {
        let _ = make_store(); // ensure it compiles; tests only need validate()
        let args = NewBgSessionArgs {
            host_alias: "local".into(),
            name: "test-session".into(),
            prompt: "".into(),
        };
        assert!(args.validate().is_err());
    }

    #[test]
    fn new_bg_session_args_validates_empty_name() {
        let args = NewBgSessionArgs {
            host_alias: "local".into(),
            name: "".into(),
            prompt: "Do the thing".into(),
        };
        assert!(args.validate().is_err());
    }

    #[test]
    fn stamp_bg_row_names_and_stamps_the_reconciled_row() {
        let store = make_store();
        {
            let s = store.lock().unwrap();
            s.upsert_host("local").unwrap();
            s.upsert_bg_session("local", "bg:u1", None, "u1", Some("working"), 5)
                .unwrap();
        }
        let row = stamp_bg_row(&store, "u1", "Review the auth PR, carefully!").expect("row");
        assert_eq!(
            row.friendly_name.as_deref(),
            Some("review the auth pr carefully")
        );
        assert_eq!(
            row.last_prompt.as_deref(),
            Some("Review the auth PR, carefully!")
        );
        assert!(row.started_at.is_some());
        // Unknown id ⇒ None, no panic.
        assert!(stamp_bg_row(&store, "nope", "x").is_none());
    }

    #[test]
    fn resolve_peek_target_accepts_fleet_id_or_claude_id() {
        let store = make_store();
        let (tracked_id, untracked_id) = {
            let s = store.lock().unwrap();
            s.upsert_host("local").unwrap();
            let bg = s
                .upsert_bg_session(
                    "local",
                    &format!("bg:{UUID}"),
                    None,
                    UUID,
                    Some("working"),
                    5,
                )
                .unwrap();
            let plain = s
                .upsert_session("dev-plain", "local", None, None, 1, 1, "running", None)
                .unwrap();
            (bg, plain)
        };
        let s = store.lock().unwrap();
        assert_eq!(
            resolve_peek_target(&s, Some(tracked_id), None, None).unwrap(),
            ("local".to_string(), UUID.to_string())
        );
        assert_eq!(
            resolve_peek_target(&s, Some(untracked_id), None, None)
                .unwrap_err()
                .code,
            "E_INVALID_STATE"
        );
        assert_eq!(
            resolve_peek_target(&s, Some(999), None, None)
                .unwrap_err()
                .code,
            "E_NOTFOUND"
        );
        // Claude id alone resolves through the tracked row …
        assert_eq!(
            resolve_peek_target(&s, None, None, Some(UUID)).unwrap().0,
            "local"
        );
        // … an untracked id needs its host …
        let other = "6ba7b810-9dad-11d1-80b4-00c04fd430c8";
        assert_eq!(
            resolve_peek_target(&s, None, None, Some(other))
                .unwrap_err()
                .code,
            "E_INVALID"
        );
        assert_eq!(
            resolve_peek_target(&s, None, Some("mefistos"), Some(other)).unwrap(),
            ("mefistos".to_string(), other.to_string())
        );
        // … and garbage is rejected before any lookup.
        assert_eq!(
            resolve_peek_target(&s, None, Some("local"), Some("--foo"))
                .unwrap_err()
                .code,
            "E_INVALID"
        );
        assert_eq!(
            resolve_peek_target(&s, None, None, None).unwrap_err().code,
            "E_INVALID"
        );
    }

    #[test]
    fn bg_session_result_warns_when_id_missing() {
        let res = bg_session_result(None);
        assert!(res.claude_session_id.is_none());
        assert_eq!(res.warning.as_deref(), Some(BG_NO_ID_WARNING));
    }

    #[test]
    fn bg_session_result_no_warning_when_id_present() {
        let res = bg_session_result(Some("abc-123".into()));
        assert_eq!(res.claude_session_id.as_deref(), Some("abc-123"));
        assert!(res.warning.is_none());
    }

    #[test]
    fn peek_session_args_validates_missing_session_id() {
        let args = PeekSessionArgs {
            host_alias: "local".into(),
            claude_session_id: "".into(),
        };
        assert!(args.validate().is_err());
    }

    const UUID: &str = "550e8400-e29b-41d4-a716-446655440000";

    #[test]
    fn new_bg_session_args_rejects_option_like_values_and_bad_host() {
        let ok = NewBgSessionArgs {
            host_alias: "mefistos".into(),
            name: "review-1".into(),
            prompt: "Summarise the diff".into(),
        };
        assert!(ok.validate().is_ok());

        // A prompt may start with `-` (markdown list): bg_script emits `--`
        // before it, so it can never be parsed as a flag.
        let dash_prompt = NewBgSessionArgs {
            prompt: "- fix login\n- add test".into(),
            ..ok
        };
        assert!(dash_prompt.validate().is_ok());

        let blank_prompt = NewBgSessionArgs {
            prompt: "   ".into(),
            ..dash_prompt
        };
        assert_eq!(blank_prompt.validate().unwrap_err().code, "E_INVALID");

        let bad_name = NewBgSessionArgs {
            name: "-n".into(),
            prompt: "fine".into(),
            ..blank_prompt
        };
        assert_eq!(bad_name.validate().unwrap_err().code, "E_INVALID");

        let ctrl_name = NewBgSessionArgs {
            name: "a\nb".into(),
            ..bad_name
        };
        assert_eq!(ctrl_name.validate().unwrap_err().code, "E_INVALID");

        let bad_host = NewBgSessionArgs {
            host_alias: "-oProxyCommand=id".into(),
            name: "ok".into(),
            ..ctrl_name
        };
        assert_eq!(bad_host.validate().unwrap_err().code, "E_INVALID");
    }

    #[test]
    fn peek_session_args_requires_uuid_id_and_valid_host() {
        let ok = PeekSessionArgs {
            host_alias: "local".into(),
            claude_session_id: UUID.into(),
        };
        assert!(ok.validate().is_ok());
        for bad in ["--foo", "-h", "abc-123", "'; rm -rf / #"] {
            let args = PeekSessionArgs {
                host_alias: "local".into(),
                claude_session_id: bad.into(),
            };
            assert_eq!(args.validate().unwrap_err().code, "E_INVALID", "{bad:?}");
        }
        let bad_host = PeekSessionArgs {
            host_alias: "-tt".into(),
            claude_session_id: UUID.into(),
        };
        assert_eq!(bad_host.validate().unwrap_err().code, "E_INVALID");
    }

    fn purge_args(hosts: &[&str], path: &str, project_id: i64) -> PurgeProjectArgs {
        PurgeProjectArgs {
            host_aliases: hosts.iter().map(|h| h.to_string()).collect(),
            project_path: path.into(),
            project_id,
        }
    }

    #[test]
    fn purge_project_args_validates_hosts_and_requires_an_absolute_path() {
        assert!(purge_args(&["local"], "/home/me/projects/x", 1)
            .validate()
            .is_ok());
        assert!(purge_args(&["local", "box"], "/x", 1).validate().is_ok());
        for bad in ["", "  ", "--all", "-rf", "/a\nb", "rel/p", "~/p", "./p"] {
            let err = purge_args(&["local"], bad, 1).validate().unwrap_err();
            assert_eq!(err.code, "E_INVALID", "{bad:?}");
        }
        for hosts in [&[][..], &["has space"][..], &["local", "-tt"][..]] {
            let err = purge_args(hosts, "/x", 1).validate().unwrap_err();
            assert_eq!(err.code, "E_INVALID", "{hosts:?}");
        }
    }

    /// A project with one session on each of `hosts`; returns its id.
    fn seed_project(store: &Mutex<Store>, hosts: &[&str]) -> i64 {
        let s = store.lock().unwrap();
        let pid = s.upsert_project("o", "r", "/home/u/p/r").unwrap();
        for (i, host) in hosts.iter().enumerate() {
            s.upsert_host(host).unwrap();
            s.upsert_session(
                &format!("dev-{i}"),
                host,
                Some(pid),
                None,
                1,
                1,
                "running",
                None,
            )
            .unwrap();
        }
        pid
    }

    /// (project row still present, session rows left on `hosts`).
    fn project_state(store: &Mutex<Store>, pid: i64, hosts: &[&str]) -> (bool, usize) {
        let s = store.lock().unwrap();
        let exists = s.list_projects().unwrap().iter().any(|p| p.id == pid);
        let sessions = hosts
            .iter()
            .map(|h| s.list_sessions_for_host(h).unwrap().len())
            .sum();
        (exists, sessions)
    }

    fn report_for(host: &str, path: &str) -> PurgeReport {
        PurgeReport {
            host_alias: host.into(),
            logical_path: path.into(),
            physical_path: Some(path.into()),
            purged: vec![path.into()],
            not_found: vec![],
        }
    }

    #[tokio::test]
    async fn purge_project_keeps_row_and_sessions_when_a_later_host_fails() {
        let store = make_store();
        let pid = seed_project(&store, &["alpha", "beta"]);
        let calls = Mutex::new(Vec::new());
        let err = purge_project_with(
            purge_args(&["alpha", "beta"], "/home/u/p/r", pid),
            &store,
            |host, path| {
                calls.lock().unwrap().push(host.clone());
                async move {
                    if host == "beta" {
                        Err(IpcError::new("E_CLAUDE_CLI", "claude CLI failed on beta"))
                    } else {
                        Ok(report_for(&host, &path))
                    }
                }
            },
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "E_CLAUDE_CLI");
        assert_eq!(*calls.lock().unwrap(), vec!["alpha", "beta"]);
        assert_eq!(project_state(&store, pid, &["alpha", "beta"]), (true, 2));
    }

    #[tokio::test]
    async fn purge_project_deletes_row_only_after_every_host_succeeds() {
        let store = make_store();
        let pid = seed_project(&store, &["alpha", "beta"]);
        let reports = purge_project_with(
            purge_args(&["alpha", "beta"], "/home/u/p/r", pid),
            &store,
            |host, path| async move { Ok(report_for(&host, &path)) },
        )
        .await
        .unwrap();
        let hosts: Vec<_> = reports.iter().map(|r| r.host_alias.as_str()).collect();
        assert_eq!(hosts, vec!["alpha", "beta"]);
        assert_eq!(project_state(&store, pid, &["alpha", "beta"]), (false, 0));
    }

    #[tokio::test]
    async fn purge_project_rejects_invalid_or_unknown_hosts_before_purging() {
        let store = make_store();
        let pid = seed_project(&store, &["alpha"]);
        let cases: [(&[&str], &str); 4] = [
            (&["-oProxyCommand=id"], "E_INVALID"),
            (&["has space"], "E_INVALID"),
            (&["alpha", "a;b"], "E_INVALID"),
            (&["alpha", "ghost"], "E_NOTFOUND"),
        ];
        let called = std::sync::atomic::AtomicBool::new(false);
        for (hosts, code) in cases {
            let err = purge_project_with(purge_args(hosts, "/home/u/p/r", pid), &store, |_, _| {
                called.store(true, std::sync::atomic::Ordering::SeqCst);
                async { Err::<PurgeReport, _>(IpcError::new("E_TEST", "must not run")) }
            })
            .await
            .unwrap_err();
            assert_eq!(err.code, code, "{hosts:?}");
        }
        assert!(!called.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(project_state(&store, pid, &["alpha"]), (true, 1));
    }

    #[tokio::test]
    async fn purge_project_rejects_invalid_host_and_keeps_the_project() {
        let store = make_store();
        let pid = store
            .lock()
            .unwrap()
            .upsert_project("o", "r", "/home/u/p/r")
            .unwrap();
        let ssh = Arc::new(SshClient::new());
        for bad in ["-oProxyCommand=id", "has space", "", "a;b", "x\ny"] {
            let err = purge_project(
                PurgeProjectArgs {
                    host_aliases: vec![bad.into()],
                    project_path: "/home/u/p/r".into(),
                    project_id: pid,
                },
                &store,
                &ssh,
            )
            .await
            .unwrap_err();
            assert_eq!(err.code, "E_INVALID", "{bad:?}");
        }
        let projects = store.lock().unwrap().list_projects().unwrap();
        assert!(projects.iter().any(|p| p.id == pid), "row must survive");
    }
}
