//! Session environment at launch (spec 2026-10-11-session-env-and-secrets-design.md).
//! The merged set is written to a 0600 nonce file on the host and the pane
//! command sources it; no value is ever in an argv. Files are deleted by
//! the tick sweep after ENV_FILE_MAX_AGE_MIN (several launchers may run one
//! pane command, so the pane does not delete its own file).

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Instant;

use crate::ipc_error::{codes, lock, IpcError};
use crate::service::forms::{sweep_backoff_delay, SweepBackoff};
use crate::ssh::SshExec;
use crate::store::{
    check_env_name, check_env_value, now_unix, read_credential_ref, EnvSource, Secret, Store,
};

pub const ENV_DIR: &str = "~/.claude-fleet/env";
pub const ENV_FILES_HOSTS_KEY: &str = "internal.env_files_hosts";
pub const ENV_FILE_MAX_AGE_MIN: u32 = 10;
const MAX_SET_BYTES: usize = 256 * 1024;

#[derive(Debug)]
pub struct EnvFile {
    lines: Vec<(String, Secret)>,
}

impl EnvFile {
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn render(&self) -> String {
        let mut s = String::from("# claude-fleet session environment; removed by the sweep\n");
        for (name, v) in &self.lines {
            s.push_str(&format!(
                "export {name}={}\n",
                crate::shell::quote(v.expose())
            ));
        }
        s
    }
}

/// The person whose `person` scope a launch sources: the session row's
/// owner at launch time when the row exists (restart, recreate, repair,
/// move), else `owner` — the owner a create path is about to record (a new
/// session, a review). An unclaimed session has none, and a disabled
/// person's identity is not lent to anything still running under their
/// name.
fn launch_person(
    s: &Store,
    session: Option<i64>,
    owner: Option<i64>,
) -> Result<Option<i64>, IpcError> {
    let row = match session {
        Some(id) => s.get_session_by_id(id)?,
        None => None,
    };
    let person = match row {
        Some(row) => row.owner_person_id,
        None => owner,
    };
    let Some(id) = person else {
        return Ok(None);
    };
    Ok(s.get_person(id)?
        .filter(|p| p.disabled_at.is_none())
        .map(|p| p.id))
}

pub fn resolve(
    store: &Mutex<Store>,
    host: &str,
    project: Option<&str>,
    owner: Option<i64>,
    session: Option<i64>,
    extra: &BTreeMap<String, String>,
) -> Result<EnvFile, IpcError> {
    for (n, v) in extra {
        check_env_name(n)?;
        check_env_value(v)?;
    }
    // Read rows under the lock; resolve references (file/env reads) after it.
    let rows = {
        let s = lock(store)?;
        let person = launch_person(&s, session, owner)?;
        s.env_for_launch(host, project, person, session)?
    };
    let mut merged: BTreeMap<String, Secret> = BTreeMap::new();
    for (name, src) in rows {
        let v = match src {
            EnvSource::Value(v) => v,
            EnvSource::Ref(r) => {
                let kind = r.split_once(':').map(|(k, _)| k).unwrap_or("ref");
                match read_credential_ref(&r) {
                    Some(v) => {
                        // The message names the variable, never the value.
                        if let Err(e) = check_env_value(&v) {
                            return Err(IpcError::new(
                                codes::E_INVALID,
                                format!("{name}: its {kind}: reference: {}", e.message),
                            ));
                        }
                        Secret::new(v)
                    }
                    None => {
                        return Err(IpcError::new(
                            codes::E_ENV_UNRESOLVED,
                            format!("{name}: its {kind}: reference resolved to nothing"),
                        ))
                    }
                }
            }
        };
        merged.insert(name, v);
    }
    for (n, v) in extra {
        merged.insert(n.clone(), Secret::new(v.clone()));
    }
    let total: usize = merged.iter().map(|(n, v)| n.len() + v.expose().len()).sum();
    if total > MAX_SET_BYTES {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("the session's environment is {total} bytes; at most {MAX_SET_BYTES}"),
        ));
    }
    Ok(EnvFile {
        lines: merged.into_iter().collect(),
    })
}

/// Prefix `pane_cmd` so it sources `tilde_path` (`~/...`). Only the path is
/// in the result.
pub fn wrap(tilde_path: &str, pane_cmd: &str) -> String {
    let rest = tilde_path
        .strip_prefix("~/")
        .expect("env file paths are under ~/");
    format!(
        "__fleet_env_file=\"$HOME\"/{}; if [ -r \"$__fleet_env_file\" ]; then . \"$__fleet_env_file\"; fi; unset __fleet_env_file; {pane_cmd}",
        crate::shell::quote(rest)
    )
}

fn project_slug(store: &Mutex<Store>, project_id: Option<i64>) -> Result<Option<String>, IpcError> {
    let Some(id) = project_id else {
        return Ok(None);
    };
    Ok(lock(store)?
        .get_project(id)?
        .map(|p| format!("{}/{}", p.owner, p.repo)))
}

/// The hosts an env file was written to, each with when `prepare` last
/// recorded it (unix seconds): the settings row `ENV_FILES_HOSTS_KEY` holds
/// `{"<host>": <noted_at>}`.
type NotedHosts = BTreeMap<String, i64>;

fn parse_noted(raw: Option<String>) -> NotedHosts {
    let Some(raw) = raw else {
        return NotedHosts::new();
    };
    match serde_json::from_str::<serde_json::Value>(&raw) {
        Ok(serde_json::Value::Object(m)) => m
            .into_iter()
            .filter_map(|(h, t)| t.as_i64().map(|t| (h, t)))
            .collect(),
        // The record's first shape: a list of hosts, noted at no time.
        Ok(serde_json::Value::Array(a)) => a
            .into_iter()
            .filter_map(|v| v.as_str().map(|h| (h.to_string(), 0)))
            .collect(),
        _ => NotedHosts::new(),
    }
}

