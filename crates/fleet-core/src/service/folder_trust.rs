//! Folder trust follows a session to another login (docs/accounts.md,
//! *Folder trust*).
//!
//! Claude Code asks "Is this a project you created or one you trust?" once
//! per directory and records the answer per CONFIG:
//! `projects["<abs dir>"].hasTrustDialogAccepted` in that config's
//! `.claude.json`. A session under a login profile runs with
//! `CLAUDE_CONFIG_DIR` at `~/.claude-profiles/<name>`
//! (`tmux::ClaudeLaunch`), so it reads that directory's file and not the
//! host's `~/.claude.json`. A session relaunched under another login
//! therefore met the dialog again, in a directory the person had trusted
//! long ago, and waited there (2026-11-17, mefistos: 24 sessions an account
//! switch moved to a new profile, each stopped at the dialog for hours).
//!
//! [`carry`] copies the answer, and only an answer the person already gave
//! on that host: the trusted path is read from the config the session is
//! leaving or from the host's own, and written for that same path into the
//! config it is about to run under. A directory no config there trusts is
//! left alone: the dialog is then the person's to answer.

use crate::ipc_error::IpcError;
use crate::service::provision::{login_config, read_host_file, rewrite_login_config};
use crate::ssh::SshExec;

/// The key Claude Code writes the person's answer under.
const TRUST_KEY: &str = "hasTrustDialogAccepted";

/// `path` without its trailing slashes (`/` stays `/`).
fn trimmed(path: &str) -> &str {
    match path.trim_end_matches('/') {
        "" if path.starts_with('/') => "/",
        t => t,
    }
}

/// Whether trust of `root` covers `dir`: the same directory, or one under it.
fn covers(root: &str, dir: &str) -> bool {
    let (root, dir) = (trimmed(root), trimmed(dir));
    root.starts_with('/')
        && (root == dir
            || root == "/"
            || dir
                .strip_prefix(root)
                .is_some_and(|rest| rest.starts_with('/')))
}

/// PURE: the path `claude_json` records as trusted that covers `dir`: `dir`
/// itself or its nearest ancestor with `hasTrustDialogAccepted: true`
/// (trust of a directory covers what is under it, so a trusted checkout
/// covers its `.worktrees/*`). The key is returned as the file spells it.
/// `None` for a file that does not parse, has no such entry, or a `dir`
/// that is not an absolute POSIX path.
pub fn trusted_root(claude_json: &str, dir: &str) -> Option<String> {
    if !dir.starts_with('/') {
        return None;
    }
    let root: serde_json::Value = serde_json::from_str(claude_json).ok()?;
    root.get("projects")?
        .as_object()?
        .iter()
        .filter(|(_, entry)| entry.get(TRUST_KEY) == Some(&serde_json::Value::Bool(true)))
        .map(|(path, _)| path)
        .filter(|path| covers(path, dir))
        .max_by_key(|path| trimmed(path).len())
        .cloned()
}

/// PURE: `existing` with `dir` recorded as trusted, the way Claude Code
/// records the person's own answer, and nothing else: every sibling key
/// stays, and no `.mcp.json` server is approved
/// (`operator::pre_trust_claude_json` does that for Control's own
/// directory). `E_PROVISION` for a file that is not the JSON object Claude
/// Code writes, before the caller writes anything.
pub fn trust_claude_json(existing: &str, dir: &str) -> Result<String, IpcError> {
    use crate::service::operator::json_object_entry;
    let mut root = crate::service::provision::parse_claude_json(existing)?;
    let projects = json_object_entry(&mut root, "projects")?;
    let project = json_object_entry(projects, dir)?
        .as_object_mut()
        .expect("json_object_entry returns an object");
    let yes = serde_json::Value::Bool(true);
    if project.get(TRUST_KEY) == Some(&yes) {
        return Ok(existing.to_string());
    }
    project.insert(TRUST_KEY.to_string(), yes);
    serde_json::to_string_pretty(&root).map_err(|e| {
        IpcError::new(
            crate::ipc_error::codes::E_PROVISION,
            format!("serialize: {e}"),
        )
    })
}

