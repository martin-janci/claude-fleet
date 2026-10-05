//! Assets M6 (Rulings R7): the catalog's and the host's text of one asset's
//! files, for the Drift card's DiffView. The desktop computes the diff.
//!
//! What may be read, and what may be answered:
//! - Only the files the asset renders (`RenderPlan.files`) are read on the
//!   host — never a config file a merge writes into (`.claude.json`,
//!   `settings.json`), which holds other assets' entries and secrets.
//! - The catalog side is rendered WITHOUT secret substitution, so it keeps
//!   its `${NAME}` placeholders. A file that holds a placeholder is not read
//!   on the host at all (its host copy has the secret's value in it); the
//!   answer says `secret` and carries neither text.
//! - Each side is cut at [`MAX_SIDE_BYTES`].
//! - A path must be one apply would write: `~/`-relative, no `..`.

use super::catalogs::PERSONAL;
use super::harness::{self, Harness};
use super::model::{find_placeholders, Kind};
use super::sync::apply::is_safe_path;
use super::validate::check_name;
use super::{effective, inventory, require_dialable_host, CatalogTarget};
use crate::ipc_error::{codes, IpcError};
use crate::ssh::SshClient;
use crate::store::Store;
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// The most of one side that is answered; the rest is cut and `truncated`
/// says so.
pub const MAX_SIDE_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriftDiffArgs {
    pub host_alias: String,
    pub kind: Kind,
    pub name: String,
    /// default `claude`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriftFile {
    /// As the plan names it: relative to the host's `$HOME` (`~/.claude/…`).
    pub path: String,
    /// The catalog's rendered text, placeholders unsubstituted.
    #[serde(default)]
    pub catalog: Option<String>,
    /// The host's text; `None` when the file is missing on the host.
    #[serde(default)]
    pub host: Option<String>,
    /// Either side is not UTF-8 (then both texts are `None`).
    #[serde(default)]
    pub binary: bool,
    /// Either side was cut at [`MAX_SIDE_BYTES`].
    #[serde(default)]
    pub truncated: bool,
    /// The file holds a `${NAME}` secret: the host copy has its value, so
    /// neither side is read or shown (both texts are `None`).
    #[serde(default)]
    pub secret: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriftDiff {
    pub host_alias: String,
    pub harness: String,
    pub files: Vec<DriftFile>,
    /// The asset renders only into config files (an MCP entry): its diff is
    /// not shown, and no config file was read (R7).
    #[serde(default)]
    pub merges_only: bool,
}

/// What one remote read found.
enum HostFile {
    Missing,
    Bytes(Vec<u8>),
}

/// One remote read: each path (`~/`-relative, already vetted by
/// `is_safe_path`) as `==FILE <rel>`, base64 of at most `MAX_SIDE_BYTES + 1`
/// bytes, `==END`; or `==MISSING <rel>`; or `==UNREADABLE <rel>`. The path
/// reaches the shell only through `shell::quote`.
fn read_script(rels: &[&str]) -> String {
    let mut s = String::from("cd \"$HOME\" || exit 1\n");
    for rel in rels {
        s.push_str(&format!(
            "f={}\n\
             if [ -f \"$f\" ] && [ -r \"$f\" ]; then\n\
             echo \"==FILE $f\"; head -c {} \"$f\" | base64 | tr -d '\\n'; echo; echo \"==END\"\n\
             elif [ -e \"$f\" ]; then echo \"==UNREADABLE $f\"\n\
             else echo \"==MISSING $f\"; fi\n",
            crate::shell::quote(rel),
            MAX_SIDE_BYTES + 1,
        ));
    }
    s
}

/// Parse [`read_script`]'s output. A path nobody asked for, or a block that
/// does not decode, is an error; anything outside a block (a login shell's
/// chatter) is ignored.
fn parse_read(
    host: &str,
    stdout: &str,
    asked: &[&str],
) -> Result<BTreeMap<String, HostFile>, IpcError> {
    let bad = |what: &str| IpcError::new(codes::E_SCAN, format!("{host}: drift read: {what}"));
    let known = |rel: &str| -> Result<(), IpcError> {
        if asked.contains(&rel) {
            Ok(())
        } else {
            Err(bad("an unrequested path in the answer"))
        }
    };
    let mut out = BTreeMap::new();
    let mut open: Option<(String, String)> = None;
    for line in stdout.lines() {
        let line = line.trim();
        if let Some((rel, b64)) = open.as_mut() {
            if line == "==END" {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(b64.as_bytes())
                    .map_err(|_| bad("undecodable content"))?;
                out.insert(std::mem::take(rel), HostFile::Bytes(bytes));
                open = None;
            } else {
                b64.push_str(line);
            }
        } else if let Some(rel) = line.strip_prefix("==FILE ") {
            known(rel)?;
            open = Some((rel.to_string(), String::new()));
        } else if let Some(rel) = line.strip_prefix("==MISSING ") {
            known(rel)?;
            out.insert(rel.to_string(), HostFile::Missing);
        } else if let Some(rel) = line.strip_prefix("==UNREADABLE ") {
            known(rel)?;
            return Err(IpcError::new(
                codes::E_IO,
                format!("{host}: cannot read ~/{rel}"),
            ));
        }
    }
    if open.is_some() {
        return Err(bad("a file block was cut short"));
    }
    if let Some(rel) = asked.iter().find(|r| !out.contains_key(**r)) {
        return Err(bad(&format!("no answer for ~/{rel}")));
    }
    Ok(out)
}

/// One side as text: cut at the cap (on a character boundary), and `None`
/// when it is not UTF-8. Answers `(text, truncated)`.
fn side(bytes: &[u8]) -> (Option<String>, bool) {
    let cut = bytes.len() > MAX_SIDE_BYTES;
    let bytes = &bytes[..bytes.len().min(MAX_SIDE_BYTES)];
    match std::str::from_utf8(bytes) {
        Ok(t) => (Some(t.to_string()), cut),
        // A multi-byte character the cut fell inside is not binary.
        Err(e) if cut && e.error_len().is_none() => (
            Some(String::from_utf8_lossy(&bytes[..e.valid_up_to()]).into_owned()),
            true,
        ),
        Err(_) => (None, cut),
    }
}

/// The two texts of every file `args.kind`/`args.name` renders for the host,
/// on the host's `harness` (default `claude`). The asset must be one the
/// host is planned to have from the catalog `target` names (a grant on that
/// catalog is what lets the caller read it).
pub async fn drift_diff(
    target: CatalogTarget<'_>,
    args: DriftDiffArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<DriftDiff, IpcError> {
    check_name(&args.name)?;
    let harness_id = args.harness.as_deref().unwrap_or("claude");
    let harness: Box<dyn Harness> = harness::by_id(harness_id).ok_or_else(|| {
        IpcError::new(
            codes::E_INVALID,
            format!("unknown harness {harness_id:?}; one of claude, codex"),
        )
    })?;
    if args.host_alias == "local" {
        crate::service::hub::ensure_local_allowed(&args.host_alias)?;
    } else {
        require_dialable_host(store, &args.host_alias)?;
    }
    let (kind, name) = (args.kind, args.name.as_str());
    let from = match target {
        CatalogTarget::Personal => PERSONAL,
        CatalogTarget::Row(r) => r.name.as_str(),
    };
    // What the planner renders from for this host. An asset of another
    // catalog than the one the caller may read is "not planned" to them.
    let eff = effective::effective_for_host(store, &args.host_alias)?;
    let not_planned = || {
        IpcError::new(
            codes::E_NOTFOUND,
            format!(
                "{}/{name} is not planned for {}",
                kind.as_str(),
                args.host_alias
            ),
        )
    };
    let asset = eff
        .catalog
        .find(kind, name)
        .filter(|_| eff.catalog.origin_of(kind, name).name == from)
        .ok_or_else(not_planned)?;
    // No secret substitution: the catalog side keeps its placeholders.
    let plan = harness.render(asset).map_err(|u| u.into_ipc())?;
    let done = |files| DriftDiff {
        host_alias: args.host_alias.clone(),
        harness: harness_id.to_string(),
        files,
        merges_only: false,
    };
    if plan.files.is_empty() {
        // An MCP entry (or a disabled asset): nothing but config merges.
        // No config file is read.
        return Ok(DriftDiff {
            merges_only: !plan.merges.is_empty(),
            ..done(Vec::new())
        });
    }
    for f in &plan.files {
        if !is_safe_path(&f.path) {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("{}: not a path a sync writes", f.path),
            ));
        }
    }
    // A file that holds a secret placeholder is not read on the host.
    let secret: Vec<bool> = plan
        .files
        .iter()
        .map(|f| !find_placeholders(&String::from_utf8_lossy(&f.bytes)).is_empty())
        .collect();
    let rel = |path: &str| path.trim_start_matches("~/").to_string();
    let rels: Vec<String> = plan
        .files
        .iter()
        .zip(&secret)
        .filter(|(_, s)| !**s)
        .map(|(f, _)| rel(&f.path))
        .collect();
    let rels: Vec<&str> = rels.iter().map(String::as_str).collect();
    let mut on_host = if rels.is_empty() {
        BTreeMap::new()
    } else {
        let out = inventory::run_host_script(ssh, &args.host_alias, &read_script(&rels)).await?;
        parse_read(&args.host_alias, &out, &rels)?
    };
    let files = plan
        .files
        .iter()
        .zip(&secret)
        .map(|(f, secret)| {
            let path = f.path.clone();
            if *secret {
                return DriftFile {
                    path,
                    catalog: None,
                    host: None,
                    binary: false,
                    truncated: false,
                    secret: true,
                };
            }
            let (catalog, cat_cut) = side(&f.bytes);
            let (host, host_cut, host_binary) = match on_host.remove(&rel(&f.path)) {
                Some(HostFile::Bytes(b)) => {
                    let (text, cut) = side(&b);
                    let binary = text.is_none();
                    (text, cut, binary)
                }
                _ => (None, false, false),
            };
            let binary = catalog.is_none() || host_binary;
            DriftFile {
                path,
                catalog: if binary { None } else { catalog },
                host: if binary { None } else { host },
                binary,
                truncated: cat_cut || host_cut,
                secret: false,
            }
        })
        .collect();
    Ok(done(files))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cut_never_splits_a_character() {
        let mut s = "é".repeat(MAX_SIDE_BYTES / 2 + 1).into_bytes();
        s.push(b'x');
        let (text, cut) = side(&s);
        assert!(cut);
        assert_eq!(text.unwrap().len(), MAX_SIDE_BYTES);
        let (text, cut) = side(&[0xff, b'a']);
        assert_eq!((text, cut), (None, false));
    }

    #[test]
    fn the_answer_names_only_what_was_asked() {
        let asked = ["a/b"];
        let ok = parse_read("h", "motd\n==FILE a/b\nWA==\n==END\n", &asked).unwrap();
        assert!(matches!(ok.get("a/b"), Some(HostFile::Bytes(b)) if b == b"X"));
        for bad in [
            "==FILE other\nWA==\n==END\n",
            "==MISSING other\n",
            "==FILE a/b\nWA==\n",
            "",
        ] {
            assert_eq!(
                parse_read("h", bad, &asked).err().map(|e| e.code),
                Some(codes::E_SCAN.to_string()),
                "{bad:?}"
            );
        }
        assert_eq!(
            parse_read("h", "==UNREADABLE a/b\n", &asked)
                .err()
                .map(|e| e.code),
            Some(codes::E_IO.to_string())
        );
    }

    #[test]
    fn the_script_quotes_every_path() {
        let s = read_script(&[".claude/skills/a'b/SKILL.md"]);
        assert!(s.contains(r#"f='.claude/skills/a'\''b/SKILL.md'"#), "{s}");
    }
}

/// Against a real `$HOME` through the fake `ssh`: unix only.
#[cfg(all(test, unix))]
mod tests_host {
    use super::*;
    use crate::service::catalog::changesets::testkit::{
        fleet_with_core, person_syncs_oci, ssh_with_home,
    };
    use crate::service::catalog::lock_registry_for_test;

    fn args(name: &str) -> DriftDiffArgs {
        DriftDiffArgs {
            host_alias: "oci".into(),
            kind: Kind::Skill,
            name: name.into(),
            harness: None,
        }
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn the_diff_holds_the_catalog_and_the_host_text_of_each_file() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let ssh = ssh_with_home(bin.path(), home.path());
        person_syncs_oci(&f, &ssh).await;
        std::fs::write(
            home.path().join(".claude/skills/w/SKILL.md"),
            "edited on the host\n",
        )
        .unwrap();
        let d = drift_diff(CatalogTarget::Personal, args("w"), &f.store, &ssh)
            .await
            .unwrap();
        assert_eq!(
            (d.host_alias.as_str(), d.harness.as_str()),
            ("oci", "claude")
        );
        let skill = d
            .files
            .iter()
            .find(|x| x.path.ends_with("skills/w/SKILL.md"))
            .unwrap();
        assert!(skill.catalog.as_deref().unwrap().contains("Steps."));
        assert_eq!(skill.host.as_deref(), Some("edited on the host\n"));
        assert!(!skill.binary && !skill.truncated && !skill.secret);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_file_missing_on_the_host_has_no_host_text() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let ssh = ssh_with_home(bin.path(), home.path());
        person_syncs_oci(&f, &ssh).await;
        std::fs::remove_file(home.path().join(".claude/skills/w/SKILL.md")).unwrap();
        let d = drift_diff(CatalogTarget::Personal, args("w"), &f.store, &ssh)
            .await
            .unwrap();
        let skill = d
            .files
            .iter()
            .find(|x| x.path.ends_with("SKILL.md"))
            .unwrap();
        assert_eq!(skill.host, None);
        assert!(skill.catalog.is_some() && !skill.binary);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_large_host_file_is_cut_and_marked() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let ssh = ssh_with_home(bin.path(), home.path());
        person_syncs_oci(&f, &ssh).await;
        std::fs::write(
            home.path().join(".claude/skills/w/SKILL.md"),
            "x".repeat(MAX_SIDE_BYTES + 10),
        )
        .unwrap();
        let d = drift_diff(CatalogTarget::Personal, args("w"), &f.store, &ssh)
            .await
            .unwrap();
        let skill = d
            .files
            .iter()
            .find(|x| x.path.ends_with("SKILL.md"))
            .unwrap();
        assert!(skill.truncated);
        assert_eq!(skill.host.as_ref().unwrap().len(), MAX_SIDE_BYTES);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_host_file_that_is_not_utf8_is_binary_and_shows_no_text() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let ssh = ssh_with_home(bin.path(), home.path());
        person_syncs_oci(&f, &ssh).await;
        std::fs::write(
            home.path().join(".claude/skills/w/SKILL.md"),
            [0xff, 0xfe, 0x00, 0x80],
        )
        .unwrap();
        let d = drift_diff(CatalogTarget::Personal, args("w"), &f.store, &ssh)
            .await
            .unwrap();
        let skill = d
            .files
            .iter()
            .find(|x| x.path.ends_with("SKILL.md"))
            .unwrap();
        assert!(skill.binary);
        assert_eq!((&skill.catalog, &skill.host), (&None, &None));
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn an_asset_not_planned_for_the_host_is_refused() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let ssh = ssh_with_home(bin.path(), home.path());
        let e = drift_diff(CatalogTarget::Personal, args("nope"), &f.store, &ssh)
            .await
            .unwrap_err();
        assert_eq!(e.code, codes::E_NOTFOUND);
        assert!(
            e.message.contains("skill/nope is not planned for oci"),
            "{}",
            e.message
        );
    }

    /// The grant is on the catalog the call names: an asset another catalog
    /// supplies to the host is not planned, for a caller who may read only
    /// this one.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn an_asset_of_another_catalog_is_not_planned_for_this_one() {
        let _g = lock_registry_for_test();
        let mut f = fleet_with_core(&["oci"]);
        let (acme, _root) = f.add_org_catalog("acme");
        let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let ssh = ssh_with_home(bin.path(), home.path());
        let e = drift_diff(CatalogTarget::Row(&acme), args("w"), &f.store, &ssh)
            .await
            .unwrap_err();
        assert_eq!(e.code, codes::E_NOTFOUND);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn an_unknown_host_and_an_unknown_harness_are_refused() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let ssh = ssh_with_home(bin.path(), home.path());
        let mut a = args("w");
        a.host_alias = "nowhere".into();
        let e = drift_diff(CatalogTarget::Personal, a, &f.store, &ssh)
            .await
            .unwrap_err();
        assert_eq!(e.code, codes::E_NOTFOUND);
        let mut a = args("w");
        a.harness = Some("vim".into());
        let e = drift_diff(CatalogTarget::Personal, a, &f.store, &ssh)
            .await
            .unwrap_err();
        assert_eq!(e.code, codes::E_INVALID);
        let e = drift_diff(CatalogTarget::Personal, args("../x"), &f.store, &ssh)
            .await
            .unwrap_err();
        assert_eq!(e.code, codes::E_INVALID);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn an_mcp_entry_is_merges_only_and_reads_no_config_file() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        f.add_mcp_server_to_core("jira");
        let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let ssh = ssh_with_home(bin.path(), home.path());
        std::fs::write(
            home.path().join(".claude.json"),
            r#"{"mcpServers":{"jira":{"env":{"TOKEN":"secret-value"}}}}"#,
        )
        .unwrap();
        let d = drift_diff(
            CatalogTarget::Personal,
            DriftDiffArgs {
                host_alias: "oci".into(),
                kind: Kind::McpServer,
                name: "jira".into(),
                harness: None,
            },
            &f.store,
            &ssh,
        )
        .await
        .unwrap();
        assert!(d.merges_only);
        assert!(d.files.is_empty());
        assert!(!serde_json::to_string(&d).unwrap().contains("secret-value"));
    }

    /// A rendered file with a `${NAME}` placeholder has the secret's value
    /// on the host: it is not read, and neither text is answered.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_file_holding_a_secret_placeholder_is_not_read() {
        let _g = lock_registry_for_test();
        let f = fleet_with_core(&["oci"]);
        f.commit_files(
            &f.personal_root,
            f.personal.id,
            &[("skills/w/body.md", "token: ${W_TOKEN}\n")],
        );
        f.store
            .lock()
            .unwrap()
            .set_secret("W_TOKEN", None, "hunter2-the-secret")
            .unwrap();
        let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let ssh = ssh_with_home(bin.path(), home.path());
        person_syncs_oci(&f, &ssh).await;
        let on_host =
            std::fs::read_to_string(home.path().join(".claude/skills/w/SKILL.md")).unwrap();
        assert!(on_host.contains("hunter2-the-secret"), "the host has it");
        let d = drift_diff(CatalogTarget::Personal, args("w"), &f.store, &ssh)
            .await
            .unwrap();
        let skill = d
            .files
            .iter()
            .find(|x| x.path.ends_with("SKILL.md"))
            .unwrap();
        assert!(skill.secret);
        assert_eq!((&skill.catalog, &skill.host), (&None, &None));
        assert!(!serde_json::to_string(&d)
            .unwrap()
            .contains("hunter2-the-secret"));
    }
}