fn read_noted(s: &Store) -> NotedHosts {
    parse_noted(s.get_setting(ENV_FILES_HOSTS_KEY).ok().flatten())
}

fn write_noted(s: &Store, hosts: &NotedHosts) -> rusqlite::Result<()> {
    if hosts.is_empty() {
        s.delete_setting(ENV_FILES_HOSTS_KEY)
    } else {
        s.set_setting(
            ENV_FILES_HOSTS_KEY,
            &serde_json::to_string(hosts).unwrap_or_default(),
        )
    }
}

/// Record that `host` holds (or is about to hold) an env file, as of now.
fn note_host(store: &Mutex<Store>, host: &str) -> Result<(), IpcError> {
    let s = lock(store)?;
    let mut hosts = read_noted(&s);
    hosts.insert(host.to_string(), now_unix());
    write_noted(&s, &hosts)?;
    Ok(())
}

/// What stays recorded after a sweep that began at `started` and found the
/// hosts in `clean` holding no file: every host noted now, minus the clean
/// ones — unless `prepare` noted one of them again since the sweep began,
/// because its new file landed after the sweep looked.
fn hosts_after_sweep(current: NotedHosts, clean: &[String], started: i64) -> NotedHosts {
    current
        .into_iter()
        .filter(|(h, noted_at)| !(clean.contains(h) && *noted_at < started))
        .collect()
}

/// In-process only: a restart tries every host once more, which is fine.
static ENV_SWEEP_BACKOFF: std::sync::LazyLock<SweepBackoff> =
    std::sync::LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));

/// Delete env files older than ENV_FILE_MAX_AGE_MIN on every recorded host;
/// forget a host once its directory holds no file, or once it is no longer
/// a host at all. A host that does not answer stays recorded and is tried
/// again after a growing delay (the forms sweep's: 10 min, doubling, at most
/// 6 h). Returns the number of hosts swept.
pub async fn sweep_env_files(store: &Mutex<Store>, ssh: &dyn SshExec) -> usize {
    sweep_env_files_at(store, ssh, &ENV_SWEEP_BACKOFF, Instant::now()).await
}

async fn sweep_env_files_at(
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    backoff: &SweepBackoff,
    at: Instant,
) -> usize {
    let started = now_unix();
    let read = match lock(store) {
        Ok(s) => read_noted(&s),
        Err(_) => return 0,
    };
    if read.is_empty() {
        return 0;
    }
    let script = format!(
        "d=\"$HOME\"/'.claude-fleet/env'; [ -d \"$d\" ] || {{ echo clean; exit 0; }}; \
         find \"$d\" -type f -name '*.sh' -mmin +{ENV_FILE_MAX_AGE_MIN} -exec rm -f {{}} +; \
         ls -A \"$d\" | grep -q . && echo left || echo clean"
    );
    let mut swept = 0;
    let mut clean = Vec::new();
    for host in read.keys() {
        // The host was removed: there is nothing left to reach.
        let gone = host != crate::service::projects::LOCAL_HOST
            && lock(store)
                .and_then(|s| Ok(s.get_host_row(host)?))
                .is_ok_and(|h| h.is_none());
        if gone {
            tracing::debug!(%host, "env file sweep: host is gone; forgetting it");
            clean.push(host.clone());
            continue;
        }
        let waiting = backoff
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(host)
            .is_some_and(|(next, _)| *next > at);
        if waiting {
            continue;
        }
        let outcome =
            crate::ssh::run_shell(ssh, host, &script, std::time::Duration::from_secs(20)).await;
        let failure = match outcome {
            Ok(out) if out.status.success() => {
                swept += 1;
                backoff
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(host);
                if !String::from_utf8_lossy(&out.stdout)
                    .trim_end()
                    .ends_with("left")
                {
                    clean.push(host.clone());
                }
                None
            }
            Ok(out) => Some(format!("exit {:?}", out.status.code())),
            Err(e) => Some(e.code.to_string()),
        };
        if let Some(why) = failure {
            let mut map = backoff.lock().unwrap_or_else(|e| e.into_inner());
            let failures = map.get(host).map_or(0, |(_, n)| *n).saturating_add(1);
            let delay = sweep_backoff_delay(failures);
            tracing::debug!(%host, %why, ?delay, "env file sweep deferred");
            map.insert(host.clone(), (at + delay, failures));
        }
    }
    if let Ok(s) = lock(store) {
        // Re-read under the lock: a `prepare` may have recorded a host while
        // the sweep was on the wire, and that record wins over a clean find.
        let keep = hosts_after_sweep(read_noted(&s), &clean, started);
        let _ = write_noted(&s, &keep);
    }
    swept
}

/// Resolve a launch's environment and write nothing: the refusal `prepare`
/// would give, early. A move runs it among its opening checks, so a dry run
/// predicts it and a real move refuses before any work is carried.
pub fn check(
    store: &Mutex<Store>,
    host: &str,
    project_id: Option<i64>,
    session: Option<i64>,
) -> Result<(), IpcError> {
    let project = project_slug(store, project_id)?;
    resolve(
        store,
        host,
        project.as_deref(),
        None,
        session,
        &BTreeMap::new(),
    )
    .map(|_| ())
}