/// Before a session starts in `dir` on `host` under the login `entering`
/// (`None` = the host's own): when that login's config does not trust
/// `dir`, and the config of the login it is `leaving` or the host's own
/// does, record the same trusted path in `entering`'s config. The path it
/// wrote, or `None` when there was nothing to carry.
pub async fn carry(
    ssh: &dyn SshExec,
    host: &str,
    dir: &str,
    leaving: Option<&str>,
    entering: Option<&str>,
) -> Result<Option<String>, IpcError> {
    // A name reaches a path below: checked first, whoever stored it.
    for profile in [leaving, entering].into_iter().flatten() {
        crate::validate::claude_profile(profile)?;
    }
    if !dir.starts_with('/') {
        return Ok(None);
    }
    // Whose answer may be carried: the login it leaves, then the host's
    // own. Never the one it enters, and with none left there is nothing
    // to ask the host.
    let mut sources: Vec<Option<&str>> = Vec::new();
    for login in [leaving, None] {
        if login != entering && !sources.contains(&login) {
            sources.push(login);
        }
    }
    if sources.is_empty() {
        return Ok(None);
    }
    let target = read_host_file(ssh, host, &login_config(entering).1).await?;
    if trusted_root(&target, dir).is_some() {
        return Ok(None);
    }
    let mut root = None;
    for login in sources {
        let file = login_config(login).1;
        match read_host_file(ssh, host, &file).await {
            Ok(text) => {
                if let Some(found) = trusted_root(&text, dir) {
                    root = Some(found);
                    break;
                }
            }
            // One config that cannot be read does not hide the other's answer.
            Err(e) => tracing::debug!(host, file = %file, "[trust] not read: {}", e.message),
        }
    }
    let Some(root) = root else {
        return Ok(None);
    };
    // Merged into the file as it is at the write, not as it was read above:
    // a live session under that login rewrites it all the time.
    let wrote = rewrite_login_config(ssh, host, entering, &|existing| {
        if trusted_root(existing, dir).is_some() {
            return Ok(None);
        }
        trust_claude_json(existing, &root).map(Some)
    })
    .await?;
    Ok(wrote.then_some(root))
}

