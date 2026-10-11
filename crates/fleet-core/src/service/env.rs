//! Session environment at launch (spec 2026-10-11-session-env-and-secrets-design.md).
//! The merged set is written to a 0600 nonce file on the host and the pane
//! command sources it; no value is ever in an argv. Files are deleted by
//! the tick sweep after ENV_FILE_MAX_AGE_MIN (several launchers may run one
//! pane command, so the pane does not delete its own file).

use std::collections::BTreeMap;
use std::sync::Mutex;

use crate::ipc_error::{codes, lock, IpcError};
use crate::ssh::SshExec;
use crate::store::{
    check_env_name, check_env_value, read_credential_ref, EnvSource, Secret, Store,
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

pub fn resolve(
    store: &Mutex<Store>,
    host: &str,
    project: Option<&str>,
    session: Option<i64>,
    extra: &BTreeMap<String, String>,
) -> Result<EnvFile, IpcError> {
    for (n, v) in extra {
        check_env_name(n)?;
        check_env_value(v)?;
    }
    // Read rows under the lock; resolve references (file/env reads) after it.
    let rows = lock(store)?.env_for_launch(host, project, session)?;
    let mut merged: BTreeMap<String, Secret> = BTreeMap::new();
    for (name, src) in rows {
        let v = match src {
            EnvSource::Value(v) => v,
            EnvSource::Ref(r) => {
                let kind = r.split_once(':').map(|(k, _)| k).unwrap_or("ref");
                match read_credential_ref(&r) {
                    Some(v) => Secret::new(v),
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
        "f=\"$HOME\"/{}; if [ -r \"$f\" ]; then . \"$f\"; fi; unset f; {pane_cmd}",
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

fn note_host(store: &Mutex<Store>, host: &str) -> Result<(), IpcError> {
    let s = lock(store)?;
    let mut hosts: Vec<String> = s
        .get_setting(ENV_FILES_HOSTS_KEY)?
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_default();
    if !hosts.iter().any(|h| h == host) {
        hosts.push(host.to_string());
        s.set_setting(
            ENV_FILES_HOSTS_KEY,
            &serde_json::to_string(&hosts).unwrap_or_default(),
        )?;
    }
    Ok(())
}

/// Resolve, write the file and wrap `pane_cmd`. An empty set returns
/// `pane_cmd` unchanged and writes nothing.
pub async fn prepare(
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    host: &str,
    project_id: Option<i64>,
    session: Option<i64>,
    extra: &BTreeMap<String, String>,
    pane_cmd: &str,
) -> Result<String, IpcError> {
    let project = project_slug(store, project_id)?;
    let file = resolve(store, host, project.as_deref(), session, extra)?;
    if file.is_empty() {
        return Ok(pane_cmd.to_string());
    }
    let path = format!("{ENV_DIR}/{}.sh", uuid::Uuid::new_v4().simple());
    // Record the host first: a file written but never swept is the leak.
    note_host(store, host)?;
    crate::service::provision::write_host_file_secret(ssh, host, ENV_DIR, &path, &file.render())
        .await?;
    Ok(wrap(&path, pane_cmd))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{EnvValue, Store};
    use std::sync::Mutex;

    fn store() -> Mutex<Store> {
        Mutex::new(Store::open_in_memory().unwrap())
    }

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
            "f=\"$HOME\"/'.claude-fleet/env/abc.sh'; if [ -r \"$f\" ]; then . \"$f\"; fi; unset f; cl --name 'x'"
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
    fn extra_values_win_and_are_checked() {
        let s = store();
        s.lock()
            .unwrap()
            .set_env_var("global", "", "A", &EnvValue::Plain("g".into()))
            .unwrap();
        let extra = BTreeMap::from([("A".to_string(), "x".to_string())]);
        let f = resolve(&s, "local", None, None, &extra).unwrap();
        assert!(f.render().contains("export A='x'"));
        let bad = BTreeMap::from([("PATH".to_string(), "x".to_string())]);
        assert_eq!(
            resolve(&s, "local", None, None, &bad).unwrap_err().code,
            codes::E_INVALID
        );
    }

    #[test]
    fn an_unresolved_reference_refuses_without_leaking() {
        let s = store();
        s.lock()
            .unwrap()
            .set_env_var(
                "global",
                "",
                "TOKEN",
                &EnvValue::Ref("file:/nonexistent/fleet-env-test".into()),
            )
            .unwrap();
        let e = resolve(&s, "local", None, None, &BTreeMap::new()).unwrap_err();
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
        let f = resolve(&s, "local", None, None, &BTreeMap::new()).unwrap();
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
            resolve(&s, "local", None, None, &BTreeMap::new())
                .unwrap_err()
                .code,
            codes::E_INVALID
        );
    }

    #[test]
    fn an_empty_set_leaves_the_pane_command_alone() {
        let f = resolve(&store(), "local", None, None, &BTreeMap::new()).unwrap();
        assert!(f.is_empty());
    }
}