/// Resolve, write the file and wrap `pane_cmd`. An empty set returns
/// `pane_cmd` unchanged and writes nothing. `owner` is the person a create
/// path records as the new row's owner; with an existing `session` row its
/// own owner is read instead ([`launch_person`]).
#[allow(clippy::too_many_arguments)]
pub async fn prepare(
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    host: &str,
    project_id: Option<i64>,
    owner: Option<i64>,
    session: Option<i64>,
    extra: &BTreeMap<String, String>,
    pane_cmd: &str,
) -> Result<String, IpcError> {
    let project = project_slug(store, project_id)?;
    let file = resolve(store, host, project.as_deref(), owner, session, extra)?;
    if file.is_empty() {
        return Ok(pane_cmd.to_string());
    }
    let path = format!("{ENV_DIR}/{}.sh", uuid::Uuid::new_v4().simple());
    // Record the host first: a file written but never swept is the leak.
    note_host(store, host)?;
    crate::service::provision::write_host_file_secret(ssh, host, ENV_DIR, &path, &file.render())
        .await?;
    // And again once the file is there: a sweep that looked at this host
    // between the two notes found it clean, and this note is what stops
    // it forgetting the host under the new file.
    note_host(store, host)?;
    Ok(wrap(&path, pane_cmd))
}

/// The `env_vars` tool's and the routed desktop command's arguments.
#[derive(
    Debug, Clone, Default, serde::Serialize, serde::Deserialize, rmcp::schemars::JsonSchema,
)]
#[schemars(crate = "rmcp::schemars")]
pub struct EnvVarsArgs {
    /// list | set | unset.
    pub action: String,
    /// global | host | project | person | session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// '' (global), host alias, owner/repo, ''|me (person) or session id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope_key: Option<String>,
    /// The variable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// set: the value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// set: env:NAME or file:/abs, read at launch (master only).
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub r#ref: Option<String>,
    /// set: write-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<bool>,
}

/// `env_var_set`'s arguments (Settings › Environment's Add): `env_vars set`
/// without the action, which the command supplies.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct EnvVarSetArgs {
    pub scope: String,
    #[serde(default)]
    pub scope_key: Option<String>,
    pub name: String,
    pub value: String,
    #[serde(default)]
    pub secret: Option<bool>,
}

impl EnvVarSetArgs {
    pub fn into_args(self) -> EnvVarsArgs {
        EnvVarsArgs {
            action: "set".into(),
            scope: Some(self.scope),
            scope_key: self.scope_key,
            name: Some(self.name),
            value: Some(self.value),
            r#ref: None,
            secret: self.secret,
        }
    }
}

/// `env_var_unset`'s arguments (Settings › Environment's Remove).
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct EnvVarUnsetArgs {
    pub scope: String,
    #[serde(default)]
    pub scope_key: Option<String>,
    pub name: String,
}

impl EnvVarUnsetArgs {
    pub fn into_args(self) -> EnvVarsArgs {
        EnvVarsArgs {
            action: "unset".into(),
            scope: Some(self.scope),
            scope_key: self.scope_key,
            name: Some(self.name),
            ..Default::default()
        }
    }
}

/// Who is calling [`run`], as the tool or the desktop command resolved it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EnvCaller {
    /// May set a reference (the master, or the standalone desktop): it makes
    /// the controlling process read its own files or environment at launch
    /// (review r04 S1's rule for tracker secrets).
    pub refs_allowed: bool,
    /// May list and change the `global`, `host`, `project` and `session`
    /// scopes: the master or a fleet admin's own device. (A write there also
    /// needs a trusted full device, which the tool checks.)
    pub admin: bool,
    /// The caller's own person (`people.id`): the one `person` scope it may
    /// list, set or unset. `None` for a caller that speaks for no person.
    pub person: Option<i64>,
}

/// The `scope_key` a `person`-scope call acts on: the caller's own person,
/// named as `''` / `me` or by its id. Any other person's is refused, the
/// admin's included: a person's identity (git author, `GH_TOKEN`) is theirs.
fn own_person_key(c: &EnvCaller, key: Option<&str>) -> Result<String, IpcError> {
    let Some(me) = c.person else {
        return Err(IpcError::new(
            codes::E_FORBIDDEN,
            "the person scope is a person's own, and this caller speaks for no person",
        ));
    };
    match key.unwrap_or("").trim() {
        "" | "me" => Ok(me.to_string()),
        k if k == me.to_string() => Ok(me.to_string()),
        _ => Err(IpcError::new(
            codes::E_FORBIDDEN,
            "a person's variables are their own: scope_key '' or 'me' (or your own person id)",
        )),
    }
}