/// [`carry`] for a launcher: best-effort. A config that cannot be read,
/// merged or written leaves the dialog for the person and never stops the
/// launch.
pub async fn carry_for_launch(
    ssh: &dyn SshExec,
    host: &str,
    dir: &str,
    leaving: Option<&str>,
    entering: Option<&str>,
) {
    match carry(ssh, host, dir, leaving, entering).await {
        Ok(Some(root)) => tracing::info!(
            host,
            dir,
            login = entering.unwrap_or("(host)"),
            trusted = %root,
            "[trust] the folder's trust followed the session to this login"
        ),
        Ok(None) => {}
        Err(e) => tracing::warn!(
            host,
            dir,
            login = entering.unwrap_or("(host)"),
            "[trust] the folder's trust was not carried to this login: {}",
            e.message
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc_error::codes;
    use crate::service::provision::remote_read_script;
    use crate::shell::quote;
    use crate::ssh_fake::{Call, FakeSsh, Match, Reply};

    const HOME: &str = "/home/fake";
    const REPO: &str = "/mnt/sda4/projects/papayapos-backend";
    const HOST_JSON: &str = "~/.claude.json";

    fn profile_json(name: &str) -> String {
        format!("~/.claude-profiles/{name}/.claude.json")
    }

    /// A host whose files read as given (a missing one reads empty).
    fn host_with(files: &[(&str, &str)]) -> FakeSsh {
        let fake = FakeSsh::new();
        fake.with_home(HOME);
        for (path, body) in files {
            fake.on(Match::script(&remote_read_script(path)), Reply::ok(body));
        }
        fake
    }

    /// What was uploaded to `abs` (the secret writer streams the content to
    /// `<path>.fleet-tmp` and renames it).
    fn uploaded(fake: &FakeSsh, abs: &str) -> Option<String> {
        let cmd = format!("cat > {}", quote(abs));
        fake.calls()
            .iter()
            .find(|c| c.command() == cmd)
            .and_then(Call::stdin_str)
    }

    fn trusted(dir: &str) -> String {
        serde_json::json!({
            "oauthAccount": { "emailAddress": "host@example.com" },
            "projects": { dir: { "hasTrustDialogAccepted": true, "allowedTools": ["Bash"] } }
        })
        .to_string()
    }

    #[test]
    fn the_trusted_root_is_the_directory_or_its_nearest_trusted_ancestor() {
        let json = serde_json::json!({ "projects": {
            "/mnt/sda4/projects": { "hasTrustDialogAccepted": false },
            REPO: { "hasTrustDialogAccepted": true },
            "/mnt/sda4/projects/papayapos-backend-old": { "hasTrustDialogAccepted": true },
            "/srv": { "hasTrustDialogAccepted": true },
            "/srv/app/": { "hasTrustDialogAccepted": true },
        }})
        .to_string();
        assert_eq!(trusted_root(&json, REPO).as_deref(), Some(REPO));
        assert_eq!(
            trusted_root(&json, &format!("{REPO}/.worktrees/pd-1")).as_deref(),
            Some(REPO),
            "a trusted checkout covers its worktrees"
        );
        assert_eq!(
            trusted_root(&json, &format!("{REPO}/")).as_deref(),
            Some(REPO)
        );
        // The nearest one, spelled as the file spells it.
        assert_eq!(
            trusted_root(&json, "/srv/app/api").as_deref(),
            Some("/srv/app/")
        );
        assert_eq!(trusted_root(&json, "/srv/other").as_deref(), Some("/srv"));
        // A sibling that only shares a prefix is not an ancestor, and an
        // entry that says no is no.
        assert_eq!(
            trusted_root(&json, "/mnt/sda4/projects/papayapos-backend-new"),
            None
        );
        assert_eq!(trusted_root(&json, "/mnt/sda4/projects/other"), None);
        // Nothing to go on.
        assert_eq!(trusted_root("", REPO), None);
        assert_eq!(trusted_root("not json", REPO), None);
        assert_eq!(trusted_root(r#"{"projects":[]}"#, REPO), None);
        assert_eq!(trusted_root(&json, "relative/dir"), None);
        assert_eq!(trusted_root(&json, ""), None);
    }

    #[test]
    fn trusting_a_directory_changes_that_one_answer_and_nothing_else() {
        let existing = serde_json::json!({
            "hasCompletedOnboarding": true,
            "projects": {
                "/other": { "hasTrustDialogAccepted": false, "allowedTools": ["Bash"] },
                REPO: { "hasTrustDialogAccepted": false, "lastCost": 1.5 }
            }
        })
        .to_string();
        let out = trust_claude_json(&existing, REPO).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["projects"][REPO]["hasTrustDialogAccepted"], true);
        assert_eq!(v["projects"][REPO]["lastCost"], 1.5);
        assert!(
            v["projects"][REPO].get("enabledMcpjsonServers").is_none(),
            "no .mcp.json server is approved on the person's behalf: {out}"
        );
        assert_eq!(v["projects"]["/other"]["hasTrustDialogAccepted"], false);
        assert_eq!(v["hasCompletedOnboarding"], true);
        assert_eq!(trust_claude_json(&out, REPO).unwrap(), out, "idempotent");

        let fresh = trust_claude_json("", REPO).unwrap();
        let v: serde_json::Value = serde_json::from_str(&fresh).unwrap();
        assert_eq!(v["projects"][REPO]["hasTrustDialogAccepted"], true);

        for bad in ["not json", "[]", r#"{"projects": []}"#] {
            assert_eq!(
                trust_claude_json(bad, REPO).unwrap_err().code,
                codes::E_PROVISION,
                "{bad}"
            );
        }
    }

    /// 2026-11-17, mefistos: the profile `tech_chuck` was minutes old, its
    /// `.claude.json` held only `hasCompletedOnboarding`, and an account
    /// switch relaunched 24 sessions under it. Each stopped at the trust
    /// dialog although the host's own config (and the login they left)
    /// trusted the checkout. The trust the host already has goes with them.
    #[tokio::test]
    async fn a_session_relaunched_under_a_new_login_takes_the_hosts_trust_with_it() {
        let own = r#"{"hasCompletedOnboarding":true}"#;
        let fake = host_with(&[
            (HOST_JSON, &trusted(REPO)),
            (&profile_json("tech_chuck"), own),
        ]);
        let dir = format!("{REPO}/.worktrees/pd-3167");
        let carried = carry(&fake, "mefistos", &dir, Some("work"), Some("tech_chuck"))
            .await
            .unwrap();
        assert_eq!(carried.as_deref(), Some(REPO), "the path the host trusts");

        let file = format!("{HOME}/.claude-profiles/tech_chuck/.claude.json");
        let written = uploaded(&fake, &format!("{file}.fleet-tmp")).expect("the profile's config");
        let v: serde_json::Value = serde_json::from_str(&written).unwrap();
        assert_eq!(v["projects"][REPO]["hasTrustDialogAccepted"], true);
        assert_eq!(v["hasCompletedOnboarding"], true, "the rest of it stays");
        assert!(
            v["projects"][REPO].get("allowedTools").is_none(),
            "only the answer to the dialog is carried: {written}"
        );
        assert!(v.get("oauthAccount").is_none(), "{written}");
        assert_eq!(
            uploaded(&fake, &format!("{file}.fleet-bak.fleet-tmp")).as_deref(),
            Some(own),
            "backed up first"
        );
    }

    /// The config the session is leaving is asked too: a checkout the
    /// person trusted only under their other login.
    #[tokio::test]
    async fn the_login_a_session_leaves_is_a_source_of_trust_too() {
        let fake = host_with(&[
            (HOST_JSON, r#"{"projects":{}}"#),
            (&profile_json("work"), &trusted(REPO)),
        ]);
        let carried = carry(&fake, "h1", REPO, Some("work"), Some("tech_chuck"))
            .await
            .unwrap();
        assert_eq!(carried.as_deref(), Some(REPO));
        let file = format!("{HOME}/.claude-profiles/tech_chuck/.claude.json.fleet-tmp");
        assert!(uploaded(&fake, &file).is_some());

        // And back to the host's own login, which never saw the dialog.
        let back = host_with(&[(&profile_json("work"), &trusted(REPO))]);
        let carried = carry(&back, "h1", REPO, Some("work"), None).await.unwrap();
        assert_eq!(carried.as_deref(), Some(REPO));
        let written = uploaded(&back, &format!("{HOME}/.claude.json.fleet-tmp")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&written).unwrap();
        assert_eq!(v["projects"][REPO]["hasTrustDialogAccepted"], true);
    }

    fn wrote_nothing(fake: &FakeSsh) -> bool {
        fake.calls()
            .iter()
            .all(|c| !c.is_upload() && !c.command().contains(".fleet-"))
    }

    /// Fleet never answers the dialog for the person: a directory no config
    /// on the host trusts is left alone.
    #[tokio::test]
    async fn a_directory_nobody_trusted_on_the_host_is_left_for_the_person() {
        let fake = host_with(&[
            (HOST_JSON, &trusted("/somewhere/else")),
            (&profile_json("work"), &trusted("/another/place")),
            (&profile_json("tech_chuck"), "{}"),
        ]);
        let carried = carry(&fake, "h1", REPO, Some("work"), Some("tech_chuck"))
            .await
            .unwrap();
        assert_eq!(carried, None);
        assert!(wrote_nothing(&fake), "{:?}", fake.commands());
    }

    /// A login that trusts the directory already is not written again, and
    /// a start under the host's own login with nothing left behind reads
    /// nothing at all.
    #[tokio::test]
    async fn a_login_that_trusts_it_already_is_not_rewritten() {
        let fake = host_with(&[
            (HOST_JSON, &trusted(REPO)),
            (&profile_json("tech_chuck"), &trusted(REPO)),
        ]);
        let dir = format!("{REPO}/.worktrees/x");
        assert_eq!(
            carry(&fake, "h1", &dir, None, Some("tech_chuck"))
                .await
                .unwrap(),
            None
        );
        assert!(wrote_nothing(&fake), "{:?}", fake.commands());

        let plain = host_with(&[(HOST_JSON, &trusted(REPO))]);
        assert_eq!(carry(&plain, "h1", REPO, None, None).await.unwrap(), None);
        assert!(plain.calls().is_empty(), "{:?}", plain.commands());
        // A directory that is not an absolute path: nothing to ask either.
        assert_eq!(
            carry(&plain, "h1", "not/absolute", Some("work"), Some("b"))
                .await
                .unwrap(),
            None
        );
        assert!(plain.calls().is_empty(), "{:?}", plain.commands());
    }

    /// A profile name never reaches a path unchecked, and a config that
    /// does not parse is refused before anything is written; the launcher's
    /// wrapper swallows both.
    #[tokio::test]
    async fn a_bad_name_or_a_broken_config_writes_nothing_and_stops_nothing() {
        let fake = host_with(&[(HOST_JSON, &trusted(REPO))]);
        let err = carry(&fake, "h1", REPO, None, Some("../evil"))
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::E_INVALID, "{err:?}");
        assert!(fake.calls().is_empty(), "{:?}", fake.commands());

        let broken = host_with(&[
            (HOST_JSON, &trusted(REPO)),
            (&profile_json("tech_chuck"), "{ not json"),
        ]);
        let err = carry(&broken, "h1", REPO, None, Some("tech_chuck"))
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::E_PROVISION, "{err:?}");
        assert!(wrote_nothing(&broken), "{:?}", broken.commands());
        // Best-effort: returns, having logged.
        carry_for_launch(&broken, "h1", REPO, None, Some("tech_chuck")).await;
        assert!(wrote_nothing(&broken));
    }
}
