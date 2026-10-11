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

/// The hosts to record after a sweep: those that still hold a file, plus any
/// host `prepare` recorded while the sweep ran (`current` has it, `read`, the
/// list the sweep started from, did not).
fn merge_hosts(keep: Vec<String>, read: &[String], current: &[String]) -> Vec<String> {
    let mut out = keep;
    for h in current {
        if !read.contains(h) && !out.contains(h) {
            out.push(h.clone());
        }
    }
    out
}

/// Delete env files older than ENV_FILE_MAX_AGE_MIN on every recorded host;
/// forget a host once its directory holds no file. Unreachable hosts stay
/// recorded and are retried next tick. Returns the number of hosts swept.
pub async fn sweep_env_files(store: &Mutex<Store>, ssh: &dyn SshExec) -> usize {
    let read: Vec<String> = match lock(store)
        .ok()
        .and_then(|s| s.get_setting(ENV_FILES_HOSTS_KEY).ok().flatten())
    {
        Some(v) => serde_json::from_str(&v).unwrap_or_default(),
        None => return 0,
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
    let mut keep = Vec::new();
    for host in &read {
        match crate::ssh::run_shell(ssh, host, &script, std::time::Duration::from_secs(20)).await {
            Ok(out) if out.status.success() => {
                swept += 1;
                if String::from_utf8_lossy(&out.stdout)
                    .trim_end()
                    .ends_with("left")
                {
                    keep.push(host.clone());
                }
            }
            Ok(_) => {
                tracing::debug!(host = %host, "env file sweep exited non-zero; retrying next tick");
                keep.push(host.clone());
            }
            Err(e) => {
                tracing::debug!(host = %host, code = %e.code, "env file sweep failed; retrying next tick");
                keep.push(host.clone());
            }
        }
    }
    if let Ok(s) = lock(store) {
        // Re-read under the lock: a `prepare` may have recorded a host while
        // the sweep was on the wire.
        let current: Vec<String> = s
            .get_setting(ENV_FILES_HOSTS_KEY)
            .ok()
            .flatten()
            .and_then(|v| serde_json::from_str(&v).ok())
            .unwrap_or_default();
        let keep = merge_hosts(keep, &read, &current);
        let _ = if keep.is_empty() {
            s.delete_setting(ENV_FILES_HOSTS_KEY)
        } else {
            s.set_setting(
                ENV_FILES_HOSTS_KEY,
                &serde_json::to_string(&keep).unwrap_or_default(),
            )
        };
    }
    swept
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
        let e = resolve(&s, "local", None, None, &BTreeMap::new()).unwrap_err();
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

    #[test]
    fn a_host_recorded_during_the_sweep_is_kept() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        // a was clean, b still has files, c was recorded mid-sweep.
        assert_eq!(
            merge_hosts(s(&["b"]), &s(&["a", "b"]), &s(&["a", "b", "c"])),
            s(&["b", "c"])
        );
        // Nothing new: only the hosts that still hold files.
        assert_eq!(merge_hosts(s(&[]), &s(&["a"]), &s(&["a"])), s(&[]));
        // A host already kept is not doubled.
        assert_eq!(
            merge_hosts(s(&["c"]), &s(&["a"]), &s(&["a", "c"])),
            s(&["c"])
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
            s.lock()
                .unwrap()
                .set_setting(ENV_FILES_HOSTS_KEY, r#"["h"]"#)
                .unwrap();
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