/// `list` / `set` / `unset` for the tool and the desktop command. A
/// reference is accepted only with [`EnvCaller::refs_allowed`]; the scopes
/// other than `person` are an admin's ([`EnvCaller::admin`]); the `person`
/// scope is the caller's own only, listed and changed by nobody else.
pub fn run(
    store: &Mutex<Store>,
    a: &EnvVarsArgs,
    caller: &EnvCaller,
) -> Result<serde_json::Value, IpcError> {
    use crate::store::EnvValue;
    let need = |v: &Option<String>, what: &str| {
        v.clone().ok_or_else(|| {
            IpcError::new(
                codes::E_INVALID,
                format!("env_vars {} needs {what}", a.action),
            )
        })
    };
    // The scope and key a write acts on, after the person/admin rule.
    let target = |scope: &str| -> Result<String, IpcError> {
        if scope == "person" {
            own_person_key(caller, a.scope_key.as_deref())
        } else if caller.admin {
            Ok(a.scope_key.clone().unwrap_or_default())
        } else {
            Err(IpcError::new(
                codes::E_FORBIDDEN,
                "only a fleet admin changes the global, host, project and session scopes; \
                 your own variables are scope person",
            ))
        }
    };
    match a.action.as_str() {
        "list" => {
            let me = caller.person.map(|id| id.to_string());
            let rows: Vec<_> = lock(store)?
                .list_env_vars()?
                .into_iter()
                .filter(|r| {
                    if r.scope == "person" {
                        me.as_deref() == Some(r.scope_key.as_str())
                    } else {
                        caller.admin
                    }
                })
                .collect();
            Ok(serde_json::to_value(rows).unwrap_or_default())
        }
        "set" => {
            let (scope, name) = (need(&a.scope, "scope")?, need(&a.name, "name")?);
            let key = target(&scope)?;
            let refs_allowed = caller.refs_allowed;
            let v = match (&a.value, &a.r#ref) {
                (Some(_), Some(_)) | (None, None) => {
                    return Err(IpcError::new(
                        codes::E_INVALID,
                        "pass exactly one of value or ref",
                    ))
                }
                (None, Some(r)) => {
                    if !refs_allowed {
                        return Err(IpcError::new(
                            codes::E_FORBIDDEN,
                            "a device sends the value itself; a reference (env: or file: on \
                             the hub) is the hub operator's, since the hub reads it at launch",
                        ));
                    }
                    EnvValue::Ref(r.clone())
                }
                (Some(v), None) if a.secret.unwrap_or(false) => {
                    EnvValue::Secret(Secret::new(v.clone()))
                }
                (Some(v), None) => EnvValue::Plain(v.clone()),
            };
            lock(store)?.set_env_var(&scope, &key, &name, &v)?;
            Ok(serde_json::json!({
                "set": name,
                "scope": scope,
                "scope_key": key,
                "applies": "on the next start or restart",
            }))
        }
        "unset" => {
            let (scope, name) = (need(&a.scope, "scope")?, need(&a.name, "name")?);
            let key = target(&scope)?;
            let removed = lock(store)?.unset_env_var(&scope, &key, &name)?;
            Ok(serde_json::json!({ "removed": removed }))
        }
        other => Err(IpcError::new(
            codes::E_INVALID,
            format!("unknown action {other:?}: list | set | unset"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{EnvValue, Store};
    use std::sync::Mutex;

    fn store() -> Mutex<Store> {
        Mutex::new(Store::open_in_memory().unwrap())
    }

    /// A fleet admin's device: shared scopes, no references.
    const DEVICE: EnvCaller = EnvCaller {
        refs_allowed: false,
        admin: true,
        person: Some(1),
    };
    /// The master: everything, references included.
    const MASTER: EnvCaller = EnvCaller {
        refs_allowed: true,
        admin: true,
        person: Some(1),
    };

    #[test]
    fn render_round_trips_awkward_values_through_sh() {
        let values = [
            "plain",
            "it's",
            "a\nb",
            "$HOME `id` $(id)",
            "\\back\\",
            "\"dq\"",
            " lead and trail ",
            "",
        ];
        let file = EnvFile {
            lines: values
                .iter()
                .enumerate()
                .map(|(i, v)| (format!("V{i}"), Secret::new(*v)))
                .collect(),
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("e.sh");
        std::fs::write(&path, file.render()).unwrap();
        for (i, v) in values.iter().enumerate() {
            let out = std::process::Command::new("sh")
                .arg("-c")
                .arg(format!(
                    ". {}; printf %s \"$V{i}\"",
                    crate::shell::quote(path.to_str().unwrap())
                ))
                .output()
                .unwrap();
            assert_eq!(String::from_utf8(out.stdout).unwrap(), *v, "V{i}");
        }
    }

    #[test]
    fn wrap_names_only_the_path_and_reads_it_under_home() {
        let w = wrap("~/.claude-fleet/env/abc.sh", "cl --name 'x'");
        assert_eq!(
            w,
            "__fleet_env_file=\"$HOME\"/'.claude-fleet/env/abc.sh'; if [ -r \"$__fleet_env_file\" ]; then . \"$__fleet_env_file\"; fi; unset __fleet_env_file; cl --name 'x'"
        );
    }

    #[test]
    fn a_wrapped_command_sees_the_variables() {
        let home = tempfile::tempdir().unwrap();
        let envdir = home.path().join(".claude-fleet/env");
        std::fs::create_dir_all(&envdir).unwrap();
        let file = EnvFile {
            lines: vec![("GREETING".into(), Secret::new("hi there"))],
        };
        std::fs::write(envdir.join("n.sh"), file.render()).unwrap();
        let cmd = wrap("~/.claude-fleet/env/n.sh", "printf %s \"$GREETING\"");
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(&cmd)
            .env("HOME", home.path())
            .output()
            .unwrap();
        assert_eq!(String::from_utf8(out.stdout).unwrap(), "hi there");
    }

    #[test]
    fn a_user_variable_named_f_survives_the_wrap() {
        let home = tempfile::tempdir().unwrap();
        let envdir = home.path().join(".claude-fleet/env");
        std::fs::create_dir_all(&envdir).unwrap();
        let file = EnvFile {
            lines: vec![("f".into(), Secret::new("keep"))],
        };
        std::fs::write(envdir.join("n.sh"), file.render()).unwrap();
        let cmd = wrap("~/.claude-fleet/env/n.sh", "printf %s \"$f\"");
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(&cmd)
            .env("HOME", home.path())
            .output()
            .unwrap();
        assert_eq!(String::from_utf8(out.stdout).unwrap(), "keep");
    }

    #[test]
    fn a_referenced_value_is_checked_and_the_refusal_names_only_the_variable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cred");
        std::fs::write(&path, "sec\0ret-tail").unwrap();
        let s = store();
        s.lock()
            .unwrap()
            .set_env_var(
                "global",
                "",
                "NULLY",
                &EnvValue::Ref(format!("file:{}", path.display())),
            )
            .unwrap();
        let e = resolve(&s, "local", None, None, None, &BTreeMap::new()).unwrap_err();
        assert_eq!(e.code, codes::E_INVALID);
        assert!(
            e.message.contains("NULLY") && !e.message.contains("ret-tail"),
            "{}",
            e.message
        );
    }

    #[test]
    fn extra_values_win_and_are_checked() {
        let s = store();
        s.lock()
            .unwrap()
            .set_env_var("global", "", "A", &EnvValue::Plain("g".into()))
            .unwrap();
        let extra = BTreeMap::from([("A".to_string(), "x".to_string())]);
        let f = resolve(&s, "local", None, None, None, &extra).unwrap();
        assert!(f.render().contains("export A='x'"));
        let bad = BTreeMap::from([("PATH".to_string(), "x".to_string())]);
        assert_eq!(
            resolve(&s, "local", None, None, None, &bad)
                .unwrap_err()
                .code,
            codes::E_INVALID
        );
    }

    #[test]
    fn an_unresolved_reference_refuses_without_leaking() {
        let s = store();
        // `/nonexistent` is not an absolute path on Windows, and the reference
        // must be one to be stored at all.
        let reference = if cfg!(windows) {
            r"file:C:\nonexistent\fleet-env-test"
        } else {
            "file:/nonexistent/fleet-env-test"
        };
        s.lock()
            .unwrap()
            .set_env_var("global", "", "TOKEN", &EnvValue::Ref(reference.into()))
            .unwrap();
        let e = resolve(&s, "local", None, None, None, &BTreeMap::new()).unwrap_err();
        assert_eq!(e.code, codes::E_ENV_UNRESOLVED);
        assert!(
            e.message.contains("TOKEN") && !e.message.contains("nonexistent"),
            "{}",
            e.message
        );
    }

    #[test]
    fn a_reference_resolves_from_the_environment() {
        let s = store();
        // SAFETY: a test-unique variable name; no other test reads it.
        unsafe { std::env::set_var("FLEET_ENV_TEST_REF_7731", "from-env") };
        s.lock()
            .unwrap()
            .set_env_var(
                "global",
                "",
                "R",
                &EnvValue::Ref("env:FLEET_ENV_TEST_REF_7731".into()),
            )
            .unwrap();
        let f = resolve(&s, "local", None, None, None, &BTreeMap::new()).unwrap();
        assert!(f.render().contains("export R='from-env'"));
    }

    #[test]
    fn the_merged_set_is_capped() {
        let s = store();
        let big = "x".repeat(32 * 1024);
        for i in 0..8 {
            s.lock()
                .unwrap()
                .set_env_var(
                    "global",
                    "",
                    &format!("V{i}"),
                    &EnvValue::Plain(big.clone()),
                )
                .unwrap();
        }
        assert_eq!(
            resolve(&s, "local", None, None, None, &BTreeMap::new())
                .unwrap_err()
                .code,
            codes::E_INVALID
        );
    }

    #[test]
    fn an_empty_set_leaves_the_pane_command_alone() {
        let f = resolve(&store(), "local", None, None, None, &BTreeMap::new()).unwrap();
        assert!(f.is_empty());
    }

    #[test]
    fn run_sets_lists_and_unsets_without_echoing_secrets() {
        let s = store();
        let set = |name: &str, value: Option<&str>, secret: bool, r: Option<&str>| EnvVarsArgs {
            action: "set".into(),
            scope: Some("global".into()),
            scope_key: None,
            name: Some(name.into()),
            value: value.map(Into::into),
            r#ref: r.map(Into::into),
            secret: Some(secret),
        };
        let out = run(&s, &set("A", Some("plain"), false, None), &DEVICE).unwrap();
        assert_eq!(out["applies"], "on the next start or restart");
        assert!(!out.to_string().contains("plain"), "{out}");
        run(&s, &set("S", Some("top-secret-1"), true, None), &DEVICE).unwrap();
        let list = EnvVarsArgs {
            action: "list".into(),
            ..Default::default()
        };
        let listed = run(&s, &list, &DEVICE).unwrap().to_string();
        assert!(
            listed.contains("plain") && !listed.contains("top-secret-1"),
            "{listed}"
        );
        let e = run(&s, &set("R", None, true, Some("env:X")), &DEVICE).unwrap_err();
        assert_eq!(
            e.code,
            codes::E_FORBIDDEN,
            "a device may not set a reference"
        );
        assert!(!e.message.contains("env:X"), "{}", e.message);
        run(&s, &set("R", None, true, Some("env:X")), &MASTER).unwrap();
        let both = run(&s, &set("B", Some("v"), false, Some("env:X")), &MASTER).unwrap_err();
        assert_eq!(both.code, codes::E_INVALID);
        let neither = run(&s, &set("B", None, false, None), &MASTER).unwrap_err();
        assert_eq!(neither.code, codes::E_INVALID);
        let unset = EnvVarsArgs {
            action: "unset".into(),
            scope: Some("global".into()),
            name: Some("A".into()),
            ..Default::default()
        };
        assert_eq!(run(&s, &unset, &DEVICE).unwrap()["removed"], true);
        assert_eq!(run(&s, &unset, &DEVICE).unwrap()["removed"], false);
        let missing = run(
            &s,
            &EnvVarsArgs {
                action: "unset".into(),
                ..Default::default()
            },
            &DEVICE,
        )
        .unwrap_err();
        assert_eq!(missing.code, codes::E_INVALID);
        let bogus = run(
            &s,
            &EnvVarsArgs {
                action: "dump".into(),
                ..Default::default()
            },
            &MASTER,
        )
        .unwrap_err();
        assert_eq!(bogus.code, codes::E_INVALID);
    }

    fn person_set(key: Option<&str>, name: &str, value: &str, secret: bool) -> EnvVarsArgs {
        EnvVarsArgs {
            action: "set".into(),
            scope: Some("person".into()),
            scope_key: key.map(Into::into),
            name: Some(name.into()),
            value: Some(value.into()),
            r#ref: None,
            secret: Some(secret),
        }
    }

    fn listed(s: &Mutex<Store>, c: &EnvCaller) -> Vec<(String, String, String)> {
        let list = EnvVarsArgs {
            action: "list".into(),
            ..Default::default()
        };
        run(s, &list, c)
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                (
                    r["scope"].as_str().unwrap().to_string(),
                    r["scope_key"].as_str().unwrap().to_string(),
                    r["name"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    }

    #[test]
    fn a_person_sets_lists_and_unsets_only_their_own_person_scope() {
        let s = store();
        let ada = EnvCaller {
            person: Some(5),
            ..Default::default()
        };
        let bob = EnvCaller {
            person: Some(6),
            ..Default::default()
        };
        // '' and 'me' are the caller's own person; the stored key is the id.
        let out = run(&s, &person_set(None, "GIT_AUTHOR_NAME", "Ada", false), &ada).unwrap();
        assert_eq!(out["scope_key"], "5");
        run(
            &s,
            &person_set(Some("me"), "GH_TOKEN", "ghp_ada_secret_1", true),
            &ada,
        )
        .unwrap();
        run(
            &s,
            &person_set(Some("6"), "GIT_AUTHOR_NAME", "Bob", false),
            &bob,
        )
        .unwrap();
        // Another person's key is refused, for a set and an unset alike,
        // and so is an admin's reach into someone else's person scope.
        for (who, key) in [(&bob, "5"), (&MASTER, "5"), (&ada, "6")] {
            let e = run(&s, &person_set(Some(key), "GH_TOKEN", "stolen", true), who).unwrap_err();
            assert_eq!(e.code, codes::E_FORBIDDEN, "{who:?} on {key}");
            let unset = EnvVarsArgs {
                action: "unset".into(),
                scope: Some("person".into()),
                scope_key: Some(key.into()),
                name: Some("GH_TOKEN".into()),
                ..Default::default()
            };
            assert_eq!(run(&s, &unset, who).unwrap_err().code, codes::E_FORBIDDEN);
        }
        // A non-admin person may not touch the shared scopes.
        let mut global = person_set(None, "X", "v", false);
        global.scope = Some("global".into());
        assert_eq!(run(&s, &global, &ada).unwrap_err().code, codes::E_FORBIDDEN);
        run(&s, &global, &MASTER).unwrap();
        // Each lists their own rows only; the admin also sees the shared
        // scopes, but never another person's.
        assert_eq!(
            listed(&s, &ada),
            [
                ("person".into(), "5".into(), "GH_TOKEN".into()),
                ("person".into(), "5".into(), "GIT_AUTHOR_NAME".into())
            ]
        );
        assert_eq!(
            listed(&s, &bob),
            [("person".into(), "6".into(), "GIT_AUTHOR_NAME".into())]
        );
        assert_eq!(
            listed(&s, &MASTER),
            [("global".into(), "".into(), "X".into())]
        );
        // The secret is never echoed back.
        let raw = run(
            &s,
            &EnvVarsArgs {
                action: "list".into(),
                ..Default::default()
            },
            &ada,
        )
        .unwrap()
        .to_string();
        assert!(
            raw.contains("Ada") && !raw.contains("ghp_ada_secret_1"),
            "{raw}"
        );
        // A caller with no person has no person scope.
        let nobody = EnvCaller {
            admin: true,
            ..Default::default()
        };
        assert_eq!(
            run(&s, &person_set(None, "A", "v", false), &nobody)
                .unwrap_err()
                .code,
            codes::E_FORBIDDEN
        );
        let unset = EnvVarsArgs {
            action: "unset".into(),
            scope: Some("person".into()),
            scope_key: Some("me".into()),
            name: Some("GH_TOKEN".into()),
            ..Default::default()
        };
        assert_eq!(run(&s, &unset, &ada).unwrap()["removed"], true);
    }

    /// A session row on host `h` owned by `owner` (`None`: unclaimed).
    fn session_row(s: &Mutex<Store>, id: i64, owner: Option<i64>) {
        let st = s.lock().unwrap();
        st.upsert_host("h").unwrap();
        st.conn_for_test()
            .execute(
                "INSERT INTO sessions (id, tmux_name, host_alias, created_at, last_activity_at, \
                 status, owner_person_id) VALUES (?1, ?2, 'h', 0, 0, 'running', ?3)",
                rusqlite::params![id, format!("t{id}"), owner],
            )
            .unwrap();
    }

    fn person(s: &Mutex<Store>, name: &str) -> i64 {
        s.lock()
            .unwrap()
            .conn_for_test()
            .query_row(
                "INSERT INTO people (name, is_personal_owner, created_at) VALUES (?1, 0, 1) \
                 RETURNING id",
                [name],
                |r| r.get(0),
            )
            .unwrap()
    }

    #[test]
    fn a_launch_sources_its_owners_person_scope_and_an_unclaimed_one_none() {
        let s = store();
        let ada = person(&s, "ada");
        let bob = person(&s, "bob");
        {
            let st = s.lock().unwrap();
            st.set_env_var(
                "host",
                "h",
                "GH_TOKEN",
                &EnvValue::Secret(Secret::new("host-fallback")),
            )
            .unwrap();
            st.set_env_var(
                "person",
                &ada.to_string(),
                "GH_TOKEN",
                &EnvValue::Secret(Secret::new("ada-token")),
            )
            .unwrap();
            st.set_env_var(
                "person",
                &ada.to_string(),
                "GIT_AUTHOR_EMAIL",
                &EnvValue::Plain("ada@x".into()),
            )
            .unwrap();
            st.set_env_var(
                "person",
                &bob.to_string(),
                "GIT_AUTHOR_EMAIL",
                &EnvValue::Plain("bob@x".into()),
            )
            .unwrap();
        }
        session_row(&s, 1, Some(ada));
        session_row(&s, 2, None);
        let none = BTreeMap::new();
        // An existing row: its owner at launch time (restart, recreate, move).
        let owned = resolve(&s, "h", None, None, Some(1), &none)
            .unwrap()
            .render();
        assert!(owned.contains("export GH_TOKEN='ada-token'"), "{owned}");
        assert!(owned.contains("GIT_AUTHOR_EMAIL='ada@x'") && !owned.contains("bob@x"));
        // The row's owner wins over a stated one.
        let row_wins = resolve(&s, "h", None, Some(bob), Some(1), &none)
            .unwrap()
            .render();
        assert!(row_wins.contains("ada@x") && !row_wins.contains("bob@x"));
        // Unclaimed: the host fallback, no person scope.
        let unclaimed = resolve(&s, "h", None, None, Some(2), &none)
            .unwrap()
            .render();
        assert!(
            unclaimed.contains("export GH_TOKEN='host-fallback'"),
            "{unclaimed}"
        );
        assert!(!unclaimed.contains("GIT_AUTHOR_EMAIL"));
        // A create path names the owner it is about to record.
        let created = resolve(&s, "h", None, Some(bob), None, &none)
            .unwrap()
            .render();
        assert!(created.contains("bob@x") && created.contains("host-fallback"));
        // A session-scope value still wins over the person's.
        s.lock()
            .unwrap()
            .set_env_var(
                "session",
                "1",
                "GH_TOKEN",
                &EnvValue::Plain("one-off".into()),
            )
            .unwrap();
        let over = resolve(&s, "h", None, None, Some(1), &none)
            .unwrap()
            .render();
        assert!(over.contains("export GH_TOKEN='one-off'"), "{over}");
        // A change of owner applies at the next launch.
        s.lock()
            .unwrap()
            .conn_for_test()
            .execute(
                "UPDATE sessions SET owner_person_id = ?1 WHERE id = 1",
                [bob],
            )
            .unwrap();
        let moved = resolve(&s, "h", None, None, Some(1), &none)
            .unwrap()
            .render();
        assert!(moved.contains("bob@x") && !moved.contains("ada@x"));
        // A disabled person's identity is not used.
        s.lock()
            .unwrap()
            .conn_for_test()
            .execute("UPDATE people SET disabled_at = 1 WHERE id = ?1", [bob])
            .unwrap();
        let gone = resolve(&s, "h", None, None, Some(1), &none)
            .unwrap()
            .render();
        assert!(!gone.contains("bob@x"), "{gone}");
    }

    #[tokio::test]
    async fn a_person_secret_reaches_the_env_file_never_the_pane_command() {
        use crate::ssh_fake::FakeSsh;
        let s = store();
        let ada = person(&s, "ada");
        s.lock()
            .unwrap()
            .set_env_var(
                "person",
                &ada.to_string(),
                "GH_TOKEN",
                &EnvValue::Secret(Secret::new("ghp_never_in_argv")),
            )
            .unwrap();
        let fake = FakeSsh::new();
        fake.with_home("/home/ada");
        let cmd = prepare(
            &s,
            &fake,
            "h",
            None,
            Some(ada),
            None,
            &BTreeMap::new(),
            "claude",
        )
        .await
        .unwrap();
        assert!(
            cmd.contains(".claude-fleet/env/") && cmd.ends_with("claude"),
            "{cmd}"
        );
        assert!(!cmd.contains("ghp_never_in_argv"), "{cmd}");
        let calls = fake.calls();
        for call in &calls {
            assert!(
                !call.command().contains("ghp_never_in_argv"),
                "a secret in an ssh argv: {}",
                call.command()
            );
        }
        // It travelled as the env file's bytes on stdin instead.
        assert!(calls.iter().any(|c| c
            .stdin
            .as_deref()
            .is_some_and(
                |b| String::from_utf8_lossy(b).contains("export GH_TOKEN='ghp_never_in_argv'")
            )));
    }

    fn noted(v: &[(&str, i64)]) -> NotedHosts {
        v.iter().map(|(h, t)| (h.to_string(), *t)).collect()
    }

    #[test]
    fn a_clean_host_is_forgotten_unless_it_was_noted_again_during_the_sweep() {
        let clean = ["a".to_string(), "c".to_string()];
        // a was clean and last noted before the sweep began; b still holds
        // a file; c was clean when looked at, but `prepare` noted it again
        // since; d was noted for the first time while the sweep ran.
        let current = noted(&[("a", 90), ("b", 90), ("c", 100), ("d", 105)]);
        assert_eq!(
            hosts_after_sweep(current, &clean, 100),
            noted(&[("b", 90), ("c", 100), ("d", 105)])
        );
        // Nothing noted since: only the hosts that still hold files.
        assert_eq!(
            hosts_after_sweep(noted(&[("a", 0)]), &["a".to_string()], 100),
            noted(&[])
        );
    }

    #[test]
    fn the_record_reads_its_first_shape_and_notes_a_host_as_of_now() {
        assert_eq!(
            parse_noted(Some(r#"["h","k"]"#.into())),
            noted(&[("h", 0), ("k", 0)])
        );
        assert_eq!(parse_noted(Some("nonsense".into())), noted(&[]));
        assert_eq!(parse_noted(None), noted(&[]));
        let s = store();
        note_host(&s, "h").unwrap();
        let t = read_noted(&s.lock().unwrap())["h"];
        assert!(t >= now_unix() - 5, "{t}");
        note_host(&s, "h").unwrap();
        assert!(read_noted(&s.lock().unwrap())["h"] >= t);
    }

    #[tokio::test]
    async fn a_removed_host_is_forgotten_without_being_reached() {
        use crate::ssh_fake::FakeSsh;
        let s = store();
        s.lock()
            .unwrap()
            .set_setting(ENV_FILES_HOSTS_KEY, r#"{"ghost":5}"#)
            .unwrap();
        let fake = FakeSsh::new();
        assert_eq!(sweep_env_files(&s, &fake).await, 0);
        assert!(fake.calls().is_empty(), "nothing to reach");
        assert_eq!(
            s.lock().unwrap().get_setting(ENV_FILES_HOSTS_KEY).unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn an_unreachable_host_stays_recorded_and_is_tried_again_after_its_backoff() {
        use crate::service::forms::SWEEP_BACKOFF_BASE;
        use crate::ssh_fake::FakeSsh;
        let s = store();
        {
            let st = s.lock().unwrap();
            st.insert_host("down", None).unwrap();
            st.set_setting(ENV_FILES_HOSTS_KEY, r#"{"down":5}"#)
                .unwrap();
        }
        let fake = FakeSsh::new();
        fake.unreachable("down");
        let backoff = SweepBackoff::default();
        let t0 = Instant::now();
        assert_eq!(sweep_env_files_at(&s, &fake, &backoff, t0).await, 0);
        assert_eq!(fake.calls().len(), 1, "tried once");
        assert!(s
            .lock()
            .unwrap()
            .get_setting(ENV_FILES_HOSTS_KEY)
            .unwrap()
            .unwrap()
            .contains("\"down\""));
        // The next pass skips it; after the delay it is tried again.
        sweep_env_files_at(&s, &fake, &backoff, t0).await;
        assert_eq!(fake.calls().len(), 1, "skipped while backing off");
        sweep_env_files_at(&s, &fake, &backoff, t0 + SWEEP_BACKOFF_BASE).await;
        assert_eq!(fake.calls().len(), 2, "tried again once the delay passed");
        // A host that answers again (clean) is swept and forgotten.
        let up = FakeSsh::new();
        up.on_host(
            "down",
            crate::ssh_fake::Match::Any,
            crate::ssh_fake::Reply::ok("clean\n"),
        );
        assert_eq!(
            sweep_env_files_at(&s, &up, &backoff, t0 + SWEEP_BACKOFF_BASE * 4).await,
            1
        );
        assert!(backoff.lock().unwrap().is_empty());
        assert_eq!(
            s.lock().unwrap().get_setting(ENV_FILES_HOSTS_KEY).unwrap(),
            None
        );
    }

    #[cfg(unix)]
    mod sweep {
        use super::*;
        use crate::ssh::SshClient;
        use crate::tmux::fake_exec::{write_exec, PROBE_GUARD};

        /// A host `h` whose `ssh` runs each remote script on this machine
        /// with HOME at a tempdir (the `env_launch` harness's fake).
        fn fake(dir: &std::path::Path) -> SshClient {
            let home = dir.join("home");
            let bin = dir.join("bin");
            std::fs::create_dir_all(&home).unwrap();
            std::fs::create_dir_all(&bin).unwrap();
            let ssh_bin = write_exec(
                &bin,
                "ssh",
                &format!(
                    "#!/bin/sh\n{PROBE_GUARD}\
                     case \"$*\" in *'-O check'*|*'-O exit'*) exit 0;; esac\n\
                     while [ \"$#\" -gt 0 ] && [ \"$1\" != \"--\" ]; do shift; done\n\
                     shift 2\n\
                     HOME='{}' exec sh -c \"$*\"\n",
                    home.display()
                ),
            );
            SshClient::with_ssh_binary(ssh_bin)
        }

        fn age(path: &std::path::Path, mins: u64) {
            let t = std::time::SystemTime::now() - std::time::Duration::from_secs(mins * 60);
            std::fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(t)
                .unwrap();
        }

        #[tokio::test]
        async fn the_sweep_removes_old_files_and_forgets_a_clean_host() {
            let tmp = tempfile::tempdir().unwrap();
            let ssh = fake(tmp.path());
            let dir = tmp.path().join("home/.claude-fleet/env");
            std::fs::create_dir_all(&dir).unwrap();
            let old = dir.join("old.sh");
            let fresh = dir.join("fresh.sh");
            std::fs::write(&old, "x").unwrap();
            std::fs::write(&fresh, "x").unwrap();
            age(&old, 11);
            let s = store();
            {
                let st = s.lock().unwrap();
                st.insert_host("h", None).unwrap();
                st.set_setting(ENV_FILES_HOSTS_KEY, r#"{"h":5}"#).unwrap();
            }
            assert_eq!(sweep_env_files(&s, &ssh).await, 1);
            assert!(!old.exists() && fresh.exists());
            // A fresh file remains, so the host is still recorded.
            assert!(s
                .lock()
                .unwrap()
                .get_setting(ENV_FILES_HOSTS_KEY)
                .unwrap()
                .unwrap()
                .contains("\"h\""));
            // Once it ages out, the next sweep empties the dir and forgets the host.
            age(&fresh, 11);
            assert_eq!(sweep_env_files(&s, &ssh).await, 1);
            assert!(!fresh.exists());
            assert_eq!(
                s.lock().unwrap().get_setting(ENV_FILES_HOSTS_KEY).unwrap(),
                None
            );
            // Nothing recorded: nothing swept.
            assert_eq!(sweep_env_files(&s, &ssh).await, 0);
        }
    }
}
